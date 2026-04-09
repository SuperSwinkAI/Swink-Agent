use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
use tracing::warn;

use crate::playwright::{ExtractionPreset, PlaywrightBridge, PlaywrightError};

/// Tool for extracting structured content from web pages.
///
/// Uses a headless browser (Playwright) to render the page with full JavaScript
/// execution, then extracts elements matching a CSS selector or a built-in preset
/// (links, headings, tables).
pub struct ExtractTool {
    bridge: Arc<tokio::sync::Mutex<Option<PlaywrightBridge>>>,
    playwright_path: Option<PathBuf>,
    _timeout: Duration,
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
            _timeout: timeout,
            schema,
        }
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
            // Extract URL from params.
            let url = match params.get("url").and_then(Value::as_str) {
                Some(u) => u,
                None => return AgentToolResult::error("Missing required parameter: url"),
            };

            // Extract optional selector.
            let selector = params.get("selector").and_then(Value::as_str);

            // Extract and map optional preset.
            let preset = match params.get("preset").and_then(Value::as_str) {
                Some("links") => Some(ExtractionPreset::Links),
                Some("headings") => Some(ExtractionPreset::Headings),
                Some("tables") => Some(ExtractionPreset::Tables),
                Some(other) => {
                    return AgentToolResult::error(format!(
                        "Unknown preset '{other}'. Valid values: links, headings, tables."
                    ));
                }
                None => None,
            };

            // Validate mutual exclusivity.
            if selector.is_some() && preset.is_some() {
                return AgentToolResult::error(
                    "Parameters 'selector' and 'preset' are mutually exclusive. Provide one or neither.",
                );
            }

            // Lazily start the bridge if not already running.
            let mut guard = self.bridge.lock().await;
            if guard.is_none() {
                match PlaywrightBridge::start(self.playwright_path.as_deref()).await {
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

            let bridge = guard.as_mut().expect("bridge was just initialized");

            // Call the bridge extract method.
            let result = tokio::select! {
                result = bridge.extract(url, selector, preset) => result,
                () = cancellation_token.cancelled() => {
                    return AgentToolResult::error("Request cancelled");
                }
            };

            match result {
                Ok(elements) => {
                    if elements.is_empty() {
                        return AgentToolResult::text(
                            "No elements found matching the given criteria.",
                        );
                    }

                    match serde_json::to_string_pretty(&elements) {
                        Ok(json) => AgentToolResult::text(json),
                        Err(e) => {
                            warn!("Failed to serialize extracted elements: {e}");
                            AgentToolResult::error(format!(
                                "Failed to serialize extraction results: {e}"
                            ))
                        }
                    }
                }
                Err(PlaywrightError::NotInstalled) => AgentToolResult::error(
                    "Playwright/Node.js not found. Install with:\n\
                     npm install -g playwright && npx playwright install chromium",
                ),
                Err(e) => AgentToolResult::error(format!("Extraction failed: {e}")),
            }
        })
    }
}
