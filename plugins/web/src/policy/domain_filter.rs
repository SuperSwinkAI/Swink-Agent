use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
use url::Url;

use crate::domain::DomainFilter;

/// `PreDispatchPolicy` that enforces domain allowlist/denylist and SSRF
/// protection on all `web.*` tool calls.
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
        // Only apply to web.* tools.
        if !ctx.tool_name.starts_with("web.") {
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
