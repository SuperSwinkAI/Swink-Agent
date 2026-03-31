//! Standalone module for [`AgentId`] to avoid circular imports between
//! `agent` and `registry`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique identifier assigned to every [`crate::Agent`] on construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentId(u64);

impl AgentId {
    pub(crate) fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentId({})", self.0)
    }
}
