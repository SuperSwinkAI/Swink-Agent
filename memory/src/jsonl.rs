//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].
//!
//! Concurrent writes to the same session may corrupt the file.
//! Callers are expected to enforce single-writer access.

use std::io::{self, BufRead, Seek, Write};
use std::path::{Path, PathBuf};

use swink_agent::{
    AgentMessage, CustomMessageRegistry, LlmMessage, deserialize_custom_message,
    serialize_custom_message,
};

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
        match message {
            AgentMessage::Llm(llm) => Some(Self::Llm(Box::new(llm.clone()))),
            AgentMessage::Custom(custom) => {
                let mut envelope = serialize_custom_message(custom.as_ref())?;
                envelope
                    .as_object_mut()
                    .expect("custom message envelope must be an object")
                    .insert("_custom".to_string(), serde_json::Value::Bool(true));
                Some(Self::Custom(envelope))
            }
        }
        .or_else(|| {
            if let AgentMessage::Custom(custom) = message {
                tracing::warn!(
                    "skipping non-serializable CustomMessage in session {session_id}: {:?}",
                    custom
                );
            }
            None
        })
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
                value.get("data").cloned().unwrap_or(serde_json::Value::Null),
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

fn not_found(id: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("session not found: {id}"))
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

fn rewrite_session_file(path: &Path, meta: &SessionMeta, lines: &[String]) -> io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);

    serde_json::to_writer(&mut writer, meta).map_err(io::Error::other)?;
    writeln!(writer)?;

    for line in lines {
        if !line.is_empty() {
            writeln!(writer, "{line}")?;
        }
    }

    writer.flush()?;
    Ok(())
}

fn parse_message_record(
    line: &str,
    line_num: usize,
    id: &str,
    registry: Option<&CustomMessageRegistry>,
) -> Option<AgentMessage> {
    match SessionRecord::parse(line) {
        Ok(SessionRecord::Llm(message)) => Some(AgentMessage::Llm(*message)),
        Ok(SessionRecord::Custom(envelope)) => registry.and_then(|reg| {
            match deserialize_custom_message(reg, &envelope) {
                Ok(custom) => Some(AgentMessage::Custom(custom)),
                Err(error) => {
                    tracing::warn!(
                        line = line_num,
                        error = %error,
                        "skipping unrestorable custom message in session {id}"
                    );
                    None
                }
            }
        }),
        Ok(SessionRecord::State(_)) => None,
        Err(error) => {
            tracing::warn!(
                line = line_num,
                error = %error,
                "skipping unparseable line in session {id}"
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

        let path = session_path(&self.sessions_dir, id);

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

        let path = session_path(&self.sessions_dir, id);

        if !path.exists() {
            return Err(not_found(id));
        }

        // Read only line 1 to get current meta and its byte length.
        let mut first_line = String::new();
        {
            let file = std::fs::File::open(&path)?;
            let mut reader = io::BufReader::new(file);
            reader.read_line(&mut first_line)?;
        }
        if first_line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty session file",
            ));
        }
        // Byte length of original line 1 including its newline
        let old_line1_bytes = first_line.len();
        let first_line_trimmed = first_line.trim_end();

        let mut meta: SessionMeta =
            serde_json::from_str(first_line_trimmed).map_err(io::Error::other)?;
        meta.updated_at = now_utc();

        let new_meta_line = serde_json::to_string(&meta).map_err(io::Error::other)?;
        // New line 1 including newline
        let new_line1 = format!("{new_meta_line}\n");

        if new_line1.len() == old_line1_bytes {
            // Fast path: meta line is same byte length — overwrite in place, then append.
            let mut file = std::fs::OpenOptions::new().write(true).open(&path)?;
            file.seek(io::SeekFrom::Start(0))?;
            file.write_all(new_line1.as_bytes())?;
            file.seek(io::SeekFrom::End(0))?;
            for msg in messages {
                let json = SessionRecord::Llm(Box::new(msg.clone())).to_json_line()?;
                writeln!(file, "{json}")?;
            }
            file.flush()?;
        } else {
            let (_, mut lines) = read_meta_and_message_lines(&path, id)?;
            for msg in messages {
                lines.push(SessionRecord::Llm(Box::new(msg.clone())).to_json_line()?);
            }
            rewrite_session_file(&path, &meta, &lines)?;
        }

        Ok(())
    }

    fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        let (meta, lines) = read_meta_and_message_lines(&path, id)?;

        // Remaining lines: LlmMessages with corruption tolerance
        let mut messages = Vec::new();
        for (line_num, line) in lines.into_iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match SessionRecord::parse(&line) {
                Ok(SessionRecord::Llm(message)) => messages.push(*message),
                Ok(SessionRecord::Custom(_) | SessionRecord::State(_)) => {
                    tracing::warn!(
                        line = line_num + 2,
                        "skipping non-llm record in session {id}"
                    );
                }
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
        let path = session_path(&self.sessions_dir, id);
        std::fs::remove_file(path)
    }

    fn save_full(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
    ) -> io::Result<()> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);

        let file = std::fs::File::create(&path)?;
        let mut writer = io::BufWriter::new(file);

        // First line: metadata
        serde_json::to_writer(&mut writer, meta).map_err(io::Error::other)?;
        writeln!(writer)?;

        // Subsequent lines: one message per line
        for msg in messages {
            if let Some(record) = SessionRecord::from_message(msg, id) {
                writer.write_all(record.to_json_line()?.as_bytes())?;
                writeln!(writer)?;
            }
        }

        writer.flush()?;
        Ok(())
    }

    fn load_full(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        let (meta, lines) = read_meta_and_message_lines(&path, id)?;

        // Remaining lines: LlmMessage or custom message envelopes
        let mut messages = Vec::new();
        for (line_num, line) in lines.into_iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(message) = parse_message_record(&line, line_num + 2, id, registry) {
                messages.push(message);
            }
        }

        Ok((meta, messages))
    }

    fn save_state(&self, id: &str, state: &serde_json::Value) -> io::Result<()> {
        validate_session_id(id)?;

        let path = session_path(&self.sessions_dir, id);
        if !path.exists() {
            return Err(not_found(id));
        }

        let (meta, mut lines) = read_meta_and_message_lines(&path, id)?;
        let state_line = SessionRecord::state(state.clone()).to_json_line()?;

        // Find existing _state line and replace, or append.
        let mut found = false;
        for line in &mut lines {
            if matches!(SessionRecord::parse(line), Ok(SessionRecord::State(_))) {
                line.clone_from(&state_line);
                found = true;
                break;
            }
        }
        if !found {
            lines.push(state_line);
        }
        rewrite_session_file(&path, &meta, &lines)
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
    fn save_full_load_full_roundtrip_with_custom_messages() {
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

        store.save_full("test-full", &meta, &messages).unwrap();

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

        let (loaded_meta, loaded_messages) =
            store.load_full("test-full", Some(&registry)).unwrap();
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
        let custom = loaded_messages[1]
            .downcast_ref::<TestCustomMsg>()
            .unwrap();
        assert_eq!(custom.data, "custom-payload");

        // Load without registry — custom messages skipped
        let (_, loaded_no_reg) = store.load_full("test-full", None).unwrap();
        assert_eq!(loaded_no_reg.len(), 2);
        assert!(matches!(loaded_no_reg[0], AgentMessage::Llm(_)));
        assert!(matches!(loaded_no_reg[1], AgentMessage::Llm(_)));

        // Regular load() still works (skips custom lines with warning)
        let (_, llm_only) = store.load("test-full").unwrap();
        assert_eq!(llm_only.len(), 2);
    }
}
