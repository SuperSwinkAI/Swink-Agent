use serde::Deserialize;

use super::{SearchError, SearchProvider, SearchResult};

/// Brave Search API provider.
///
/// Requires a Brave Search API subscription token.
#[derive(Debug)]
pub struct BraveProvider {
    pub(crate) api_key: String,
    pub(crate) client: reqwest::Client,
}

impl BraveProvider {
    pub fn new(api_key: String, client: reqwest::Client) -> Self {
        Self { api_key, client }
    }
}

// ─── Brave API response types ───────────────────────────────────────────────

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveWebResult>,
}

#[derive(Deserialize)]
struct BraveWebResult {
    title: String,
    url: String,
    description: Option<String>,
}

// ─── SearchProvider impl ────────────────────────────────────────────────────

impl SearchProvider for BraveProvider {
    fn name(&self) -> &str {
        "brave"
    }

    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>> + Send + '_>,
    > {
        let query = query.to_owned();
        Box::pin(async move {
            if self.api_key.is_empty() {
                return Err(SearchError::ApiKeyMissing);
            }

            let encoded_query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("q", &query)
                .append_pair("count", &max_results.to_string())
                .finish();
            let url = format!("https://api.search.brave.com/res/v1/web/search?{encoded_query}");
            let response = self
                .client
                .get(&url)
                .header("X-Subscription-Token", &self.api_key)
                .header("Accept", "application/json")
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
                    "Brave returned status {}",
                    response.status()
                )));
            }

            let body_text = response
                .text()
                .await
                .map_err(|e| SearchError::ParseError(e.to_string()))?;
            let body: BraveResponse = serde_json::from_str(&body_text)
                .map_err(|e| SearchError::ParseError(e.to_string()))?;

            let results = body
                .web
                .map(|w| {
                    w.results
                        .into_iter()
                        .take(max_results)
                        .map(|r| SearchResult {
                            title: r.title,
                            url: r.url,
                            snippet: r.description.unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(results)
        })
    }
}
