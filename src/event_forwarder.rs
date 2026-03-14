use std::sync::Arc;

use crate::loop_::AgentEvent;

/// A function that receives forwarded agent events.
pub type EventForwarderFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;
