use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};

/// Viewport dimensions for screenshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// Preset extraction types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractionPreset {
    Links,
    Headings,
    Tables,
}

/// A single extracted element from a web page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedElement {
    pub tag: String,
    pub text: String,
    pub attributes: HashMap<String, String>,
}

/// Request sent to the Playwright bridge subprocess.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum PlaywrightRequest {
    Screenshot {
        id: u64,
        url: String,
        viewport: Option<Viewport>,
    },
    Extract {
        id: u64,
        url: String,
        selector: Option<String>,
        preset: Option<ExtractionPreset>,
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
    ) -> Result<String, PlaywrightError> {
        let id = self.next_id();
        let resp = self
            .send_request(PlaywrightRequest::Screenshot {
                id,
                url: url.to_owned(),
                viewport,
            })
            .await?;

        if !resp.ok {
            return Err(PlaywrightError::BridgeError(
                resp.error.unwrap_or_else(|| "screenshot failed".into()),
            ));
        }

        resp.data
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| PlaywrightError::Communication("missing screenshot data".into()))
    }

    /// Extract elements from a web page using a CSS selector or preset.
    pub async fn extract(
        &mut self,
        url: &str,
        selector: Option<&str>,
        preset: Option<ExtractionPreset>,
    ) -> Result<Vec<ExtractedElement>, PlaywrightError> {
        let id = self.next_id();
        let resp = self
            .send_request(PlaywrightRequest::Extract {
                id,
                url: url.to_owned(),
                selector: selector.map(String::from),
                preset,
            })
            .await?;

        if !resp.ok {
            return Err(PlaywrightError::BridgeError(
                resp.error.unwrap_or_else(|| "extraction failed".into()),
            ));
        }

        let data = resp
            .data
            .ok_or_else(|| PlaywrightError::Communication("missing extraction data".into()))?;

        serde_json::from_value(data)
            .map_err(|e| PlaywrightError::Communication(format!("failed to parse elements: {e}")))
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

/// Resolve the path to the `node` binary.
///
/// Priority: explicit path > `which node` > bare `"node"`.
fn resolve_node_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }

    // Try `which node` synchronously (called once at startup, acceptable).
    if let Ok(output) = std::process::Command::new("which").arg("node").output()
        && output.status.success()
    {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path_str.is_empty() {
            return PathBuf::from(path_str);
        }
    }

    PathBuf::from("node")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;
    use std::process::Command as StdCommand;

    use super::{BRIDGE_SCRIPT, resolve_node_path, write_bridge_script_temp_file};

    #[tokio::test]
    async fn writes_unique_bridge_scripts_for_concurrent_startups() {
        let handles = (0..8).map(|_| tokio::spawn(write_bridge_script_temp_file()));
        let mut paths = Vec::new();

        for handle in handles {
            let path = handle
                .await
                .expect("task should complete")
                .expect("temp script creation should succeed");
            paths.push(path);
        }

        let unique_paths: HashSet<_> = paths.iter().cloned().collect();
        assert_eq!(unique_paths.len(), paths.len());

        for path in &paths {
            let contents = tokio::fs::read_to_string(path)
                .await
                .expect("script contents should be readable");
            assert_eq!(contents, BRIDGE_SCRIPT);
            tokio::fs::remove_file(path)
                .await
                .expect("temp script cleanup should succeed");
        }
    }

    #[test]
    fn bridge_script_exports_data_only_extract_helpers() {
        assert!(!BRIDGE_SCRIPT.contains("eval("));

        let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/playwright_bridge.js");
        let node_script = format!(
            r"
const bridge = require({bridge_path});

const linksPlan = bridge.buildExtractionPlan({{ preset: 'links' }});
if (linksPlan.selector !== 'a[href]' || linksPlan.preset !== 'links') {{
  throw new Error('unexpected links plan: ' + JSON.stringify(linksPlan));
}}

const selectorPlan = bridge.buildExtractionPlan({{ selector: '.card' }});
if (selectorPlan.selector !== '.card' || selectorPlan.preset !== null) {{
  throw new Error('unexpected selector plan: ' + JSON.stringify(selectorPlan));
}}

const customElement = bridge.extractElementData(
  {{
    tagName: 'DIV',
    textContent: '  Hello world  ',
    attributes: [{{ name: 'data-id', value: '42' }}],
    getAttribute(name) {{
      return name === 'data-id' ? '42' : null;
    }},
    innerHTML: '<span>Hello world</span>',
  }},
  null
);
if (customElement.tag !== 'div' || customElement.text !== 'Hello world' || customElement.attributes['data-id'] !== '42') {{
  throw new Error('unexpected custom element: ' + JSON.stringify(customElement));
}}

const linkElement = bridge.extractElementData(
  {{
    tagName: 'A',
    textContent: ' Docs ',
    attributes: [{{ name: 'href', value: '/docs' }}],
    getAttribute(name) {{
      return name === 'href' ? '/docs' : null;
    }},
    innerHTML: 'Docs',
  }},
  'links'
);
if (linkElement.attributes.href !== '/docs' || Object.keys(linkElement.attributes).length !== 1) {{
  throw new Error('unexpected link element: ' + JSON.stringify(linkElement));
}}

const headingElement = bridge.extractElementData(
  {{
    tagName: 'H2',
    textContent: ' Section ',
    attributes: [{{ name: 'id', value: 'section' }}],
    getAttribute() {{
      return null;
    }},
    innerHTML: 'Section',
  }},
  'headings'
);
if (headingElement.tag !== 'h2' || headingElement.text !== 'Section' || Object.keys(headingElement.attributes).length !== 0) {{
  throw new Error('unexpected heading element: ' + JSON.stringify(headingElement));
}}

const tableElement = bridge.extractElementData(
  {{
    tagName: 'TABLE',
    textContent: '',
    attributes: [],
    getAttribute() {{
      return null;
    }},
    innerHTML: '<tbody><tr><td>value</td></tr></tbody>',
  }},
  'tables'
);
if (tableElement.tag !== 'table' || !tableElement.text.includes('<tbody>')) {{
  throw new Error('unexpected table element: ' + JSON.stringify(tableElement));
}}
",
            bridge_path = serde_json::to_string(&bridge_path.display().to_string())
                .expect("path should serialize"),
        );

        let output = StdCommand::new(resolve_node_path(None))
            .arg("-e")
            .arg(node_script)
            .output()
            .expect("node should run bridge helper assertions");

        assert!(
            output.status.success(),
            "node assertions failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
