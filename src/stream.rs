//! Streaming interface traits and types.
//!
//! Defines the `StreamFn` trait (the pluggable boundary between the harness and
//! LLM providers), the event protocol for incremental message delivery, and a
//! delta-accumulation function that reconstructs a finalized `AssistantMessage`
//! from a collected sequence of events.

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub use crate::stream_error_kind::StreamErrorKind;
use crate::types::{
    AgentContext, AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage,
};

// ─── StreamTransport ─────────────────────────────────────────────────────────

/// Transport protocol for streaming responses.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamTransport {
    /// Server-Sent Events (default).
    #[default]
    Sse,
}

// ─── CacheStrategy ──────────────────────────────────────────────────────────

/// Provider-agnostic caching configuration.
///
/// Adapters translate this to provider-specific cache markers at request
/// construction time. Adapters that don't support caching silently ignore
/// the strategy.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub enum CacheStrategy {
    /// No caching (default) — no cache markers injected.
    #[default]
    None,
    /// Adapter determines optimal cache points (e.g., system prompt + tool
    /// definitions for Anthropic, long context for Google).
    Auto,
    /// Anthropic-specific: inject `cache_control: { type: "ephemeral" }`
    /// blocks on system prompt and tool definitions.
    Anthropic,
    /// Google-specific: reference a `CachedContent` resource with the given TTL.
    Google {
        /// Time-to-live for the cached content.
        ttl: Duration,
    },
}

// ─── OnRawPayload ───────────────────────────────────────────────────────────

/// Callback for observing raw SSE data lines before event parsing.
///
/// Fires synchronously with each raw `data:` line. Must return quickly
/// (fire-and-forget semantics). Panics are caught and do not interrupt
/// the streaming pipeline.
pub type OnRawPayload = Arc<dyn Fn(&str) + Send + Sync>;

// ─── Rate-limit snapshot ─────────────────────────────────────────────────────

/// Provider quota state read from a response's headers, delivered once per
/// request through [`StreamOptions::on_rate_limit`] before the first
/// [`AssistantMessageEvent`].
///
/// The typed fields are a convenience over `raw`, not a gate: an adapter
/// whose provider this crate has never heard of still fills `raw` with
/// every rate-limit-shaped header, so a caller can act on it. Every typed
/// field is `None` when the provider did not send it or sent something
/// unparseable — a malformed header never fails the turn.
///
/// | Field | OpenAI | Anthropic | Codex (subscription) |
/// |---|---|---|---|
/// | `used_percent` | — | — | `x-codex-primary-used-percent` |
/// | `remaining_requests` | `x-ratelimit-remaining-requests` | `anthropic-ratelimit-requests-remaining` | — |
/// | `remaining_tokens` | `x-ratelimit-remaining-tokens` | `anthropic-ratelimit-tokens-remaining` | — |
/// | `resets_in` | `x-ratelimit-reset-requests` (`6m0s`) | — (RFC 3339, `raw` only) | `x-codex-primary-reset-after-seconds` |
/// | `window` | — | — | `x-codex-primary-window-minutes` |
/// | `plan` | — | — | `x-codex-plan-type` |
///
/// `retry-after` (seconds form) fills `resets_in` when nothing more specific
/// is present.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLimitSnapshot {
    /// Share of the primary quota window already consumed, 0–100.
    pub used_percent: Option<f32>,
    /// Requests left in the current window.
    pub remaining_requests: Option<u64>,
    /// Tokens left in the current window.
    pub remaining_tokens: Option<u64>,
    /// Time until the window resets.
    pub resets_in: Option<Duration>,
    /// Length of the quota window.
    pub window: Option<Duration>,
    /// Provider plan / tier label.
    pub plan: Option<String>,
    /// Every rate-limit-shaped header, lower-cased name → verbatim value.
    pub raw: std::collections::BTreeMap<String, String>,
}

impl RateLimitSnapshot {
    /// Build a snapshot from response headers.
    ///
    /// Header names are matched case-insensitively. A header is kept in
    /// `raw` when its name contains `ratelimit` / `rate-limit`, starts with
    /// `x-codex-`, or is `retry-after`; everything else is ignored.
    pub fn from_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut snapshot = Self::default();
        for (name, value) in headers {
            let name = name.to_ascii_lowercase();
            let value = value.trim();
            if !is_rate_limit_header(&name) {
                continue;
            }
            match name.as_str() {
                "x-codex-primary-used-percent" => snapshot.used_percent = value.parse().ok(),
                "x-ratelimit-remaining-requests" | "anthropic-ratelimit-requests-remaining" => {
                    snapshot.remaining_requests = value.parse().ok();
                }
                "x-ratelimit-remaining-tokens" | "anthropic-ratelimit-tokens-remaining" => {
                    snapshot.remaining_tokens = value.parse().ok();
                }
                "x-codex-primary-reset-after-seconds" | "x-ratelimit-reset-requests" => {
                    snapshot.resets_in = parse_reset_duration(value);
                }
                // Weakest signal: only fills the gap.
                "retry-after" if snapshot.resets_in.is_none() => {
                    snapshot.resets_in = parse_reset_duration(value);
                }
                "x-codex-primary-window-minutes" => {
                    snapshot.window = value
                        .parse::<u64>()
                        .ok()
                        .map(|m| Duration::from_secs(m * 60));
                }
                "x-codex-plan-type" => snapshot.plan = Some(value.to_owned()),
                _ => {}
            }
            snapshot.raw.insert(name, value.to_owned());
        }
        snapshot
    }

    /// `true` when the provider sent no rate-limit-shaped header at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

fn is_rate_limit_header(name: &str) -> bool {
    name.contains("ratelimit")
        || name.contains("rate-limit")
        || name.starts_with("x-codex-")
        || name == "retry-after"
}

/// Parse a reset value as either plain seconds (`288059`, `1.5`) or the
/// OpenAI `1h2m3s` / `250ms` shape. Anything else yields `None`.
fn parse_reset_duration(value: &str) -> Option<Duration> {
    if let Ok(secs) = value.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs));
    }
    let mut total = Duration::ZERO;
    let mut number = String::new();
    let mut saw_unit = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
            continue;
        }
        let amount: f64 = number.parse().ok()?;
        number.clear();
        let unit = match c {
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                0.001
            }
            'h' => 3600.0,
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        total += Duration::from_secs_f64(amount * unit);
        saw_unit = true;
    }
    (saw_unit && number.is_empty()).then_some(total)
}

/// Callback invoked once per request with the provider's quota headers.
pub type OnRateLimit = Arc<dyn Fn(&RateLimitSnapshot) + Send + Sync>;

// ─── ServingOptions ──────────────────────────────────────────────────────────

/// Provider-native serving options, primarily for self-hosted/local backends.
///
/// Provider-agnostic names; each adapter maps them onto its protocol's native
/// knobs and silently ignores fields its protocol has no equivalent for
/// (marked `—` below). `extra` is the exception: every adapter either honors
/// it or rejects it loudly — never a silent no-op:
///
/// | Adapter | `context_length` | `top_p` | `keep_alive` | `format` | `extra` |
/// |---|---|---|---|---|---|
/// | Ollama | `options.num_ctx` | `options.top_p` | `keep_alive` | `format` (top level) | merged into `options` |
/// | OpenAI-compatible (OpenAI, Azure, xAI) | — | `top_p` | — | `response_format` | merged into the body (top level) |
/// | Mistral | — | — | — | — | merged into the body (top level) |
/// | Anthropic | — | — | — | — | merged into the body (top level) |
/// | Google (Gemini) | — | — | — | — | merged into `generationConfig` |
/// | AWS Bedrock | — | — | — | — | `additionalModelRequestFields` |
/// | Proxy | — | — | — | — | unsupported: dropped with a `tracing` warning naming the keys |
///
/// # `extra`: merge targets and key names
///
/// `extra` keys are the provider's *wire* names, verbatim — e.g. snake_case
/// `top_k` for Anthropic and Ollama, camelCase `topK` for Gemini. Each adapter
/// merges them where that provider keeps its native generation knobs:
///
/// - **Anthropic** — top level of the `/v1/messages` body (`top_k`,
///   `stop_sequences`, `metadata`, `service_tier`, …).
/// - **Google (Gemini)** — inside `generationConfig` (`topK`, `topP`,
///   `candidateCount`, `stopSequences`, `responseMimeType`, `seed`, …),
///   mirroring Ollama's `options` namespace.
/// - **AWS Bedrock** — as the Converse API's `additionalModelRequestFields`
///   object, its own verbatim pass-through for model-native parameters
///   beyond the base `inferenceConfig` set (e.g. `top_k` for Anthropic
///   models on Bedrock).
/// - **Proxy** — the proxy wire protocol has a fixed options schema with no
///   pass-through channel, so `extra` is *not* supported: non-empty `extra`
///   is dropped with one `tracing::warn!` per stream call naming the keys.
///
/// Keys the provider does not recognize are surfaced by the provider API
/// itself (Anthropic, Gemini, and Bedrock all reject unknown fields with an
/// HTTP 4xx), so a typo fails loudly rather than silently.
///
/// # Ollama: top-level fields vs `options.*`
///
/// Ollama splits its request body in two: *sampling* knobs live under the
/// nested `options` object (`num_ctx`, `top_p`, `temperature`, …), while
/// *request-level* knobs are top-level siblings of `model`/`messages`
/// (`keep_alive`, `format`). The distinction is protocol-level, not
/// stylistic — Ollama ignores `options.format` entirely, so JSON mode is
/// only reachable as a top-level `format` field.
///
/// This is why [`format`] is a typed field rather than an `extra` entry: the
/// Ollama adapter merges `extra` into `options.*`, which structurally cannot
/// express a top-level field. [`keep_alive`] is typed for the same reason.
/// Conversely, `top_p` and `context_length` map into `options.*` and could in
/// principle have been expressed through `extra`.
///
/// `extra` is the escape hatch for provider knobs without a typed field
/// (e.g. Ollama's `repeat_penalty`). On key collision, typed fields win over
/// `extra` entries. The default (all `None`, empty `extra`) leaves request
/// bodies byte-identical to builds without serving options.
///
/// # Per-adapter support
///
/// Each adapter consumes only the fields its protocol can express and
/// ignores the rest. The bundled adapters honor:
///
/// | Adapter                                        | `context_length` | `top_p` | `keep_alive` | `format` | `reasoning_effort` | `extra` |
/// |------------------------------------------------|------------------|---------|--------------|----------|--------------------|---------|
/// | Ollama                                         | ✓                | ✓       | ✓            | ✓        | —                  | ✓       |
/// | OpenAI-protocol (OpenAI, compat, xAI, Azure)   | —                | ✓       | —            | ✓        | —                  | ✓       |
/// | Anthropic                                      | —                | —       | —            | —        | —                  | ✓       |
/// | Gemini                                         | —                | —       | —            | —        | —                  | ✓       |
/// | Bedrock                                        | —                | —       | —            | —        | —                  | ✓       |
/// | Mistral                                        | —                | —       | —            | —        | —                  | ✓       |
/// | Proxy                                          | —                | —       | —            | —        | —                  | — (warns) |
///
/// Query it programmatically via [`StreamFn::supported_serving_options`] and
/// [`ServingOptions::unsupported_fields`] instead of hard-coding this table.
///
/// [`format`]: ServingOptions::format
/// [`keep_alive`]: ServingOptions::keep_alive
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServingOptions {
    /// Model context window to serve this request with (Ollama `num_ctx`).
    pub context_length: Option<u64>,
    /// Nucleus-sampling probability mass.
    pub top_p: Option<f64>,
    /// How long the backend should keep the model loaded after the request
    /// (Ollama `keep_alive`, e.g. `"5m"`).
    pub keep_alive: Option<String>,
    /// Structured-output ("JSON mode") constraint for the response.
    ///
    /// `None` (the default) leaves request bodies untouched. See
    /// [`ResponseFormat`] for the per-adapter wire mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ResponseFormat>,
    /// How much reasoning effort the model should spend on this request.
    ///
    /// `None` (the default) leaves request bodies untouched. See
    /// [`ReasoningEffort`] for the per-adapter wire mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Additional provider-native options passed through verbatim.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extra: std::collections::BTreeMap<String, Value>,
}

/// Structured-output constraint for a response ("JSON mode").
///
/// Adapters map this onto their protocol's native structured-output knob and
/// silently ignore it when the protocol has no equivalent:
///
/// | Variant     | Ollama (top-level `format`) | Chat Completions (`response_format`)                                  | Responses (`text.format`)                                      |
/// |-------------|-----------------------------|-----------------------------------------------------------------------|----------------------------------------------------------------|
/// | `Json`      | `"json"`                    | `{"type": "json_object"}`                                             | `{"type": "json_object"}`                                      |
/// | `Schema(s)` | `s` (the schema verbatim)   | `{"type": "json_schema", "json_schema": {"name": …, "schema": s, …}}` | `{"type": "json_schema", "name": …, "strict": true, "schema": s}` |
///
/// In every variant `s` is a bare [JSON Schema] object. Ollama consumes it
/// verbatim; the OpenAI-protocol adapters wrap it in the envelope their
/// protocol requires (Chat Completions nests it under `json_schema`,
/// Responses keeps it flat under `text.format`). Callers therefore pass the
/// same value regardless of backend.
///
/// [JSON Schema]: https://json-schema.org/
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Constrain the response to some syntactically valid JSON value.
    Json,
    /// Constrain the response to a specific JSON Schema.
    Schema(Value),
}

/// How much reasoning effort a model should spend on a request.
///
/// One concept, a different wire shape in every protocol — the same
/// arrangement as [`ResponseFormat`]. Adapters map this onto their
/// protocol's native knob and silently ignore it when the protocol has no
/// equivalent:
///
/// | Variant     | OpenAI Responses (`reasoning.effort`) | Anthropic (`thinking`)      |
/// |-------------|---------------------------------------|-----------------------------|
/// | `Off`       | omitted                               | `{"type": "disabled"}`      |
/// | `Minimal`   | `"minimal"`                           | smallest enabled budget     |
/// | `Low`       | `"low"`                               | small budget                |
/// | `Medium`    | `"medium"`                            | medium budget               |
/// | `High`      | `"high"`                              | large budget                |
/// | `XHigh`     | `"xhigh"`                             | largest budget              |
/// | `Max`       | `"max"`                               | largest budget              |
///
/// The variant set is the union of what real providers accept: `off` through
/// `extra_high` as SuperSwink-Core's tier config already validates, and
/// `low`/`medium`/`high`/`xhigh`/`max` as the Codex model catalog reports per
/// model. Nothing here is invented — an adapter that cannot express a variant
/// maps it to its nearest neighbour or ignores it, and says so via
/// [`ServingOptionSupport`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// No reasoning; fastest.
    Off,
    /// The smallest amount of reasoning the provider offers.
    Minimal,
    /// Light reasoning.
    Low,
    /// The provider's balanced default.
    Medium,
    /// Deep reasoning.
    High,
    /// Deeper than `High`, where the provider offers a level above it.
    ///
    /// Three spellings exist in the wild — OpenAI's wire value is `xhigh`,
    /// SuperSwink-Core's tier config validates `extra_high`, and serde's
    /// default snake_case for this variant is `x_high`. All three
    /// deserialize; the provider's spelling is what serializes.
    #[serde(rename = "xhigh", alias = "x_high", alias = "extra_high")]
    XHigh,
    /// The most the provider offers.
    Max,
}

impl ServingOptions {
    /// Set the model context window to serve this request with (Ollama `num_ctx`).
    #[must_use]
    pub const fn with_context_length(mut self, context_length: u64) -> Self {
        self.context_length = Some(context_length);
        self
    }

    /// Set the nucleus-sampling probability mass.
    #[must_use]
    pub const fn with_top_p(mut self, top_p: f64) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set how long the backend should keep the model loaded after the
    /// request (Ollama `keep_alive`, e.g. `"5m"`).
    #[must_use]
    pub fn with_keep_alive(mut self, keep_alive: impl Into<String>) -> Self {
        self.keep_alive = Some(keep_alive.into());
        self
    }

    /// Set the structured-output ("JSON mode") constraint for the response.
    #[must_use]
    pub fn with_format(mut self, format: ResponseFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Set how much reasoning effort the model should spend.
    #[must_use]
    pub const fn with_reasoning_effort(mut self, reasoning_effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(reasoning_effort);
        self
    }

    /// Set additional provider-native options passed through verbatim.
    #[must_use]
    pub fn with_extra(mut self, extra: std::collections::BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// True when nothing is set — adapters can skip serialization work.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Names of the fields set on `self` that `support` does not honor.
    ///
    /// This is the host-side warning primitive: pair a tier's configured
    /// [`ServingOptions`] with the adapter's
    /// [`StreamFn::supported_serving_options`] to tell the operator exactly
    /// which knobs the request shape will drop, instead of hard-coding a
    /// per-provider table downstream.
    #[must_use]
    pub fn unsupported_fields(&self, support: ServingOptionSupport) -> Vec<&'static str> {
        let mut dropped = Vec::new();
        if self.context_length.is_some() && !support.context_length {
            dropped.push("context_length");
        }
        if self.top_p.is_some() && !support.top_p {
            dropped.push("top_p");
        }
        if self.keep_alive.is_some() && !support.keep_alive {
            dropped.push("keep_alive");
        }
        if self.format.is_some() && !support.format {
            dropped.push("format");
        }
        if self.reasoning_effort.is_some() && !support.reasoning_effort {
            dropped.push("reasoning_effort");
        }
        if !self.extra.is_empty() && !support.extra {
            dropped.push("extra");
        }
        dropped
    }
}

/// Which [`ServingOptions`] fields a [`StreamFn`]'s request shape honors.
///
/// Reported by [`StreamFn::supported_serving_options`]. A `false` field
/// means the adapter ignores that option — configuring it has no effect on
/// the request. Hosts can compare against a configured [`ServingOptions`]
/// via [`ServingOptions::unsupported_fields`] to warn accurately without
/// maintaining a per-provider table.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A per-field capability bitmap is exactly N independent bools — a state
// machine would misrepresent it.
#[allow(clippy::struct_excessive_bools)]
pub struct ServingOptionSupport {
    /// `context_length` reaches the request (Ollama `num_ctx`).
    pub context_length: bool,
    /// `top_p` reaches the request.
    pub top_p: bool,
    /// `keep_alive` reaches the request (Ollama).
    pub keep_alive: bool,
    /// `format` (structured output / JSON mode) reaches the request.
    pub format: bool,
    /// `reasoning_effort` reaches the request.
    pub reasoning_effort: bool,
    /// `extra` entries are merged into the request.
    pub extra: bool,
}

impl ServingOptionSupport {
    /// Every field honored.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            context_length: true,
            top_p: true,
            keep_alive: true,
            format: true,
            reasoning_effort: true,
            extra: true,
        }
    }

    /// No field honored.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            context_length: false,
            top_p: false,
            keep_alive: false,
            format: false,
            reasoning_effort: false,
            extra: false,
        }
    }

    /// Set whether `context_length` is honored.
    #[must_use]
    pub const fn with_context_length(mut self, supported: bool) -> Self {
        self.context_length = supported;
        self
    }

    /// Set whether `top_p` is honored.
    #[must_use]
    pub const fn with_top_p(mut self, supported: bool) -> Self {
        self.top_p = supported;
        self
    }

    /// Set whether `keep_alive` is honored.
    #[must_use]
    pub const fn with_keep_alive(mut self, supported: bool) -> Self {
        self.keep_alive = supported;
        self
    }

    /// Set whether `format` is honored.
    #[must_use]
    pub const fn with_format(mut self, supported: bool) -> Self {
        self.format = supported;
        self
    }

    /// Set whether `reasoning_effort` is honored.
    #[must_use]
    pub const fn with_reasoning_effort(mut self, supported: bool) -> Self {
        self.reasoning_effort = supported;
        self
    }

    /// Set whether `extra` is honored.
    #[must_use]
    pub const fn with_extra(mut self, supported: bool) -> Self {
        self.extra = supported;
        self
    }
}

// ─── StreamOptions ───────────────────────────────────────────────────────────

/// Per-call configuration passed through to the LLM provider.
#[non_exhaustive]
#[derive(Clone, Default)]
pub struct StreamOptions {
    /// Sampling temperature (optional).
    pub temperature: Option<f64>,
    /// Output token limit (optional).
    pub max_tokens: Option<u64>,
    /// Provider-side session identifier for caching (optional).
    pub session_id: Option<String>,
    /// Dynamically resolved API key for this specific request (optional).
    pub api_key: Option<String>,
    /// Preferred transport protocol.
    pub transport: StreamTransport,
    /// Provider-agnostic caching configuration.
    pub cache_strategy: CacheStrategy,
    /// Optional callback for observing raw SSE data lines before parsing.
    pub on_raw_payload: Option<OnRawPayload>,
    /// Optional callback receiving the provider's rate-limit headers, once
    /// per request, before the first event. Absent = nothing changes.
    pub on_rate_limit: Option<OnRateLimit>,
    /// Provider-native serving options (local backends). Default = none set.
    pub serving: ServingOptions,
}

impl StreamOptions {
    /// Set the sampling temperature.
    #[must_use]
    pub const fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set the output token limit.
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u64) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the provider-side session identifier for caching.
    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set a dynamically resolved API key for this specific request.
    #[must_use]
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set the preferred transport protocol.
    #[must_use]
    pub const fn with_transport(mut self, transport: StreamTransport) -> Self {
        self.transport = transport;
        self
    }

    /// Set the provider-agnostic caching configuration.
    #[must_use]
    pub fn with_cache_strategy(mut self, cache_strategy: CacheStrategy) -> Self {
        self.cache_strategy = cache_strategy;
        self
    }

    /// Set a callback for observing raw SSE data lines before parsing.
    #[must_use]
    pub fn with_on_raw_payload(mut self, on_raw_payload: OnRawPayload) -> Self {
        self.on_raw_payload = Some(on_raw_payload);
        self
    }

    /// Set a callback receiving the provider's rate-limit headers.
    #[must_use]
    pub fn with_on_rate_limit(mut self, on_rate_limit: OnRateLimit) -> Self {
        self.on_rate_limit = Some(on_rate_limit);
        self
    }

    /// Set the provider-native serving options (local backends).
    #[must_use]
    pub fn with_serving(mut self, serving: ServingOptions) -> Self {
        self.serving = serving;
        self
    }
}

impl std::fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamOptions")
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("session_id", &self.session_id)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("transport", &self.transport)
            .field("cache_strategy", &self.cache_strategy)
            .field(
                "on_raw_payload",
                &self.on_raw_payload.as_ref().map(|_| "<callback>"),
            )
            .field(
                "on_rate_limit",
                &self.on_rate_limit.as_ref().map(|_| "<callback>"),
            )
            .field("serving", &self.serving)
            .finish()
    }
}

// ─── AssistantMessageEvent ───────────────────────────────────────────────────

/// An incremental event emitted by a `StreamFn` implementation.
///
/// Events follow a strict start/delta/end protocol per content block. Each
/// block carries a `content_index` that identifies its position in the final
/// message's content vec.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    /// The stream has opened.
    Start,

    /// A new text content block is starting at `content_index`.
    TextStart { content_index: usize },
    /// An incremental text fragment for the block at `content_index`.
    TextDelta { content_index: usize, delta: String },
    /// The text block at `content_index` is complete.
    TextEnd { content_index: usize },

    /// A new thinking content block is starting at `content_index`.
    ThinkingStart { content_index: usize },
    /// An incremental thinking fragment for the block at `content_index`.
    ThinkingDelta { content_index: usize, delta: String },
    /// The thinking block at `content_index` is complete, with an optional
    /// provider verification signature.
    ThinkingEnd {
        content_index: usize,
        signature: Option<String>,
    },

    /// A new tool call content block is starting at `content_index`.
    ToolCallStart {
        content_index: usize,
        id: String,
        name: String,
    },
    /// An incremental JSON argument fragment for the tool call at `content_index`.
    ToolCallDelta { content_index: usize, delta: String },
    /// The tool call at `content_index` is complete.
    ToolCallEnd { content_index: usize },

    /// The stream completed successfully.
    Done {
        stop_reason: StopReason,
        usage: Usage,
        cost: Cost,
    },

    /// The stream ended with an error.
    Error {
        stop_reason: StopReason,
        error_message: String,
        usage: Option<Usage>,
        /// Optional structured error classification.
        ///
        /// When set, the agent loop uses this to classify the error without
        /// falling back to string matching on `error_message`.
        error_kind: Option<StreamErrorKind>,
        /// Provider-supplied retry-after timing, when the error response
        /// carried a hint (e.g. a `Retry-After` header on a 429/529
        /// response).
        ///
        /// `None` when the provider gave no such hint, or when the error
        /// did not originate from an HTTP response (e.g. a mid-stream SSE
        /// `error` event). Adapters populate this field; the core crate
        /// only carries it through.
        retry_after: Option<std::time::Duration>,
    },
}

impl AssistantMessageEvent {
    /// Create a stream error event with no structured classification.
    ///
    /// Convenience constructor used by adapters when the stream encounters
    /// an error condition. The `error_kind` is set to `None`, so the agent
    /// loop will fall back to string-based classification.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: None,
            retry_after: None,
        }
    }

    /// Create a throttle/rate-limit error event.
    ///
    /// Sets [`StreamErrorKind::Throttled`] so the agent loop can classify
    /// the error structurally.
    pub fn error_throttled(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::Throttled),
            retry_after: None,
        }
    }

    /// Create a context-window overflow error event.
    ///
    /// Sets [`StreamErrorKind::ContextWindowExceeded`] so the agent loop
    /// can trigger context compaction.
    pub fn error_context_overflow(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::ContextWindowExceeded),
            retry_after: None,
        }
    }

    /// Create an authentication error event.
    ///
    /// Sets [`StreamErrorKind::Auth`] so the agent loop can treat this as
    /// a non-retryable failure.
    pub fn error_auth(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::Auth),
            retry_after: None,
        }
    }

    /// Create a network/server error event.
    ///
    /// Sets [`StreamErrorKind::Network`] so the agent loop can classify
    /// the error as retryable.
    pub fn error_network(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::Network),
            retry_after: None,
        }
    }

    /// Create a content-filtered error event.
    ///
    /// Sets [`StreamErrorKind::ContentFiltered`] so the agent loop can
    /// treat this as a non-retryable safety policy violation.
    pub fn error_content_filtered(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::ContentFiltered),
            retry_after: None,
        }
    }

    /// Create a model-retired error event.
    ///
    /// Sets [`StreamErrorKind::ModelRetired`] so the agent loop can surface
    /// a user-legible "model has been retired" signal instead of a generic
    /// unclassified error. Adapters use this when a provider error response
    /// indicates the requested model has been retired/decommissioned.
    pub fn error_model_retired(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::ModelRetired),
            retry_after: None,
        }
    }

    /// Build a complete single-text-block response event sequence.
    ///
    /// Useful for testing and mock `StreamFn` implementations. Returns the
    /// five events needed for a valid text-only response: `Start`, `TextStart`,
    /// `TextDelta`, `TextEnd`, and `Done`.
    pub fn text_response(text: &str) -> Vec<Self> {
        vec![
            Self::Start,
            Self::TextStart { content_index: 0 },
            Self::TextDelta {
                content_index: 0,
                delta: text.to_string(),
            },
            Self::TextEnd { content_index: 0 },
            Self::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ]
    }
}

// ─── AssistantMessageDelta ───────────────────────────────────────────────────

/// A typed incremental update during streaming, used in `MessageUpdate` events.
///
/// The `delta` field uses [`Cow<'static, str>`] to avoid cloning on the hot
/// path when the caller can transfer ownership of the underlying `String`.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageDelta {
    /// An appended text string fragment.
    Text {
        content_index: usize,
        delta: Cow<'static, str>,
    },
    /// An appended reasoning fragment.
    Thinking {
        content_index: usize,
        delta: Cow<'static, str>,
    },
    /// An appended JSON argument fragment for a tool call.
    ToolCall {
        content_index: usize,
        delta: Cow<'static, str>,
    },
}

// ─── StreamFn Trait ──────────────────────────────────────────────────────────

/// The pluggable boundary between the harness and LLM providers.
///
/// Callers supply an implementation that accepts a model specification, an
/// agent context, and stream options, and returns an async stream of
/// `AssistantMessageEvent` values. The harness consumes this stream to build
/// up the assistant message incrementally.
///
/// This trait is object-safe and requires `Send + Sync` so that it can be
/// stored behind an `Arc` and shared across async tasks.
pub trait StreamFn: Send + Sync {
    /// Initiate a streaming LLM call.
    ///
    /// The returned stream yields `AssistantMessageEvent` values following the
    /// start/delta/end protocol. Implementations must respect the provided
    /// `cancellation_token` — when the token is cancelled, the stream should
    /// terminate promptly.
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;

    /// Which [`ServingOptions`] fields this implementation's request shape
    /// honors.
    ///
    /// Defaults to [`ServingOptionSupport::all`] — optimistic, so external
    /// adapters that predate this method are not falsely reported as
    /// dropping options. Adapters that ignore fields should override this so
    /// hosts can warn accurately (see
    /// [`ServingOptions::unsupported_fields`]).
    fn supported_serving_options(&self) -> ServingOptionSupport {
        ServingOptionSupport::all()
    }
}

// ─── Owned-input streaming & decorators ─────────────────────────────────────

/// Stream from `stream_fn` with owned inputs, yielding a `'static` stream.
///
/// [`StreamFn::stream`] ties the returned stream's lifetime to the caller's
/// borrows, so a decorator that *modifies* the request (clamping
/// `max_tokens`, rewriting the context) cannot delegate directly — the
/// modified values are locals the returned stream may not borrow. Every
/// downstream decorator ends up hand-rolling the same spawn-a-task,
/// forward-over-a-channel dance. This helper is that dance, written once:
/// a task owns the inputs and drives the inner stream; the returned stream
/// yields its events and is `'static`.
///
/// Must be called within a Tokio runtime. Cancellation flows through
/// `cancellation_token` exactly as with a direct [`StreamFn::stream`] call;
/// if the consumer drops the returned stream, the forwarding task stops on
/// its next send.
#[must_use]
pub fn stream_owned(
    stream_fn: Arc<dyn StreamFn>,
    model: ModelSpec,
    context: AgentContext,
    options: StreamOptions,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'static>> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        let mut events = stream_fn.stream(&model, &context, &options, cancellation_token);
        while let Some(event) = events.next().await {
            if tx.send(event).await.is_err() {
                break;
            }
        }
    });
    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// Outcome of a [`MapOptionsStreamFn`] rewrite callback.
///
/// `Ok` carries the options to delegate with; `Err` refuses the request and
/// the carried events are emitted as the response stream (typically
/// [`AssistantMessageEvent::Start`] followed by an
/// [`AssistantMessageEvent::Error`]).
pub type MappedOptions = Result<StreamOptions, Vec<AssistantMessageEvent>>;

/// A [`StreamFn`] decorator that rewrites per-request [`StreamOptions`]
/// before delegating to an inner stream function.
///
/// This is the supported shape for per-request policy around a provider
/// adapter — reply-budget clamping, request refusal, option injection —
/// without each host reimplementing the owned-input plumbing (see
/// [`stream_owned`]). The callback sees the model, the context, and the
/// caller's options, and either returns the options to delegate with or
/// refuses the request with a ready-made event sequence:
///
/// ```
/// # use std::{pin::Pin, sync::Arc};
/// # use futures::Stream;
/// # use tokio_util::sync::CancellationToken;
/// # use swink_agent::{
/// #     AgentContext, AssistantMessageEvent, MapOptionsStreamFn, ModelSpec, StreamFn,
/// #     StreamOptions,
/// # };
/// # struct Silent;
/// # impl StreamFn for Silent {
/// #     fn stream<'a>(
/// #         &'a self,
/// #         _model: &'a ModelSpec,
/// #         _context: &'a AgentContext,
/// #         _options: &'a StreamOptions,
/// #         _cancellation_token: CancellationToken,
/// #     ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
/// #         Box::pin(futures::stream::empty())
/// #     }
/// # }
/// let inner: Arc<dyn StreamFn> = Arc::new(Silent);
/// let clamped = MapOptionsStreamFn::new(inner, |_model, _context, mut options| {
///     options.max_tokens = Some(options.max_tokens.unwrap_or(4096).min(1024));
///     Ok(options)
/// });
/// # let _: Arc<dyn StreamFn> = Arc::new(clamped);
/// ```
///
/// The rewritten options are owned locals, so delegation goes through
/// [`stream_owned`]; the context crosses via
/// [`AgentContext::clone_for_send`], whose best-effort snapshot semantics
/// are safe here because provider adapters never consume custom messages.
pub struct MapOptionsStreamFn<F> {
    inner: Arc<dyn StreamFn>,
    map: F,
}

impl<F> MapOptionsStreamFn<F>
where
    F: Fn(&ModelSpec, &AgentContext, StreamOptions) -> MappedOptions + Send + Sync,
{
    /// Wrap `inner`, rewriting each request's options through `map`.
    pub fn new(inner: Arc<dyn StreamFn>, map: F) -> Self {
        Self { inner, map }
    }
}

impl<F> StreamFn for MapOptionsStreamFn<F>
where
    F: Fn(&ModelSpec, &AgentContext, StreamOptions) -> MappedOptions + Send + Sync,
{
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        match (self.map)(model, context, options.clone()) {
            Ok(mapped) => stream_owned(
                Arc::clone(&self.inner),
                model.clone(),
                context.clone_for_send(),
                mapped,
                cancellation_token,
            ),
            Err(events) => Box::pin(futures::stream::iter(events)),
        }
    }

    // The decorator rewrites options but delegates the request shape, so it
    // honors exactly what the inner adapter honors.
    fn supported_serving_options(&self) -> ServingOptionSupport {
        self.inner.supported_serving_options()
    }
}

// ─── Tool-call sanitization ──────────────────────────────────────────────────

/// Scrub incomplete `ToolCall` blocks in an assistant message so it can safely
/// be replayed to a provider.
///
/// When a stream hits [`StopReason::Length`] mid tool-use, the resulting
/// [`ContentBlock::ToolCall`] may carry `arguments: Value::Null` with
/// `partial_json: Some(..)` (see `accumulate_message`). Provider adapters
/// forward `arguments` verbatim; Anthropic/Google/Bedrock reject null inputs
/// and OpenAI-compatible providers reject the literal string `"null"`. On the
/// next turn this causes a 400.
///
/// This helper coerces any `ToolCall` block whose `partial_json` is still set
/// OR whose `arguments` is not a JSON object into a valid empty-object call:
/// `arguments = Value::Object({})` and `partial_json = None`. The loop pairs
/// this with a synthetic tool-result message (see
/// `recover_incomplete_tool_calls`) so the provider sees a well-formed pair.
///
/// Safe to call multiple times and safe to call on messages that contain no
/// tool-use blocks. Returns the number of blocks modified.
///
/// See <https://github.com/SuperSwinkAI/Swink-Agent/issues/619>.
pub fn sanitize_incomplete_tool_calls(message: &mut AssistantMessage) -> usize {
    let mut fixed = 0;
    for block in &mut message.content {
        if let ContentBlock::ToolCall {
            arguments,
            partial_json,
            ..
        } = block
        {
            let needs_fix = partial_json.is_some() || !arguments.is_object();
            if needs_fix {
                *arguments = Value::Object(serde_json::Map::new());
                *partial_json = None;
                fixed += 1;
            }
        }
    }
    fixed
}

// ─── Delta Accumulation ──────────────────────────────────────────────────────

/// Reconstruct a finalized `AssistantMessage` from a collected list of stream
/// events.
///
/// # Errors
///
/// Returns a descriptive error string if the event sequence is malformed (e.g.
/// delta for a non-existent content index, missing `Start` or terminal event).
#[allow(clippy::too_many_lines)]
pub fn accumulate_message(
    events: Vec<AssistantMessageEvent>,
    provider: &str,
    model_id: &str,
) -> Result<AssistantMessage, String> {
    fn ensure_block_open(
        open_blocks: &[bool],
        content_index: usize,
        event_name: &str,
    ) -> Result<(), String> {
        match open_blocks.get(content_index) {
            Some(false) => Err(format!(
                "{event_name}: block at index {content_index} is already closed"
            )),
            Some(true) | None => Ok(()),
        }
    }

    fn all_open_blocks_are_tool_calls(content: &[ContentBlock], open_blocks: &[bool]) -> bool {
        open_blocks
            .iter()
            .enumerate()
            .filter(|(_, open)| **open)
            .all(|(content_index, _)| {
                matches!(
                    content.get(content_index),
                    Some(ContentBlock::ToolCall { .. })
                )
            })
    }

    fn validate_terminal_open_blocks(
        event_name: &str,
        content: Option<&[ContentBlock]>,
        open_blocks: &[bool],
        tolerate_truncated_tool_args: bool,
    ) -> Result<(), String> {
        if let Some(idx) = open_blocks.iter().position(|open| *open) {
            let content = content.ok_or_else(|| format!("{event_name} before Start"))?;
            if tolerate_truncated_tool_args && all_open_blocks_are_tool_calls(content, open_blocks)
            {
                // Max-tokens truncation: leave open tool-call blocks with
                // `partial_json` set so the loop can recover on the next turn.
                tracing::debug!(
                    "{event_name}(Length) with unterminated content block at index {idx} - tolerating for max-tokens recovery"
                );
            } else {
                return Err(format!(
                    "{event_name} received with unterminated content block at index {idx}"
                ));
            }
        }

        Ok(())
    }

    let mut content: Option<Vec<ContentBlock>> = None;
    // Parallel to `content`: tracks whether each block is still open (awaiting
    // its matching `*End` event). Finalization (on `Done`) fails if any block
    // is still open, preventing silently-corrupt assistant messages.
    let mut open_blocks: Vec<bool> = Vec::new();
    let mut stop_reason: Option<StopReason> = None;
    let mut usage: Option<Usage> = None;
    let mut cost: Option<Cost> = None;
    let mut error_message: Option<String> = None;
    let mut error_kind: Option<StreamErrorKind> = None;
    let mut saw_start = false;
    let mut saw_terminal = false;

    // Pre-scan for a Length stop reason. Providers can emit `ToolCallEnd` with
    // truncated JSON arguments (or omit it entirely) when hitting the max-token
    // limit mid tool-call. We must preserve the incomplete block so the loop's
    // `recover_incomplete_tool_calls` path can convert it into an error tool
    // result and continue on the next turn. See issue #221.
    let tolerate_truncated_tool_args = events.iter().any(|e| {
        matches!(
            e,
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Length,
                ..
            }
        )
    });

    for event in events {
        // Reject content-block events after a terminal event.
        match &event {
            AssistantMessageEvent::TextStart { .. }
            | AssistantMessageEvent::TextDelta { .. }
            | AssistantMessageEvent::TextEnd { .. }
            | AssistantMessageEvent::ThinkingStart { .. }
            | AssistantMessageEvent::ThinkingDelta { .. }
            | AssistantMessageEvent::ThinkingEnd { .. }
            | AssistantMessageEvent::ToolCallStart { .. }
            | AssistantMessageEvent::ToolCallDelta { .. }
            | AssistantMessageEvent::ToolCallEnd { .. } => {
                if saw_terminal {
                    return Err("content event after terminal event".into());
                }
            }
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => {
                if saw_terminal {
                    return Err("duplicate terminal event".into());
                }
            }
            AssistantMessageEvent::Start => {
                if saw_terminal {
                    return Err("Start event after terminal event".into());
                }
            }
        }

        match event {
            AssistantMessageEvent::Start => {
                if saw_start {
                    return Err("duplicate Start event".into());
                }
                saw_start = true;
                content = Some(Vec::new());
            }

            AssistantMessageEvent::TextStart { content_index } => {
                let blocks = content.as_mut().ok_or("TextStart before Start")?;
                if content_index != blocks.len() {
                    return Err(format!(
                        "TextStart content_index {content_index} != content length {}",
                        blocks.len()
                    ));
                }
                blocks.push(ContentBlock::Text {
                    text: String::new(),
                });
                open_blocks.push(true);
            }

            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("TextDelta before Start")?;
                ensure_block_open(&open_blocks, content_index, "TextDelta")?;
                let block = blocks
                    .get_mut(content_index)
                    .ok_or_else(|| format!("TextDelta: invalid content_index {content_index}"))?;
                match block {
                    ContentBlock::Text { text } => text.push_str(&delta),
                    _ => {
                        return Err(format!(
                            "TextDelta: block at index {content_index} is not Text"
                        ));
                    }
                }
            }

            AssistantMessageEvent::TextEnd { content_index } => {
                let blocks = content.as_ref().ok_or("TextEnd before Start")?;
                let block = blocks
                    .get(content_index)
                    .ok_or_else(|| format!("TextEnd: invalid content_index {content_index}"))?;
                if !matches!(block, ContentBlock::Text { .. }) {
                    return Err(format!(
                        "TextEnd: block at index {content_index} is not Text"
                    ));
                }
                ensure_block_open(&open_blocks, content_index, "TextEnd")?;
                if let Some(open) = open_blocks.get_mut(content_index) {
                    *open = false;
                }
            }

            AssistantMessageEvent::ThinkingStart { content_index } => {
                let blocks = content.as_mut().ok_or("ThinkingStart before Start")?;
                if content_index != blocks.len() {
                    return Err(format!(
                        "ThinkingStart content_index {content_index} != content length {}",
                        blocks.len()
                    ));
                }
                blocks.push(ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                });
                open_blocks.push(true);
            }

            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("ThinkingDelta before Start")?;
                ensure_block_open(&open_blocks, content_index, "ThinkingDelta")?;
                let block = blocks.get_mut(content_index).ok_or_else(|| {
                    format!("ThinkingDelta: invalid content_index {content_index}")
                })?;
                match block {
                    ContentBlock::Thinking { thinking, .. } => thinking.push_str(&delta),
                    _ => {
                        return Err(format!(
                            "ThinkingDelta: block at index {content_index} is not Thinking"
                        ));
                    }
                }
            }

            AssistantMessageEvent::ThinkingEnd {
                content_index,
                signature,
            } => {
                let blocks = content.as_mut().ok_or("ThinkingEnd before Start")?;
                ensure_block_open(&open_blocks, content_index, "ThinkingEnd")?;
                let block = blocks
                    .get_mut(content_index)
                    .ok_or_else(|| format!("ThinkingEnd: invalid content_index {content_index}"))?;
                match block {
                    ContentBlock::Thinking { signature: sig, .. } => *sig = signature,
                    _ => {
                        return Err(format!(
                            "ThinkingEnd: block at index {content_index} is not Thinking"
                        ));
                    }
                }
                if let Some(open) = open_blocks.get_mut(content_index) {
                    *open = false;
                }
            }

            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => {
                let blocks = content.as_mut().ok_or("ToolCallStart before Start")?;
                if content_index != blocks.len() {
                    return Err(format!(
                        "ToolCallStart content_index {content_index} != content length {}",
                        blocks.len()
                    ));
                }
                blocks.push(ContentBlock::ToolCall {
                    id,
                    name,
                    arguments: Value::Null,
                    partial_json: Some(String::new()),
                });
                open_blocks.push(true);
            }

            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("ToolCallDelta before Start")?;
                ensure_block_open(&open_blocks, content_index, "ToolCallDelta")?;
                let block = blocks.get_mut(content_index).ok_or_else(|| {
                    format!("ToolCallDelta: invalid content_index {content_index}")
                })?;
                match block {
                    ContentBlock::ToolCall { partial_json, .. } => {
                        let pj = partial_json
                            .as_mut()
                            .ok_or("ToolCallDelta: partial_json already consumed")?;
                        pj.push_str(&delta);
                    }
                    _ => {
                        return Err(format!(
                            "ToolCallDelta: block at index {content_index} is not ToolCall"
                        ));
                    }
                }
            }

            AssistantMessageEvent::ToolCallEnd { content_index } => {
                let blocks = content.as_mut().ok_or("ToolCallEnd before Start")?;
                let block = blocks
                    .get_mut(content_index)
                    .ok_or_else(|| format!("ToolCallEnd: invalid content_index {content_index}"))?;
                ensure_block_open(&open_blocks, content_index, "ToolCallEnd")?;
                match block {
                    ContentBlock::ToolCall {
                        arguments,
                        partial_json,
                        ..
                    } => {
                        let json_str = partial_json
                            .as_ref()
                            .ok_or("ToolCallEnd: partial_json already consumed")?
                            .clone();
                        if json_str.is_empty() {
                            *arguments = Value::Object(serde_json::Map::new());
                            *partial_json = None;
                        } else {
                            match serde_json::from_str::<Value>(&json_str) {
                                Ok(v) => {
                                    *arguments = v;
                                    *partial_json = None;
                                }
                                Err(e) => {
                                    if tolerate_truncated_tool_args {
                                        // Leave `partial_json` as Some so the
                                        // block is flagged incomplete and the
                                        // loop recovers on the next turn.
                                    } else {
                                        return Err(format!(
                                            "ToolCallEnd: failed to parse arguments JSON: {e}"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        return Err(format!(
                            "ToolCallEnd: block at index {content_index} is not ToolCall"
                        ));
                    }
                }
                if let Some(open) = open_blocks.get_mut(content_index) {
                    *open = false;
                }
            }

            AssistantMessageEvent::Done {
                stop_reason: sr,
                usage: u,
                cost: c,
            } => {
                validate_terminal_open_blocks(
                    "Done",
                    content.as_deref(),
                    &open_blocks,
                    tolerate_truncated_tool_args,
                )?;
                stop_reason = Some(sr);
                usage = Some(u);
                cost = Some(c);
                saw_terminal = true;
            }

            AssistantMessageEvent::Error {
                stop_reason: sr,
                error_message: em,
                usage: u,
                error_kind: ek,
                retry_after: _,
            } => {
                validate_terminal_open_blocks("Error", content.as_deref(), &open_blocks, false)?;
                stop_reason = Some(sr);
                error_message = Some(em);
                error_kind = ek;
                if let Some(u) = u {
                    usage = Some(u);
                }
                saw_terminal = true;
            }
        }
    }

    let content = content.ok_or("no Start event found")?;
    let stop_reason = stop_reason.ok_or("no terminal event (Done or Error) found")?;

    let timestamp = crate::util::now_timestamp();

    Ok(AssistantMessage {
        content,
        provider: provider.to_owned(),
        model_id: model_id.to_owned(),
        usage: usage.unwrap_or_default(),
        cost: cost.unwrap_or_default(),
        stop_reason,
        error_message,
        error_kind,
        timestamp,
        cache_hint: None,
    })
}

// ─── Compile-time Send + Sync assertions ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StreamErrorKind>();
    assert_send_sync::<StreamTransport>();
    assert_send_sync::<StreamOptions>();
    assert_send_sync::<AssistantMessageEvent>();
    assert_send_sync::<AssistantMessageDelta>();
};

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stream_reasoning_effort_tests.rs"]
mod reasoning_effort_tests;

#[cfg(test)]
#[path = "stream_reasoning_effort_alias_tests.rs"]
mod reasoning_effort_alias_tests;
