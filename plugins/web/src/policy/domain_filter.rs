use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
use url::Url;

use crate::domain::DomainFilter;

/// `PreDispatchPolicy` that enforces domain allowlist/denylist and IP-literal
/// SSRF protection on all `web_*` tool calls without performing DNS lookups.
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

        // Run the I/O-free part of the domain filter. This policy is
        // evaluated synchronously, so it must not do blocking DNS; the web
        // tools re-run the full filter (including DNS-resolved private-IP
        // checks) on tokio's blocking pool before any request is sent.
        match self.filter.check_without_dns(&parsed) {
            Ok(_) => PreDispatchVerdict::Continue,
            Err(e) => PreDispatchVerdict::Skip(e.to_string()),
        }
    }
}

#[cfg(test)]
#[path = "domain_filter_tests.rs"]
mod tests;
