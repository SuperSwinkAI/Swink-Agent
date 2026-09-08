//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].
//!
//! Concurrent writes to the same session may corrupt the file.
//! Callers are expected to enforce single-writer access.

use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use swink_agent::atomic_fs::{atomic_write, atomic_write_unlocked, with_target_lock};
use swink_agent::{AgentMessage, CustomMessageRegistry, LlmMessage};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;
#[cfg(not(feature = "search"))]
use crate::search;
use crate::search::{SessionHit, SessionSearchOptions};
use crate::store::SessionStore;
use crate::time::{format_session_id, now_utc};

const META_LINE_PADDING: usize = 64;

#[derive(Debug, Clone)]
enum SessionRecord {
    Llm(Box<LlmMessage>),
    Custom(serde_json::Value),
    AppendBegin,
    Meta(Box<SessionMeta>),
    State(serde_json::Value),
}

impl SessionRecord {
    fn from_message(message: &AgentMessage, session_id: &str) -> Option<Self> {
        let line = crate::codec::encode_jsonl_message_line(message, session_id)?;
        match Self::parse(&line).ok()? {
            Self::Llm(llm) => Some(Self::Llm(llm)),
            Self::Custom(envelope) => Some(Self::Custom(envelope)),
            Self::AppendBegin | Self::Meta(_) | Self::State(_) => None,
        }
    }

    const fn state(state: serde_json::Value) -> Self {
        Self::State(state)
    }

    fn to_json_line(&self) -> io::Result<String> {
        match self {
            Self::Llm(message) => serde_json::to_string(message).map_err(io::Error::other),
            Self::Custom(envelope) => serde_json::to_string(envelope).map_err(io::Error::other),
            Self::AppendBegin => serde_json::to_string(&serde_json::json!({
                "_append": true,
                "phase": "begin"
            }))
            .map_err(io::Error::other),
            Self::Meta(meta) => serde_json::to_string(&serde_json::json!({
                "_meta": true,
                "data": meta
            }))
            .map_err(io::Error::other),
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
        if value.get("_append").and_then(serde_json::Value::as_bool) == Some(true) {
            return Ok(Self::AppendBegin);
        }
        if value.get("_meta").and_then(serde_json::Value::as_bool) == Some(true) {
            return serde_json::from_value::<SessionMeta>(
                value
                    .get("data")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
            .map(Box::new)
            .map(Self::Meta)
            .map_err(io::Error::other);
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
    let mut reader = io::BufReader::new(file);

    let meta_bytes = read_raw_line(&mut reader)?.ok_or_else(empty_file)?;
    let meta_line = String::from_utf8(meta_bytes).map_err(invalid_meta)?;
    let mut meta =
        canonical_meta_for_id(serde_json::from_str(&meta_line).map_err(invalid_meta)?, id);

    // Read message lines as raw bytes and validate UTF-8 per line. A crash
    // mid-append can truncate the final line inside a multi-byte UTF-8
    // sequence; `BufRead::lines()` would surface that as an `io::Error` and
    // abort the whole load before any per-line record classification runs.
    // Instead, skip the corrupt line with a warning so the rest of the
    // session is recovered (spec 021 FR-004: partial recovery over total
    // failure, same as bad-JSON lines in `classify_and_migrate`).
    let mut remaining_lines = Vec::new();
    let mut pending_append_lines: Option<Vec<String>> = None;
    let mut line_num = 1_usize;
    while let Some(raw) = read_raw_line(&mut reader)? {
        line_num += 1;
        let line = match String::from_utf8(raw) {
            Ok(line) => line,
            Err(error) => {
                tracing::warn!(
                    line = line_num,
                    error = %error,
                    "skipping invalid-UTF-8 line in session {id} \
                     (likely truncated by a crash mid-write)"
                );
                continue;
            }
        };
        match SessionRecord::parse(&line) {
            Ok(SessionRecord::AppendBegin) => {
                if pending_append_lines.is_some() {
                    tracing::warn!("discarding uncommitted nested append records in session {id}");
                }
                pending_append_lines = Some(Vec::new());
            }
            Ok(SessionRecord::Meta(meta_update)) => {
                if let Some(mut lines) = pending_append_lines.take() {
                    remaining_lines.append(&mut lines);
                }
                meta = canonical_meta_for_id(*meta_update, id);
            }
            Ok(SessionRecord::Llm(_) | SessionRecord::Custom(_) | SessionRecord::State(_))
            | Err(_) => {
                if let Some(lines) = pending_append_lines.as_mut() {
                    lines.push(line);
                } else {
                    remaining_lines.push(line);
                }
            }
        }
    }
    if pending_append_lines.is_some() {
        tracing::warn!("discarding uncommitted append records in session {id}");
    }

    Ok((meta, remaining_lines))
}

/// Read one `\n`-terminated line as raw bytes, without requiring valid UTF-8.
///
/// Returns `Ok(None)` at EOF. A trailing `\n` (and preceding `\r`, if any) is
/// stripped, matching [`BufRead::lines`] semantics. The final line is
/// returned even when it lacks a terminating newline (e.g. a write cut short
/// by a crash).
fn read_raw_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    if reader.read_until(b'\n', &mut buf)? == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    Ok(Some(buf))
}

fn extract_state_from_lines(lines: &[String], id: &str) -> io::Result<Option<serde_json::Value>> {
    for line in lines {
        if let Some(state) = parse_state_line(line, id)? {
            return Ok(Some(state));
        }
    }

    Ok(None)
}

/// Reads the authoritative [`SessionMeta`] and the byte length of the padded
/// first line (used for in-place metadata patches).
///
/// The append-in-place protocol commits metadata by appending a
/// [`SessionRecord::Meta`] record at the tail and only afterwards patches the
/// padded first line, so after a crash the first line may be stale: the LAST
/// Meta record in the file wins, falling back to the first line when no Meta
/// record exists. Meta records fully replace each other (no merging), and
/// lines that fail to parse (e.g. a torn tail line) are skipped.
fn read_meta_with_line_len(path: &Path, id: &str) -> io::Result<(SessionMeta, usize)> {
    let mut first_line = String::new();
    let file = open_session_file(path, id)?;
    let mut reader = io::BufReader::new(file);
    reader.read_line(&mut first_line)?;

    if first_line.is_empty() {
        return Err(empty_file());
    }

    let line_len = first_line.len();
    let meta = canonical_meta_for_id(
        serde_json::from_str(first_line.trim_end()).map_err(invalid_meta)?,
        id,
    );
    let mut file = reader.into_inner();
    match find_last_meta_commit(&mut file, line_len as u64)? {
        Some(meta_update) => Ok((canonical_meta_for_id(meta_update, id), line_len)),
        None => Ok((meta, line_len)),
    }
}

/// Initial tail-window size for [`find_last_meta_commit`]'s backward scan.
const META_TAIL_WINDOW: u64 = 64 * 1024;

/// Finds the most recent committed [`SessionRecord::Meta`] record after the
/// first line, or `None` if the file contains no parseable Meta record.
///
/// Because every committed append ends with a Meta commit record, the record
/// we want is almost always within the last few bytes of the file. Rather
/// than forward-parsing every line (O(file size), which made
/// `SessionStore::list` a full-corpus scan), this reads geometrically growing
/// tail windows (starting at [`META_TAIL_WINDOW`]) and scans their complete
/// lines in reverse; when a window grows to cover the whole body it falls
/// back to the historical forward line-by-line scan, so the result — last
/// parseable Meta wins, unparseable lines skipped — is identical.
///
/// An invalid-UTF-8 line (e.g. a multi-byte character torn by a crash
/// mid-write) is skipped with a warning like any other unparseable line,
/// matching the tolerant forward scan (spec 021 FR-004 addendum / issue
/// #1067). Invalid UTF-8 that lies before the tail window of a large file
/// goes unnoticed when a later Meta commit is found, favoring recovery of
/// the committed metadata.
fn find_last_meta_commit(
    file: &mut std::fs::File,
    body_start: u64,
) -> io::Result<Option<SessionMeta>> {
    let file_len = file.metadata()?.len();
    let body_len = file_len.saturating_sub(body_start);

    let mut window = META_TAIL_WINDOW;
    while window < body_len {
        // `window < body_len` guarantees `start > body_start`.
        let start = file_len - window;
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; usize::try_from(window).map_err(io::Error::other)?];
        file.read_exact(&mut buf)?;

        // The window starts mid-line: bytes before the first newline are a
        // partial line (they may even split a multi-byte character at the
        // window boundary), so only the bytes after it are complete lines.
        if let Some(pos) = buf.iter().position(|&b| b == b'\n')
            && let Some(meta) = last_meta_in_complete_lines(&buf[pos + 1..])
        {
            return Ok(Some(meta));
        }

        window = window.saturating_mul(2);
    }

    // Terminal case: the remaining window covers the whole body. Stream
    // forward line-by-line exactly like the historical implementation so
    // memory stays bounded, tolerating invalid-UTF-8 lines the same way
    // the full-load forward scan does.
    file.seek(SeekFrom::Start(body_start))?;
    let mut meta = None;
    let mut reader = io::BufReader::new(file);
    while let Some(raw) = read_raw_line(&mut reader)? {
        let Ok(line) = String::from_utf8(raw) else {
            tracing::warn!(
                "skipping invalid-UTF-8 line in session meta scan \
                 (likely truncated by a crash mid-write)"
            );
            continue;
        };
        if let Ok(SessionRecord::Meta(meta_update)) = SessionRecord::parse(&line) {
            meta = Some(*meta_update);
        }
    }
    Ok(meta)
}

/// Scans complete lines in reverse for the last parseable Meta record.
///
/// `bytes` must start at a real line boundary and extend to end of file.
fn last_meta_in_complete_lines(bytes: &[u8]) -> Option<SessionMeta> {
    // Validate UTF-8 per line: an invalid-UTF-8 line (e.g. a torn tail line
    // that truncates a multi-byte character) is skipped like any other
    // unparseable line, matching the tolerant forward scan (spec 021 FR-004
    // addendum / issue #1067).
    for line in bytes.split(|&b| b == b'\n').rev() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            tracing::warn!(
                "skipping invalid-UTF-8 line in session meta scan \
                 (likely truncated by a crash mid-write)"
            );
            continue;
        };
        if let Ok(SessionRecord::Meta(meta_update)) = SessionRecord::parse(text) {
            return Some(*meta_update);
        }
    }
    None
}

fn canonical_meta_for_id(mut meta: SessionMeta, id: &str) -> SessionMeta {
    meta.id = id.to_string();
    meta
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
        Ok(
            SessionRecord::AppendBegin
            | SessionRecord::Llm(_)
            | SessionRecord::Custom(_)
            | SessionRecord::Meta(_),
        ) => false,
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
        write_meta.id = id.to_string();
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
    after_commit_append: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<bool> {
    append_records_in_place_with_hooks(
        path,
        meta,
        meta_line_len,
        record_lines,
        |_| Ok(()),
        after_commit_append,
    )
}

fn append_records_in_place_with_hooks(
    path: &Path,
    meta: &SessionMeta,
    meta_line_len: usize,
    record_lines: &[String],
    after_records_append: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    after_commit_append: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<bool> {
    let meta_line = serde_json::to_string(meta).map_err(io::Error::other)?;
    if meta_line.len() + 1 > meta_line_len {
        return Ok(false);
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;

    file.seek(SeekFrom::End(0))?;
    writeln!(file, "{}", SessionRecord::AppendBegin.to_json_line()?)?;
    for line in record_lines {
        if !line.is_empty() {
            writeln!(file, "{line}")?;
        }
    }
    file.flush()?;
    after_records_append(&mut file)?;
    writeln!(
        file,
        "{}",
        SessionRecord::Meta(Box::new(meta.clone())).to_json_line()?
    )?;
    file.flush()?;
    after_commit_append(&mut file)?;

    write_meta_line_in_place(&mut file, &meta_line, meta_line_len)?;
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
///
/// With the `search` feature enabled, a tantivy index is maintained in
/// `<sessions_dir>/.search_index/` and used by
/// [`SessionStore::search()`].  Without the feature, search falls back to a
/// linear scan of all JSONL files.
pub struct JsonlSessionStore {
    sessions_dir: PathBuf,
    migrators: Vec<Box<dyn crate::migrate::SessionMigrator>>,
    /// Lazily-opened tantivy index.  Populated on first `search()` call (or
    /// via `open_search_index()`).  Only present when the `search` feature is
    /// enabled.
    ///
    /// Uses `Mutex<Option<...>>` rather than `OnceLock` because we need
    /// fallible initialization (`OnceLock::get_or_try_init` is not stable).
    #[cfg(feature = "search")]
    tantivy_index: std::sync::Mutex<Option<crate::search::index::TantivyIndex>>,
    /// Tracks whether the index has been initially populated from all JSONL
    /// files on first search.
    #[cfg(feature = "search")]
    index_built: std::sync::Mutex<bool>,
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
            #[cfg(feature = "search")]
            tantivy_index: std::sync::Mutex::new(None),
            #[cfg(feature = "search")]
            index_built: std::sync::Mutex::new(false),
        })
    }

    /// Open (or create) the tantivy search index eagerly.
    ///
    /// Normally the index is opened lazily on the first `search()` call.
    /// Call this method if you want to detect index-creation errors at startup
    /// rather than at search time.
    ///
    /// Only available with the `search` feature.
    #[cfg(feature = "search")]
    pub fn open_search_index(&self) -> io::Result<()> {
        self.with_tantivy_index(|_| Ok(()))
    }

    /// Build (or rebuild) the tantivy index from all current JSONL files.
    ///
    /// Existing index data is replaced.  Use this after bulk-importing sessions
    /// outside of the store API.
    ///
    /// Only available with the `search` feature.
    #[cfg(feature = "search")]
    pub fn rebuild_search_index(&self) -> io::Result<()> {
        self.with_tantivy_index(|index| {
            // Wipe all existing docs so sessions removed outside the store API
            // don't produce ghost hits after the rebuild.
            index.clear_all()?;
            for meta in self.list()? {
                let (_, entries) = self.load_entries(&meta.id)?;
                index.index_session(&meta, &entries)?;
            }
            Ok(())
        })
    }

    /// Obtain a snapshot clone of the tantivy index (creating it if needed),
    /// then call `f` with it.
    ///
    /// Using a clone of `TantivyIndex` is safe because it is `Arc`-based
    /// internally.
    #[cfg(feature = "search")]
    fn with_tantivy_index<T>(
        &self,
        f: impl FnOnce(&crate::search::index::TantivyIndex) -> io::Result<T>,
    ) -> io::Result<T> {
        let index_clone = {
            let mut guard = self.tantivy_index.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some(crate::search::index::TantivyIndex::open_or_create(
                    &self.sessions_dir,
                )?);
            }
            guard.as_ref().expect("just set").clone()
        };
        f(&index_clone)
    }

    #[cfg(feature = "search")]
    fn active_tantivy_index(&self) -> Option<crate::search::index::TantivyIndex> {
        self.tantivy_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Best-effort full re-index of one session in an already-open index.
    ///
    /// Reloads the session from disk and replaces every document for it, so
    /// this is O(session).  It is the right tool for rewrite paths (`save`,
    /// `save_full`) where existing content may have changed wholesale;
    /// append paths must use [`Self::append_to_active_search_index`] instead.
    #[cfg(feature = "search")]
    fn refresh_active_search_index(&self, id: &str, operation: &str) {
        let Some(index) = self.active_tantivy_index() else {
            return;
        };

        match self.load_entries(id) {
            Ok((meta, entries)) => {
                if let Err(err) = index.index_session(&meta, &entries) {
                    tracing::warn!(
                        session_id = %id,
                        error = %err,
                        operation,
                        "failed to update search index after session mutation"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    session_id = %id,
                    error = %err,
                    operation,
                    "failed to load session for search index refresh"
                );
            }
        }
    }

    /// Best-effort incremental index update for append paths.
    ///
    /// Adds documents for only the newly appended `entries` instead of
    /// reloading and re-indexing the whole session, making the index cost of
    /// an append O(new entries) rather than O(session).  Rewrite paths must
    /// keep using [`Self::refresh_active_search_index`] for its replacement
    /// semantics.
    #[cfg(feature = "search")]
    fn append_to_active_search_index(
        &self,
        id: &str,
        meta: &SessionMeta,
        entries: &[SessionEntry],
    ) {
        let Some(index) = self.active_tantivy_index() else {
            return;
        };
        if let Err(err) = index.append_session_entries(meta, entries) {
            tracing::warn!(
                session_id = %id,
                error = %err,
                "failed to append entries to search index after session append"
            );
        }
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
        save_messages_with_hooks(&path, id, meta, messages, || Ok(()), write_messages_locked)?;
        #[cfg(feature = "search")]
        self.refresh_active_search_index(id, "save");
        Ok(())
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
        let persisted_meta = with_target_lock(&path, || {
            check_sequence_path(&path, id, meta.sequence)?;

            let mut write_meta = meta.clone();
            write_meta.id = id.to_string();
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
        })?;

        #[cfg(feature = "search")]
        self.refresh_active_search_index(id, "save_full");

        Ok(persisted_meta)
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
        )?;

        // Index only the newly appended messages (best-effort). Custom
        // messages are not `SessionEntry`s and were never indexed by the
        // previous full-refresh either, so only LLM messages are converted.
        #[cfg(feature = "search")]
        if self.active_tantivy_index().is_some() {
            let entries: Vec<SessionEntry> = messages
                .iter()
                .filter_map(|msg| match msg {
                    AgentMessage::Llm(llm) => Some(SessionEntry::Message(llm.clone())),
                    // Custom (and any future) variants are not `SessionEntry`s
                    // and were never indexed by the previous full refresh.
                    _ => None,
                })
                .collect();
            if !entries.is_empty() {
                match read_meta_with_line_len(&path, id) {
                    Ok((meta, _)) => self.append_to_active_search_index(id, &meta, &entries),
                    Err(err) => tracing::warn!(
                        session_id = %id,
                        error = %err,
                        "failed to read session meta for search index append"
                    ),
                }
            }
        }
        Ok(())
    }

    fn load(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        with_target_lock(&path, || {
            let (meta, lines) = read_meta_and_message_lines(&path, id)?;
            let (meta, classified) = self.classify_and_migrate(meta, lines, id)?;
            Ok((meta, classified_lines_to_messages(classified, registry, id)))
        })
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

            let read_meta = with_target_lock(&path, || {
                let Some(file_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    return Ok(None);
                };
                validate_session_id(file_id)?;
                read_meta_with_line_len(&path, file_id)
                    .map(|(meta, _)| Some((file_id.to_string(), meta)))
            });

            match read_meta {
                Ok(Some((file_id, meta))) => {
                    sessions.push(canonical_meta_for_id(meta, &file_id));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "skipping unreadable session file"
                    );
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
        })?;

        // Remove from tantivy index (best-effort).
        #[cfg(feature = "search")]
        {
            let id_owned = id.to_string();
            let _ = self.with_tantivy_index(|index| {
                if let Err(err) = index.delete_session(&id_owned) {
                    tracing::warn!(
                        session_id = %id_owned,
                        error = %err,
                        "failed to remove session from search index after delete"
                    );
                }
                Ok(())
            });
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

        with_target_lock(&path, || {
            let (_, lines) = read_meta_and_message_lines(&path, id)?;
            extract_state_from_lines(&lines, id)
        })
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
        with_session_lock(&self.sessions_dir, id, |session, path| {
            if !session.exists() || !path.exists() {
                return Ok(None);
            }
            let contents = std::fs::read_to_string(path)?;
            let state: InterruptState = serde_json::from_str(&contents).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("corrupted interrupt file for session {id}: {e}"),
                )
            })?;
            Ok(Some(state))
        })
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

    fn search(&self, query: &str, options: &SessionSearchOptions) -> io::Result<Vec<SessionHit>> {
        // When the `search` feature is enabled, delegate to the tantivy index.
        // The index is built lazily on the first call and kept warm for
        // subsequent calls.
        #[cfg(feature = "search")]
        return self.with_tantivy_index(|index| {
            self.build_index_if_empty(index)?;
            index.search(query, options)
        });

        // Fallback linear scan (no `search` feature).
        #[cfg(not(feature = "search"))]
        linear_search(self, query, options)
    }
}

/// Linear (no-index) search implementation used when the `search` feature is
/// not enabled.
#[cfg(not(feature = "search"))]
fn linear_search(
    store: &JsonlSessionStore,
    query: &str,
    options: &SessionSearchOptions,
) -> io::Result<Vec<SessionHit>> {
    let terms = search::query_terms(query);
    let limit = options.limit();
    if terms.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let session_ids = if let Some(ids) = &options.session_ids {
        ids.iter()
            .map(|id| {
                validate_session_id(id)?;
                Ok(id.clone())
            })
            .collect::<io::Result<Vec<_>>>()?
    } else {
        store
            .list()?
            .into_iter()
            .map(|meta| meta.id)
            .collect::<Vec<_>>()
    };

    let mut hits = Vec::new();
    for id in session_ids {
        let (meta, entries) = store.load_entries(&id)?;
        for entry in entries {
            if !search::entry_matches_type(&entry, options)
                || !search::entry_matches_time_range(&entry, options)
            {
                continue;
            }
            let Some((relevance, snippet)) = search::search_entry(&entry, &terms) else {
                continue;
            };
            hits.push(SessionHit::new(
                meta.id.clone(),
                meta.title.clone(),
                entry,
                relevance,
                snippet,
            ));
        }
    }

    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.entry.timestamp().cmp(&left.entry.timestamp()))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    hits.truncate(limit);
    Ok(hits)
}

#[cfg(feature = "search")]
impl JsonlSessionStore {
    /// Index all sessions if the initial build has not yet been done.
    ///
    /// This provides lazy-build-on-first-search semantics: the first `search()`
    /// call populates the index from all existing JSONL files; subsequent calls
    /// find the index already populated.
    fn build_index_if_empty(&self, index: &crate::search::index::TantivyIndex) -> io::Result<()> {
        let already_built = {
            let guard = self.index_built.lock().unwrap_or_else(|e| e.into_inner());
            *guard
        };
        if already_built {
            return Ok(());
        }
        // Populate from all current sessions.
        for meta in self.list()? {
            let (_, entries) = self.load_entries(&meta.id)?;
            index.index_session(&meta, &entries)?;
        }
        *self.index_built.lock().unwrap_or_else(|e| e.into_inner()) = true;
        Ok(())
    }
}

impl JsonlSessionStore {
    /// Save a session with rich entry types.
    ///
    /// Lines 2+ are [`SessionEntry`] values serialized with an `entry_type` tag.
    ///
    /// When the `search` feature is enabled, the tantivy index is updated
    /// after the file is written (best-effort — index errors are logged but
    /// do not fail the save).
    pub fn save_entries(
        &self,
        id: &str,
        meta: &SessionMeta,
        entries: &[SessionEntry],
    ) -> io::Result<()> {
        validate_session_id(id)?;
        let path = session_path(&self.sessions_dir, id);
        #[allow(unused_variables)]
        let write_meta = with_target_lock(&path, || {
            check_sequence_path(&path, id, meta.sequence)?;

            // Increment sequence for the write
            let mut write_meta = meta.clone();
            write_meta.id = id.to_string();
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
            })?;
            Ok(write_meta)
        })?;

        // Update tantivy index (best-effort).
        #[cfg(feature = "search")]
        {
            let entries_for_index: Vec<_> = entries.to_vec();
            let meta_for_index = write_meta;
            let _ = self.with_tantivy_index(|index| {
                if let Err(err) = index.index_session(&meta_for_index, &entries_for_index) {
                    tracing::warn!(
                        session_id = %id,
                        error = %err,
                        "failed to update search index after save_entries"
                    );
                }
                Ok(())
            });
        }

        Ok(())
    }

    /// Append entries to an existing session without rewriting the whole file.
    ///
    /// This is the append-only counterpart to [`Self::save_entries`]. Instead of
    /// rewriting every record on each turn, it appends only the new `entries`
    /// and patches the metadata line in place (reusing the same on-disk
    /// machinery and locking guarantees as the internal append path).
    ///
    /// The caller's `meta.sequence` must match the on-disk sequence
    /// (optimistic concurrency, identical to [`Self::save_entries`]); a mismatch
    /// returns an error. Every other `meta` field is written as provided —
    /// only `id` and `sequence` are set by the store. The returned
    /// [`SessionMeta`] carries the incremented sequence so callers stay in
    /// sync without re-reading the file.
    ///
    /// If the metadata line can no longer be patched in place — its serialized
    /// form would outgrow the slot reserved on disk — this transparently falls
    /// back to a full rewrite that preserves all existing lines.
    ///
    /// Errors if the session file does not exist; use [`Self::save_entries`] to
    /// create a new session.
    ///
    /// When the `search` feature is enabled, only the newly appended
    /// `entries` are added to the tantivy index afterwards — the session's
    /// existing documents are untouched (best-effort — index errors are
    /// logged but do not fail the append).
    pub fn append_entries(
        &self,
        id: &str,
        meta: &SessionMeta,
        entries: &[SessionEntry],
    ) -> io::Result<SessionMeta> {
        validate_session_id(id)?;
        let path = session_path(&self.sessions_dir, id);

        let write_meta = with_target_lock(&path, || {
            check_sequence_path(&path, id, meta.sequence)?;
            let (_, meta_line_len) = read_meta_with_line_len(&path, id)?;

            let mut write_meta = meta.clone();
            write_meta.id = id.to_string();
            write_meta.sequence += 1;

            let record_lines = entries
                .iter()
                .map(|entry| serde_json::to_string(entry).map_err(io::Error::other))
                .collect::<io::Result<Vec<_>>>()?;

            if !append_records_in_place(&path, &write_meta, meta_line_len, &record_lines)? {
                // The patched metadata line would outgrow its on-disk slot —
                // fall back to a full rewrite that keeps every existing line.
                let (_, mut existing_lines) = read_meta_and_message_lines(&path, id)?;
                existing_lines.extend(record_lines);
                rewrite_session_file_locked(&path, &write_meta, &existing_lines)?;
            }

            Ok(write_meta)
        })?;

        #[cfg(feature = "search")]
        self.append_to_active_search_index(id, &write_meta, entries);

        Ok(write_meta)
    }

    /// Load a session with rich entry types.
    ///
    /// Parses each line after metadata as a [`SessionEntry`]. Old-format lines
    /// (raw `LlmMessage` without `entry_type`) are interpreted as
    /// [`SessionEntry::Message`] for backward compatibility.
    pub fn load_entries(&self, id: &str) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        with_target_lock(&path, || {
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
        })
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

        crate::migrate::run_migrations(&mut meta, &mut entries, &self.migrators)?;

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
#[path = "jsonl_tests.rs"]
mod tests;
