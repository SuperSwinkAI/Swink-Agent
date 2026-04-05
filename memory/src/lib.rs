#![forbid(unsafe_code)]
//! Session persistence and memory management for swink-agent.
//!
//! This crate provides pluggable session storage and context compaction
//! strategies for the swink-agent framework. It extracts session persistence
//! from the TUI into a reusable library and lays the groundwork for
//! multi-layer memory research (summarization, RAG, tool-aware compaction).
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use swink_agent_memory::{JsonlSessionStore, SessionStore, SessionMeta};
//! use swink_agent_memory::time::{now_utc, format_session_id};
//!
//! let dir = JsonlSessionStore::default_dir().expect("config dir");
//! let store = JsonlSessionStore::new(dir)?;
//! let id = format_session_id();
//! let meta = SessionMeta {
//!     id: id.clone(),
//!     title: "My session".into(),
//!     created_at: now_utc(),
//!     updated_at: now_utc(),
//!     version: 1,
//!     sequence: 0,
//! };
//! store.save(&id, &meta, &messages)?;
//! ```

pub mod codec;
pub mod compaction;
pub mod entry;
pub mod interrupt;
pub mod jsonl;
pub mod load_options;
pub mod meta;
pub mod migrate;
pub mod store;
pub mod store_async;
pub mod time;

#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteSessionStore;

pub use compaction::{CompactionResult, SummarizingCompactor};
pub use entry::SessionEntry;
pub use interrupt::{InterruptState, PendingToolCall};
pub use jsonl::JsonlSessionStore;
pub use load_options::LoadOptions;
pub use meta::SessionMeta;
pub use migrate::SessionMigrator;
pub use store::SessionStore;
pub use store_async::{BlockingSessionStore, SessionStoreFuture};
pub use time::{format_session_id, now_utc};
