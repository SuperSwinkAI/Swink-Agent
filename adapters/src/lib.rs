#![forbid(unsafe_code)]
pub mod classify;
mod proxy;
pub mod sse;

mod anthropic;
mod convert;
mod ollama;
#[allow(clippy::doc_markdown)] // "OpenAI" is a proper noun, not code.
mod openai;

pub use anthropic::AnthropicStreamFn;
pub use ollama::OllamaStreamFn;
pub use openai::OpenAiStreamFn;
pub use proxy::ProxyStreamFn;
