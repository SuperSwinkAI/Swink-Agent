//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use swink_agent::{AgentMessage, LlmMessage};

use crate::meta::SessionMeta;
use crate::store::SessionStore;
use crate::time::{days_to_ymd, unix_now};

/// JSONL file-based session store.
///
/// Each session is a single `.jsonl` file in the configured directory.
/// Line 1 is [`SessionMeta`] (JSON), lines 2+ are one [`LlmMessage`] each.
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
}

impl SessionStore for JsonlSessionStore {
    fn save(
        &self,
        id: &str,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
    ) -> io::Result<()> {
        let path = self.sessions_dir.join(format!("{id}.jsonl"));

        // Read existing metadata to preserve created_at if the file already exists.
        let created_at = std::fs::File::open(&path)
            .ok()
            .and_then(|f| {
                let reader = io::BufReader::new(f);
                reader.lines().next()
            })
            .and_then(Result::ok)
            .and_then(|line| serde_json::from_str::<SessionMeta>(&line).ok())
            .map_or_else(unix_now, |meta| meta.created_at);

        // Collect only Llm variants.
        let llm_messages: Vec<&LlmMessage> = messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm),
                AgentMessage::Custom(_) => None,
            })
            .collect();

        let meta = SessionMeta {
            id: id.to_string(),
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            created_at,
            updated_at: unix_now(),
            message_count: llm_messages.len(),
            custom: serde_json::Value::Null,
        };

        let file = std::fs::File::create(&path)?;
        let mut writer = io::BufWriter::new(file);

        // First line: metadata
        serde_json::to_writer(&mut writer, &meta).map_err(io::Error::other)?;
        writeln!(writer)?;

        // Subsequent lines: one LlmMessage per line
        for msg in &llm_messages {
            serde_json::to_writer(&mut writer, msg).map_err(io::Error::other)?;
            writeln!(writer)?;
        }

        writer.flush()?;
        Ok(())
    }

    fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        let file = std::fs::File::open(&path)?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();

        // First line: metadata
        let meta_line = lines
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty session file"))??;
        let meta: SessionMeta = serde_json::from_str(&meta_line).map_err(io::Error::other)?;

        // Remaining lines: LlmMessages
        let mut messages = Vec::new();
        for line in lines {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let llm: LlmMessage = serde_json::from_str(&line).map_err(io::Error::other)?;
            messages.push(AgentMessage::Llm(llm));
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

            let file = std::fs::File::open(&path)?;
            let reader = io::BufReader::new(file);
            if let Some(Ok(first_line)) = reader.lines().next()
                && let Ok(meta) = serde_json::from_str::<SessionMeta>(&first_line)
            {
                sessions.push(meta);
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        std::fs::remove_file(path)
    }

    fn new_session_id(&self) -> String {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();

        let days_since_epoch = secs / 86_400;
        let time_of_day = secs % 86_400;

        let hours = time_of_day / 3_600;
        let minutes = (time_of_day % 3_600) / 60;
        let seconds = time_of_day % 60;

        let (year, month, day) = days_to_ymd(days_since_epoch);

        format!("{year:04}{month:02}{day:02}_{hours:02}{minutes:02}{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_id_format() {
        let store = JsonlSessionStore {
            sessions_dir: PathBuf::from("/tmp"),
        };
        let id = store.new_session_id();
        // Should be YYYYMMDD_HHMMSS format: 15 chars with underscore at index 8
        assert_eq!(id.len(), 15);
        assert_eq!(id.as_bytes()[8], b'_');
        for (i, ch) in id.chars().enumerate() {
            if i == 8 {
                assert_eq!(ch, '_');
            } else {
                assert!(
                    ch.is_ascii_digit(),
                    "char at index {i} should be a digit, got {ch}"
                );
            }
        }
    }
}
