//! Agent registry for naming and looking up agents at runtime.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::agent::Agent;

// ─── AgentRef ───────────────────────────────────────────────────────────────

/// A shareable, async-safe handle to an [`Agent`].
pub type AgentRef = Arc<tokio::sync::Mutex<Agent>>;

// ─── AgentRegistry ──────────────────────────────────────────────────────────

/// Thread-safe registry that maps string names to [`AgentRef`] handles.
///
/// Uses [`std::sync::RwLock`] (not `tokio::sync`) because all operations are
/// fast `HashMap` lookups — no `.await` is held across the lock.
#[derive(Clone)]
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentRef>>>,
}

impl AgentRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an agent under the given name, returning a shareable handle.
    ///
    /// If an agent was already registered with this name it is replaced.
    pub fn register(&self, name: impl Into<String>, agent: Agent) -> AgentRef {
        let agent_ref: AgentRef = Arc::new(tokio::sync::Mutex::new(agent));
        self.agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.into(), Arc::clone(&agent_ref));
        agent_ref
    }

    /// Look up an agent by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<AgentRef> {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// Remove an agent by name, returning its handle if it existed.
    pub fn remove(&self, name: &str) -> Option<AgentRef> {
        self.agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name)
    }

    /// List all registered agent names.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// Number of registered agents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Returns `true` if no agents are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
