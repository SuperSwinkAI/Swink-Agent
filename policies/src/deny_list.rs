//! Tool deny list policy — rejects tool calls by name.
#![forbid(unsafe_code)]

use std::collections::HashSet;

use swink_agent::{PolicyContext, PreDispatchPolicy, PreDispatchVerdict, ToolPolicyContext};

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

    fn evaluate(
        &self,
        _ctx: &PolicyContext<'_>,
        tool: &mut ToolPolicyContext<'_>,
    ) -> PreDispatchVerdict {
        if self.denied.contains(tool.tool_name) {
            PreDispatchVerdict::Skip(format!(
                "tool '{}' is denied by policy",
                tool.tool_name
            ))
        } else {
            PreDispatchVerdict::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{Cost, Usage};

    fn make_ctx<'a>(usage: &'a Usage, cost: &'a Cost) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 0,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
        }
    }

    #[test]
    fn denies_listed_tool() {
        let policy = ToolDenyListPolicy::new(["bash", "write_file"]);
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({"command": "ls"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "bash",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("denied")));
    }

    #[test]
    fn allows_unlisted_tool() {
        let policy = ToolDenyListPolicy::new(["bash"]);
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({"path": "/tmp/file"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "read_file",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Continue));
    }

    #[test]
    fn empty_deny_list_allows_all() {
        let policy = ToolDenyListPolicy::new(Vec::<String>::new());
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "bash",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Continue));
    }
}
