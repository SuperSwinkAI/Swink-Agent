use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use url::Url;

use crate::content::{extract_readable_content, is_html_content_type, truncate_content};
use crate::domain::{DomainFilter, ResolvedHost};
use crate::policy::ContentSanitizerPolicy;
use crate::tools::sanitize_web_tool_text;

/// Tool for fetching and reading web pages.
///
/// Sends an HTTP GET request, extracts readable content from HTML responses
/// using the readability algorithm, and returns clean text with navigation,
/// ads, and boilerplate removed.
pub struct FetchTool {
    client: reqwest::Client,
    max_content_length: usize,
    request_timeout: Duration,
    domain_filter: Option<DomainFilter>,
    max_redirects: usize,
    sanitizer: Option<ContentSanitizerPolicy>,
    user_agent: Option<String>,
    schema: Value,
}

impl FetchTool {
    /// Create a new `FetchTool` with the given content length limit and
    /// request timeout.
    ///
    /// The tool always builds its own no-redirect HTTP clients internally:
    /// fetch redirects must be observed and validated by this tool before any
    /// follow-up request is sent, so callers cannot supply a pre-configured
    /// client. Use [`FetchTool::with_user_agent`] to set the `User-Agent`
    /// header on every request the tool makes.
    pub fn new(max_content_length: usize, request_timeout: Duration) -> Self {
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
            client: Self::build_no_redirect_client(request_timeout, None),
            max_content_length,
            request_timeout,
            domain_filter: Some(DomainFilter::blocking_private_ips()),
            max_redirects: 10,
            sanitizer: Some(ContentSanitizerPolicy::new()),
            user_agent: None,
            schema,
        }
    }

    fn build_no_redirect_client(
        request_timeout: Duration,
        user_agent: Option<&str>,
    ) -> reqwest::Client {
        crate::ensure_default_crypto_provider();
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(request_timeout);
        if let Some(user_agent) = user_agent {
            builder = builder.user_agent(user_agent);
        }
        builder
            .build()
            .expect("building no-redirect fetch HTTP client should not fail")
    }

    /// Set the `User-Agent` header sent with every fetch request.
    ///
    /// Applies to both the fallback client and the DNS-pinned per-request
    /// clients built for SSRF protection.
    #[must_use]
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        let user_agent = user_agent.into();
        self.client = Self::build_no_redirect_client(self.request_timeout, Some(&user_agent));
        self.user_agent = Some(user_agent);
        self
    }

    /// Re-validate initial and redirect targets inside the tool.
    #[must_use]
    pub fn with_domain_filter(mut self, filter: DomainFilter, max_redirects: u32) -> Self {
        self.domain_filter = Some(filter);
        self.max_redirects = max_redirects as usize;
        self
    }

    /// Enable or disable prompt-injection sanitization of fetched text.
    #[must_use]
    pub fn with_sanitizer_enabled(mut self, enabled: bool) -> Self {
        self.sanitizer = enabled.then(ContentSanitizerPolicy::new);
        self
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

    async fn send_get_following_checked_redirects(
        &self,
        initial_url: Url,
        cancellation_token: &CancellationToken,
    ) -> Result<(reqwest::Response, Url), String> {
        let mut current_url = initial_url;

        for redirect_count in 0..=self.max_redirects {
            let phase = if redirect_count == 0 {
                "Initial"
            } else {
                "Redirect"
            };
            let resolved_host = self.validate_url_for_fetch(&current_url, phase)?;
            let client = self.client_for_request(resolved_host)?;

            let request = client
                .get(current_url.clone())
                .timeout(self.request_timeout);

            let response = tokio::select! {
                result = request.send() => {
                    match result {
                        Ok(resp) => resp,
                        Err(e) => return Err(format!("HTTP request failed: {e}")),
                    }
                }
                () = cancellation_token.cancelled() => {
                    return Err("Request cancelled".to_string());
                }
            };

            if !response.status().is_redirection() {
                return Ok((response, current_url));
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    format!(
                        "Redirect response from {current_url} did not include a valid Location header"
                    )
                })?;

            current_url = current_url
                .join(location)
                .map_err(|error| format!("Invalid redirect Location '{location}': {error}"))?;
        }

        Err(format!(
            "Too many redirects while fetching URL; limit is {}",
            self.max_redirects
        ))
    }

    fn validate_url_for_fetch(
        &self,
        url: &Url,
        phase: &str,
    ) -> Result<Option<ResolvedHost>, String> {
        let Some(filter) = self.domain_filter.as_ref() else {
            return Ok(None);
        };

        filter
            .validate_and_resolve(url)
            .map_err(|error| format!("{phase} URL blocked by domain filter: {error}"))
    }

    fn client_for_request(
        &self,
        resolved_host: Option<ResolvedHost>,
    ) -> Result<reqwest::Client, String> {
        let Some(resolved_host) = resolved_host else {
            return Ok(self.client.clone());
        };

        crate::ensure_default_crypto_provider();
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(self.request_timeout)
            .resolve(&resolved_host.host, resolved_host.addr);
        if let Some(user_agent) = self.user_agent.as_deref() {
            builder = builder.user_agent(user_agent);
        }
        builder
            .build()
            .map_err(|error| format!("Failed to build pinned HTTP client: {error}"))
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

    // One linear request flow (validate → fetch → redirect checks → cap →
    // sanitize); splitting it would scatter the security checks across helpers.
    #[allow(clippy::too_many_lines)]
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

            // FR-016: every web request must log its target, status, size, and
            // latency. The timer starts here, right before network I/O begins.
            let start = Instant::now();

            let (mut response, final_url) = match self
                .send_get_following_checked_redirects(parsed_url, &cancellation_token)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    warn!(
                        url = %url_str,
                        latency_ms = start.elapsed().as_millis(),
                        error = %error,
                        "web fetch request failed"
                    );
                    return AgentToolResult::error(error);
                }
            };

            let status = response.status();
            if !status.is_success() {
                warn!(
                    url = %final_url,
                    status = status.as_u16(),
                    latency_ms = start.elapsed().as_millis(),
                    "web fetch returned non-success status"
                );
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
                info!(
                    url = %final_url,
                    status = status.as_u16(),
                    content_type = %content_type,
                    latency_ms = start.elapsed().as_millis(),
                    "web fetch returned non-HTML content type"
                );
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
                Err(error) => {
                    warn!(
                        url = %final_url,
                        status = status.as_u16(),
                        latency_ms = start.elapsed().as_millis(),
                        error = %error,
                        "web fetch body read failed"
                    );
                    return AgentToolResult::error(error);
                }
            };
            let size_bytes = bytes.len();

            // Extract readable content.
            let fetched = match extract_readable_content(&bytes, &final_url) {
                Ok(f) => f,
                Err(e) => {
                    warn!(
                        url = %final_url,
                        status = status.as_u16(),
                        size_bytes,
                        latency_ms = start.elapsed().as_millis(),
                        error = %e,
                        "web fetch content extraction failed"
                    );
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

            let output = sanitize_web_tool_text("web_fetch", output, self.sanitizer.as_ref());

            info!(
                url = %final_url,
                status = status.as_u16(),
                size_bytes,
                truncated,
                latency_ms = start.elapsed().as_millis(),
                "web fetch completed"
            );

            AgentToolResult::text(output)
        })
    }
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
