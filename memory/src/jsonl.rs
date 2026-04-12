//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].
//!
//! Concurrent writes to the same session may corrupt the file.
//! Callers are expected to enforce single-writer access.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use swink_agent::atomic_fs::{atomic_write, atomic_write_unlocked, with_target_lock};
use swink_agent::{AgentMessage, CustomMessageRegistry, LlmMessage};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;
use crate::store::SessionStore;
use crate::time::{format_session_id, now_utc};

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
        serde_json::to_writer(&mut *writer, meta).map_err(io::Error::other)?;
        writeln!(writer)?;

        for line in lines {
            if !line.is_empty() {
                writeln!(writer, "{line}")?;
            }
        }
        Ok(())
    })
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
    atomic_write_unlocked(path, |writer| {
        serde_json::to_writer(&mut *writer, meta).map_err(io::Error::other)?;
        writeln!(writer)?;

        for msg in messages {
            if let Some(record) = SessionRecord::from_message(msg, id) {
                writer.write_all(record.to_json_line()?.as_bytes())?;
                writeln!(writer)?;
            }
        }
        Ok(())
    })
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
        let (mut meta, _) = read_meta_with_line_len(path, id)?;
        meta.updated_at = now_utc();
        meta.sequence += 1;

        let record_lines = records
            .into_iter()
            .map(|record| record.to_json_line())
            .collect::<io::Result<Vec<_>>>()?;

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

fn parse_message_record(
    line: &str,
    line_num: usize,
    id: &str,
    registry: Option<&CustomMessageRegistry>,
) -> Option<AgentMessage> {
    match crate::codec::decode_jsonl_message_line(line, registry) {
        Ok(message) => message,
        Err(error) => {
            tracing::warn!(
                line = line_num,
                error = %error,
                "skipping unparseable message line in session {id}"
            );
            None
        }
    }
}

/// Validate a session ID, rejecting unsafe filesystem characters.
///
/// Rejects IDs containing `/`, `\`, `..`, or null bytes.
fn validate_session_id(id: &str) -> io::Result<()> {
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session ID must not be empty",
        ));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") || id.contains('\0') {
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

    /// Generate a new unique session ID using `YYYYMMDD_HHMMSS` format.
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

        // Check version
        if meta.version > crate::migrate::CURRENT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported session version {} (current: {})",
                    meta.version,
                    crate::migrate::CURRENT_VERSION
                ),
            ));
        }

        // Parse lines using dual strategy: try SessionRecord first (handles
        // raw LlmMessage and _custom/_state markers), then fall back to
        // SessionEntry parsing (handles entry_type-tagged lines from
        // save_entries). This ensures backward and forward compatibility.
        let mut messages = Vec::new();
        for (line_num, line) in lines.into_iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            // Try SessionRecord parse first (raw format)
            if let Some(message) = parse_message_record(&line, line_num + 2, id, registry) {
                messages.push(message);
                continue;
            }
            // Fall back to SessionEntry parse (tagged format)
            if let Ok(SessionEntry::Message(llm_msg)) = SessionEntry::parse(&line) {
                messages.push(AgentMessage::Llm(llm_msg));
            }
        }

        Ok((meta, messages))
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

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        validate_session_id(id)?;
        let path = session_path(&self.sessions_dir, id);
        std::fs::remove_file(path)?;
        // Cascade-delete the interrupt file if it exists
        let int_path = interrupt_path(&self.sessions_dir, id);
        if int_path.exists() {
            std::fs::remove_file(int_path)?;
        }
        Ok(())
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
            let state_line = SessionRecord::state(state.clone()).to_json_line()?;

            if let Some(line) = find_record_line_mut(&mut lines, id, |record| {
                matches!(record, SessionRecord::State(_))
            }) {
                line.clone_from(&state_line);
            } else {
                lines.push(state_line);
            }
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
        for line in lines {
            if let Ok(SessionRecord::State(state)) = SessionRecord::parse(&line) {
                return Ok(Some(state));
            }
        }

        Ok(None)
    }

    fn save_interrupt(&self, id: &str, state: &InterruptState) -> io::Result<()> {
        validate_session_id(id)?;
        let session = session_path(&self.sessions_dir, id);
        if !session.exists() {
            return Err(not_found(id));
        }
        let path = interrupt_path(&self.sessions_dir, id);
        atomic_write(&path, |writer| {
            serde_json::to_writer_pretty(&mut *writer, state).map_err(io::Error::other)
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
        let path = interrupt_path(&self.sessions_dir, id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
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

            atomic_write_unlocked(&path, |writer| {
                // First line: metadata
                serde_json::to_writer(&mut *writer, &write_meta).map_err(io::Error::other)?;
                writeln!(writer)?;

                // Subsequent lines: one SessionEntry per line
                for entry in entries {
                    serde_json::to_writer(&mut *writer, entry).map_err(io::Error::other)?;
                    writeln!(writer)?;
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
        let (mut meta, lines) = read_meta_and_message_lines(&path, id)?;

        let mut entries: Vec<SessionEntry> = lines
            .into_iter()
            .enumerate()
            .filter_map(|(line_num, line)| {
                if line.trim().is_empty() {
                    return None;
                }
                match SessionEntry::parse(&line) {
                    Ok(entry) => Some(entry),
                    Err(error) => {
                        tracing::warn!(
                            line = line_num + 2,
                            error = %error,
                            "skipping unparseable entry in session {id}"
                        );
                        None
                    }
                }
            })
            .collect();

        // Run migrations if needed
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

        Ok((meta, entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_id_format() {
        let id = JsonlSessionStore::new_session_id();
        assert_eq!(id.len(), 15);
        assert_eq!(id.as_bytes()[8], b'_');
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
    fn validate_session_id_rejects_null() {
        let err = validate_session_id("foo\0bar").unwrap_err();
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
    fn append_rewrite_failure_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let meta = fresh_meta("append-atomic");
        store
            .save("append-atomic", &meta, &[user_msg("first", 1)])
            .unwrap();

        let path = session_path(dir.path(), "append-atomic");
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

        let (loaded_meta, loaded_messages) = store.load("append-atomic", None).unwrap();
        assert_eq!(loaded_meta.sequence, 1);
        assert_eq!(loaded_messages.len(), 1);
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

        let thread_path = path.clone();
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
        let competitor_meta = stale_meta.clone();
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
