//! Loopback-redirect [`AuthorizationHandler`] for consumer-login flows.
//!
//! Every "log in with your account" provider ends the authorization-code
//! dance the same way: the browser is redirected to `http://localhost:<port>/…?code=…&state=…`
//! and something local has to catch that one request. This handler is that
//! something, written once so adapters don't each reinvent it:
//!
//! 1. bind an ephemeral listener on the configured loopback address;
//! 2. hand the authorization URL to the caller-supplied `on_authorization_url`
//!    (print it, open a browser — the handler does no I/O of its own);
//! 3. serve exactly one callback, verify `state`, return `code`.
//!
//! **Headless hosts**: when a browser can't reach this machine, an optional
//! `manual_code` reader (typically "read a line from stdin") is raced against
//! the listener. The user pastes either the full redirected URL or the bare
//! code. If the port can't be bound at all, the manual reader is the only
//! path; if neither is possible the handler fails immediately rather than
//! hanging.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{AuthorizationHandler, CredentialError, CredentialFuture};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// Default bind address; matches the `localhost:1455` redirect registered by
/// the OpenAI public client.
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:1455";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_REQUEST_BYTES: usize = 8 * 1024;

type UrlSink = Arc<dyn Fn(&str) + Send + Sync>;
type ManualReader = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Catches the OAuth2 redirect on a loopback port, with an optional manual
/// paste fallback. See the [module docs](self).
pub struct LoopbackAuthorizationHandler {
    bind_addr: SocketAddr,
    on_authorization_url: UrlSink,
    manual_code: Option<ManualReader>,
    timeout: Duration,
}

impl LoopbackAuthorizationHandler {
    /// Listen on [`DEFAULT_BIND_ADDR`] and show the URL through
    /// `on_authorization_url`.
    #[must_use]
    pub fn new(on_authorization_url: UrlSink) -> Self {
        Self {
            bind_addr: DEFAULT_BIND_ADDR
                .parse()
                .expect("DEFAULT_BIND_ADDR is a valid socket address"),
            on_authorization_url,
            manual_code: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Bind somewhere other than [`DEFAULT_BIND_ADDR`]. Must agree with the
    /// `redirect_uri` registered with the provider.
    #[must_use]
    pub const fn with_bind_addr(mut self, bind_addr: SocketAddr) -> Self {
        self.bind_addr = bind_addr;
        self
    }

    /// Add a manual fallback: a blocking reader that returns the pasted
    /// redirect URL or bare code (`None` = the user gave up). Raced against
    /// the listener, and the only path when the port can't be bound.
    #[must_use]
    pub fn with_manual_code(mut self, reader: ManualReader) -> Self {
        self.manual_code = Some(reader);
        self
    }

    /// How long to wait for the callback (default 5 minutes).
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl std::fmt::Debug for LoopbackAuthorizationHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackAuthorizationHandler")
            .field("bind_addr", &self.bind_addr)
            .field("manual_code", &self.manual_code.is_some())
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl AuthorizationHandler for LoopbackAuthorizationHandler {
    fn authorize(&self, auth_url: &str, state: &str) -> CredentialFuture<'_, String> {
        let auth_url = auth_url.to_owned();
        let state = state.to_owned();
        Box::pin(async move {
            let listener = TcpListener::bind(self.bind_addr).await.ok();
            if listener.is_none() && self.manual_code.is_none() {
                return Err(CredentialError::AuthorizationFailed {
                    key: String::new(),
                    reason: format!(
                        "could not listen on {} and no manual code fallback is configured",
                        self.bind_addr
                    ),
                });
            }

            (self.on_authorization_url)(&auth_url);

            let manual = self
                .manual_code
                .clone()
                .map(|reader| tokio::task::spawn_blocking(move || reader()));

            let loopback = async {
                match listener {
                    Some(listener) => wait_for_callback(&listener, &state).await,
                    None => std::future::pending().await,
                }
            };
            let manual = async {
                match manual {
                    Some(handle) => match handle.await {
                        Ok(Some(input)) => parse_manual_input(&input, &state),
                        Ok(None) => Err("no authorization code was entered".to_owned()),
                        Err(_) => Err("manual code reader failed".to_owned()),
                    },
                    None => std::future::pending().await,
                }
            };

            let outcome = tokio::select! {
                result = loopback => result,
                result = manual => result,
                () = tokio::time::sleep(self.timeout) => {
                    return Err(CredentialError::Timeout { key: String::new() });
                }
            };
            outcome.map_err(|reason| CredentialError::AuthorizationFailed {
                key: String::new(),
                reason,
            })
        })
    }
}

/// Serve requests until one carries the callback; stray requests (favicon,
/// preflight) get a 404 and the wait continues.
async fn wait_for_callback(listener: &TcpListener, expected_state: &str) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("loopback accept failed: {e}"))?;
        let Some(target) = read_request_target(&mut stream).await else {
            let _ = respond(&mut stream, 400, "Malformed request.").await;
            continue;
        };
        // Any path is accepted; only the query matters.
        let Some(query) = target.split_once('?').map(|(_, q)| q.to_owned()) else {
            let _ = respond(&mut stream, 404, "Not the OAuth callback.").await;
            continue;
        };
        match parse_callback_query(&query, expected_state) {
            Ok(code) => {
                let _ = respond(&mut stream, 200, "Signed in. You can close this tab.").await;
                return Ok(code);
            }
            Err(reason) => {
                let _ = respond(
                    &mut stream,
                    400,
                    "Sign-in failed; return to the application.",
                )
                .await;
                return Err(reason);
            }
        }
    }
}

/// Read the HTTP request line and return its target (`/path?query`).
async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < MAX_REQUEST_BYTES {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut parts = text.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method == "GET").then(|| target.to_owned())
}

async fn respond(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    let html =
        format!("<!doctype html><meta charset=\"utf-8\"><title>swink-agent</title><p>{body}</p>");
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// Extract `code` from a callback query string, enforcing `state`.
fn parse_callback_query(query: &str, expected_state: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(&format!("http://localhost/?{query}"))
        .map_err(|_| "unparseable callback query".to_owned())?;
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match &*k {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            // Only the stable error code; `error_description` is free text
            // from the provider and stays out of surfaced errors.
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Err(format!("authorization server returned error \"{error}\""));
    }
    if state.as_deref() != Some(expected_state) {
        return Err("state mismatch on OAuth callback".to_owned());
    }
    code.filter(|c| !c.is_empty())
        .ok_or_else(|| "callback carried no authorization code".to_owned())
}

/// Manual input is either the whole redirected URL (state is checked when
/// present) or a bare code.
fn parse_manual_input(input: &str, expected_state: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("no authorization code was entered".to_owned());
    }
    if let Some((_, query)) = input.split_once('?') {
        return parse_callback_query(query, expected_state);
    }
    if input.contains("code=") {
        return parse_callback_query(input, expected_state);
    }
    Ok(input.to_owned())
}

#[cfg(test)]
#[path = "loopback_tests.rs"]
mod tests;
