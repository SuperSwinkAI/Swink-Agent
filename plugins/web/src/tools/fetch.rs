use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::content::{extract_readable_content, is_html_content_type, truncate_content};

/// Tool for fetching and reading web pages.
///
/// Sends an HTTP GET request, extracts readable content from HTML responses
/// using the readability algorithm, and returns clean text with navigation,
/// ads, and boilerplate removed.
pub struct FetchTool {
    client: reqwest::Client,
    max_content_length: usize,
    request_timeout: Duration,
    schema: Value,
}

impl FetchTool {
    /// Create a new `FetchTool` with the given HTTP client, content length limit,
    /// and request timeout.
    pub fn new(
        client: reqwest::Client,
        max_content_length: usize,
        request_timeout: Duration,
    ) -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch. Must be http:// or https://."
                }
            },
            "required": ["url"]
        });
        Self {
            client,
            max_content_length,
            request_timeout,
            schema,
        }
    }

    async fn read_body_with_cap(
        response: &mut reqwest::Response,
        max_bytes: usize,
        cancellation_token: &CancellationToken,
    ) -> Result<Vec<u8>, String> {
        let mut body = Vec::with_capacity(max_bytes.min(8 * 1024));

        while let Some(chunk) = tokio::select! {
            result = response.chunk() => {
                match result {
                    Ok(next) => next,
                    Err(error) => {
                        return Err(format!("Failed to read response body: {error}"));
                    }
                }
            }
            () = cancellation_token.cancelled() => {
                return Err("Request cancelled".to_string());
            }
        } {
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(format!(
                    "Response body exceeded configured limit of {max_bytes} bytes before readability extraction."
                ));
            }

            body.extend_from_slice(&chunk);
        }

        Ok(body)
    }
}

impl AgentTool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }

    fn label(&self) -> &str {
        "Fetch Web Page"
    }

    fn description(&self) -> &str {
        "Fetch a web page and return its main content as clean, readable text. \
         Navigation, ads, scripts, and boilerplate are automatically removed."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: tokio_util::sync::CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            // Extract URL from params.
            let url_str = match params.get("url").and_then(Value::as_str) {
                Some(u) => u,
                None => return AgentToolResult::error("Missing required parameter: url"),
            };

            // Parse URL.
            let parsed_url = match url::Url::parse(url_str) {
                Ok(u) => u,
                Err(e) => return AgentToolResult::error(format!("Invalid URL: {e}")),
            };

            // Validate scheme.
            match parsed_url.scheme() {
                "http" | "https" => {}
                scheme => {
                    return AgentToolResult::error(format!(
                        "Unsupported URL scheme '{scheme}'. Only http:// and https:// are supported."
                    ));
                }
            }

            // Send GET request with timeout.
            let request = self
                .client
                .get(parsed_url.clone())
                .timeout(self.request_timeout);

            let mut response = tokio::select! {
                result = request.send() => {
                    match result {
                        Ok(resp) => resp,
                        Err(e) => return AgentToolResult::error(format!("HTTP request failed: {e}")),
                    }
                }
                () = cancellation_token.cancelled() => {
                    return AgentToolResult::error("Request cancelled");
                }
            };

            let status = response.status();
            if !status.is_success() {
                return AgentToolResult::error(format!(
                    "HTTP {}: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown error")
                ));
            }

            // Check Content-Type header.
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            if !is_html_content_type(&content_type) {
                return AgentToolResult::text(format!(
                    "This URL points to a {content_type} resource. \
                     Only HTML pages can be fetched and extracted."
                ));
            }

            // Bound the raw response body before readability extraction so the
            // configured content limit caps network and parsing cost too.
            let bytes = match Self::read_body_with_cap(
                &mut response,
                self.max_content_length,
                &cancellation_token,
            )
            .await
            {
                Ok(body) => body,
                Err(error) => return AgentToolResult::error(error),
            };

            // Extract readable content.
            let fetched = match extract_readable_content(&bytes, &parsed_url) {
                Ok(f) => f,
                Err(e) => {
                    warn!("Content extraction failed for {url_str}: {e}");
                    return AgentToolResult::error(format!("Content extraction failed: {e}"));
                }
            };

            // Truncate if needed.
            let (text, truncated) = truncate_content(&fetched.text, self.max_content_length);

            if truncated {
                warn!(
                    "Content from {url_str} was truncated from {} to ~{} chars",
                    fetched.text_length, self.max_content_length
                );
            }

            // Build output with optional title prefix.
            let output = match &fetched.title {
                Some(title) => format!("# {title}\n\n{text}"),
                None => text,
            };

            AgentToolResult::text(output)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    use serde_json::json;
    use swink_agent::{AgentTool, SessionState};
    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::FetchTool;

    #[tokio::test]
    async fn execute_returns_readable_content_for_html_under_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"<!DOCTYPE html>
                        <html>
                        <head><title>Fetch Test</title></head>
                        <body>
                            <article>
                                <p>This is the readable content for the fetch tool test.</p>
                                <p>It should survive readability extraction.</p>
                            </article>
                        </body>
                        </html>"#,
                "text/html; charset=utf-8",
            ))
            .mount(&server)
            .await;

        let tool = FetchTool::new(reqwest::Client::new(), 4_096, Duration::from_secs(5));
        let state = Arc::new(RwLock::new(SessionState::default()));
        let result = tool
            .execute(
                "call-1",
                json!({ "url": format!("{}/article", server.uri()) }),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;

        assert!(!result.is_error);
        let text = format!("{:?}", result.content);
        assert!(text.contains("Fetch Test"));
        assert!(text.contains("readable content for the fetch tool test"));
    }

    #[tokio::test]
    async fn execute_rejects_body_that_exceeds_cap_before_extraction() {
        let server = MockServer::start().await;
        let oversized_html = format!(
            "<!DOCTYPE html><html><body><article><p>{}</p></article></body></html>",
            "x".repeat(2_048)
        );
        Mock::given(method("GET"))
            .and(path("/oversized"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(oversized_html, "text/html; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let tool = FetchTool::new(reqwest::Client::new(), 512, Duration::from_secs(5));
        let state = Arc::new(RwLock::new(SessionState::default()));
        let result = tool
            .execute(
                "call-2",
                json!({ "url": format!("{}/oversized", server.uri()) }),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;

        assert!(result.is_error);
        let text = format!("{:?}", result.content);
        assert!(text.contains("Response body exceeded configured limit of 512 bytes"));
        assert!(text.contains("before readability extraction"));
    }
}
