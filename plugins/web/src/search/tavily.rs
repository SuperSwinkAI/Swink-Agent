use serde::{Deserialize, Serialize};

use super::{SearchError, SearchProvider, SearchResult};

/// Tavily Search API provider.
///
/// Requires a Tavily API key.
#[derive(Debug)]
pub struct TavilyProvider {
    pub(crate) api_key: String,
    pub(crate) client: reqwest::Client,
}

impl TavilyProvider {
    pub fn new(api_key: String, client: reqwest::Client) -> Self {
        Self { api_key, client }
    }
}

// ─── Tavily API request/response types ──────────────────────────────────────

#[derive(Serialize)]
struct TavilyRequest<'a> {
    api_key: &'a str,
    query: &'a str,
    max_results: usize,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: Option<String>,
}

// ─── SearchProvider impl ────────────────────────────────────────────────────

impl SearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>> + Send + '_>> {
        let query = query.to_owned();
        Box::pin(async move {
        if self.api_key.is_empty() {
            return Err(SearchError::ApiKeyMissing);
        }

        let request_body = TavilyRequest {
            api_key: &self.api_key,
            query: &query,
            max_results,
        };

        let body_json = serde_json::to_string(&request_body)
            .map_err(|e| SearchError::ParseError(e.to_string()))?;
        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .body(body_json)
            .send()
            .await
            .map_err(|e| SearchError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SearchError::RateLimited);
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(SearchError::ApiKeyMissing);
        }

        if !response.status().is_success() {
            return Err(SearchError::ProviderUnavailable(format!(
                "Tavily returned status {}",
                response.status()
            )));
        }

        let body_text = response
            .text()
            .await
            .map_err(|e| SearchError::ParseError(e.to_string()))?;
        let body: TavilyResponse = serde_json::from_str(&body_text)
            .map_err(|e| SearchError::ParseError(e.to_string()))?;

        let results = body
            .results
            .into_iter()
            .take(max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.unwrap_or_default(),
            })
            .collect();

        Ok(results)
        })
    }
}
