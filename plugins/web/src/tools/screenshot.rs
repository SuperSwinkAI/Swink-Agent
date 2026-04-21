use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use swink_agent::{AgentTool, AgentToolResult, ContentBlock, ImageSource, ToolFuture};

use crate::playwright::{PlaywrightBridge, PlaywrightError, Viewport};

enum OperationOutcome<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

async fn await_with_cancellation<F, T>(
    cancellation_token: &CancellationToken,
    timeout: Duration,
    future: F,
) -> OperationOutcome<T>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        result = tokio::time::timeout(timeout, future) => match result {
            Ok(value) => OperationOutcome::Completed(value),
            Err(_) => OperationOutcome::TimedOut,
        },
        () = cancellation_token.cancelled() => OperationOutcome::Cancelled,
    }
}

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
                    bridge.screenshot(&url, viewport),
                )
                .await
            };

            match operation {
                OperationOutcome::Completed(Ok(base64_data)) => AgentToolResult {
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
                OperationOutcome::Completed(Err(PlaywrightError::NotInstalled)) => {
                    AgentToolResult::error(
                        "Playwright/Node.js not found. Install with:\n\
                     npm install -g playwright && npx playwright install chromium",
                    )
                }
                OperationOutcome::Completed(Err(e)) => {
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
    use std::future::pending;
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::{OperationOutcome, await_with_cancellation};

    #[tokio::test]
    async fn await_with_cancellation_returns_cancelled_before_completion() {
        let cancellation_token = CancellationToken::new();
        cancellation_token.cancel();

        let outcome =
            await_with_cancellation(&cancellation_token, Duration::from_secs(1), pending::<()>())
                .await;

        assert!(matches!(outcome, OperationOutcome::Cancelled));
    }

    #[tokio::test]
    async fn await_with_cancellation_returns_timed_out_for_slow_operations() {
        let outcome = await_with_cancellation(
            &CancellationToken::new(),
            Duration::from_millis(10),
            tokio::time::sleep(Duration::from_millis(50)),
        )
        .await;

        assert!(matches!(outcome, OperationOutcome::TimedOut));
    }
}
