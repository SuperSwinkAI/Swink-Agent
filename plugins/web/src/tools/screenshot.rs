use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

use swink_agent::{AgentTool, AgentToolResult, ContentBlock, ImageSource, ToolFuture};

use crate::domain::DomainFilter;
use crate::playwright::{PlaywrightBridge, PlaywrightError, ScreenshotOutput, Viewport};
use crate::tools::{
    OperationOutcome, await_with_cancellation, reset_bridge_after_ambiguous_playwright_error,
    validate_url_against_filter,
};

/// Tool for taking screenshots of web pages.
///
/// Lazily starts a Playwright bridge subprocess on first use and reuses it for
/// subsequent calls.
pub struct ScreenshotTool {
    bridge: Arc<Mutex<Option<PlaywrightBridge>>>,
    playwright_path: Option<PathBuf>,
    default_viewport: Viewport,
    timeout: Duration,
    domain_filter: Option<DomainFilter>,
    schema: Value,
}

impl ScreenshotTool {
    /// Create a new `ScreenshotTool`.
    ///
    /// The `bridge` is shared (e.g. with `ExtractTool`) so the same subprocess
    /// serves both tools. Pass `None` inside the mutex for lazy initialization.
    pub fn new(
        bridge: Arc<Mutex<Option<PlaywrightBridge>>>,
        playwright_path: Option<PathBuf>,
        default_viewport: Viewport,
        timeout: Duration,
    ) -> Self {
        Self {
            bridge,
            playwright_path,
            default_viewport,
            timeout,
            domain_filter: Some(DomainFilter::blocking_private_ips()),
            schema: build_schema(),
        }
    }

    /// Re-validate initial and final browser navigation URLs inside the tool.
    #[must_use]
    pub fn with_domain_filter(mut self, filter: DomainFilter) -> Self {
        self.domain_filter = Some(filter);
        self
    }
}

impl AgentTool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn label(&self) -> &str {
        "Screenshot Web Page"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of a web page and return it as an image. \
         Requires Playwright and Node.js to be installed."
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
        Box::pin(async move {
            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("Request cancelled");
            }

            // Parse parameters.
            let url = match params.get("url").and_then(|v| v.as_str()) {
                Some(u) => u.to_owned(),
                None => return AgentToolResult::error("missing required parameter: url"),
            };
            let parsed_url = match Url::parse(&url) {
                Ok(url) => url,
                Err(error) => return AgentToolResult::error(format!("Invalid URL: {error}")),
            };
            if let Err(error) =
                validate_url_against_filter(self.domain_filter.as_ref(), &parsed_url, "Initial")
            {
                return AgentToolResult::error(error);
            }

            let width = params
                .get("width")
                .and_then(|v| v.as_u64())
                .map_or(self.default_viewport.width, |v| v as u32);
            let height = params
                .get("height")
                .and_then(|v| v.as_u64())
                .map_or(self.default_viewport.height, |v| v as u32);

            let viewport = Some(Viewport { width, height });

            // Acquire bridge lock and lazily start if needed.
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
                        return AgentToolResult::error(format!("Failed to start browser: {e}"));
                    }
                }
            }

            let operation = {
                let bridge = guard.as_mut().expect("bridge initialized above");
                await_with_cancellation(
                    &cancellation_token,
                    self.timeout,
                    bridge.screenshot(&url, viewport, self.domain_filter.as_ref()),
                )
                .await
            };

            match operation {
                OperationOutcome::Completed(Ok(screenshot)) => build_screenshot_result(
                    &url,
                    width,
                    height,
                    screenshot,
                    self.domain_filter.as_ref(),
                ),
                OperationOutcome::Completed(Err(PlaywrightError::NotInstalled)) => {
                    AgentToolResult::error(
                        "Playwright/Node.js not found. Install with:\n\
                     npm install -g playwright && npx playwright install chromium",
                    )
                }
                OperationOutcome::Completed(Err(e)) => {
                    reset_bridge_after_ambiguous_playwright_error(&mut guard, &e);
                    AgentToolResult::error(format!("Screenshot failed: {e}"))
                }
                OperationOutcome::Cancelled => {
                    // Dropping the bridge avoids leaving an unread response on stdout after the
                    // request was already written to the subprocess.
                    *guard = None;
                    AgentToolResult::error("Request cancelled")
                }
                OperationOutcome::TimedOut => {
                    *guard = None;
                    AgentToolResult::error(format!("Screenshot timed out after {:?}", self.timeout))
                }
            }
        })
    }
}

fn build_screenshot_result(
    url: &str,
    width: u32,
    height: u32,
    screenshot: ScreenshotOutput,
    domain_filter: Option<&DomainFilter>,
) -> AgentToolResult {
    match Url::parse(&screenshot.final_url) {
        Ok(final_url) => {
            if let Err(error) = validate_url_against_filter(domain_filter, &final_url, "Final") {
                return AgentToolResult::error(error);
            }
        }
        Err(error) => {
            return AgentToolResult::error(format!("Browser returned invalid final URL: {error}"));
        }
    }

    AgentToolResult {
        content: vec![ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: screenshot.base64,
            },
        }],
        details: serde_json::json!({
            "url": url,
            "final_url": screenshot.final_url,
            "width": width,
            "height": height,
        }),
        is_error: false,
        transfer_signal: None,
    }
}

fn build_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The URL of the web page to screenshot."
            },
            "width": {
                "type": "integer",
                "description": "Viewport width in pixels (default: 1280).",
                "minimum": 320,
                "maximum": 3840
            },
            "height": {
                "type": "integer",
                "description": "Viewport height in pixels (default: 720).",
                "minimum": 240,
                "maximum": 2160
            }
        },
        "required": ["url"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_constructor_blocks_private_ips_by_default() {
        let tool = ScreenshotTool::new(
            Arc::new(Mutex::new(None)),
            None,
            Viewport {
                width: 1280,
                height: 720,
            },
            Duration::from_secs(15),
        );

        let filter = tool
            .domain_filter
            .as_ref()
            .expect("direct screenshot tool should install a default filter");
        let localhost = Url::parse("http://127.0.0.1/admin").unwrap();

        assert!(filter.is_allowed(&localhost).is_err());
    }
}
