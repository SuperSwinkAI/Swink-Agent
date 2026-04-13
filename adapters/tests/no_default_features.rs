//! Regression test for issue #441: `--no-default-features` must keep every
//! provider gate disabled in `swink-agent-adapters`.
//!
//! This file compiles under
//! `cargo test -p swink-agent-adapters --no-default-features --test no_default_features`.
//! If a default or dev-dependency leaks adapter features back in, the
//! `#[cfg(feature = ...)]` assertions below flip and fail loudly.

#[cfg(not(feature = "anthropic"))]
#[test]
fn anthropic_feature_is_off() {}

#[cfg(feature = "anthropic")]
#[test]
fn anthropic_must_not_leak_into_no_default_features() {
    panic!("anthropic feature is enabled but should not be");
}

#[cfg(not(feature = "openai"))]
#[test]
fn openai_feature_is_off() {}

#[cfg(feature = "openai")]
#[test]
fn openai_must_not_leak_into_no_default_features() {
    panic!("openai feature is enabled but should not be");
}

#[cfg(not(feature = "openai-compat"))]
#[test]
fn openai_compat_feature_is_off() {}

#[cfg(feature = "openai-compat")]
#[test]
fn openai_compat_must_not_leak_into_no_default_features() {
    panic!("openai-compat feature is enabled but should not be");
}

#[cfg(not(feature = "ollama"))]
#[test]
fn ollama_feature_is_off() {}

#[cfg(feature = "ollama")]
#[test]
fn ollama_must_not_leak_into_no_default_features() {
    panic!("ollama feature is enabled but should not be");
}

#[cfg(not(feature = "gemini"))]
#[test]
fn gemini_feature_is_off() {}

#[cfg(feature = "gemini")]
#[test]
fn gemini_must_not_leak_into_no_default_features() {
    panic!("gemini feature is enabled but should not be");
}

#[cfg(not(feature = "proxy"))]
#[test]
fn proxy_feature_is_off() {}

#[cfg(feature = "proxy")]
#[test]
fn proxy_must_not_leak_into_no_default_features() {
    panic!("proxy feature is enabled but should not be");
}

#[cfg(not(feature = "azure"))]
#[test]
fn azure_feature_is_off() {}

#[cfg(feature = "azure")]
#[test]
fn azure_must_not_leak_into_no_default_features() {
    panic!("azure feature is enabled but should not be");
}

#[cfg(not(feature = "bedrock"))]
#[test]
fn bedrock_feature_is_off() {}

#[cfg(feature = "bedrock")]
#[test]
fn bedrock_must_not_leak_into_no_default_features() {
    panic!("bedrock feature is enabled but should not be");
}

#[cfg(not(feature = "mistral"))]
#[test]
fn mistral_feature_is_off() {}

#[cfg(feature = "mistral")]
#[test]
fn mistral_must_not_leak_into_no_default_features() {
    panic!("mistral feature is enabled but should not be");
}

#[cfg(not(feature = "xai"))]
#[test]
fn xai_feature_is_off() {}

#[cfg(feature = "xai")]
#[test]
fn xai_must_not_leak_into_no_default_features() {
    panic!("xai feature is enabled but should not be");
}
