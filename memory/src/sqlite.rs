//! SQLite-backed session persistence with WAL mode.
//!
//! Replaces the ad hoc JSONL file protocol with transactional storage.
//! Uses a single `SQLite` database in WAL mode for concurrent-read safety
//! and atomic writes. Session data is stored across four tables:
//!
//! - `sessions` — metadata (id, title, timestamps, version, sequence)
//! - `entries` — ordered session entries (messages, events, etc.)
//! - `state` — session state snapshots (one per session)
//! - `interrupts` — interrupt state for resuming interrupted sessions

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use rusqlite::{Connection, params};
use swink_agent::{AgentMessage, CustomMessageRegistry, LlmMessage};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;
use crate::store::SessionStore;
use crate::time::now_utc;

fn to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn not_found(id: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("session not found: {id}"))
}

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

/// SQLite-backed session store using WAL mode for concurrent-read safety.
///
/// All mutations are wrapped in transactions for atomicity. The database
/// file is created at the configured path (or in-memory for testing).
pub struct SqliteSessionStore {
    conn: Mutex<Connection>,
}

#[allow(clippy::significant_drop_tightening)]
impl SqliteSessionStore {
    /// Open (or create) a `SQLite` session store at the given path.
    ///
    /// Enables WAL mode and creates tables if they don't exist.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(to_io)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Create an in-memory `SQLite` session store (useful for testing).
    pub fn in_memory() -> io::Result<Self> {
        let conn = Connection::open_in_memory().map_err(to_io)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Default database path: `<config_dir>/swink-agent/sessions.db`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("swink-agent").join("sessions.db"))
    }

    fn init_schema(&self) -> io::Result<()> {
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS sessions (
                 id         TEXT PRIMARY KEY NOT NULL,
                 title      TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 version    INTEGER NOT NULL DEFAULT 1,
                 sequence   INTEGER NOT NULL DEFAULT 0
             );

             CREATE TABLE IF NOT EXISTS entries (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 position   INTEGER NOT NULL,
                 kind       TEXT NOT NULL,
                 data       TEXT NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_entries_session
                 ON entries(session_id, position);

             CREATE TABLE IF NOT EXISTS state (
                 session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 data       TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS interrupts (
                 session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 data       TEXT NOT NULL
             );",
        )
        .map_err(to_io)?;
        Ok(())
    }

    fn session_exists(conn: &Connection, id: &str) -> io::Result<bool> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(to_io)?;
        Ok(count > 0)
    }

    fn load_meta(conn: &Connection, id: &str) -> io::Result<SessionMeta> {
        conn.query_row(
            "SELECT id, title, created_at, updated_at, version, sequence
             FROM sessions WHERE id = ?1",
            params![id],
            |row| {
                let created_str: String = row.get(2)?;
                let updated_str: String = row.get(3)?;
                Ok(SessionMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                        .map(|dt| dt.to_utc())
                        .unwrap_or_default(),
                    updated_at: chrono::DateTime::parse_from_rfc3339(&updated_str)
                        .map(|dt| dt.to_utc())
                        .unwrap_or_default(),
                    version: row.get::<_, u32>(4)?,
                    sequence: row.get::<_, u64>(5)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => not_found(id),
            other => to_io(other),
        })
    }

    fn upsert_meta(conn: &Connection, meta: &SessionMeta) -> io::Result<()> {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, version, sequence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 title = excluded.title,
                 updated_at = excluded.updated_at,
                 version = excluded.version,
                 sequence = excluded.sequence",
            params![
                meta.id,
                meta.title,
                meta.created_at.to_rfc3339(),
                meta.updated_at.to_rfc3339(),
                meta.version,
                meta.sequence,
            ],
        )
        .map_err(to_io)?;
        Ok(())
    }

    fn delete_entries(conn: &Connection, id: &str) -> io::Result<()> {
        conn.execute("DELETE FROM entries WHERE session_id = ?1", params![id])
            .map_err(to_io)?;
        Ok(())
    }
}


#[allow(clippy::significant_drop_tightening)]
impl SessionStore for SqliteSessionStore {
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[AgentMessage]) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        // Check optimistic concurrency
        if Self::session_exists(&conn, id)? {
            let stored = Self::load_meta(&conn, id)?;
            if stored.sequence != meta.sequence {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "sequence conflict for session {id}: expected {}, found {}",
                        meta.sequence, stored.sequence
                    ),
                ));
            }
        }

        let mut write_meta = meta.clone();
        write_meta.sequence += 1;

        let tx = conn.unchecked_transaction().map_err(to_io)?;

        Self::upsert_meta(&tx, &write_meta)?;
        Self::delete_entries(&tx, id)?;

        // Insert messages, skipping non-serializable custom messages
        let mut stmt = tx
            .prepare(
                "INSERT INTO entries (session_id, position, kind, data) VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(to_io)?;

        let mut pos = 0i64;
        for msg in messages {
            if let Some((kind, data)) = crate::codec::encode(msg, id) {
                stmt.execute(params![id, pos, kind.as_str(), data])
                    .map_err(to_io)?;
                pos += 1;
            }
        }
        drop(stmt);

        tx.commit().map_err(to_io)?;
        Ok(())
    }

    fn append(&self, id: &str, messages: &[AgentMessage]) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        if !Self::session_exists(&conn, id)? {
            return Err(not_found(id));
        }

        let tx = conn.unchecked_transaction().map_err(to_io)?;

        // Get current max position
        let max_pos: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), -1) FROM entries WHERE session_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(to_io)?;

        let mut stmt = tx
            .prepare(
                "INSERT INTO entries (session_id, position, kind, data) VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(to_io)?;

        let mut pos = max_pos + 1;
        for msg in messages {
            if let Some((kind, data)) = crate::codec::encode(msg, id) {
                stmt.execute(params![id, pos, kind.as_str(), data])
                    .map_err(to_io)?;
                pos += 1;
            }
        }
        drop(stmt);

        // Update updated_at
        tx.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![now_utc().to_rfc3339(), id],
        )
        .map_err(to_io)?;

        tx.commit().map_err(to_io)?;
        Ok(())
    }

    fn load(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        let meta = Self::load_meta(&conn, id)?;

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

        let mut stmt = conn
            .prepare("SELECT kind, data FROM entries WHERE session_id = ?1 ORDER BY position ASC")
            .map_err(to_io)?;

        let messages: Vec<AgentMessage> = stmt
            .query_map(params![id], |row| {
                let kind: String = row.get(0)?;
                let data: String = row.get(1)?;
                Ok((kind, data))
            })
            .map_err(to_io)?
            .filter_map(|row| {
                let (kind_str, data) = row.ok()?;
                let kind = crate::codec::MessageKind::parse(&kind_str)?;
                crate::codec::decode(kind, &data, registry).ok().flatten()
            })
            .collect();

        Ok((meta, messages))
    }

    fn list(&self) -> io::Result<Vec<SessionMeta>> {
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        let mut stmt = conn
            .prepare(
                "SELECT id, title, created_at, updated_at, version, sequence
                 FROM sessions ORDER BY updated_at DESC",
            )
            .map_err(to_io)?;

        let sessions: Vec<SessionMeta> = stmt
            .query_map([], |row| {
                let created_str: String = row.get(2)?;
                let updated_str: String = row.get(3)?;
                Ok(SessionMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                        .map(|dt| dt.to_utc())
                        .unwrap_or_default(),
                    updated_at: chrono::DateTime::parse_from_rfc3339(&updated_str)
                        .map(|dt| dt.to_utc())
                        .unwrap_or_default(),
                    version: row.get(4)?,
                    sequence: row.get(5)?,
                })
            })
            .map_err(to_io)?
            .filter_map(Result::ok)
            .collect();

        Ok(sessions)
    }

    fn delete(&self, id: &str) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        // CASCADE handles entries, state, and interrupts
        let changes = conn
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(to_io)?;

        if changes == 0 {
            return Err(not_found(id));
        }
        Ok(())
    }

    fn save_state(&self, id: &str, state: &serde_json::Value) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        if !Self::session_exists(&conn, id)? {
            return Err(not_found(id));
        }

        let json = serde_json::to_string(state).map_err(to_io)?;
        conn.execute(
            "INSERT INTO state (session_id, data) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET data = excluded.data",
            params![id, json],
        )
        .map_err(to_io)?;
        Ok(())
    }

    fn load_state(&self, id: &str) -> io::Result<Option<serde_json::Value>> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        match conn.query_row(
            "SELECT data FROM state WHERE session_id = ?1",
            params![id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(json) => {
                let val = serde_json::from_str(&json).map_err(to_io)?;
                Ok(Some(val))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(to_io(e)),
        }
    }

    fn save_interrupt(&self, id: &str, state: &InterruptState) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        if !Self::session_exists(&conn, id)? {
            return Err(not_found(id));
        }

        let json = serde_json::to_string(state).map_err(to_io)?;
        conn.execute(
            "INSERT INTO interrupts (session_id, data) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET data = excluded.data",
            params![id, json],
        )
        .map_err(to_io)?;
        Ok(())
    }

    fn load_interrupt(&self, id: &str) -> io::Result<Option<InterruptState>> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        match conn.query_row(
            "SELECT data FROM interrupts WHERE session_id = ?1",
            params![id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(json) => {
                let state: InterruptState = serde_json::from_str(&json).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("corrupted interrupt data for session {id}: {e}"),
                    )
                })?;
                Ok(Some(state))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(to_io(e)),
        }
    }

    fn clear_interrupt(&self, id: &str) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        conn.execute("DELETE FROM interrupts WHERE session_id = ?1", params![id])
            .map_err(to_io)?;
        Ok(())
    }

    fn load_with_options(
        &self,
        id: &str,
        options: &LoadOptions,
    ) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        let meta = Self::load_meta(&conn, id)?;

        let mut stmt = conn
            .prepare("SELECT kind, data FROM entries WHERE session_id = ?1 ORDER BY position ASC")
            .map_err(to_io)?;

        let mut entries: Vec<SessionEntry> = stmt
            .query_map(params![id], |row| {
                let kind: String = row.get(0)?;
                let data: String = row.get(1)?;
                Ok((kind, data))
            })
            .map_err(to_io)?
            .filter_map(|row| {
                let (kind, data) = row.ok()?;
                deserialize_session_entry(&kind, &data)
            })
            .collect();

        // Apply filters
        if let Some(ref types) = options.entry_types {
            entries.retain(|entry| types.iter().any(|t| t == entry.entry_type_name()));
        }

        if let Some(after) = options.after_timestamp {
            let after_secs = after.timestamp().cast_unsigned();
            entries.retain(|entry| entry.timestamp().is_some_and(|ts| ts > after_secs));
        }

        if let Some(n) = options.last_n_entries
            && entries.len() > n
        {
            entries.drain(..entries.len() - n);
        }

        Ok((meta, entries))
    }
}

#[allow(clippy::significant_drop_tightening)]
impl SqliteSessionStore {
    /// Save a session with rich entry types.
    pub fn save_entries(
        &self,
        id: &str,
        meta: &SessionMeta,
        entries: &[SessionEntry],
    ) -> io::Result<()> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        // Check optimistic concurrency
        if Self::session_exists(&conn, id)? {
            let stored = Self::load_meta(&conn, id)?;
            if stored.sequence != meta.sequence {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "sequence conflict for session {id}: expected {}, found {}",
                        meta.sequence, stored.sequence
                    ),
                ));
            }
        }

        let mut write_meta = meta.clone();
        write_meta.sequence += 1;

        let tx = conn.unchecked_transaction().map_err(to_io)?;

        Self::upsert_meta(&tx, &write_meta)?;
        Self::delete_entries(&tx, id)?;

        let mut stmt = tx
            .prepare(
                "INSERT INTO entries (session_id, position, kind, data) VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(to_io)?;

        for (i, entry) in entries.iter().enumerate() {
            let (kind, data) = serialize_session_entry(entry)?;
            let pos = i64::try_from(i).map_err(to_io)?;
            stmt.execute(params![id, pos, kind, data]).map_err(to_io)?;
        }
        drop(stmt);

        tx.commit().map_err(to_io)?;
        Ok(())
    }

    /// Load a session with rich entry types.
    pub fn load_entries(&self, id: &str) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
        validate_session_id(id)?;
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);

        let meta = Self::load_meta(&conn, id)?;

        let mut stmt = conn
            .prepare("SELECT kind, data FROM entries WHERE session_id = ?1 ORDER BY position ASC")
            .map_err(to_io)?;

        let entries: Vec<SessionEntry> = stmt
            .query_map(params![id], |row| {
                let kind: String = row.get(0)?;
                let data: String = row.get(1)?;
                Ok((kind, data))
            })
            .map_err(to_io)?
            .filter_map(|row| {
                let (kind, data) = row.ok()?;
                deserialize_session_entry(&kind, &data)
            })
            .collect();

        Ok((meta, entries))
    }
}

fn serialize_session_entry(entry: &SessionEntry) -> io::Result<(String, String)> {
    let kind = entry.entry_type_name().to_string();
    let data = serde_json::to_string(entry).map_err(to_io)?;
    Ok((kind, data))
}

fn deserialize_session_entry(kind: &str, data: &str) -> Option<SessionEntry> {
    match kind {
        "llm" => {
            // Stored via save() as AgentMessage — unwrap to SessionEntry::Message
            serde_json::from_str::<LlmMessage>(data)
                .ok()
                .map(SessionEntry::Message)
        }
        _ => {
            // Stored via save_entries() as tagged SessionEntry
            serde_json::from_str::<SessionEntry>(data).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{
        AssistantMessage, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason, Usage, UserMessage,
    };

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn assistant_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            provider: "test".to_string(),
            model_id: "test-model".to_string(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn llm_user(text: &str) -> LlmMessage {
        LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            timestamp: 0,
            cache_hint: None,
        })
    }

    fn llm_user_at(text: &str, ts: u64) -> LlmMessage {
        LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            timestamp: ts,
            cache_hint: None,
        })
    }

    fn meta(id: &str, title: &str) -> SessionMeta {
        let now = now_utc();
        SessionMeta {
            id: id.to_string(),
            title: title.to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            sequence: 0,
        }
    }

    #[test]
    fn save_and_load_roundtrip() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("s1", "Test session");
        let msgs = vec![user_msg("hello"), assistant_msg("hi")];

        store.save("s1", &m, &msgs).unwrap();
        let (loaded_meta, loaded_msgs) = store.load("s1", None).unwrap();

        assert_eq!(loaded_meta.id, "s1");
        assert_eq!(loaded_meta.title, "Test session");
        assert_eq!(loaded_meta.sequence, 1);
        assert_eq!(loaded_msgs.len(), 2);
    }

    #[test]
    fn save_overwrites_existing() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("s1", "V1");
        store.save("s1", &m, &[user_msg("first")]).unwrap();

        let (saved, _) = store.load("s1", None).unwrap();
        let m2 = SessionMeta {
            title: "V2".to_string(),
            ..saved
        };
        store
            .save("s1", &m2, &[user_msg("second"), user_msg("third")])
            .unwrap();

        let (loaded, msgs) = store.load("s1", None).unwrap();
        assert_eq!(loaded.title, "V2");
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn load_nonexistent_returns_not_found() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let err = store.load("nope", None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn list_returns_sorted_by_updated_at() {
        let store = SqliteSessionStore::in_memory().unwrap();

        let old_time = chrono::DateTime::from_timestamp(1_000_000, 0)
            .unwrap()
            .to_utc();
        let new_time = chrono::DateTime::from_timestamp(2_000_000, 0)
            .unwrap()
            .to_utc();

        let m_old = SessionMeta {
            id: "old".to_string(),
            title: "Old".to_string(),
            created_at: old_time,
            updated_at: old_time,
            version: 1,
            sequence: 0,
        };
        let m_new = SessionMeta {
            id: "new".to_string(),
            title: "New".to_string(),
            created_at: new_time,
            updated_at: new_time,
            version: 1,
            sequence: 0,
        };

        store.save("old", &m_old, &[]).unwrap();
        store.save("new", &m_new, &[]).unwrap();

        let sessions = store.list().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "new");
        assert_eq!(sessions[1].id, "old");
    }

    #[test]
    fn delete_removes_session_and_cascades() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("del", "Delete me");
        store.save("del", &m, &[user_msg("hi")]).unwrap();
        store
            .save_state("del", &serde_json::json!({"key": "val"}))
            .unwrap();
        store
            .save_interrupt(
                "del",
                &InterruptState {
                    interrupted_at: 100,
                    pending_tool_calls: vec![],
                    context_snapshot: vec![],
                    system_prompt: String::new(),
                    model: ModelSpec::new("t", "t"),
                },
            )
            .unwrap();

        store.delete("del").unwrap();

        assert_eq!(
            store.load("del", None).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(store.load_state("del").unwrap().is_none());
        assert!(store.load_interrupt("del").unwrap().is_none());
    }

    #[test]
    fn sequence_conflict_rejected() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("conc", "Concurrency");
        store.save("conc", &m, &[user_msg("v1")]).unwrap();

        let (stale, _) = store.load("conc", None).unwrap();
        // Another write bumps sequence
        store.save("conc", &stale, &[user_msg("v2")]).unwrap();

        // Stale write should fail
        let err = store.save("conc", &stale, &[user_msg("v3")]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("sequence conflict"));
    }

    #[test]
    fn state_save_and_load() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("st", "State test");
        store.save("st", &m, &[user_msg("hi")]).unwrap();

        store
            .save_state("st", &serde_json::json!({"cursor": 42}))
            .unwrap();
        let state = store.load_state("st").unwrap();
        assert_eq!(state, Some(serde_json::json!({"cursor": 42})));

        // Overwrite
        store
            .save_state("st", &serde_json::json!({"cursor": 99}))
            .unwrap();
        let state = store.load_state("st").unwrap();
        assert_eq!(state, Some(serde_json::json!({"cursor": 99})));
    }

    #[test]
    fn state_none_for_missing_session() {
        let store = SqliteSessionStore::in_memory().unwrap();
        assert!(store.load_state("nope").unwrap().is_none());
    }

    #[test]
    fn append_preserves_state() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("ap", "Append");
        store.save("ap", &m, &[user_msg("hello")]).unwrap();
        store
            .save_state("ap", &serde_json::json!({"cursor": 1}))
            .unwrap();

        store.append("ap", &[user_msg("world")]).unwrap();

        let state = store.load_state("ap").unwrap();
        assert_eq!(state, Some(serde_json::json!({"cursor": 1})));

        let (_, msgs) = store.load("ap", None).unwrap();
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn interrupt_roundtrip() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("int", "Interrupt");
        store.save("int", &m, &[user_msg("hi")]).unwrap();

        let interrupt = InterruptState {
            interrupted_at: 1_710_500_000,
            pending_tool_calls: vec![crate::PendingToolCall {
                tool_call_id: "tc_1".to_string(),
                tool_name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            }],
            context_snapshot: vec![llm_user("hello")],
            system_prompt: "You are helpful.".to_string(),
            model: ModelSpec::new("openai", "gpt-4"),
        };

        store.save_interrupt("int", &interrupt).unwrap();
        let loaded = store.load_interrupt("int").unwrap().unwrap();
        assert_eq!(loaded.interrupted_at, 1_710_500_000);
        assert_eq!(loaded.pending_tool_calls.len(), 1);
        assert_eq!(loaded.system_prompt, "You are helpful.");
    }

    #[test]
    fn clear_interrupt_idempotent() {
        let store = SqliteSessionStore::in_memory().unwrap();
        // Clear on nonexistent is fine
        store.clear_interrupt("nope").unwrap();

        let m = meta("ci", "Clear");
        store.save("ci", &m, &[]).unwrap();
        store
            .save_interrupt(
                "ci",
                &InterruptState {
                    interrupted_at: 100,
                    pending_tool_calls: vec![],
                    context_snapshot: vec![],
                    system_prompt: String::new(),
                    model: ModelSpec::new("t", "t"),
                },
            )
            .unwrap();

        store.clear_interrupt("ci").unwrap();
        assert!(store.load_interrupt("ci").unwrap().is_none());
        // Clear again is fine
        store.clear_interrupt("ci").unwrap();
    }

    #[test]
    fn invalid_session_id_rejected() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("bad", "Bad");

        for bad_id in &["foo/bar", "foo\\bar", "..secret", "null\0byte", ""] {
            let err = store.save(bad_id, &m, &[]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn rich_entries_roundtrip() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("rich", "Rich entries");

        let entries = vec![
            SessionEntry::Message(llm_user("hello")),
            SessionEntry::ModelChange {
                from: ModelSpec::new("openai", "gpt-4"),
                to: ModelSpec::new("anthropic", "claude-3"),
                timestamp: 100,
            },
            SessionEntry::Label {
                text: "bookmark".to_string(),
                message_index: 0,
                timestamp: 200,
            },
            SessionEntry::Custom {
                type_name: "my_event".to_string(),
                data: serde_json::json!({"key": "value"}),
                timestamp: 300,
            },
        ];

        store.save_entries("rich", &m, &entries).unwrap();
        let (loaded_meta, loaded_entries) = store.load_entries("rich").unwrap();

        assert_eq!(loaded_meta.sequence, 1);
        assert_eq!(loaded_entries.len(), 4);
        assert!(matches!(
            loaded_entries[0],
            SessionEntry::Message(LlmMessage::User(_))
        ));
        assert!(matches!(
            loaded_entries[1],
            SessionEntry::ModelChange { .. }
        ));
        assert!(matches!(loaded_entries[2], SessionEntry::Label { .. }));
        assert!(matches!(loaded_entries[3], SessionEntry::Custom { .. }));
    }

    #[test]
    fn load_with_options_filters() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("opts", "Options test");

        let entries: Vec<SessionEntry> = (0..50)
            .map(|i| SessionEntry::Message(llm_user_at(&format!("msg_{i}"), i)))
            .collect();

        store.save_entries("opts", &m, &entries).unwrap();

        // last_n
        let opts = LoadOptions {
            last_n_entries: Some(10),
            ..Default::default()
        };
        let (_, loaded) = store.load_with_options("opts", &opts).unwrap();
        assert_eq!(loaded.len(), 10);
        for (i, entry) in loaded.iter().enumerate() {
            assert_eq!(entry.timestamp(), Some(40 + i as u64));
        }

        // after_timestamp
        let after = chrono::DateTime::from_timestamp(25, 0).unwrap().to_utc();
        let opts = LoadOptions {
            after_timestamp: Some(after),
            ..Default::default()
        };
        let (_, loaded) = store.load_with_options("opts", &opts).unwrap();
        assert_eq!(loaded.len(), 24); // entries with timestamps 26..49
        for entry in &loaded {
            assert!(entry.timestamp().unwrap() > 25);
        }

        // by entry type
        let mixed_entries = vec![
            SessionEntry::Message(llm_user("hello")),
            SessionEntry::ModelChange {
                from: ModelSpec::new("a", "a"),
                to: ModelSpec::new("b", "b"),
                timestamp: 100,
            },
            SessionEntry::Message(llm_user("world")),
        ];
        let m2 = meta("opts2", "Type filter");
        store.save_entries("opts2", &m2, &mixed_entries).unwrap();

        let opts = LoadOptions {
            entry_types: Some(vec!["message".to_string()]),
            ..Default::default()
        };
        let (_, loaded) = store.load_with_options("opts2", &opts).unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn file_based_store() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = SqliteSessionStore::open(&db_path).unwrap();

        let m = meta("file", "File test");
        store.save("file", &m, &[user_msg("hello")]).unwrap();

        // Re-open and verify data persists
        drop(store);
        let store2 = SqliteSessionStore::open(&db_path).unwrap();
        let (loaded, msgs) = store2.load("file", None).unwrap();
        assert_eq!(loaded.id, "file");
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn empty_list() {
        let store = SqliteSessionStore::in_memory().unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn custom_messages_with_registry() {
        use swink_agent::{CustomMessage, CustomMessageRegistry};

        #[derive(Debug)]
        struct TestCustom {
            data: String,
        }
        impl CustomMessage for TestCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn type_name(&self) -> Option<&str> {
                Some("TestCustom")
            }
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::json!({ "data": self.data }))
            }
        }

        let store = SqliteSessionStore::in_memory().unwrap();
        let m = meta("custom", "Custom msg");
        let messages: Vec<AgentMessage> = vec![
            user_msg("hello"),
            AgentMessage::Custom(Box::new(TestCustom {
                data: "payload".to_string(),
            })),
            user_msg("world"),
        ];

        store.save("custom", &m, &messages).unwrap();

        // Without registry — custom messages skipped
        let (_, loaded) = store.load("custom", None).unwrap();
        assert_eq!(loaded.len(), 2);

        // With registry — custom messages restored
        let mut reg = CustomMessageRegistry::new();
        reg.register(
            "TestCustom",
            Box::new(|val: serde_json::Value| {
                let data = val
                    .get("data")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing data".to_string())?;
                Ok(Box::new(TestCustom {
                    data: data.to_string(),
                }) as Box<dyn CustomMessage>)
            }),
        );

        let (_, loaded) = store.load("custom", Some(&reg)).unwrap();
        assert_eq!(loaded.len(), 3);
        assert!(matches!(loaded[1], AgentMessage::Custom(_)));
        let custom = loaded[1].downcast_ref::<TestCustom>().unwrap();
        assert_eq!(custom.data, "payload");
    }
}
