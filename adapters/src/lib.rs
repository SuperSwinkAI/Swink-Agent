#![forbid(unsafe_code)]

// ── Shared infrastructure (always compiled) ───────────────────────────────
#[cfg_attr(
    not(any(
        feature = "anthropic",
        feature = "openai",
        feature = "ollama",
        feature = "gemini",
        feature = "proxy",
        feature = "azure",
        feature = "bedrock",
        feature = "mistral",
        feature = "xai",
    )),
    allow(dead_code)
)]
mod base;
pub mod classify;
pub mod convert;
#[cfg_attr(
    not(any(
        feature = "anthropic",
        feature = "openai",
        feature = "ollama",
        feature = "gemini",
        feature = "proxy",
        feature = "azure",
        feature = "bedrock",
        feature = "mistral",
        feature = "xai",
    )),
    allow(dead_code)
)]
mod finalize;
#[cfg_attr(
    not(any(
        feature = "openai",
        feature = "azure",
        feature = "mistral",
        feature = "xai"
    )),
    allow(dead_code)
)]
mod oai_transport;
#[cfg_attr(
    not(any(
        feature = "openai",
        feature = "azure",
        feature = "mistral",
        feature = "xai"
    )),
    allow(dead_code)
)]
mod openai_compat;
mod remote_presets;
pub mod sse;

pub use remote_presets::{
    RemoteModelConnectionError, RemotePresetKey, build_remote_connection, preset,
    remote_preset_keys, remote_presets,
};

// ── Provider adapters (feature-gated) ─────────────────────────────────────
//
// Each adapter is gated behind its own feature flag. Enable the corresponding
// feature in Cargo.toml to compile and use a provider:
//
//   swink-agent-adapters = { features = ["anthropic", "openai"] }

#[cfg(feature = "anthropic")]
mod anthropic;
#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicStreamFn;

#[cfg(feature = "openai")]
#[allow(clippy::doc_markdown)]
mod openai;
#[cfg(feature = "openai")]
pub use openai::OpenAiStreamFn;

#[cfg(feature = "ollama")]
mod ollama;
#[cfg(feature = "ollama")]
pub use ollama::OllamaStreamFn;

#[cfg(feature = "gemini")]
mod google;
#[cfg(feature = "gemini")]
pub use google::GeminiStreamFn;

#[cfg(feature = "proxy")]
mod proxy;
#[cfg(feature = "proxy")]
pub use proxy::ProxyStreamFn;

#[cfg(feature = "azure")]
mod azure;
#[cfg(feature = "azure")]
pub use azure::{AzureAuth, AzureStreamFn};

#[cfg(feature = "bedrock")]
mod bedrock;
#[cfg(feature = "bedrock")]
pub use bedrock::BedrockStreamFn;

#[cfg(feature = "mistral")]
mod mistral;
#[cfg(feature = "mistral")]
pub use mistral::MistralStreamFn;

#[cfg(feature = "xai")]
mod xai;
#[cfg(feature = "xai")]
pub use xai::XAiStreamFn;
