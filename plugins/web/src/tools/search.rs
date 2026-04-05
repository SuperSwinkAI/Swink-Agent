use std::sync::Arc;

use serde_json::Value;
use swink_agent::tool::{AgentTool, AgentToolResult, ToolFuture};
use tokio_util::sync::CancellationToken;

use crate::search::SearchProvider;

/// Tool for searching the web via a pluggable [`SearchProvider`].
pub struct SearchTool {
    provider: Arc<dyn SearchProvider>,
    max_search_results: usize,
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
            schema,
        }
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
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::credential::ResolvedCredential>,
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

            match provider.search(&query, max_results).await {
                Ok(results) if results.is_empty() => {
                    AgentToolResult::text(format!("No results found for '{query}'."))
                }
                Ok(results) => AgentToolResult::text(Self::format_results(&results)),
                Err(e) => AgentToolResult::error(e.to_string()),
            }
        })
    }
}
