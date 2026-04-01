//! JSONL-based session persistence.
//!
//! Stores each session as a `.jsonl` file: the first line contains
//! [`SessionMeta`], and subsequent lines each contain one [`LlmMessage`].
//!
//! Concurrent writes to the same session may corrupt the file.
//! Callers are expected to enforce single-writer access.

use std::io::{self, BufRead, Read as _, Seek, Write};
use std::path::PathBuf;

use swink_agent::{
    AgentMessage, CustomMessageRegistry, LlmMessage, deserialize_custom_message,
    serialize_custom_message,
};

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
                let json = serde_json::to_string(msg).map_err(io::Error::other)?;
                writeln!(file, "{json}")?;
            }
            file.flush()?;
        } else {
            // Slow path: meta line length changed — full rewrite required.
            let mut all_content = String::new();
            {
                let file = std::fs::File::open(&path)?;
                let mut buf_reader = io::BufReader::new(file);
                buf_reader.read_to_string(&mut all_content)?;
            }

            let lines: Vec<&str> = all_content.lines().collect();

            let mut file = std::fs::File::create(&path)?;
            writeln!(file, "{new_meta_line}")?;
            for line in lines.iter().skip(1) {
                if !line.is_empty() {
                    writeln!(file, "{line}")?;
                }
            }
            for msg in messages {
                let json = serde_json::to_string(msg).map_err(io::Error::other)?;
                writeln!(file, "{json}")?;
            }
            file.flush()?;
        }

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
        let meta: SessionMeta = serde_json::from_str(&meta_line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid session metadata: {e}"),
            )
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

    fn save_full(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
    ) -> io::Result<()> {
        validate_session_id(id)?;

        let path = self.sessions_dir.join(format!("{id}.jsonl"));

        let file = std::fs::File::create(&path)?;
        let mut writer = io::BufWriter::new(file);

        // First line: metadata
        serde_json::to_writer(&mut writer, meta).map_err(io::Error::other)?;
        writeln!(writer)?;

        // Subsequent lines: one message per line
        for msg in messages {
            match msg {
                AgentMessage::Llm(llm) => {
                    serde_json::to_writer(&mut writer, llm).map_err(io::Error::other)?;
                    writeln!(writer)?;
                }
                AgentMessage::Custom(custom) => {
                    if let Some(mut envelope) = serialize_custom_message(custom.as_ref()) {
                        // Tag the envelope so load_full can distinguish it from LlmMessage
                        envelope
                            .as_object_mut()
                            .expect("envelope is an object")
                            .insert("_custom".to_string(), serde_json::Value::Bool(true));
                        serde_json::to_writer(&mut writer, &envelope)
                            .map_err(io::Error::other)?;
                        writeln!(writer)?;
                    } else {
                        tracing::warn!(
                            "skipping non-serializable CustomMessage in session {id}: {:?}",
                            custom
                        );
                    }
                }
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

        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        let file = std::fs::File::open(&path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                io::Error::new(io::ErrorKind::NotFound, format!("session not found: {id}"))
            } else {
                e
            }
        })?;

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
        let meta: SessionMeta = serde_json::from_str(&meta_line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid session metadata: {e}"),
            )
        })?;

        // Remaining lines: LlmMessage or custom message envelopes
        let mut messages = Vec::new();
        for (line_num, line_result) in lines.enumerate() {
            let line = line_result?;
            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as a JSON value first to check for _custom tag
            match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(value) => {
                    if value.get("_custom").and_then(serde_json::Value::as_bool) == Some(true) {
                        // Custom message envelope
                        if let Some(reg) = registry {
                            match deserialize_custom_message(reg, &value) {
                                Ok(custom) => messages.push(AgentMessage::Custom(custom)),
                                Err(e) => {
                                    tracing::warn!(
                                        line = line_num + 2,
                                        error = %e,
                                        "skipping unrestorable custom message in session {id}"
                                    );
                                }
                            }
                        }
                        // If no registry, silently skip custom messages
                    } else {
                        // Standard LlmMessage
                        match serde_json::from_value::<LlmMessage>(value) {
                            Ok(msg) => messages.push(AgentMessage::Llm(msg)),
                            Err(e) => {
                                tracing::warn!(
                                    line = line_num + 2,
                                    error = %e,
                                    "skipping corrupted message line in session {id}"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        line = line_num + 2,
                        error = %e,
                        "skipping unparseable line in session {id}"
                    );
                }
            }
        }

        Ok((meta, messages))
    }

    fn save_state(&self, id: &str, state: &serde_json::Value) -> io::Result<()> {
        validate_session_id(id)?;

        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("session not found: {id}"),
            ));
        }

        // Read all lines, replace or append the state line.
        let content = std::fs::read_to_string(&path)?;
        let mut lines: Vec<String> = content.lines().map(String::from).collect();

        let state_line = serde_json::to_string(&serde_json::json!({
            "_state": true,
            "data": state
        }))
        .map_err(io::Error::other)?;

        // Find existing _state line and replace, or append.
        let mut found = false;
        for line in &mut lines {
            if line.contains("\"_state\"")
                && serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| v.get("_state").and_then(serde_json::Value::as_bool))
                    == Some(true)
            {
                line.clone_from(&state_line);
                found = true;
                break;
            }
        }
        if !found {
            lines.push(state_line);
        }

        let mut file = std::fs::File::create(&path)?;
        for line in &lines {
            writeln!(file, "{line}")?;
        }
        file.flush()?;
        Ok(())
    }

    fn load_state(&self, id: &str) -> io::Result<Option<serde_json::Value>> {
        validate_session_id(id)?;

        let path = self.sessions_dir.join(format!("{id}.jsonl"));
        if !path.exists() {
            return Ok(None);
        }

        let file = std::fs::File::open(&path)?;
        let reader = io::BufReader::new(file);

        for line_result in reader.lines() {
            let line = line_result?;
            if line.contains("\"_state\"")
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                && val.get("_state").and_then(serde_json::Value::as_bool) == Some(true)
            {
                return Ok(val.get("data").cloned());
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
