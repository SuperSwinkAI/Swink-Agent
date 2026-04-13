//! Regression test for issue #441: `--no-default-features` must keep the
//! optional local-LLM feature gates disabled.
//!
//! This file compiles under
//! `cargo test -p swink-agent-local-llm --no-default-features --test no_default_features`.
//! If a dependency leak re-enables one of these features, the matching panic
//! test becomes active and fails the build.

#[cfg(not(feature = "gemma4"))]
#[test]
fn gemma4_feature_is_off() {}

#[cfg(feature = "gemma4")]
#[test]
fn gemma4_must_not_leak_into_no_default_features() {
    panic!("gemma4 feature is enabled but should not be");
}

#[cfg(not(feature = "metal"))]
#[test]
fn metal_feature_is_off() {}

#[cfg(feature = "metal")]
#[test]
fn metal_must_not_leak_into_no_default_features() {
    panic!("metal feature is enabled but should not be");
}

#[cfg(not(feature = "cuda"))]
#[test]
fn cuda_feature_is_off() {}

#[cfg(feature = "cuda")]
#[test]
fn cuda_must_not_leak_into_no_default_features() {
    panic!("cuda feature is enabled but should not be");
}

#[cfg(not(feature = "cudnn"))]
#[test]
fn cudnn_feature_is_off() {}

#[cfg(feature = "cudnn")]
#[test]
fn cudnn_must_not_leak_into_no_default_features() {
    panic!("cudnn feature is enabled but should not be");
}

#[cfg(not(feature = "flash-attn"))]
#[test]
fn flash_attn_feature_is_off() {}

#[cfg(feature = "flash-attn")]
#[test]
fn flash_attn_must_not_leak_into_no_default_features() {
    panic!("flash-attn feature is enabled but should not be");
}

#[cfg(not(feature = "mkl"))]
#[test]
fn mkl_feature_is_off() {}

#[cfg(feature = "mkl")]
#[test]
fn mkl_must_not_leak_into_no_default_features() {
    panic!("mkl feature is enabled but should not be");
}

#[cfg(not(feature = "accelerate"))]
#[test]
fn accelerate_feature_is_off() {}

#[cfg(feature = "accelerate")]
#[test]
fn accelerate_must_not_leak_into_no_default_features() {
    panic!("accelerate feature is enabled but should not be");
}
