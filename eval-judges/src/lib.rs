#![forbid(unsafe_code)]
//! Judge client implementations for `swink-agent-eval`.

pub mod client;

#[cfg(feature = "anthropic")]
mod anthropic;
#[cfg(feature = "azure")]
mod azure;
#[cfg(feature = "bedrock")]
mod bedrock;
#[cfg(feature = "gemini")]
mod gemini;
#[cfg(feature = "mistral")]
mod mistral;
#[cfg(feature = "ollama")]
mod ollama;
#[cfg(feature = "openai")]
mod openai;
#[cfg(feature = "proxy")]
mod proxy;
#[cfg(feature = "xai")]
mod xai;

#[cfg(feature = "anthropic")]
pub use anthropic::{AnthropicJudgeClient, BlockingAnthropicJudgeClient};
#[cfg(feature = "azure")]
pub use azure::{AzureJudgeClient, BlockingAzureJudgeClient};
#[cfg(feature = "bedrock")]
pub use bedrock::{BedrockJudgeClient, BlockingBedrockJudgeClient};
#[cfg(feature = "gemini")]
pub use gemini::{BlockingGeminiJudgeClient, GeminiJudgeClient};
#[cfg(feature = "mistral")]
pub use mistral::{BlockingMistralJudgeClient, MistralJudgeClient};
#[cfg(feature = "ollama")]
pub use ollama::{BlockingOllamaJudgeClient, OllamaJudgeClient};
#[cfg(feature = "openai")]
pub use openai::{
    BlockingOpenAIJudgeClient, BlockingOpenAiJudgeClient, OpenAIJudgeClient, OpenAiJudgeClient,
};
#[cfg(feature = "proxy")]
pub use proxy::{BlockingProxyJudgeClient, ProxyJudgeClient};
#[cfg(feature = "xai")]
pub use xai::{BlockingXaiJudgeClient, XaiJudgeClient};
