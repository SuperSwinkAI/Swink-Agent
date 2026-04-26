//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].
//!
//! Concurrent writes to the same session may corrupt the file.
//! Callers are expected to enforce single-writer access.

use std::io::{self, BufRead, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use swink_agent::atomic_fs::{atomic_write, atomic_write_unlocked, with_target_lock};
use swink_agent::{AgentMessage, CustomMessageRegistry, LlmMessage};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;
use crate::store::SessionStore;
use crate::time::{format_session_id, now_utc};

const META_LINE_PADDING: usize = 64;

#[derive(Debug, Clone)]
enum SessionRecord {
    Llm(Box<LlmMessage>),
    Custom(serde_json::Value),
    State(serde_json::Value),
}

impl SessionRecord {
    fn from_message(message: &AgentMessage, session_id: &str) -> Option<Self> {
        let line = crate::codec::encode_jsonl_message_line(message, session_id)?;
        match Self::parse(&line).ok()? {
            Self::Llm(llm) => Some(Self::Llm(llm)),
            Self::Custom(envelope) => Some(Self::Custom(envelope)),
            Self::State(_) => None,
        }
    }

    const fn state(state: serde_json::Value) -> Self {
        Self::State(state)
    }

    fn to_json_line(&self) -> io::Result<String> {
        match self {
            Self::Llm(message) => serde_json::to_string(message).map_err(io::Error::other),
            Self::Custom(envelope) => serde_json::to_string(envelope).map_err(io::Error::other),
            Self::State(state) => serde_json::to_string(&serde_json::json!({
                "_state": true,
                "data": state
            }))
            .map_err(io::Error::other),
        }
    }

    fn parse(line: &str) -> io::Result<Self> {
        let value: serde_json::Value = serde_json::from_str(line).map_err(io::Error::other)?;
        if value.get("_state").and_then(serde_json::Value::as_bool) == Some(true) {
            return Ok(Self::State(
                value
                    .get("data")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ));
        }
        if value.get("_custom").and_then(serde_json::Value::as_bool) == Some(true) {
            return Ok(Self::Custom(value));
        }
        serde_json::from_value::<LlmMessage>(value)
            .map(Box::new)
            .map(Self::Llm)
            .map_err(io::Error::other)
    }
}

fn session_path(sessions_dir: &Path, id: &str) -> PathBuf {
    sessions_dir.join(format!("{id}.jsonl"))
}

fn interrupt_path(sessions_dir: &Path, id: &str) -> PathBuf {
    sessions_dir.join(format!("{id}.interrupt.json"))
}

fn with_session_lock<T>(
    sessions_dir: &Path,
    id: &str,
    op: impl FnOnce(&Path, &Path) -> io::Result<T>,
) -> io::Result<T> {
    let session = session_path(sessions_dir, id);
    let interrupt = interrupt_path(sessions_dir, id);
    with_target_lock(&session, || op(&session, &interrupt))
}

fn not_found(id: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("session not found: {id}"))
}

fn sequence_conflict(id: &str, expected: u64, actual: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("sequence conflict for session {id}: expected {expected}, found {actual}"),
    )
}

fn check_sequence_path(path: &Path, id: &str, caller_sequence: u64) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let (stored_meta, _) = read_meta_with_line_len(path, id)?;
    if stored_meta.sequence != caller_sequence {
        return Err(sequence_conflict(id, caller_sequence, stored_meta.sequence));
    }
    Ok(())
}

fn empty_file() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "empty session file")
}

fn invalid_meta(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid session metadata: {error}"),
    )
}

fn open_session_file(path: &Path, id: &str) -> io::Result<std::fs::File> {
    let file = std::fs::File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            not_found(id)
        } else {
            error
        }
    })?;

    if file.metadata()?.len() == 0 {
        return Err(empty_file());
    }

    Ok(file)
}

fn read_meta_and_message_lines(path: &Path, id: &str) -> io::Result<(SessionMeta, Vec<String>)> {
    let file = open_session_file(path, id)?;
    let reader = io::BufReader::new(file);
    let mut lines = reader.lines();

    let meta_line = lines.next().ok_or_else(empty_file)??;
    let meta = serde_json::from_str(&meta_line).map_err(invalid_meta)?;
    let remaining_lines = lines.collect::<io::Result<Vec<_>>>()?;

    Ok((meta, remaining_lines))
}

fn extract_state_from_lines(lines: &[String], id: &str) -> io::Result<Option<serde_json::Value>> {
    for line in lines {
        if let Some(state) = parse_state_line(line, id)? {
            return Ok(Some(state));
        }
    }

    Ok(None)
}

fn read_meta_with_line_len(path: &Path, id: &str) -> io::Result<(SessionMeta, usize)> {
    let mut first_line = String::new();
    let file = open_session_file(path, id)?;
    let mut reader = io::BufReader::new(file);
    reader.read_line(&mut first_line)?;

    if first_line.is_empty() {
        return Err(empty_file());
    }

    let line_len = first_line.len();
    let meta = serde_json::from_str(first_line.trim_end()).map_err(invalid_meta)?;
    Ok((meta, line_len))
}

fn rewrite_session_file_locked(
    path: &Path,
    meta: &SessionMeta,
    lines: &[String],
) -> io::Result<()> {
    atomic_write_unlocked(path, |writer| {
        write_meta_line(writer, meta, META_LINE_PADDING)?;

        for line in lines {
            if !line.is_empty() {
                writeln!(writer, "{line}")?;
            }
        }
        Ok(())
    })
}

fn preserve_existing_lines(
    path: &Path,
    id: &str,
    should_preserve: impl Fn(&str) -> bool,
) -> io::Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let (_, lines) = read_meta_and_message_lines(path, id)?;
    Ok(lines
        .into_iter()
        .filter(|line| should_preserve(line))
        .collect())
}

fn preserve_for_message_save(line: &str) -> bool {
    match SessionRecord::parse(line) {
        Ok(SessionRecord::State(_)) => true,
        Ok(SessionRecord::Llm(_) | SessionRecord::Custom(_)) => false,
        Err(_) => matches!(
            SessionEntry::parse(line),
            Ok(entry) if !matches!(entry, SessionEntry::Message(_))
        ),
    }
}

fn preserve_for_entry_save(line: &str) -> bool {
    matches!(
        SessionRecord::parse(line),
        Ok(SessionRecord::State(_) | SessionRecord::Custom(_))
    )
}

fn save_messages_with_hooks<AfterValidation, WriteOp>(
    path: &Path,
    id: &str,
    meta: &SessionMeta,
    messages: &[AgentMessage],
    after_validation: AfterValidation,
    write_op: WriteOp,
) -> io::Result<()>
where
    AfterValidation: FnOnce() -> io::Result<()>,
    WriteOp: FnOnce(&Path, &SessionMeta, &[AgentMessage], &str) -> io::Result<()>,
{
    with_target_lock(path, || {
        check_sequence_path(path, id, meta.sequence)?;
        after_validation()?;

        let mut write_meta = meta.clone();
        write_meta.sequence += 1;
        write_op(path, &write_meta, messages, id)
    })
}

fn write_messages_locked(
    path: &Path,
    meta: &SessionMeta,
    messages: &[AgentMessage],
    id: &str,
) -> io::Result<()> {
    let preserved_lines = preserve_existing_lines(path, id, preserve_for_message_save)?;

    write_messages_with_preserved_lines(path, meta, messages, id, &preserved_lines)
}

fn write_messages_with_preserved_lines(
    path: &Path,
    meta: &SessionMeta,
    messages: &[AgentMessage],
    id: &str,
    preserved_lines: &[String],
) -> io::Result<()> {
    atomic_write_unlocked(path, |writer| {
        write_meta_line(writer, meta, META_LINE_PADDING)?;

        for msg in messages {
            if let Some(record) = SessionRecord::from_message(msg, id) {
                writer.write_all(record.to_json_line()?.as_bytes())?;
                writeln!(writer)?;
            }
        }
        for line in preserved_lines {
            if !line.is_empty() {
                writeln!(writer, "{line}")?;
            }
        }
        Ok(())
    })
}

fn write_meta_line(
    writer: &mut impl Write,
    meta: &SessionMeta,
    padding: usize,
) -> io::Result<usize> {
    let line = serde_json::to_string(meta).map_err(io::Error::other)?;
    writer.write_all(line.as_bytes())?;
    for _ in 0..padding {
        writer.write_all(b" ")?;
    }
    writeln!(writer)?;
    Ok(line.len() + padding + 1)
}

fn write_meta_line_in_place(
    file: &mut std::fs::File,
    line: &str,
    existing_line_len: usize,
) -> io::Result<bool> {
    if line.len() + 1 > existing_line_len {
        return Ok(false);
    }

    file.seek(SeekFrom::Start(0))?;
    file.write_all(line.as_bytes())?;
    for _ in line.len()..existing_line_len - 1 {
        file.write_all(b" ")?;
    }
    file.write_all(b"\n")?;
    Ok(true)
}

fn append_records_in_place(
    path: &Path,
    meta: &SessionMeta,
    meta_line_len: usize,
    record_lines: &[String],
) -> io::Result<bool> {
    append_records_in_place_with_hook(path, meta, meta_line_len, record_lines, |_| Ok(()))
}

fn append_records_in_place_with_hook(
    path: &Path,
    meta: &SessionMeta,
    meta_line_len: usize,
    record_lines: &[String],
    after_meta_patch: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<bool> {
    let meta_line = serde_json::to_string(meta).map_err(io::Error::other)?;
    if meta_line.len() + 1 > meta_line_len {
        return Ok(false);
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    write_meta_line_in_place(&mut file, &meta_line, meta_line_len)?;
    file.flush()?;
    after_meta_patch(&mut file)?;

    file.seek(SeekFrom::End(0))?;
    for line in record_lines {
        if !line.is_empty() {
            writeln!(file, "{line}")?;
        }
    }
    file.flush()?;
    Ok(true)
}

fn upsert_state_line(
    lines: &mut Vec<String>,
    session_id: &str,
    state: &serde_json::Value,
) -> io::Result<()> {
    let state_line = SessionRecord::state(state.clone()).to_json_line()?;
    if let Some(line) = find_record_line_mut(lines.as_mut_slice(), session_id, |record| {
        matches!(record, SessionRecord::State(_))
    }) {
        line.clone_from(&state_line);
    } else {
        lines.push(state_line);
    }
    Ok(())
}

fn parse_state_line(line: &str, id: &str) -> io::Result<Option<serde_json::Value>> {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            if line.contains("\"_state\"") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("corrupted state line in session {id}: {error}"),
                ));
            }
            return Ok(None);
        }
    };

    let Some(state_marker) = value.get("_state") else {
        return Ok(None);
    };

    if state_marker.as_bool() != Some(true) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid state marker in session {id}: expected `_state: true`"),
        ));
    }

    Ok(Some(
        value
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ))
}

fn append_records(
    path: &Path,
    id: &str,
    records: impl IntoIterator<Item = SessionRecord>,
) -> io::Result<()> {
    append_records_with_rewrite(path, id, records, rewrite_session_file_locked)
}

fn append_records_with_rewrite<F>(
    path: &Path,
    id: &str,
    records: impl IntoIterator<Item = SessionRecord>,
    rewrite_fn: F,
) -> io::Result<()>
where
    F: FnOnce(&Path, &SessionMeta, &[String]) -> io::Result<()>,
{
    with_target_lock(path, || {
        let (mut meta, meta_line_len) = read_meta_with_line_len(path, id)?;
        meta.updated_at = now_utc();
        meta.sequence += 1;

        let record_lines = records
            .into_iter()
            .map(|record| record.to_json_line())
            .collect::<io::Result<Vec<_>>>()?;

        if append_records_in_place(path, &meta, meta_line_len, &record_lines)? {
            return Ok(());
        }

        let (_, mut existing_lines) = read_meta_and_message_lines(path, id)?;
        existing_lines.extend(record_lines);
        rewrite_fn(path, &meta, &existing_lines)
    })
}

fn find_record_line_mut<'a>(
    lines: &'a mut [String],
    session_id: &str,
    predicate: impl Fn(&SessionRecord) -> bool,
) -> Option<&'a mut String> {
    for line in lines {
        match SessionRecord::parse(line) {
            Ok(record) if predicate(&record) => return Some(line),
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "skipping unparseable line while scanning session {session_id}"
                );
            }
        }
    }
    None
}

/// Validate a session ID, rejecting unsafe filesystem characters.
///
/// Rejects IDs containing `/`, `\`, `..`, `:`, or ASCII control characters.
fn validate_session_id(id: &str) -> io::Result<()> {
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session ID must not be empty",
        ));
    }
    if id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.chars().any(|c| c == ':' || c.is_ascii_control())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("session ID contains unsafe characters: {id:?}"),
        ));
    }
    Ok(())
}

/// JSONL file-based session store.
///
/// Each session is a single `.jsonl` file in the configured directory.
/// Line 1 is [`SessionMeta`] (JSON), lines 2+ are one [`LlmMessage`] each.
///
/// Concurrent writes to the same session may corrupt the file.
/// Callers are expected to enforce single-writer access.
pub struct JsonlSessionStore {
    sessions_dir: PathBuf,
    migrators: Vec<Box<dyn crate::migrate::SessionMigrator>>,
}

impl JsonlSessionStore {
    /// Create a new store rooted at the given directory.
    ///
    /// Creates the directory (and parents) if it does not exist.
    pub fn new(sessions_dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&sessions_dir)?;
        Ok(Self {
            sessions_dir,
            migrators: Vec::new(),
        })
    }

    /// Register session migrators for automatic schema upgrades on load.
    #[must_use]
    pub fn with_migrators(
        mut self,
        migrators: Vec<Box<dyn crate::migrate::SessionMigrator>>,
    ) -> Self {
        self.migrators = migrators;
        self
    }

    /// Default sessions directory: `<config_dir>/swink-agent/sessions`.
    ///
    /// Returns `None` if the platform config directory cannot be determined.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("swink-agent").join("sessions"))
    }

    /// Generate a new unique session ID using a UTC timestamp plus UUID suffix.
    pub fn new_session_id() -> String {
        format_session_id()
    }
}

impl SessionStore for JsonlSessionStore {
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[AgentMessage]) -> io::Result<()> {
        validate_session_id(id)?;
        let path = session_path(&self.sessions_dir, id);
        save_messages_with_hooks(&path, id, meta, messages, || Ok(()), write_messages_locked)
    }

    fn save_full(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
        state: &serde_json::Value,
    ) -> io::Result<SessionMeta> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        with_target_lock(&path, || {
            check_sequence_path(&path, id, meta.sequence)?;

            let mut write_meta = meta.clone();
            write_meta.sequence += 1;

            let mut preserved_lines =
                preserve_existing_lines(&path, id, preserve_for_message_save)?;
            upsert_state_line(&mut preserved_lines, id, state)?;
            write_messages_with_preserved_lines(
                &path,
                &write_meta,
                messages,
                id,
                &preserved_lines,
            )?;

            Ok(write_meta)
        })
    }

    fn append(&self, id: &str, messages: &[AgentMessage]) -> io::Result<()> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        append_records(
            &path,
            id,
            messages
                .iter()
                .filter_map(|msg| SessionRecord::from_message(msg, id)),
        )
    }

    fn load(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        let (meta, lines) = read_meta_and_message_lines(&path, id)?;
        let (meta, classified) = self.classify_and_migrate(meta, lines, id)?;
        Ok((meta, classified_lines_to_messages(classified, registry, id)))
    }

    fn load_full(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>, Option<serde_json::Value>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        with_target_lock(&path, || {
            let (meta, lines) = read_meta_and_message_lines(&path, id)?;
            let state = extract_state_from_lines(&lines, id)?;
            let (meta, classified) = self.classify_and_migrate(meta, lines, id)?;
            let messages = classified_lines_to_messages(classified, registry, id);
            Ok((meta, messages, state))
        })
    }

    fn list(&self) -> io::Result<Vec<SessionMeta>> {
        let mut sessions = Vec::new();

        let entries = std::fs::read_dir(&self.sessions_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let reader = io::BufReader::new(file);
            if let Some(Ok(first_line)) = reader.lines().next() {
                match serde_json::from_str::<SessionMeta>(&first_line) {
                    Ok(meta) => sessions.push(meta),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping session file with invalid metadata"
                        );
                    }
                }
            }
        }

        sessions.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(sessions)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        validate_session_id(id)?;
        with_session_lock(&self.sessions_dir, id, |path, int_path| {
            std::fs::remove_file(path)?;
            // Cascade-delete the interrupt file if it exists.
            if int_path.exists() {
                std::fs::remove_file(int_path)?;
            }
            Ok(())
        })
    }

    fn save_state(&self, id: &str, state: &serde_json::Value) -> io::Result<()> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        with_target_lock(&path, || {
            if !path.exists() {
                return Err(not_found(id));
            }

            let (mut meta, mut lines) = read_meta_and_message_lines(&path, id)?;
            meta.updated_at = now_utc();
            meta.sequence += 1;
            upsert_state_line(&mut lines, id, state)?;
            rewrite_session_file_locked(&path, &meta, &lines)
        })
    }

    fn load_state(&self, id: &str) -> io::Result<Option<serde_json::Value>> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        if !path.exists() {
            return Ok(None);
        }

        let (_, lines) = read_meta_and_message_lines(&path, id)?;
        extract_state_from_lines(&lines, id)
    }

    fn save_interrupt(&self, id: &str, state: &InterruptState) -> io::Result<()> {
        validate_session_id(id)?;
        with_session_lock(&self.sessions_dir, id, |session, path| {
            if !session.exists() {
                return Err(not_found(id));
            }
            atomic_write(path, |writer| {
                serde_json::to_writer_pretty(&mut *writer, state).map_err(io::Error::other)
            })
        })
    }

    fn load_interrupt(&self, id: &str) -> io::Result<Option<InterruptState>> {
        validate_session_id(id)?;
        if !session_path(&self.sessions_dir, id).exists() {
            return Ok(None);
        }
        let path = interrupt_path(&self.sessions_dir, id);
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&path)?;
        let state: InterruptState = serde_json::from_str(&contents).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("corrupted interrupt file for session {id}: {e}"),
            )
        })?;
        Ok(Some(state))
    }

    fn clear_interrupt(&self, id: &str) -> io::Result<()> {
        validate_session_id(id)?;
        with_session_lock(&self.sessions_dir, id, |_session, path| {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            Ok(())
        })
    }

    fn load_with_options(
        &self,
        id: &str,
        options: &LoadOptions,
    ) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
        let (meta, mut entries) = self.load_entries(id)?;

        // Filter by entry type
        if let Some(ref types) = options.entry_types {
            entries.retain(|entry| types.iter().any(|t| t == entry.entry_type_name()));
        }

        // Filter by timestamp (entry timestamps are epoch seconds)
        if let Some(after) = options.after_timestamp {
            let after_secs = after.timestamp().cast_unsigned();
            entries.retain(|entry| entry.timestamp().is_some_and(|ts| ts > after_secs));
        }

        // Truncate to last N
        if let Some(n) = options.last_n_entries
            && entries.len() > n
        {
            entries.drain(..entries.len() - n);
        }

        Ok((meta, entries))
    }
}

impl JsonlSessionStore {
    /// Save a session with rich entry types.
    ///
    /// Lines 2+ are [`SessionEntry`] values serialized with an `entry_type` tag.
    pub fn save_entries(
        &self,
        id: &str,
        meta: &SessionMeta,
        entries: &[SessionEntry],
    ) -> io::Result<()> {
        validate_session_id(id)?;
        let path = session_path(&self.sessions_dir, id);
        with_target_lock(&path, || {
            check_sequence_path(&path, id, meta.sequence)?;

            // Increment sequence for the write
            let mut write_meta = meta.clone();
            write_meta.sequence += 1;
            let preserved_lines = preserve_existing_lines(&path, id, preserve_for_entry_save)?;

            atomic_write_unlocked(&path, |writer| {
                // First line: metadata
                serde_json::to_writer(&mut *writer, &write_meta).map_err(io::Error::other)?;
                writeln!(writer)?;

                // Subsequent lines: one SessionEntry per line
                for entry in entries {
                    serde_json::to_writer(&mut *writer, entry).map_err(io::Error::other)?;
                    writeln!(writer)?;
                }
                for line in &preserved_lines {
                    if !line.is_empty() {
                        writeln!(writer, "{line}")?;
                    }
                }
                Ok(())
            })
        })
    }

    /// Load a session with rich entry types.
    ///
    /// Parses each line after metadata as a [`SessionEntry`]. Old-format lines
    /// (raw `LlmMessage` without `entry_type`) are interpreted as
    /// [`SessionEntry::Message`] for backward compatibility.
    pub fn load_entries(&self, id: &str) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        let (meta, lines) = read_meta_and_message_lines(&path, id)?;
        let (meta, classified) = self.classify_and_migrate(meta, lines, id)?;

        let entries = classified
            .into_iter()
            .filter_map(|item| match item {
                ClassifiedLine::Entry(entry) => Some(*entry),
                ClassifiedLine::Custom(_) | ClassifiedLine::State => None,
            })
            .collect();

        Ok((meta, entries))
    }

    /// Parse every message line, classify each as a migrateable
    /// [`SessionEntry`] or a pass-through custom/state wrapper, then run any
    /// configured migrators over the `SessionEntry` subset.
    ///
    /// This is the single migration entry point shared by `load()` and
    /// `load_entries()` so both observe identical post-migration state.
    /// Positional ordering of pass-through wrappers relative to each other
    /// (and relative to the migrateable block as a whole) is preserved;
    /// migrators may freely insert/remove entries within the entry block.
    fn classify_and_migrate(
        &self,
        mut meta: SessionMeta,
        lines: Vec<String>,
        id: &str,
    ) -> io::Result<(SessionMeta, Vec<ClassifiedLine>)> {
        let mut classified: Vec<ClassifiedLine> = Vec::with_capacity(lines.len());
        for (idx, line) in lines.into_iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let line_num = idx + 2;
            match classify_line(&line) {
                Ok(item) => classified.push(item),
                Err(error) => {
                    tracing::warn!(
                        line = line_num,
                        error = %error,
                        "skipping unparseable line in session {id}"
                    );
                }
            }
        }

        // Split out the migrateable subset, run the migrator pipeline, then
        // weave the migrated entries back into the classified list.
        let original_entry_count = classified
            .iter()
            .filter(|item| matches!(item, ClassifiedLine::Entry(_)))
            .count();
        let mut entries: Vec<SessionEntry> = classified
            .iter()
            .filter_map(|item| match item {
                ClassifiedLine::Entry(entry) => Some((**entry).clone()),
                _ => None,
            })
            .collect();

        if !self.migrators.is_empty() {
            crate::migrate::run_migrations(&mut meta, &mut entries, &self.migrators)?;
        } else if meta.version > crate::migrate::CURRENT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported session version {} (current: {})",
                    meta.version,
                    crate::migrate::CURRENT_VERSION
                ),
            ));
        }

        let rebuilt = weave_migrated_entries(classified, entries, original_entry_count);
        Ok((meta, rebuilt))
    }
}

/// Weave a migrated entry list back into a classified line sequence.
///
/// If the migrator preserved the entry count (`migrated.len() ==
/// original_entry_count`), each entry slot is replaced 1:1 in position,
/// preserving the original interleaving with custom/state wrappers (the
/// common case; matches pre-migration-aware `load()` behavior exactly).
///
/// If the migrator added or dropped entries, positional correspondence to
/// individual slots is no longer defined — we collapse all migrated entries
/// into the first entry slot (or append them if no entry slots existed)
/// and keep pass-through wrappers in their original positions.
fn weave_migrated_entries(
    classified: Vec<ClassifiedLine>,
    migrated: Vec<SessionEntry>,
    original_entry_count: usize,
) -> Vec<ClassifiedLine> {
    let mut rebuilt: Vec<ClassifiedLine> = Vec::with_capacity(classified.len().max(migrated.len()));
    if migrated.len() == original_entry_count {
        let mut iter = migrated.into_iter();
        for item in classified {
            match item {
                ClassifiedLine::Entry(_) => {
                    if let Some(next) = iter.next() {
                        rebuilt.push(ClassifiedLine::Entry(Box::new(next)));
                    }
                }
                passthrough @ (ClassifiedLine::Custom(_) | ClassifiedLine::State) => {
                    rebuilt.push(passthrough);
                }
            }
        }
    } else {
        let mut drained = false;
        let mut migrated_iter = migrated.into_iter();
        for item in classified {
            match item {
                ClassifiedLine::Entry(_) => {
                    if !drained {
                        for e in migrated_iter.by_ref() {
                            rebuilt.push(ClassifiedLine::Entry(Box::new(e)));
                        }
                        drained = true;
                    }
                }
                passthrough @ (ClassifiedLine::Custom(_) | ClassifiedLine::State) => {
                    rebuilt.push(passthrough);
                }
            }
        }
        for e in migrated_iter {
            rebuilt.push(ClassifiedLine::Entry(Box::new(e)));
        }
    }
    rebuilt
}

/// A JSONL line after parsing, classified so the migration pipeline can treat
/// migrateable entries distinctly from opaque pass-through wrappers.
///
/// The `Entry` variant is boxed because [`SessionEntry`] is comparatively
/// large (embeds a full `LlmMessage`) and dwarfs the other variants — the
/// `Box` keeps the enum small enough to avoid `clippy::large_enum_variant`.
#[derive(Debug, Clone)]
enum ClassifiedLine {
    /// A [`SessionEntry`] — either a tagged entry or a legacy raw
    /// [`LlmMessage`] rehydrated as [`SessionEntry::Message`]. Migrators run
    /// against these.
    Entry(Box<SessionEntry>),
    /// A `_custom: true` envelope — preserved verbatim so
    /// [`SessionStore::load`] can restore it via the caller's registry.
    Custom(serde_json::Value),
    /// A `_state: true` record. `load_state()` reads state directly from the
    /// file; `load()` and `load_entries()` both skip these, so the payload
    /// is intentionally discarded here.
    State,
}

/// Classify a single non-empty JSONL message line.
///
/// Order of checks mirrors the persisted format precedence: state/custom
/// wrappers win over the tagged [`SessionEntry`] path, which in turn covers
/// the legacy raw [`LlmMessage`] format.
fn classify_line(line: &str) -> io::Result<ClassifiedLine> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(io::Error::other)?;

    if value.get("_state").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(ClassifiedLine::State);
    }
    if value.get("_custom").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(ClassifiedLine::Custom(value));
    }

    // Neither state nor custom wrapper: either a tagged SessionEntry or a
    // legacy raw LlmMessage. SessionEntry::parse handles both.
    SessionEntry::parse(line)
        .map(|entry| ClassifiedLine::Entry(Box::new(entry)))
        .map_err(io::Error::other)
}

/// Convert a `_custom` envelope (as stored on disk) back into an
/// [`AgentMessage::Custom`] using the supplied registry.
///
/// Returns `Ok(None)` when the caller supplied no registry (custom messages
/// are skipped — matching prior `load()` behavior).
fn custom_envelope_to_message(
    envelope: &serde_json::Value,
    registry: Option<&CustomMessageRegistry>,
) -> io::Result<Option<AgentMessage>> {
    let line = serde_json::to_string(envelope).map_err(io::Error::other)?;
    crate::codec::decode_jsonl_message_line(&line, registry)
}

fn classified_lines_to_messages(
    classified: Vec<ClassifiedLine>,
    registry: Option<&CustomMessageRegistry>,
    id: &str,
) -> Vec<AgentMessage> {
    let mut messages = Vec::new();
    for item in classified {
        match item {
            ClassifiedLine::Entry(entry) => {
                if let SessionEntry::Message(llm_msg) = *entry {
                    messages.push(AgentMessage::Llm(llm_msg));
                }
            }
            ClassifiedLine::Custom(envelope) => {
                match custom_envelope_to_message(&envelope, registry) {
                    Ok(Some(msg)) => messages.push(msg),
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "skipping unrestorable custom message in session {id}"
                        );
                    }
                }
            }
            ClassifiedLine::State => {}
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_id_format() {
        let id = JsonlSessionStore::new_session_id();
        let (timestamp, suffix) = id.rsplit_once('_').unwrap();
        assert_eq!(timestamp.len(), 15);
        assert_eq!(timestamp.as_bytes()[8], b'_');
        assert_eq!(suffix.len(), 32);
    }

    #[test]
    fn validate_session_id_rejects_slash() {
        let err = validate_session_id("foo/bar").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_session_id_rejects_backslash() {
        let err = validate_session_id("foo\\bar").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_session_id_rejects_dotdot() {
        let err = validate_session_id("..secret").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_session_id_rejects_colon() {
        let err = validate_session_id("C:drive").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_session_id_rejects_control_chars() {
        let err = validate_session_id("foo\nbar").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_session_id_rejects_empty() {
        let err = validate_session_id("").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_session_id_accepts_normal() {
        validate_session_id("20250315_120000").unwrap();
        validate_session_id("my-session").unwrap();
        validate_session_id("session_123").unwrap();
    }

    #[test]
    fn save_load_roundtrip_with_custom_messages() {
        use swink_agent::{AgentMessage, CustomMessage, CustomMessageRegistry};

        #[derive(Debug)]
        struct TestCustomMsg {
            data: String,
        }

        impl CustomMessage for TestCustomMsg {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn type_name(&self) -> Option<&str> {
                Some("TestCustomMsg")
            }
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::json!({ "data": self.data }))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = SessionMeta {
            id: "test-full".to_string(),
            title: "Full roundtrip".to_string(),
            created_at: chrono::DateTime::from_timestamp(1_710_500_000, 0)
                .unwrap()
                .to_utc(),
            updated_at: chrono::DateTime::from_timestamp(1_710_500_000, 0)
                .unwrap()
                .to_utc(),
            version: 1,
            sequence: 0,
        };

        let messages: Vec<AgentMessage> = vec![
            AgentMessage::Llm(swink_agent::LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
                timestamp: 100,
                cache_hint: None,
            })),
            AgentMessage::Custom(Box::new(TestCustomMsg {
                data: "custom-payload".to_string(),
            })),
            AgentMessage::Llm(swink_agent::LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "World".to_string(),
                }],
                timestamp: 200,
                cache_hint: None,
            })),
        ];

        store.save("test-full", &meta, &messages).unwrap();

        // Load with registry — custom message restored
        let mut registry = CustomMessageRegistry::new();
        registry.register(
            "TestCustomMsg",
            Box::new(|val: serde_json::Value| {
                let data = val
                    .get("data")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing data".to_string())?;
                Ok(Box::new(TestCustomMsg {
                    data: data.to_string(),
                }) as Box<dyn CustomMessage>)
            }),
        );

        let (loaded_meta, loaded_messages) = store.load("test-full", Some(&registry)).unwrap();
        assert_eq!(loaded_meta.id, "test-full");
        assert_eq!(loaded_messages.len(), 3);
        assert!(matches!(
            loaded_messages[0],
            AgentMessage::Llm(swink_agent::LlmMessage::User(_))
        ));
        assert!(matches!(loaded_messages[1], AgentMessage::Custom(_)));
        assert!(matches!(
            loaded_messages[2],
            AgentMessage::Llm(swink_agent::LlmMessage::User(_))
        ));

        // Verify custom message content via downcast
        let custom = loaded_messages[1].downcast_ref::<TestCustomMsg>().unwrap();
        assert_eq!(custom.data, "custom-payload");

        // Load without registry — custom messages skipped
        let (_, loaded_no_reg) = store.load("test-full", None).unwrap();
        assert_eq!(loaded_no_reg.len(), 2);
        assert!(matches!(loaded_no_reg[0], AgentMessage::Llm(_)));
        assert!(matches!(loaded_no_reg[1], AgentMessage::Llm(_)));
    }

    #[test]
    fn append_preserves_saved_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let now = now_utc();
        let meta = SessionMeta {
            id: "test-state".to_string(),
            title: "State".to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            sequence: 0,
        };

        let initial_messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
            swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            },
        ))];
        store.save("test-state", &meta, &initial_messages).unwrap();
        store
            .save_state("test-state", &serde_json::json!({ "cursor": 1 }))
            .unwrap();

        let appended_messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
            swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "world".to_string(),
                }],
                timestamp: 2,
                cache_hint: None,
            },
        ))];
        store.append("test-state", &appended_messages).unwrap();

        let state = store.load_state("test-state").unwrap();
        assert_eq!(state, Some(serde_json::json!({ "cursor": 1 })));

        let (_, messages) = store.load("test-state", None).unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn load_state_errors_on_corrupted_state_line() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("corrupt-state");
        store
            .save("corrupt-state", &meta, &[user_msg("hello", 1)])
            .unwrap();

        let path = session_path(dir.path(), "corrupt-state");
        let mut contents = std::fs::read_to_string(&path).unwrap();
        contents.push_str("{\"_state\":true,\"data\":\n");
        std::fs::write(&path, contents).unwrap();

        let err = store.load_state("corrupt-state").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("state line"));
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sess.jsonl");
        atomic_write(&target, |w| {
            w.write_all(b"hello\n")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello\n");
        // No leftover temp files in the directory
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                !name.contains(".tmp."),
                "unexpected temp file left behind: {name}"
            );
        }
    }

    #[test]
    fn atomic_write_cleans_up_temp_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sess.jsonl");
        let err = atomic_write(&target, |_w| {
            Err(io::Error::other("simulated mid-write failure"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "simulated mid-write failure");
        // Target must not exist (no zero-length file)
        assert!(!target.exists());
        // And no leftover temp file
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.contains(".tmp."), "temp file not cleaned up: {name}");
        }
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        // Regression: on Windows, std::fs::rename does not replace an existing
        // destination, so rewrites of existing session files would fail.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sess.jsonl");
        std::fs::write(&target, b"old content\n").unwrap();
        atomic_write(&target, |w| {
            w.write_all(b"new content\n")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content\n");
        // No leftover temp files
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.contains(".tmp."), "temp file left behind: {name}");
        }
    }

    #[test]
    fn atomic_write_concurrent_rewrites_of_same_target_do_not_collide() {
        // Regression: the temp file path must be unique per write attempt.
        // If two overlapping rewrites in the same process shared a temp path,
        // they could truncate or rename each other's files nondeterministically.
        use std::sync::Arc;
        use std::sync::Barrier;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(dir.path().join("sess.jsonl"));
        std::fs::write(&*target, b"initial\n").unwrap();

        let n = 8;
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let target = Arc::clone(&target);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                atomic_write(&target, |w| {
                    // Write enough data that a torn rename would be observable.
                    for _ in 0..256 {
                        writeln!(w, "writer-{i}")?;
                    }
                    Ok(())
                })
            }));
        }
        for h in handles {
            h.join().unwrap().expect("concurrent atomic_write failed");
        }

        // Final file must be entirely one writer's content (no interleaving).
        let final_contents = std::fs::read_to_string(&*target).unwrap();
        let first_line = final_contents.lines().next().unwrap();
        assert!(first_line.starts_with("writer-"));
        for line in final_contents.lines() {
            assert_eq!(line, first_line, "file contains interleaved writes");
        }

        // No leftover temp files.
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.contains(".tmp."), "temp file left behind: {name}");
        }
    }

    #[test]
    fn save_preserves_previous_file_when_new_write_fails() {
        // Regression for #234: a failed rewrite must not truncate the live
        // file. With atomic rename, the original content survives.
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let now = now_utc();
        let meta = SessionMeta {
            id: "atomic".to_string(),
            title: "t".to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            sequence: 0,
        };
        let messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
            swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "first".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            },
        ))];
        store.save("atomic", &meta, &messages).unwrap();

        let path = session_path(dir.path(), "atomic");
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(!before.is_empty());

        // Simulate a failed rewrite. With atomic_write, the target file must
        // remain untouched.
        let _ = atomic_write(&path, |_w| Err(io::Error::other("boom")));

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "failed write must not corrupt live file");

        // File still parses cleanly
        let (_, loaded) = store.load("atomic", None).unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn load_reads_message_entries_saved_via_save_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let now = now_utc();
        let meta = SessionMeta {
            id: "entry-messages".to_string(),
            title: "Entry messages".to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            sequence: 0,
        };

        let entries = vec![
            SessionEntry::Message(LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            })),
            SessionEntry::Label {
                text: "bookmark".to_string(),
                message_index: 0,
                timestamp: 2,
            },
            SessionEntry::Message(LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "world".to_string(),
                }],
                timestamp: 3,
                cache_hint: None,
            })),
        ];

        store
            .save_entries("entry-messages", &meta, &entries)
            .unwrap();

        let (_, messages) = store.load("entry-messages", None).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            messages[0],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
        assert!(matches!(
            messages[1],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
    }

    #[test]
    fn save_preserves_state_and_rich_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("preserve-save");
        let entries = vec![
            SessionEntry::Message(LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            })),
            SessionEntry::Label {
                text: "bookmark".to_string(),
                message_index: 0,
                timestamp: 2,
            },
        ];

        store
            .save_entries("preserve-save", &meta, &entries)
            .unwrap();
        store
            .save_state("preserve-save", &serde_json::json!({ "cursor": 7 }))
            .unwrap();

        let (loaded_meta, _) = store.load("preserve-save", None).unwrap();
        store
            .save(
                "preserve-save",
                &loaded_meta,
                &[user_msg("updated", 3), user_msg("again", 4)],
            )
            .unwrap();

        assert_eq!(
            store.load_state("preserve-save").unwrap(),
            Some(serde_json::json!({ "cursor": 7 }))
        );

        let (_, entries) = store.load_entries("preserve-save").unwrap();
        assert_eq!(entries.len(), 3, "messages plus preserved label");
        assert!(matches!(entries[0], SessionEntry::Message(_)));
        assert!(matches!(entries[1], SessionEntry::Message(_)));
        assert!(matches!(entries[2], SessionEntry::Label { .. }));
    }

    #[test]
    fn save_entries_preserve_saved_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("preserve-entry-save");
        store
            .save("preserve-entry-save", &meta, &[user_msg("hello", 1)])
            .unwrap();
        store
            .save_state("preserve-entry-save", &serde_json::json!({ "cursor": 11 }))
            .unwrap();

        let (loaded_meta, _) = store.load("preserve-entry-save", None).unwrap();
        let entries = vec![
            SessionEntry::Message(LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "updated".to_string(),
                }],
                timestamp: 2,
                cache_hint: None,
            })),
            SessionEntry::Label {
                text: "kept".to_string(),
                message_index: 0,
                timestamp: 3,
            },
        ];

        store
            .save_entries("preserve-entry-save", &loaded_meta, &entries)
            .unwrap();

        assert_eq!(
            store.load_state("preserve-entry-save").unwrap(),
            Some(serde_json::json!({ "cursor": 11 }))
        );
    }

    #[test]
    fn save_entries_preserves_existing_custom_message_envelopes() {
        use swink_agent::{CustomMessage, CustomMessageRegistry};

        #[derive(Debug)]
        struct TestCustomMsg {
            data: String,
        }

        impl CustomMessage for TestCustomMsg {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn type_name(&self) -> Option<&str> {
                Some("TestCustomMsg")
            }

            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::json!({ "data": self.data }))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("preserve-entry-custom");
        let messages = vec![
            user_msg("hello", 1),
            AgentMessage::Custom(Box::new(TestCustomMsg {
                data: "custom-payload".to_string(),
            })),
        ];
        store
            .save("preserve-entry-custom", &meta, &messages)
            .unwrap();

        let (loaded_meta, _) = store.load("preserve-entry-custom", None).unwrap();
        let entries = vec![
            SessionEntry::Message(LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "updated".to_string(),
                }],
                timestamp: 2,
                cache_hint: None,
            })),
            SessionEntry::Label {
                text: "kept".to_string(),
                message_index: 0,
                timestamp: 3,
            },
        ];

        store
            .save_entries("preserve-entry-custom", &loaded_meta, &entries)
            .unwrap();

        let mut registry = CustomMessageRegistry::new();
        registry.register(
            "TestCustomMsg",
            Box::new(|val: serde_json::Value| {
                let data = val
                    .get("data")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "missing data".to_string())?;
                Ok(Box::new(TestCustomMsg {
                    data: data.to_string(),
                }) as Box<dyn CustomMessage>)
            }),
        );

        let (_, loaded_messages) = store
            .load("preserve-entry-custom", Some(&registry))
            .unwrap();
        assert_eq!(loaded_messages.len(), 2, "message plus preserved custom");
        assert!(matches!(loaded_messages[0], AgentMessage::Llm(_)));
        let custom = loaded_messages[1].downcast_ref::<TestCustomMsg>().unwrap();
        assert_eq!(custom.data, "custom-payload");
    }

    fn user_msg(text: &str, ts: u64) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(swink_agent::UserMessage {
            content: vec![swink_agent::ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: ts,
            cache_hint: None,
        }))
    }

    fn fresh_meta(id: &str) -> SessionMeta {
        let now = now_utc();
        SessionMeta {
            id: id.to_string(),
            title: "t".to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            sequence: 0,
        }
    }

    fn rewrite_meta_without_padding(path: &Path, id: &str, update: impl FnOnce(&mut SessionMeta)) {
        let (mut meta, lines) = read_meta_and_message_lines(path, id).unwrap();
        update(&mut meta);

        let mut contents = format!("{}\n", serde_json::to_string(&meta).unwrap());
        for line in lines {
            contents.push_str(&line);
            contents.push('\n');
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn append_advances_sequence_and_rejects_stale_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("seq-append");
        store
            .save("seq-append", &meta, &[user_msg("a", 1)])
            .unwrap();
        // After save, on-disk sequence == 1. `meta` still holds 0.

        store.append("seq-append", &[user_msg("b", 2)]).unwrap();
        // After append, on-disk sequence must have advanced to 2.

        // Sanity: list() / load() sees the bumped sequence.
        let (loaded_meta, _) = store.load("seq-append", None).unwrap();
        assert_eq!(loaded_meta.sequence, 2);

        // A stale writer holding the pre-append meta (sequence == 1 after save)
        // should now be rejected by check_sequence.
        let mut stale = meta;
        stale.sequence = 1;
        let err = store
            .save("seq-append", &stale, &[user_msg("c", 3)])
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn append_extends_file_without_rewriting_existing_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("append-in-place");
        store
            .save("append-in-place", &meta, &[user_msg("first", 1)])
            .unwrap();

        let path = session_path(dir.path(), "append-in-place");
        let before = std::fs::read_to_string(&path).unwrap();
        let before_lines = before.lines().collect::<Vec<_>>();
        let before_meta_line_len = before_lines[0].len();
        let before_message_line = before_lines[1].to_string();

        store
            .append("append-in-place", &[user_msg("second", 2)])
            .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let after_lines = after.lines().collect::<Vec<_>>();
        assert_eq!(
            after_lines[0].len(),
            before_meta_line_len,
            "append should patch the reserved metadata line in place"
        );
        assert_eq!(
            after_lines[1], before_message_line,
            "append must leave existing record bytes untouched"
        );
        assert_eq!(after_lines.len(), 3);

        let (loaded_meta, loaded_messages) = store.load("append-in-place", None).unwrap();
        assert_eq!(loaded_meta.sequence, 2);
        assert_eq!(loaded_messages.len(), 2);
    }

    #[test]
    fn append_failure_after_metadata_patch_rejects_stale_save_without_new_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("append-meta-first");
        store
            .save("append-meta-first", &meta, &[user_msg("first", 1)])
            .unwrap();

        let path = session_path(dir.path(), "append-meta-first");
        let (mut append_meta, meta_line_len) =
            read_meta_with_line_len(&path, "append-meta-first").unwrap();
        append_meta.updated_at = now_utc();
        append_meta.sequence += 1;
        let second_line = SessionRecord::from_message(&user_msg("second", 2), "append-meta-first")
            .unwrap()
            .to_json_line()
            .unwrap();

        let err = append_records_in_place_with_hook(
            &path,
            &append_meta,
            meta_line_len,
            &[second_line],
            |_| Err(io::Error::other("simulated record write failure")),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "simulated record write failure");

        let (loaded_meta, loaded_messages) = store.load("append-meta-first", None).unwrap();
        assert_eq!(
            loaded_meta.sequence, 2,
            "metadata sequence must be visible before any appended records"
        );
        assert_eq!(
            loaded_messages.len(),
            1,
            "failed append must not expose uncommitted record lines"
        );

        let mut stale = meta;
        stale.sequence = 1;
        let err = store
            .save("append-meta-first", &stale, &[user_msg("stale", 3)])
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn append_rewrite_failure_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut meta = fresh_meta("append-atomic");
        meta.sequence = 9;
        let path = session_path(dir.path(), "append-atomic");
        let first_line =
            SessionRecord::from_message(&user_msg("first", 1), "append-atomic").unwrap();
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&meta).unwrap(),
                first_line.to_json_line().unwrap()
            ),
        )
        .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let err = append_records_with_rewrite(
            &path,
            "append-atomic",
            [SessionRecord::from_message(&user_msg("second", 2), "append-atomic").unwrap()],
            |_path, _meta, _lines| Err(io::Error::other("simulated append rewrite failure")),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "simulated append rewrite failure");

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            before, after,
            "failed append rewrite must not modify the live session file"
        );

        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let (loaded_meta, loaded_messages) = store.load("append-atomic", None).unwrap();
        assert_eq!(loaded_meta.sequence, 9);
        assert_eq!(loaded_messages.len(), 1);
    }

    #[test]
    fn delete_waits_for_append_lock_and_does_not_allow_resurrection() {
        use std::sync::Arc;
        use std::sync::Barrier;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(JsonlSessionStore::new(dir.path().to_path_buf()).unwrap());
        let meta = fresh_meta("delete-race");
        store
            .save("delete-race", &meta, &[user_msg("first", 1)])
            .unwrap();

        let path = session_path(dir.path(), "delete-race");
        rewrite_meta_without_padding(&path, "delete-race", |meta| meta.sequence = 9);
        let append_ready = Arc::new(Barrier::new(2));
        let allow_rewrite = Arc::new(Barrier::new(2));

        let append_path = path.clone();
        let append_ready_for_thread = Arc::clone(&append_ready);
        let allow_rewrite_for_thread = Arc::clone(&allow_rewrite);
        let append_handle = thread::spawn(move || {
            append_records_with_rewrite(
                &append_path,
                "delete-race",
                [SessionRecord::from_message(&user_msg("second", 2), "delete-race").unwrap()],
                |path, meta, lines| {
                    append_ready_for_thread.wait();
                    allow_rewrite_for_thread.wait();
                    rewrite_session_file_locked(path, meta, lines)
                },
            )
        });

        append_ready.wait();

        let delete_store = Arc::clone(&store);
        let delete_handle = thread::spawn(move || delete_store.delete("delete-race"));

        // If delete bypassed the session lock, it could return before the
        // append rewrites and the session would be recreated after delete.
        allow_rewrite.wait();

        append_handle
            .join()
            .unwrap()
            .expect("append should finish cleanly");
        delete_handle
            .join()
            .unwrap()
            .expect("delete should wait for the append lock");

        assert!(
            !path.exists(),
            "delete must not allow a concurrent append to resurrect the session"
        );
    }

    #[test]
    fn save_state_advances_sequence_and_rejects_stale_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("seq-state");
        store.save("seq-state", &meta, &[user_msg("a", 1)]).unwrap();
        // On-disk sequence == 1.

        store
            .save_state("seq-state", &serde_json::json!({ "cursor": 1 }))
            .unwrap();

        let (loaded_meta, _) = store.load("seq-state", None).unwrap();
        assert_eq!(loaded_meta.sequence, 2);

        // Stale writer with sequence == 1 must be rejected.
        let mut stale = meta;
        stale.sequence = 1;
        let err = store
            .save("seq-state", &stale, &[user_msg("c", 3)])
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn save_full_updates_messages_and_state_with_single_sequence_bump() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("save-full");
        store
            .save("save-full", &meta, &[user_msg("before", 1)])
            .unwrap();
        store
            .save_state("save-full", &serde_json::json!({ "cursor": 1 }))
            .unwrap();

        let (loaded_meta, _) = store.load("save-full", None).unwrap();
        assert_eq!(loaded_meta.sequence, 2);

        let persisted_meta = store
            .save_full(
                "save-full",
                &loaded_meta,
                &[user_msg("after", 2), user_msg("again", 3)],
                &serde_json::json!({ "cursor": 9, "draft": "synced" }),
            )
            .unwrap();

        assert_eq!(
            persisted_meta.sequence, 3,
            "combined transcript+state save should advance sequence once"
        );

        let (reloaded_meta, reloaded_messages) = store.load("save-full", None).unwrap();
        assert_eq!(reloaded_meta.sequence, 3);
        assert_eq!(reloaded_messages.len(), 2);
        assert_eq!(
            store.load_state("save-full").unwrap(),
            Some(serde_json::json!({ "cursor": 9, "draft": "synced" }))
        );
    }

    #[test]
    fn load_full_returns_messages_and_state_from_one_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let persisted_meta = store
            .save_full(
                "load-full",
                &fresh_meta("load-full"),
                &[user_msg("hello", 1), user_msg("again", 2)],
                &serde_json::json!({ "cursor": 4, "draft": "stable" }),
            )
            .unwrap();

        let (loaded_meta, loaded_messages, loaded_state) =
            store.load_full("load-full", None).unwrap();

        assert_eq!(loaded_meta.sequence, persisted_meta.sequence);
        assert_eq!(loaded_messages.len(), 2);
        assert_eq!(
            loaded_state,
            Some(serde_json::json!({ "cursor": 4, "draft": "stable" }))
        );
    }

    #[test]
    fn stale_saves_do_not_both_pass_validation() {
        use std::sync::mpsc;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("stale-race");
        store
            .save("stale-race", &meta, &[user_msg("v1", 1)])
            .unwrap();
        let (stale_meta, _) = store.load("stale-race", None).unwrap();

        let path = session_path(dir.path(), "stale-race");
        let (validated_tx, validated_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();

        let thread_path = path;
        let thread_meta = stale_meta.clone();
        let paused_writer = thread::spawn(move || {
            let messages = vec![user_msg("writer-1", 2)];
            save_messages_with_hooks(
                &thread_path,
                "stale-race",
                &thread_meta,
                &messages,
                || {
                    validated_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    Ok(())
                },
                write_messages_locked,
            )
        });

        validated_rx.recv().unwrap();

        let competitor_messages = vec![user_msg("writer-2", 3)];
        let competitor_meta = stale_meta;
        let competitor =
            thread::spawn(move || store.save("stale-race", &competitor_meta, &competitor_messages));

        resume_tx.send(()).unwrap();

        let first_result = paused_writer.join().unwrap();
        let second_result = competitor.join().unwrap();

        let conflict_count =
            usize::from(first_result.is_err()) + usize::from(second_result.is_err());
        assert_eq!(
            conflict_count, 1,
            "exactly one stale writer should conflict"
        );

        let (loaded_meta, loaded_messages) = JsonlSessionStore::new(dir.path().to_path_buf())
            .unwrap()
            .load("stale-race", None)
            .unwrap();
        assert_eq!(loaded_meta.sequence, 2);
        assert_eq!(loaded_messages.len(), 1);
    }

    #[test]
    fn save_interrupt_requires_existing_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let err = store
            .save_interrupt(
                "missing",
                &InterruptState {
                    interrupted_at: 1,
                    pending_tool_calls: vec![],
                    context_snapshot: vec![],
                    system_prompt: "system".to_string(),
                    model: swink_agent::ModelSpec::new("openai", "gpt-4o"),
                },
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    /// Migrator that transforms the first `User` text content in every
    /// `Message` entry from lower-case to upper-case. Used to detect whether
    /// the migration pipeline actually ran against a given load path.
    struct UppercasingMigrator;

    impl crate::migrate::SessionMigrator for UppercasingMigrator {
        fn source_version(&self) -> u32 {
            0
        }
        fn target_version(&self) -> u32 {
            1
        }
        fn migrate(
            &self,
            _meta: &SessionMeta,
            entries: Vec<SessionEntry>,
        ) -> io::Result<Vec<SessionEntry>> {
            Ok(entries
                .into_iter()
                .map(|entry| match entry {
                    SessionEntry::Message(LlmMessage::User(mut m)) => {
                        for block in &mut m.content {
                            if let swink_agent::ContentBlock::Text { text } = block {
                                *text = text.to_uppercase();
                            }
                        }
                        SessionEntry::Message(LlmMessage::User(m))
                    }
                    other => other,
                })
                .collect())
        }
    }

    /// Write a legacy (`version: 0`) session file containing both a
    /// migrateable raw-`LlmMessage` line AND a custom-message envelope, then
    /// assert that `load()` and `load_entries()` both observe the migrated
    /// shape and that `load()` returns the custom wrapper unchanged.
    ///
    /// Regression for #522: `load()` previously bypassed the configured
    /// migrator pipeline, so it returned the raw pre-migration text while
    /// `load_entries()` returned the migrated text.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn load_applies_migrators_identically_to_load_entries() {
        use swink_agent::{CustomMessage, CustomMessageRegistry};

        #[derive(Debug)]
        struct TestCustomMsg {
            data: String,
        }

        impl CustomMessage for TestCustomMsg {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn type_name(&self) -> Option<&str> {
                Some("TestCustomMsg")
            }
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::json!({ "data": self.data }))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let id = "legacy";

        // Write a legacy-format file by hand: line 1 = meta @ version 0,
        // line 2 = raw LlmMessage (migrateable), line 3 = _custom envelope
        // (pass-through), line 4 = _state wrapper (pass-through, skipped by
        // load but must not break migration).
        let path = session_path(dir.path(), id);
        let now = now_utc();
        let meta = SessionMeta {
            id: id.to_string(),
            title: "legacy".to_string(),
            created_at: now,
            updated_at: now,
            version: 0,
            sequence: 0,
        };

        let raw_msg_line = serde_json::to_string(&LlmMessage::User(swink_agent::UserMessage {
            content: vec![swink_agent::ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 1,
            cache_hint: None,
        }))
        .unwrap();

        let custom_envelope = serde_json::json!({
            "type": "TestCustomMsg",
            "data": { "data": "custom-payload" },
            "_custom": true,
        });
        let custom_line = serde_json::to_string(&custom_envelope).unwrap();

        let state_line = serde_json::json!({
            "_state": true,
            "data": { "cursor": 9 },
        })
        .to_string();

        let contents = format!(
            "{}\n{raw_msg_line}\n{custom_line}\n{state_line}\n",
            serde_json::to_string(&meta).unwrap()
        );
        std::fs::write(&path, contents).unwrap();

        let store = JsonlSessionStore::new(dir.path().to_path_buf())
            .unwrap()
            .with_migrators(vec![Box::new(UppercasingMigrator)]);

        // load_entries() runs migrators — this is the pre-fix baseline.
        let (entries_meta, entries) = store.load_entries(id).unwrap();
        assert_eq!(entries_meta.version, crate::migrate::CURRENT_VERSION);
        let entry_msg = entries
            .iter()
            .find_map(SessionEntry::as_message)
            .expect("message entry present");
        if let LlmMessage::User(user) = entry_msg
            && let swink_agent::ContentBlock::Text { text } = &user.content[0]
        {
            assert_eq!(text, "HELLO", "load_entries must run migrator");
        } else {
            panic!("unexpected entry shape");
        }

        // load() must observe the same post-migration text AND still return
        // the custom wrapper unchanged (requires the registry).
        let mut registry = CustomMessageRegistry::new();
        registry.register(
            "TestCustomMsg",
            Box::new(|val: serde_json::Value| {
                let data = val
                    .get("data")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing data".to_string())?;
                Ok(Box::new(TestCustomMsg {
                    data: data.to_string(),
                }) as Box<dyn CustomMessage>)
            }),
        );

        let (load_meta, messages) = store.load(id, Some(&registry)).unwrap();
        assert_eq!(load_meta.version, crate::migrate::CURRENT_VERSION);

        let llm_text = messages
            .iter()
            .find_map(|m| match m {
                AgentMessage::Llm(LlmMessage::User(u)) => u.content.iter().find_map(|b| match b {
                    swink_agent::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("user message present");
        assert_eq!(
            llm_text, "HELLO",
            "load() must route through the migrator pipeline"
        );

        let custom = messages
            .iter()
            .find_map(|m| m.downcast_ref::<TestCustomMsg>().ok())
            .expect("custom wrapper must pass through load() unchanged");
        assert_eq!(custom.data, "custom-payload");
    }

    /// Without a registered migrator, a legacy-version file must still fail
    /// `load()` with the same error that `load_entries()` emits — contract
    /// consistency between the two APIs.
    #[test]
    fn load_rejects_legacy_version_without_migrator_like_load_entries() {
        let dir = tempfile::tempdir().unwrap();
        let id = "no-migrator";
        let path = session_path(dir.path(), id);
        let now = now_utc();
        // A future version (> CURRENT_VERSION) triggers the unsupported-
        // version error in both paths.
        let meta = SessionMeta {
            id: id.to_string(),
            title: "future".to_string(),
            created_at: now,
            updated_at: now,
            version: crate::migrate::CURRENT_VERSION + 1,
            sequence: 0,
        };
        let contents = format!("{}\n", serde_json::to_string(&meta).unwrap());
        std::fs::write(&path, contents).unwrap();

        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let load_err = store.load(id, None).unwrap_err();
        let entries_err = store.load_entries(id).unwrap_err();
        assert_eq!(load_err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(entries_err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_interrupt_ignores_orphan_file_without_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let orphan_path = interrupt_path(dir.path(), "orphan");
        std::fs::write(
            &orphan_path,
            serde_json::to_string(&InterruptState {
                interrupted_at: 2,
                pending_tool_calls: vec![],
                context_snapshot: vec![],
                system_prompt: "system".to_string(),
                model: swink_agent::ModelSpec::new("openai", "gpt-4o"),
            })
            .unwrap(),
        )
        .unwrap();

        assert!(store.load_interrupt("orphan").unwrap().is_none());
    }
}
