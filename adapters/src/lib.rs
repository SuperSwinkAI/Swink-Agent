#![forbid(unsafe_code)]
pub mod classify;
mod proxy;
mod remote_presets;
pub mod sse;

mod anthropic;
mod azure;
mod bedrock;
mod convert;
mod google;
mod mistral;
mod ollama;
#[allow(clippy::doc_markdown)] // "OpenAI" is a proper noun, not code.
mod openai;
mod xai;

pub use anthropic::AnthropicStreamFn;
pub use azure::AzureStreamFn;
pub use bedrock::BedrockStreamFn;
pub use google::GeminiStreamFn;
pub use mistral::MistralStreamFn;
pub use ollama::OllamaStreamFn;
pub use openai::OpenAiStreamFn;
pub use proxy::ProxyStreamFn;
pub use remote_presets::{
    RemoteModelConnectionError, RemotePresetKey, build_remote_connection, remote_preset_keys,
    remote_presets,
};
pub use xai::XAiStreamFn;
