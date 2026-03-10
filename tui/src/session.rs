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
            io::Error::new(io::ErrorKind::NotFound, "could not determine config directory")
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
            if let Some(Ok(first_line)) = reader.lines().next() {
                if let Ok(meta) = serde_json::from_str::<SessionMeta>(&first_line) {
                    sessions.push(meta);
                }
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
        let meta: SessionMeta =
            serde_json::from_str(&meta_line).map_err(io::Error::other)?;

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
        let days_since_epoch = secs / 86400;
        let time_of_day = secs % 86400;

        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
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
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
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
