//! Single integration-test binary for this crate.
//!
//! Every `tests/*.rs` file is a module here rather than its own executable:
//! each separate test binary statically links the whole dependency graph,
//! and dozens of them across the workspace were most of a 100+ GB `target/`.
//! New test files must be added as a `mod` line below (autotests is off).

mod anthropic_test;
mod azure_test;
mod bedrock_test;
mod common;
mod common_smoke_test;
mod gemini_test;
mod mistral_test;
mod ollama_test;
mod openai_alias_test;
mod openai_test;
mod proxy_test;
mod xai_test;
