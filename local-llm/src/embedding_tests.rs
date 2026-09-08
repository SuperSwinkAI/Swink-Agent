//! Tests for `embedding`.
#![cfg(test)]

use std::sync::Arc;

use super::*;

#[test]
fn embedding_model_debug() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    let debug = format!("{model:?}");
    assert!(debug.contains("EmbeddingModel"));
}

#[tokio::test]
async fn new_model_is_not_ready() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    assert!(!model.is_ready().await);
}

#[test]
fn from_preset_creates_embedding_model() {
    let model = EmbeddingModel::from_preset(ModelPreset::EmbeddingGemma300M);
    let config = model.config();
    assert!(config.repo_id.contains("gemma"));
    assert_eq!(config.dimensions, 768);
}

#[test]
fn with_progress_before_clone_succeeds() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    let cb: ProgressCallbackFn = Arc::new(|_| {});
    let result = model.with_progress(cb);
    assert!(result.is_ok());
}

#[test]
fn with_progress_after_clone_fails() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    let _clone = model.clone();
    let cb: ProgressCallbackFn = Arc::new(|_| {});
    let result = model.with_progress(cb);
    assert!(result.is_err());
}

#[test]
fn embedding_config_default() {
    let config = EmbeddingConfig::default();
    assert_eq!(config.dimensions, 768);
    assert!(config.repo_id.contains("gemma"));
}
