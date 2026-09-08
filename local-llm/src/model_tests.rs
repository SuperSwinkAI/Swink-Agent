//! Tests for `model`.
#![cfg(test)]

use std::sync::Arc;

use super::*;

#[test]
fn local_model_debug() {
    let model = LocalModel::new(ModelConfig::default());
    let debug = format!("{model:?}");
    assert!(debug.contains("LocalModel"));
}

#[tokio::test]
async fn new_model_is_not_ready() {
    let model = LocalModel::new(ModelConfig::default());
    assert!(!model.is_ready().await);
}

#[tokio::test]
async fn new_model_state_is_unloaded() {
    let model = LocalModel::new(ModelConfig::default());
    assert_eq!(model.state().await, ModelState::Unloaded);
}

#[tokio::test]
async fn runner_returns_not_ready_when_unloaded() {
    let model = LocalModel::new(ModelConfig::default());
    assert!(model.runner().await.is_err());
}

#[test]
fn from_preset_creates_model_with_correct_config() {
    let model = LocalModel::from_preset(ModelPreset::SmolLM3_3B);
    let config = model.config();
    assert!(config.repo_id.contains("SmolLM3"));
    assert_eq!(config.context_length, 8192);
}

#[test]
fn model_config_default_has_chat_template_none() {
    let config = ModelConfig::default();
    assert!(config.chat_template.is_none());
}

#[test]
fn model_config_context_length_env_override() {
    let config = ModelConfig::default();
    assert_eq!(config.context_length, 8192);
}

#[tokio::test]
async fn send_chat_request_on_unloaded_model_returns_not_ready() {
    let model = LocalModel::new(ModelConfig::default());
    let err = model.runner().await.unwrap_err();
    assert!(err.to_string().contains("not ready"));
}

#[test]
fn with_progress_before_clone_succeeds() {
    let model = LocalModel::new(ModelConfig::default());
    let cb: ProgressCallbackFn = Arc::new(|_| {});
    let result = model.with_progress(cb);
    assert!(result.is_ok());
}

#[test]
fn with_progress_after_clone_fails() {
    let model = LocalModel::new(ModelConfig::default());
    let _clone = model.clone();
    let cb: ProgressCallbackFn = Arc::new(|_| {});
    let result = model.with_progress(cb);
    assert!(result.is_err());
}

#[cfg(feature = "gemma4")]
mod gemma4_tests {
    use super::*;

    #[test]
    fn is_gemma4_detects_bartowski_repo() {
        let config = ModelConfig {
            repo_id: "bartowski/google_gemma-4-E2B-it-GGUF".to_string(),
            ..ModelConfig::default()
        };
        assert!(config.is_gemma4());
    }

    #[test]
    fn is_gemma4_detects_ollama_style_repo() {
        let config = ModelConfig {
            repo_id: "gemma4-e2b".to_string(),
            ..ModelConfig::default()
        };
        assert!(config.is_gemma4());
    }

    #[test]
    fn is_gemma4_false_for_smollm() {
        let config = ModelConfig {
            repo_id: "unsloth/SmolLM3-3B-GGUF".to_string(),
            ..ModelConfig::default()
        };
        assert!(!config.is_gemma4());
    }
}
