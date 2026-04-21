use std::sync::Arc;

use serde_json::Value;
use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
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

            let search_result = tokio::select! {
                result = provider.search(&query, max_results) => result,
                () = cancellation_token.cancelled() => {
                    return AgentToolResult::error("Request cancelled");
                }
            };

            match search_result {
                Ok(results) if results.is_empty() => {
                    AgentToolResult::text(format!("No results found for '{query}'."))
                }
                Ok(results) => AgentToolResult::text(Self::format_results(&results)),
                Err(e) => AgentToolResult::error(e.to_string()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use swink_agent::{AgentTool, SessionState};
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    use super::SearchTool;
    use crate::search::{SearchError, SearchProvider, SearchResult};

    struct MockProvider {
        results: Vec<SearchResult>,
    }

    impl SearchProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn search(
            &self,
            _query: &str,
            max_results: usize,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move { Ok(self.results.iter().take(max_results).cloned().collect()) })
        }
    }

    struct FailingProvider;

    impl SearchProvider for FailingProvider {
        fn name(&self) -> &str {
            "failing"
        }

        fn search(
            &self,
            _query: &str,
            _max_results: usize,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move { Err(SearchError::NetworkError("connection refused".into())) })
        }
    }

    struct PendingProvider;

    impl SearchProvider for PendingProvider {
        fn name(&self) -> &str {
            "pending"
        }

        fn search(
            &self,
            _query: &str,
            _max_results: usize,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    fn formats_results_as_numbered_list() {
        let results = vec![
            SearchResult {
                title: "Rust Lang".to_owned(),
                url: "https://rust-lang.org".to_owned(),
                snippet: "A systems programming language.".to_owned(),
            },
            SearchResult {
                title: "Crates.io".to_owned(),
                url: "https://crates.io".to_owned(),
                snippet: "Rust package registry.".to_owned(),
            },
        ];
        let formatted = SearchTool::format_results(&results);
        assert!(formatted.starts_with("1. **Rust Lang**"));
        assert!(formatted.contains("2. **Crates.io**"));
    }

    #[tokio::test]
    async fn execute_returns_formatted_results() {
        let provider = Arc::new(MockProvider {
            results: vec![SearchResult {
                title: "Test".to_owned(),
                url: "https://test.com".to_owned(),
                snippet: "A test result.".to_owned(),
            }],
        });
        let tool = SearchTool::new(provider, 10);
        let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
        let result = tool
            .execute(
                "call-1",
                json!({"query": "test"}),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;

        assert!(!result.is_error);
        let text = format!("{:?}", result.content);
        assert!(text.contains("Test"));
        assert!(text.contains("https://test.com"));
    }

    #[tokio::test]
    async fn execute_returns_errors_for_bad_inputs_or_provider_failure() {
        let empty_provider = Arc::new(MockProvider { results: vec![] });
        let empty_tool = SearchTool::new(empty_provider, 10);
        let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
        let missing_query = empty_tool
            .execute(
                "call-2",
                json!({}),
                CancellationToken::new(),
                None,
                Arc::clone(&state),
                None,
            )
            .await;
        assert!(missing_query.is_error);

        let failing_tool = SearchTool::new(Arc::new(FailingProvider), 10);
        let provider_failure = failing_tool
            .execute(
                "call-3",
                json!({"query": "fail"}),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;
        assert!(provider_failure.is_error);
    }

    #[tokio::test]
    async fn execute_returns_cancelled_when_provider_request_is_interrupted() {
        let tool = SearchTool::new(Arc::new(PendingProvider), 10);
        let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
        let cancellation_token = CancellationToken::new();
        cancellation_token.cancel();

        let result = timeout(
            Duration::from_millis(100),
            tool.execute(
                "call-4",
                json!({"query": "cancel me"}),
                cancellation_token,
                None,
                state,
                None,
            ),
        )
        .await
        .expect("cancelled search should not hang");

        assert!(result.is_error);
        let text = format!("{:?}", result.content);
        assert!(text.contains("Request cancelled"));
    }
}
