//! Session persistence — save and load conversations as JSONL files.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use agent_harness::{AgentMessage, LlmMessage};

/// Metadata stored as the first line of a session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier (timestamp-based).
    pub id: String,
    /// Model identifier used in this session.
    pub model: String,
    /// System prompt for the session.
    pub system_prompt: String,
    /// Unix timestamp when the session was created.
    pub created_at: u64,
    /// Unix timestamp when the session was last updated.
    pub updated_at: u64,
    /// Number of LLM messages in the session.
    pub message_count: usize,
}

/// Session manager for JSONL-based persistence.
pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    /// Create a new session manager, ensuring the sessions directory exists.
    pub fn new() -> io::Result<Self> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine config directory",
            )
        })?;
        let sessions_dir = config_dir.join("agent-harness").join("sessions");
        std::fs::create_dir_all(&sessions_dir)?;
        Ok(Self { sessions_dir })
    }

    /// List all saved sessions, sorted by last updated (newest first).
    pub fn list_sessions(&self) -> io::Result<Vec<SessionMeta>> {
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

    /// Save the current session.
    pub fn save_session(
        &self,
        session_id: &str,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
    ) -> io::Result<()> {
        let path = self.sessions_dir.join(format!("{session_id}.jsonl"));

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
            id: session_id.to_string(),
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            created_at,
            updated_at: unix_now(),
            message_count: llm_messages.len(),
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

    /// Load a session, returning metadata and messages.
    pub fn load_session(&self, session_id: &str) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        let path = self.sessions_dir.join(format!("{session_id}.jsonl"));
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

    /// Delete a session file.
    ///
    /// Reserved for future use by session management UI (e.g. #delete command).
    #[allow(dead_code)]
    pub fn delete_session(&self, session_id: &str) -> io::Result<()> {
        let path = self.sessions_dir.join(format!("{session_id}.jsonl"));
        std::fs::remove_file(path)
    }

    /// Generate a new unique session ID using a timestamp-based format.
    pub fn new_session_id() -> String {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();

        // Convert Unix timestamp to YYYYMMDD_HHMMSS without chrono.
        // Days from epoch calculation.
        let days_since_epoch = secs / 86_400;
        let time_of_day = secs % 86_400;

        let hours = time_of_day / 3_600;
        let minutes = (time_of_day % 3_600) / 60;
        let seconds = time_of_day % 60;

        let (year, month, day) = days_to_ymd(days_since_epoch);

        format!("{year:04}{month:02}{day:02}_{hours:02}{minutes:02}{seconds:02}")
    }
}

/// Convert days since Unix epoch to (year, month, day).
const fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Civil calendar algorithm from Howard Hinnant.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Current Unix timestamp in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_to_ymd_unix_epoch() {
        // Day 0 = 1970-01-01
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_dates() {
        // 2000-01-01 is day 10957 from epoch
        let (y, m, d) = days_to_ymd(10_957);
        assert_eq!((y, m, d), (2000, 1, 1));

        // 2024-02-29 (leap day) is day 19782
        let (y, m, d) = days_to_ymd(19_782);
        assert_eq!((y, m, d), (2024, 2, 29));

        // 2025-03-15 is day 20162
        let (y, m, d) = days_to_ymd(20_162);
        assert_eq!((y, m, d), (2025, 3, 15));
    }

    #[test]
    fn days_to_ymd_end_of_year() {
        // 1970-12-31 is day 364
        let (y, m, d) = days_to_ymd(364);
        assert_eq!((y, m, d), (1970, 12, 31));
    }

    #[test]
    fn session_meta_serialization_roundtrip() {
        let meta = SessionMeta {
            id: "20250315_120000".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: "You are helpful.".to_string(),
            created_at: 1_710_500_000,
            updated_at: 1_710_500_100,
            message_count: 5,
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SessionMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, meta.id);
        assert_eq!(deserialized.model, meta.model);
        assert_eq!(deserialized.system_prompt, meta.system_prompt);
        assert_eq!(deserialized.created_at, meta.created_at);
        assert_eq!(deserialized.updated_at, meta.updated_at);
        assert_eq!(deserialized.message_count, meta.message_count);
    }

    #[test]
    fn new_session_id_format() {
        let id = SessionManager::new_session_id();
        // Should be YYYYMMDD_HHMMSS format: 15 chars with underscore at index 8
        assert_eq!(id.len(), 15);
        assert_eq!(id.as_bytes()[8], b'_');
        // All other chars should be digits
        for (i, ch) in id.chars().enumerate() {
            if i == 8 {
                assert_eq!(ch, '_');
            } else {
                assert!(ch.is_ascii_digit(), "char at index {i} should be a digit, got {ch}");
            }
        }
    }

    #[test]
    fn session_manager_save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionManager {
            sessions_dir: tmp.path().to_path_buf(),
        };

        let session_id = "test_session_001";
        let model = "test-model";
        let system_prompt = "Be concise.";

        // Create a minimal set of messages (empty for simplicity).
        let messages: Vec<AgentMessage> = Vec::new();

        manager
            .save_session(session_id, model, system_prompt, &messages)
            .unwrap();

        let (meta, loaded_messages) = manager.load_session(session_id).unwrap();
        assert_eq!(meta.id, session_id);
        assert_eq!(meta.model, model);
        assert_eq!(meta.system_prompt, system_prompt);
        assert_eq!(meta.message_count, 0);
        assert!(loaded_messages.is_empty());
    }

    #[test]
    fn list_sessions_sorted_by_updated_at() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionManager {
            sessions_dir: tmp.path().to_path_buf(),
        };

        let messages: Vec<AgentMessage> = Vec::new();

        // Save two sessions — the second one will have a later updated_at
        // because unix_now() advances between calls.
        manager
            .save_session("session_old", "model-a", "prompt-a", &messages)
            .unwrap();

        // Manually write the second session with a known higher timestamp
        let meta2 = SessionMeta {
            id: "session_new".to_string(),
            model: "model-b".to_string(),
            system_prompt: "prompt-b".to_string(),
            created_at: 9_999_999_999,
            updated_at: 9_999_999_999,
            message_count: 0,
        };
        let path2 = tmp.path().join("session_new.jsonl");
        let content = serde_json::to_string(&meta2).unwrap() + "\n";
        std::fs::write(&path2, content).unwrap();

        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        // Newest first
        assert_eq!(sessions[0].id, "session_new");
        assert_eq!(sessions[1].id, "session_old");
    }

    #[test]
    fn delete_session_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionManager {
            sessions_dir: tmp.path().to_path_buf(),
        };

        let messages: Vec<AgentMessage> = Vec::new();
        manager
            .save_session("to_delete", "model", "prompt", &messages)
            .unwrap();

        assert!(tmp.path().join("to_delete.jsonl").exists());
        manager.delete_session("to_delete").unwrap();
        assert!(!tmp.path().join("to_delete.jsonl").exists());
    }
}
