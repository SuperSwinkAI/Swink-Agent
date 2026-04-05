use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use swink_agent::{AgentTool, AgentToolResult, ToolFuture};
use swink_agent::types::{ContentBlock, ImageSource};

use crate::playwright::{PlaywrightBridge, PlaywrightError, Viewport};

/// Default viewport: 1280x720.
const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;

/// Default timeout for the screenshot operation.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Tool for taking screenshots of web pages.
///
/// Lazily starts a Playwright bridge subprocess on first use and reuses it for
/// subsequent calls.
pub struct ScreenshotTool {
    bridge: Arc<Mutex<Option<PlaywrightBridge>>>,
    playwright_path: Option<PathBuf>,
    default_viewport: Viewport,
    timeout: Duration,
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
            schema: build_schema(),
        }
    }

    /// Create a `ScreenshotTool` with sensible defaults and a fresh bridge slot.
    pub fn with_defaults() -> Self {
        Self::new(
            Arc::new(Mutex::new(None)),
            None,
            Viewport {
                width: DEFAULT_WIDTH,
                height: DEFAULT_HEIGHT,
            },
            DEFAULT_TIMEOUT,
        )
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
        _credential: Option<swink_agent::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            // Parse parameters.
            let url = match params.get("url").and_then(|v| v.as_str()) {
                Some(u) => u.to_owned(),
                None => return AgentToolResult::error("missing required parameter: url"),
            };

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
                        return AgentToolResult::error(format!("Failed to start browser: {e}"));
                    }
                }
            }

            let bridge = guard.as_mut().expect("bridge initialized above");

            // Apply tool-level timeout.
            let result =
                tokio::time::timeout(self.timeout, bridge.screenshot(&url, viewport)).await;

            match result {
                Ok(Ok(base64_data)) => AgentToolResult {
                    content: vec![ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".into(),
                            data: base64_data,
                        },
                    }],
                    details: serde_json::json!({
                        "url": url,
                        "width": width,
                        "height": height,
                    }),
                    is_error: false,
                    transfer_signal: None,
                },
                Ok(Err(PlaywrightError::NotInstalled)) => AgentToolResult::error(
                    "Playwright/Node.js not found. Install with:\n\
                     npm install -g playwright && npx playwright install chromium",
                ),
                Ok(Err(e)) => AgentToolResult::error(format!("Screenshot failed: {e}")),
                Err(_) => AgentToolResult::error(format!(
                    "Screenshot timed out after {:?}",
                    self.timeout
                )),
            }
        })
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
