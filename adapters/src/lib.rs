#![forbid(unsafe_code)]
//! LLM provider adapters for [`swink-agent`](https://docs.rs/swink-agent).
//!
//! Provides [`StreamFn`](swink_agent::StreamFn) implementations for nine LLM providers.
//! Each provider is behind a feature flag — enable only what you need:
//!
//! | Feature | Provider |
//! |---|---|
//! | `anthropic` (default) | Anthropic Claude |
//! | `openai` (default) | OpenAI GPT |
//! | `ollama` (default) | Ollama (local) |
//! | `gemini` | Google Gemini |
//! | `azure` | Azure OpenAI / AI Foundry |
//! | `bedrock` | AWS Bedrock |
//! | `mistral` | Mistral AI |
//! | `xai` | xAI Grok |
//! | `proxy` | Custom SSE proxy |
//!
//! # Quick Start
//!
//! ```no_run
//! use swink_agent_adapters::build_remote_connection_for_model;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let conn = build_remote_connection_for_model("claude-sonnet-4-6")?;
//! # Ok(())
//! # }
//! ```

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
mod block_accumulator;
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
    not(any(feature = "openai-compat", feature = "azure", feature = "mistral",)),
    allow(dead_code)
)]
mod oai_transport;
#[cfg_attr(
    not(any(feature = "openai-compat", feature = "azure", feature = "mistral",)),
    allow(dead_code)
)]
mod openai_compat;
mod remote_presets;
pub mod sse;

pub use remote_presets::{
    RemoteModelConnectionError, RemotePresetKey, all_remote_presets, build_remote_connection,
    build_remote_connection_for_model, is_provider_compiled, preset, remote_presets,
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

#[cfg(feature = "openai-compat")]
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
