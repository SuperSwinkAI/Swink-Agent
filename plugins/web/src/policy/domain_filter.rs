use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
use url::Url;

use crate::domain::DomainFilter;

/// `PreDispatchPolicy` that enforces domain allowlist/denylist and SSRF
/// protection on all `web_*` tool calls.
pub struct DomainFilterPolicy {
    filter: DomainFilter,
}

impl DomainFilterPolicy {
    /// Create a new policy wrapping the given [`DomainFilter`].
    pub fn new(filter: DomainFilter) -> Self {
        Self { filter }
    }
}

impl PreDispatchPolicy for DomainFilterPolicy {
    fn name(&self) -> &str {
        "web.domain_filter"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        // Only apply to web_* tools.
        if !ctx.tool_name.starts_with("web_") {
            return PreDispatchVerdict::Continue;
        }

        // Extract the URL argument; if absent, let the tool handle validation.
        let url_str = match ctx.arguments.get("url").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return PreDispatchVerdict::Continue,
        };

        // Parse the URL.
        let parsed = match Url::parse(url_str) {
            Ok(u) => u,
            Err(e) => {
                return PreDispatchVerdict::Skip(format!("Invalid URL '{url_str}': {e}"));
            }
        };

        // Run the domain filter.
        match self.filter.is_allowed(&parsed) {
            Ok(()) => PreDispatchVerdict::Continue,
            Err(e) => PreDispatchVerdict::Skip(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use swink_agent::SessionState;

    use super::*;

    fn make_dispatch_ctx<'a>(
        tool_name: &'a str,
        tool_call_id: &'a str,
        args: &'a mut serde_json::Value,
        state: &'a SessionState,
    ) -> ToolDispatchContext<'a> {
        ToolDispatchContext {
            tool_name,
            tool_call_id,
            arguments: args,
            execution_root: None,
            state,
        }
    }

    #[test]
    fn policy_ignores_non_web_tools() {
        let policy = DomainFilterPolicy::new(DomainFilter {
            denylist: vec!["evil.com".to_string()],
            ..Default::default()
        });
        let state = SessionState::default();
        let mut args = json!({"url": "https://evil.com"});
        let mut ctx = make_dispatch_ctx("bash", "call_1", &mut args, &state);
        assert!(matches!(
            policy.evaluate(&mut ctx),
            PreDispatchVerdict::Continue
        ));
    }

    #[test]
    fn policy_continues_without_url_argument() {
        let policy = DomainFilterPolicy::new(DomainFilter::default());
        let state = SessionState::default();
        let mut args = json!({"query": "rust programming"});
        let mut ctx = make_dispatch_ctx("web_search", "call_2", &mut args, &state);
        assert!(matches!(
            policy.evaluate(&mut ctx),
            PreDispatchVerdict::Continue
        ));
    }

    #[test]
    fn policy_blocks_denied_and_invalid_urls() {
        let policy = DomainFilterPolicy::new(DomainFilter {
            denylist: vec!["evil.com".to_string()],
            ..Default::default()
        });
        let state = SessionState::default();

        let mut denied = json!({"url": "https://evil.com/page"});
        let mut denied_ctx = make_dispatch_ctx("web_fetch", "call_3", &mut denied, &state);
        assert!(matches!(
            policy.evaluate(&mut denied_ctx),
            PreDispatchVerdict::Skip(_)
        ));

        let mut bad_scheme = json!({"url": "file:///etc/passwd"});
        let mut bad_scheme_ctx = make_dispatch_ctx("web_fetch", "call_4", &mut bad_scheme, &state);
        assert!(matches!(
            policy.evaluate(&mut bad_scheme_ctx),
            PreDispatchVerdict::Skip(_)
        ));

        let mut invalid = json!({"url": "not a url at all"});
        let mut invalid_ctx = make_dispatch_ctx("web_fetch", "call_5", &mut invalid, &state);
        assert!(matches!(
            policy.evaluate(&mut invalid_ctx),
            PreDispatchVerdict::Skip(_)
        ));
    }
}
