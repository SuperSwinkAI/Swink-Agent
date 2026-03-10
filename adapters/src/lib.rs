pub mod ollama;
#[allow(clippy::doc_markdown)] // "OpenAI" is a proper noun, not code.
pub mod openai;

pub use ollama::OllamaStreamFn;
pub use openai::OpenAiStreamFn;
