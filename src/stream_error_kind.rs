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
    /// The provider has retired/decommissioned the requested model.
    ///
    /// Typically an HTTP 400/404/410 with a provider-specific error code
    /// (e.g. OpenAI's `model_not_found`). Distinct from the adapters crate's
    /// client-side `UnknownModelId` ("not in our compiled catalog"): this is
    /// the provider saying it no longer serves the model. Consumers can look
    /// up a replacement via the model catalog's deprecation metadata
    /// ([`PresetStatus::Deprecated`](crate::PresetStatus)).
    ModelRetired,
}
