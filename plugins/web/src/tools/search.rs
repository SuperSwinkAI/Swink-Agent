use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::policy::ContentSanitizerPolicy;
use crate::search::SearchProvider;
use crate::tools::sanitize_web_tool_text;

/// Tool for searching the web via a pluggable [`SearchProvider`].
pub struct SearchTool {
    provider: Arc<dyn SearchProvider>,
    max_search_results: usize,
    sanitizer: Option<ContentSanitizerPolicy>,
    schema: Value,
}

impl SearchTool {
    pub fn new(provider: Arc<dyn SearchProvider>, max_search_results: usize) -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query string."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return. Defaults to 10.",
                    "minimum": 1,
                    "maximum": 50
                }
            },
            "required": ["query"]
        });
        Self {
            provider,
            max_search_results,
            sanitizer: Some(ContentSanitizerPolicy::new()),
            schema,
        }
    }

    /// Enable or disable prompt-injection sanitization of search result text.
    #[must_use]
    pub fn with_sanitizer_enabled(mut self, enabled: bool) -> Self {
        self.sanitizer = enabled.then(ContentSanitizerPolicy::new);
        self
    }

    /// Format search results as a numbered markdown list.
    pub fn format_results(results: &[crate::search::SearchResult]) -> String {
        let mut out = String::new();
        for (i, r) in results.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!(
                "{}. **{}**\n   {}\n   {}",
                i + 1,
                r.title,
                r.url,
                r.snippet,
            ));
        }
        out
    }
}

impl AgentTool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn label(&self) -> &str {
        "Web Search"
    }

    fn description(&self) -> &str {
        "Search the web and return a ranked list of results with titles, URLs, and snippets."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        let max_results = params
            .get("max_results")
            .and_then(Value::as_u64)
            .map_or(self.max_search_results, |n| n as usize);

        let provider = Arc::clone(&self.provider);

        Box::pin(async move {
            if query.is_empty() {
                return AgentToolResult::error("Missing required parameter: query");
            }

            // FR-016: log every web request. Search providers have no single
            // URL, so the provider name plus query is the closest equivalent;
            // there is no HTTP status to surface through the SearchProvider
            // trait, so latency and result/byte size stand in for it.
            let start = Instant::now();
            let provider_name = provider.name().to_string();

            let search_result = tokio::select! {
                result = provider.search(&query, max_results) => result,
                () = cancellation_token.cancelled() => {
                    return AgentToolResult::error("Request cancelled");
                }
            };

            match search_result {
                Ok(results) if results.is_empty() => {
                    info!(
                        provider = %provider_name,
                        query = %query,
                        result_count = 0,
                        latency_ms = start.elapsed().as_millis(),
                        "web search returned no results"
                    );
                    AgentToolResult::text(format!("No results found for '{query}'."))
                }
                Ok(results) => {
                    let output = Self::format_results(&results);
                    let output =
                        sanitize_web_tool_text("web_search", output, self.sanitizer.as_ref());
                    info!(
                        provider = %provider_name,
                        query = %query,
                        result_count = results.len(),
                        size_bytes = output.len(),
                        latency_ms = start.elapsed().as_millis(),
                        "web search completed"
                    );
                    AgentToolResult::text(output)
                }
                Err(e) => {
                    warn!(
                        provider = %provider_name,
                        query = %query,
                        latency_ms = start.elapsed().as_millis(),
                        error = %e,
                        "web search failed"
                    );
                    AgentToolResult::error(e.to_string())
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
