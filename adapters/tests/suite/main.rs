//! Consolidated integration-test suite: one binary instead of one per file.
//! Each former `tests/<name>.rs` is a module below; its former top-level
//! `#![cfg(...)]` gate is the `#[cfg(...)]` attribute on its `mod` line.

mod common;

#[cfg(feature = "anthropic")]
mod anthropic;
#[cfg(feature = "anthropic")]
mod anthropic_live;
#[cfg(feature = "azure")]
mod azure;
#[cfg(feature = "azure")]
mod azure_live;
#[cfg(feature = "bedrock")]
mod bedrock;
#[cfg(feature = "bedrock")]
mod bedrock_live;
mod cargo_manifest;
#[cfg(feature = "gemini")]
mod google;
#[cfg(feature = "gemini")]
mod google_live;
#[cfg(feature = "mistral")]
mod mistral;
#[cfg(feature = "mistral")]
mod mistral_live;
#[cfg(feature = "ollama")]
mod ollama;
#[cfg(feature = "ollama")]
mod ollama_live;
#[cfg(feature = "openai")]
mod openai;
#[cfg(feature = "openai")]
mod openai_live;
#[cfg(feature = "proxy")]
mod proxy_http;
mod tls_smoke_live;
#[cfg(feature = "xai")]
mod xai;
#[cfg(feature = "xai")]
mod xai_live;
