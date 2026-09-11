use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};

use crate::domain::DomainFilter;

/// Viewport dimensions for screenshots.
///
/// Deliberately exhaustive (never `#[non_exhaustive]`): a plain
/// width x height pair with no further fields expected, so it is safe to
/// construct this struct literally.
#[allow(clippy::exhaustive_structs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// Preset extraction types.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractionPreset {
    Links,
    Headings,
    Tables,
}

/// A single extracted element from a web page.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedElement {
    pub tag: String,
    pub text: String,
    pub attributes: HashMap<String, String>,
}

/// Screenshot response plus the browser's final URL after redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotOutput {
    pub base64: String,
    pub final_url: String,
}

/// Extraction response plus the browser's final URL after redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractOutput {
    pub elements: Vec<ExtractedElement>,
    pub final_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserNavigationFilter {
    allowlist: Vec<String>,
    denylist: Vec<String>,
    block_private_ips: bool,
}

/// Request sent to the Playwright bridge subprocess.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum PlaywrightRequest {
    Screenshot {
        id: u64,
        url: String,
        viewport: Option<Viewport>,
        filter: Option<BrowserNavigationFilter>,
    },
    Extract {
        id: u64,
        url: String,
        selector: Option<String>,
        preset: Option<ExtractionPreset>,
        filter: Option<BrowserNavigationFilter>,
    },
    Ping {
        id: u64,
    },
}

/// Response from the Playwright bridge subprocess.
#[derive(Debug, Deserialize)]
pub struct PlaywrightResponse {
    pub id: u64,
    pub ok: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Errors from the Playwright bridge.
#[derive(Debug, thiserror::Error)]
pub enum PlaywrightError {
    #[error(
        "Playwright/Node.js not found. Install with: npm install -g playwright && npx playwright install chromium"
    )]
    NotInstalled,
    #[error("Bridge communication error: {0}")]
    Communication(String),
    #[error("Bridge returned error: {0}")]
    BridgeError(String),
    #[error("Operation timed out after {0:?}")]
    Timeout(Duration),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Default timeout for bridge operations (30 seconds).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const BRIDGE_SCRIPT: &str = include_str!("playwright_bridge.js");
static BRIDGE_SCRIPT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Bridge to a Playwright Node.js subprocess for headless browser operations.
///
/// Spawns a child process running the embedded `playwright_bridge.js` script
/// and communicates via JSON lines on stdin/stdout.
pub struct PlaywrightBridge {
    _child: Child,
    stdin: BufWriter<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: AtomicU64,
    /// Path to the temp JS file — kept alive for the process lifetime.
    _bridge_script: PathBuf,
}

impl PlaywrightBridge {
    /// Start the Playwright bridge subprocess.
    ///
    /// Writes the embedded bridge JS to a temp file, spawns `node` to run it,
    /// and verifies the bridge is alive with a ping.
    ///
    /// `playwright_path` optionally overrides the Node.js binary path.
    pub async fn start(playwright_path: Option<&Path>) -> Result<Self, PlaywrightError> {
        let script_path = write_bridge_script_temp_file().await?;

        // Resolve node binary path.
        let node_path = resolve_node_path(playwright_path);

        // Spawn the bridge process.
        let mut child = Command::new(&node_path)
            .arg(&script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    PlaywrightError::NotInstalled
                } else {
                    PlaywrightError::Io(e)
                }
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PlaywrightError::Communication("failed to open stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PlaywrightError::Communication("failed to open stdout".into()))?;

        let mut bridge = Self {
            _child: child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            _bridge_script: script_path,
        };

        // Verify the bridge is alive.
        let id = bridge.next_id();
        let resp = bridge.send_request(PlaywrightRequest::Ping { id }).await?;
        if !resp.ok {
            return Err(PlaywrightError::Communication(
                resp.error.unwrap_or_else(|| "ping failed".into()),
            ));
        }

        Ok(bridge)
    }

    /// Send a request to the bridge and return the response.
    ///
    /// Applies a 30-second timeout to the entire operation.
    pub async fn send_request(
        &mut self,
        request: PlaywrightRequest,
    ) -> Result<PlaywrightResponse, PlaywrightError> {
        let result = tokio::time::timeout(DEFAULT_TIMEOUT, self.send_request_inner(request)).await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(PlaywrightError::Timeout(DEFAULT_TIMEOUT)),
        }
    }

    /// Take a screenshot of a web page and return the base64-encoded PNG.
    pub async fn screenshot(
        &mut self,
        url: &str,
        viewport: Option<Viewport>,
        domain_filter: Option<&DomainFilter>,
    ) -> Result<ScreenshotOutput, PlaywrightError> {
        let id = self.next_id();
        let resp = self
            .send_request(PlaywrightRequest::Screenshot {
                id,
                url: url.to_owned(),
                viewport,
                filter: domain_filter.map(BrowserNavigationFilter::from),
            })
            .await?;

        if !resp.ok {
            return Err(PlaywrightError::BridgeError(
                resp.error.unwrap_or_else(|| "screenshot failed".into()),
            ));
        }

        parse_screenshot_data(resp.data, url)
    }

    /// Extract elements from a web page using a CSS selector or preset.
    pub async fn extract(
        &mut self,
        url: &str,
        selector: Option<&str>,
        preset: Option<ExtractionPreset>,
        domain_filter: Option<&DomainFilter>,
    ) -> Result<ExtractOutput, PlaywrightError> {
        let id = self.next_id();
        let resp = self
            .send_request(PlaywrightRequest::Extract {
                id,
                url: url.to_owned(),
                selector: selector.map(String::from),
                preset,
                filter: domain_filter.map(BrowserNavigationFilter::from),
            })
            .await?;

        if !resp.ok {
            return Err(PlaywrightError::BridgeError(
                resp.error.unwrap_or_else(|| "extraction failed".into()),
            ));
        }

        parse_extract_data(resp.data, url)
    }

    // ── Internals ──────────────────────────────────────────────────────────

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_request_inner(
        &mut self,
        request: PlaywrightRequest,
    ) -> Result<PlaywrightResponse, PlaywrightError> {
        let mut line = serde_json::to_string(&request)
            .map_err(|e| PlaywrightError::Communication(format!("serialize error: {e}")))?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| PlaywrightError::Communication(format!("write error: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| PlaywrightError::Communication(format!("flush error: {e}")))?;

        let mut response_line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut response_line)
            .await
            .map_err(|e| PlaywrightError::Communication(format!("read error: {e}")))?;

        if bytes_read == 0 {
            return Err(PlaywrightError::Communication(
                "bridge process closed stdout".into(),
            ));
        }

        let response: PlaywrightResponse = serde_json::from_str(&response_line)
            .map_err(|e| PlaywrightError::Communication(format!("deserialize error: {e}")))?;

        let expected_id = match &request {
            PlaywrightRequest::Screenshot { id, .. }
            | PlaywrightRequest::Extract { id, .. }
            | PlaywrightRequest::Ping { id } => *id,
        };

        if response.id != expected_id {
            return Err(PlaywrightError::Communication(format!(
                "response id mismatch: expected {expected_id}, got {}",
                response.id
            )));
        }

        Ok(response)
    }
}

impl From<&DomainFilter> for BrowserNavigationFilter {
    fn from(filter: &DomainFilter) -> Self {
        Self {
            allowlist: filter.allowlist.clone(),
            denylist: filter.denylist.clone(),
            block_private_ips: filter.block_private_ips,
        }
    }
}

fn parse_screenshot_data(
    data: Option<serde_json::Value>,
    requested_url: &str,
) -> Result<ScreenshotOutput, PlaywrightError> {
    let data =
        data.ok_or_else(|| PlaywrightError::Communication("missing screenshot data".into()))?;

    if let Some(base64) = data.as_str() {
        return Ok(ScreenshotOutput {
            base64: base64.to_owned(),
            final_url: requested_url.to_owned(),
        });
    }

    let base64 = data
        .get("image")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| PlaywrightError::Communication("missing screenshot image data".into()))?
        .to_owned();
    let final_url = data
        .get("finalUrl")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(requested_url)
        .to_owned();

    Ok(ScreenshotOutput { base64, final_url })
}

fn parse_extract_data(
    data: Option<serde_json::Value>,
    requested_url: &str,
) -> Result<ExtractOutput, PlaywrightError> {
    let data =
        data.ok_or_else(|| PlaywrightError::Communication("missing extraction data".into()))?;

    if data.is_array() {
        let elements = serde_json::from_value(data).map_err(|error| {
            PlaywrightError::Communication(format!("failed to parse elements: {error}"))
        })?;
        return Ok(ExtractOutput {
            elements,
            final_url: requested_url.to_owned(),
        });
    }

    let elements_value = data
        .get("elements")
        .cloned()
        .ok_or_else(|| PlaywrightError::Communication("missing extraction elements".into()))?;
    let elements = serde_json::from_value(elements_value).map_err(|error| {
        PlaywrightError::Communication(format!("failed to parse elements: {error}"))
    })?;
    let final_url = data
        .get("finalUrl")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(requested_url)
        .to_owned();

    Ok(ExtractOutput {
        elements,
        final_url,
    })
}

async fn write_bridge_script_temp_file() -> Result<PathBuf, PlaywrightError> {
    let sequence = BRIDGE_SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let script_path = std::env::temp_dir().join(format!(
        "swink_playwright_bridge_{}_{}_{}.js",
        std::process::id(),
        timestamp,
        sequence
    ));

    tokio::fs::write(&script_path, BRIDGE_SCRIPT).await?;
    Ok(script_path)
}

/// Name of the `PATH` lookup utility for the host platform.
///
/// Windows has no `which`; the equivalent is `where.exe`, which is why probing
/// with `which` always failed there and silently fell back to bare `"node"`.
#[cfg(windows)]
const PATH_LOOKUP_PROGRAM: &str = "where";

/// Name of the `PATH` lookup utility for the host platform.
#[cfg(not(windows))]
const PATH_LOOKUP_PROGRAM: &str = "which";

/// Resolve the path to the `node` binary.
///
/// Priority: explicit path > [`PATH_LOOKUP_PROGRAM`] lookup > bare `"node"`.
fn resolve_node_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }

    // Probe `PATH` synchronously (called once at startup, acceptable).
    if let Ok(output) = std::process::Command::new(PATH_LOOKUP_PROGRAM)
        .arg("node")
        .output()
        && output.status.success()
    {
        // `where` prints every match, one per line; take the first, which is
        // the one the shell itself would run.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path_str = stdout.lines().next().unwrap_or_default().trim();
        if !path_str.is_empty() {
            return PathBuf::from(path_str);
        }
    }

    PathBuf::from("node")
}

#[cfg(test)]
#[path = "playwright_tests.rs"]
mod tests;
