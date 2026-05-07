use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
use tokio::sync::Mutex;
use tracing::warn;
use url::Url;

use crate::domain::DomainFilter;
use crate::playwright::{ExtractOutput, ExtractionPreset, PlaywrightBridge, PlaywrightError};
use crate::policy::ContentSanitizerPolicy;
use crate::tools::{
    OperationOutcome, await_with_cancellation, reset_bridge_after_ambiguous_playwright_error,
    sanitize_web_tool_text, validate_url_against_filter,
};

struct ExtractRequest {
    url: String,
    selector: Option<String>,
    preset: Option<ExtractionPreset>,
}

/// Tool for extracting structured content from web pages.
///
/// Uses a headless browser (Playwright) to render the page with full JavaScript
/// execution, then extracts elements matching a CSS selector or a built-in preset
/// (links, headings, tables).
pub struct ExtractTool {
    bridge: Arc<Mutex<Option<PlaywrightBridge>>>,
    playwright_path: Option<PathBuf>,
    timeout: Duration,
    domain_filter: Option<DomainFilter>,
    sanitizer: Option<ContentSanitizerPolicy>,
    schema: Value,
}

impl ExtractTool {
    /// Create a new `ExtractTool` with a shared Playwright bridge, optional
    /// path to the Playwright installation, and a request timeout.
    pub fn new(
        bridge: Arc<tokio::sync::Mutex<Option<PlaywrightBridge>>>,
        playwright_path: Option<PathBuf>,
        timeout: Duration,
    ) -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to extract content from. Must be http:// or https://."
                },
                "selector": {
                    "type": "string",
                    "description": "A CSS selector to match elements. Mutually exclusive with 'preset'."
                },
                "preset": {
                    "type": "string",
                    "enum": ["links", "headings", "tables"],
                    "description": "A built-in extraction preset. Mutually exclusive with 'selector'."
                }
            },
            "required": ["url"]
        });
        Self {
            bridge,
            playwright_path,
            timeout,
            domain_filter: None,
            sanitizer: Some(ContentSanitizerPolicy::new()),
            schema,
        }
    }

    /// Re-validate initial and final browser navigation URLs inside the tool.
    #[must_use]
    pub fn with_domain_filter(mut self, filter: DomainFilter) -> Self {
        self.domain_filter = Some(filter);
        self
    }

    /// Enable or disable prompt-injection sanitization of extracted text.
    #[must_use]
    pub fn with_sanitizer_enabled(mut self, enabled: bool) -> Self {
        self.sanitizer = enabled.then(ContentSanitizerPolicy::new);
        self
    }
}

impl AgentTool for ExtractTool {
    fn name(&self) -> &str {
        "extract"
    }

    fn label(&self) -> &str {
        "Extract Web Content"
    }

    fn description(&self) -> &str {
        "Extract structured content from a web page using CSS selectors or presets \
         (links, headings, tables). Uses a headless browser for full JavaScript rendering."
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
            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("Request cancelled");
            }

            let request = match parse_extract_params(&params) {
                Ok(request) => request,
                Err(error) => return AgentToolResult::error(error),
            };
            let parsed_url = match Url::parse(&request.url) {
                Ok(url) => url,
                Err(error) => return AgentToolResult::error(format!("Invalid URL: {error}")),
            };
            if let Err(error) =
                validate_url_against_filter(self.domain_filter.as_ref(), &parsed_url, "Initial")
            {
                return AgentToolResult::error(error);
            }

            // Lazily start the bridge if not already running.
            let mut guard = tokio::select! {
                guard = self.bridge.lock() => guard,
                () = cancellation_token.cancelled() => {
                    return AgentToolResult::error("Request cancelled");
                }
            };
            if guard.is_none() {
                let bridge_start = tokio::select! {
                    result = PlaywrightBridge::start(self.playwright_path.as_deref()) => result,
                    () = cancellation_token.cancelled() => {
                        return AgentToolResult::error("Request cancelled");
                    }
                };

                match bridge_start {
                    Ok(b) => *guard = Some(b),
                    Err(PlaywrightError::NotInstalled) => {
                        return AgentToolResult::error(
                            "Playwright/Node.js not found. Install with:\n\
                             npm install -g playwright && npx playwright install chromium",
                        );
                    }
                    Err(e) => {
                        return AgentToolResult::error(format!(
                            "Failed to start Playwright bridge: {e}"
                        ));
                    }
                }
            }

            let operation = {
                let bridge = guard.as_mut().expect("bridge initialized above");
                await_with_cancellation(
                    &cancellation_token,
                    self.timeout,
                    bridge.extract(
                        &request.url,
                        request.selector.as_deref(),
                        request.preset,
                        self.domain_filter.as_ref(),
                    ),
                )
                .await
            };

            match operation {
                OperationOutcome::Completed(Ok(extraction)) => build_extract_result(
                    extraction,
                    self.domain_filter.as_ref(),
                    self.sanitizer.as_ref(),
                ),
                OperationOutcome::Completed(Err(PlaywrightError::NotInstalled)) => {
                    AgentToolResult::error(
                        "Playwright/Node.js not found. Install with:\n\
                     npm install -g playwright && npx playwright install chromium",
                    )
                }
                OperationOutcome::Completed(Err(e)) => {
                    reset_bridge_after_ambiguous_playwright_error(&mut guard, &e);
                    AgentToolResult::error(format!("Extraction failed: {e}"))
                }
                OperationOutcome::Cancelled => {
                    *guard = None;
                    AgentToolResult::error("Request cancelled")
                }
                OperationOutcome::TimedOut => {
                    *guard = None;
                    AgentToolResult::error(format!("Extraction timed out after {:?}", self.timeout))
                }
            }
        })
    }
}

fn build_extract_result(
    extraction: ExtractOutput,
    domain_filter: Option<&DomainFilter>,
    sanitizer: Option<&ContentSanitizerPolicy>,
) -> AgentToolResult {
    let ExtractOutput {
        elements,
        final_url,
    } = extraction;

    match Url::parse(&final_url) {
        Ok(final_url) => {
            if let Err(error) = validate_url_against_filter(domain_filter, &final_url, "Final") {
                return AgentToolResult::error(error);
            }
        }
        Err(error) => {
            return AgentToolResult::error(format!("Browser returned invalid final URL: {error}"));
        }
    }

    if elements.is_empty() {
        return AgentToolResult::text("No elements found matching the given criteria.");
    }

    match serde_json::to_string_pretty(&elements) {
        Ok(json) => {
            let json = sanitize_web_tool_text("web_extract", json, sanitizer);
            AgentToolResult::text(json)
        }
        Err(e) => {
            warn!("Failed to serialize extracted elements: {e}");
            AgentToolResult::error(format!("Failed to serialize extraction results: {e}"))
        }
    }
}

fn parse_extract_params(params: &Value) -> Result<ExtractRequest, String> {
    let url = params
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required parameter: url".to_owned())?
        .to_owned();

    let selector = params
        .get("selector")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let preset = match params.get("preset").and_then(Value::as_str) {
        Some("links") => Some(ExtractionPreset::Links),
        Some("headings") => Some(ExtractionPreset::Headings),
        Some("tables") => Some(ExtractionPreset::Tables),
        Some(other) => {
            return Err(format!(
                "Unknown preset '{other}'. Valid values: links, headings, tables."
            ));
        }
        None => None,
    };

    if selector.is_some() && preset.is_some() {
        return Err(
            "Parameters 'selector' and 'preset' are mutually exclusive. Provide one or neither."
                .to_owned(),
        );
    }

    Ok(ExtractRequest {
        url,
        selector,
        preset,
    })
}
