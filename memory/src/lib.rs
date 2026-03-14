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
//! use swink_agent_memory::{JsonlSessionStore, SessionStore};
//!
//! let dir = JsonlSessionStore::default_dir().expect("config dir");
//! let store = JsonlSessionStore::new(dir)?;
//! let id = store.new_session_id();
//! store.save(&id, "claude-sonnet", "Be helpful.", &messages)?;
//! ```

pub mod compaction;
pub mod jsonl;
pub mod meta;
pub mod store;
pub mod store_async;
mod time;

pub use compaction::SummarizingCompactor;
pub use jsonl::JsonlSessionStore;
pub use meta::SessionMeta;
pub use store::{SessionFilter, SessionStore};
pub use store_async::{BlockingSessionStore, SessionStoreAsync};
