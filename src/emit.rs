use serde_json::Value;

/// A structured event emitted by an agent, tool, or callback.
#[derive(Debug, Clone)]
pub struct Emission {
    /// Event name (e.g., "progress", "`artifact_created`").
    pub name: String,
    /// Structured payload.
    pub payload: Value,
}

impl Emission {
    /// Create a new emission.
    pub fn new(name: impl Into<String>, payload: Value) -> Self {
        Self {
            name: name.into(),
            payload,
        }
    }
}
