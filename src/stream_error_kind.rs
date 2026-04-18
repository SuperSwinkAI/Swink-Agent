use serde::{Deserialize, Serialize};

/// Structured classification of stream errors.
///
/// Adapters can attach a `StreamErrorKind` to an `Error` event so the agent
/// loop can classify errors structurally instead of relying on string matching.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamErrorKind {
    /// The provider throttled the request (HTTP 429 / rate limit).
    Throttled,
    /// The request exceeded the model's context window.
    ContextWindowExceeded,
    /// Authentication or authorization failure (HTTP 401/403).
    Auth,
    /// Transient network or server error (connection drop, 5xx, etc.).
    Network,
    /// Provider safety/content filter blocked the response.
    ContentFiltered,
}
