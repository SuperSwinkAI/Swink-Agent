#![forbid(unsafe_code)]
//! Session persistence and memory management for swink-agent.
//!
//! This crate provides pluggable session storage, durable checkpoint storage,
//! and context compaction strategies for the swink-agent framework. It
//! extracts session persistence from the TUI into a reusable library and lays
//! the groundwork for multi-layer memory research (summarization, RAG,
//! tool-aware compaction).
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use swink_agent_memory::{
//!     JsonlSessionStore, SessionMeta, SessionStore, format_session_id, now_utc,
//! };
//!
//! let dir = JsonlSessionStore::default_dir().expect("config dir");
//! let store = JsonlSessionStore::new(dir)?;
//! let id = format_session_id(); // e.g. "20260320_143000_6f00bfe3f7c54b2f86d780df58ccf0a1"
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
//!
//! For durable agent checkpoints, use [`FileCheckpointStore`].

mod checkpoint_store;
mod codec;
mod compaction;
mod entry;
mod interrupt;
mod jsonl;
mod load_options;
mod meta;
mod migrate;
mod search;
mod store;
mod store_async;
mod time;

pub use checkpoint_store::FileCheckpointStore;
pub use compaction::{CompactionResult, SummarizingCompactor};
pub use entry::SessionEntry;
pub use interrupt::{InterruptState, PendingToolCall};
pub use jsonl::JsonlSessionStore;
pub use load_options::LoadOptions;
pub use meta::SessionMeta;
pub use migrate::SessionMigrator;
#[cfg(feature = "search")]
pub use search::index::TantivyIndex;
pub use search::{SessionHit, SessionSearchOptions};
pub use store::SessionStore;
pub use store_async::{BlockingSessionStore, SessionStoreFuture};
pub use time::{format_session_id, now_utc};
