//! Tool deny list policy — rejects tool calls by name.
#![forbid(unsafe_code)]

use std::collections::HashSet;

use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};

/// Rejects tool calls whose names appear in the deny list.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::ToolDenyListPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash", "write_file"]));
/// ```
#[derive(Debug, Clone)]
pub struct ToolDenyListPolicy {
    denied: HashSet<String>,
}

impl ToolDenyListPolicy {
    /// Create a new deny list from an iterator of tool names.
    pub fn new(denied: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            denied: denied.into_iter().map(Into::into).collect(),
        }
    }
}

impl PreDispatchPolicy for ToolDenyListPolicy {
    fn name(&self) -> &'static str {
        "tool_deny_list"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        if self.denied.contains(ctx.tool_name) {
            PreDispatchVerdict::Skip(format!("tool '{}' is denied by policy", ctx.tool_name))
        } else {
            PreDispatchVerdict::Continue
        }
    }
}

#[cfg(test)]
#[path = "deny_list_tests.rs"]
mod tests;
