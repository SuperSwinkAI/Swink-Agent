//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].
//!
//! Concurrent writes to the same session may corrupt the file.
//! Callers are expected to enforce single-writer access.

use std::io::{self, BufRead, Read as _, Write};
use std::path::PathBuf;

use swink_agent::LlmMessage;

use crate::meta::SessionMeta;
use crate::store::SessionStore;
use crate::time::{format_session_id, now_utc};

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
}

impl JsonlSessionStore {
    /// Create a new store rooted at the given directory.
    ///
    /// Creates the directory (and parents) if it does not exist.
    pub fn new(sessions_dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&sessions_dir)?;
        Ok(Self { sessions_dir })
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
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()> {
        validate_session_id(id)?;

        let path = self.sessions_dir.join(format!("{id}.jsonl"));

        let file = std::fs::File::create(&path)?;
        let mut writer = io::BufWriter::new(file);

        // First line: metadata
        serde_json::to_writer(&mut writer, meta).map_err(io::Error::other)?;
        writeln!(writer)?;

        // Subsequent lines: one LlmMessage per line
        for msg in messages {
            serde_json::to_writer(&mut writer, msg).map_err(io::Error::other)?;
            writeln!(writer)?;
        }

        writer.flush()?;
        Ok(())
    }

    fn append(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()> {
        validate_session_id(id)?;

        let path = self.sessions_dir.join(format!("{id}.jsonl"));

        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("session not found: {id}"),
            ));
        }

        // Read existing meta to update updated_at
        let existing_file = std::fs::File::open(&path)?;
        let reader = io::BufReader::new(existing_file);
        let first_line = reader
            .lines()
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty session file"))??;
        let mut meta: SessionMeta =
            serde_json::from_str(&first_line).map_err(io::Error::other)?;
        meta.updated_at = now_utc();

        // Rewrite line 1 with updated meta, keep existing message lines, then append new ones
        let mut all_content = String::new();
        {
            let file = std::fs::File::open(&path)?;
            let mut buf_reader = io::BufReader::new(file);
            buf_reader.read_to_string(&mut all_content)?;
        }

        // Split into lines, replace first line
        let lines: Vec<&str> = all_content.lines().collect();
        let new_meta_line = serde_json::to_string(&meta).map_err(io::Error::other)?;

        // Build new file content
        let mut file = std::fs::File::create(&path)?;
        // Write updated meta
        writeln!(file, "{new_meta_line}")?;
        // Write existing message lines (skip first line which was meta)
        for line in lines.iter().skip(1) {
            if !line.is_empty() {
                writeln!(file, "{line}")?;
            }
        }
        // Append new messages
        for msg in messages {
            let json = serde_json::to_string(msg).map_err(io::Error::other)?;
            writeln!(file, "{json}")?;
        }

        file.flush()?;
        Ok(())
    }

    fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)> {
        validate_session_id(id)?;

        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        let file = std::fs::File::open(&path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                io::Error::new(io::ErrorKind::NotFound, format!("session not found: {id}"))
            } else {
                e
            }
        })?;

        // Check for empty file
        let file_meta = file.metadata()?;
        if file_meta.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty session file",
            ));
        }

        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();

        // First line: metadata
        let meta_line = lines
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty session file"))??;
        let meta: SessionMeta =
            serde_json::from_str(&meta_line).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("invalid session metadata: {e}"))
            })?;

        // Remaining lines: LlmMessages with corruption tolerance
        let mut messages = Vec::new();
        for (line_num, line_result) in lines.enumerate() {
            let line = line_result?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<LlmMessage>(&line) {
                Ok(msg) => messages.push(msg),
                Err(e) => {
                    tracing::warn!(
                        line = line_num + 2,
                        error = %e,
                        "skipping corrupted message line in session {id}"
                    );
                }
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
        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        std::fs::remove_file(path)
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
}
