mod common;

use std::sync::Arc;

use serde_json::json;
use swink_agent::SessionState;
use swink_agent::tool::AgentTool;
use tokio_util::sync::CancellationToken;

// ─── DuckDuckGo HTML parsing ────────────────────────────────────────────────

#[cfg(feature = "duckduckgo")]
mod duckduckgo_parsing {
    use swink_agent_plugin_web::search::SearchResult;

    // We call the public `parse_results` directly, avoiding any HTTP calls.
    use swink_agent_plugin_web::search::DuckDuckGoProvider;

    const FIXTURE_HTML: &str = r#"
    <html><body>
    <table>
      <tr>
        <td>
          <a class="result-link" href="https://example.com/one">First Result</a>
        </td>
        <td class="result-snippet">This is the first snippet.</td>
      </tr>
      <tr>
        <td>
          <a class="result-link" href="https://example.com/two">Second Result</a>
        </td>
        <td class="result-snippet">This is the second snippet.</td>
      </tr>
      <tr>
        <td>
          <a class="result-link" href="https://example.com/three">Third Result</a>
        </td>
        <td class="result-snippet">This is the third snippet.</td>
      </tr>
    </table>
    </body></html>
    "#;

    #[test]
    fn parses_result_links_and_snippets() {
        let results = DuckDuckGoProvider::parse_results(FIXTURE_HTML, 10);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "First Result");
        assert_eq!(results[0].url, "https://example.com/one");
        assert_eq!(results[0].snippet, "This is the first snippet.");
        assert_eq!(results[1].title, "Second Result");
        assert_eq!(results[2].url, "https://example.com/three");
    }

    #[test]
    fn respects_max_results_limit() {
        let results = DuckDuckGoProvider::parse_results(FIXTURE_HTML, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn returns_empty_for_no_matches() {
        let html = "<html><body><p>No search results here.</p></body></html>";
        let results = DuckDuckGoProvider::parse_results(html, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn fallback_parsing_for_plain_table_links() {
        // HTML without result-link class — exercises the fallback path.
        let html = r#"
        <html><body>
        <table>
          <tr>
            <td><a href="https://fallback.example.com">Fallback Title</a></td>
            <td>Some extra text for snippet</td>
          </tr>
        </table>
        </body></html>
        "#;
        let results = DuckDuckGoProvider::parse_results(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://fallback.example.com");
        assert_eq!(results[0].title, "Fallback Title");
    }
}

// ─── SearchTool formatting ──────────────────────────────────────────────────

mod search_tool_formatting {
    use swink_agent_plugin_web::search::SearchResult;
    use swink_agent_plugin_web::tools::SearchTool;

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
        assert!(formatted.contains("https://rust-lang.org"));
        assert!(formatted.contains("2. **Crates.io**"));
        assert!(formatted.contains("Rust package registry."));
    }

    #[test]
    fn empty_results_produce_empty_string() {
        let formatted = SearchTool::format_results(&[]);
        assert!(formatted.is_empty());
    }
}

// ─── SearchTool execute integration (with mock provider) ────────────────────

use swink_agent_plugin_web::search::{SearchError, SearchProvider, SearchResult};

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
        Box<dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>> + Send + '_>,
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
        Box<dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>> + Send + '_>,
    > {
        Box::pin(async move { Err(SearchError::NetworkError("connection refused".into())) })
    }
}

#[tokio::test]
async fn execute_returns_formatted_results() {
    use swink_agent_plugin_web::tools::SearchTool;

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
async fn execute_returns_no_results_message() {
    use swink_agent_plugin_web::tools::SearchTool;

    let provider = Arc::new(MockProvider { results: vec![] });
    let tool = SearchTool::new(provider, 10);
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
    let result = tool
        .execute(
            "call-2",
            json!({"query": "nothing"}),
            CancellationToken::new(),
            None,
            state,
            None,
        )
        .await;

    assert!(!result.is_error);
    let text = format!("{:?}", result.content);
    assert!(text.contains("No results found"));
}

#[tokio::test]
async fn execute_returns_error_on_provider_failure() {
    use swink_agent_plugin_web::tools::SearchTool;

    let provider = Arc::new(FailingProvider);
    let tool = SearchTool::new(provider, 10);
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
    let result = tool
        .execute(
            "call-3",
            json!({"query": "fail"}),
            CancellationToken::new(),
            None,
            state,
            None,
        )
        .await;

    assert!(result.is_error);
    let text = format!("{:?}", result.content);
    assert!(text.contains("connection refused"));
}

#[tokio::test]
async fn execute_returns_error_for_empty_query() {
    use swink_agent_plugin_web::tools::SearchTool;

    let provider = Arc::new(MockProvider { results: vec![] });
    let tool = SearchTool::new(provider, 10);
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
    let result = tool
        .execute(
            "call-4",
            json!({}),
            CancellationToken::new(),
            None,
            state,
            None,
        )
        .await;

    assert!(result.is_error);
    let text = format!("{:?}", result.content);
    assert!(text.contains("Missing required parameter"));
}
