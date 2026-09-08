//! Tests for `runner`.
#![cfg(test)]

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use super::{GenerateOptions, RunnerConfig, initialize_runner_parts};
use crate::error::LocalModelError;
use crate::progress::{ProgressCallbackFn, ProgressEvent};

fn loading_messages() -> (ProgressCallbackFn, Arc<Mutex<Vec<String>>>) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let messages_clone = Arc::clone(&messages);
    let callback: ProgressCallbackFn = Arc::new(move |event| {
        if let ProgressEvent::LoadingProgress { message } = event {
            messages_clone
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(message);
        }
    });
    (callback, messages)
}

#[test]
fn effective_max_tokens_uses_context_budget_when_unset() {
    let options = GenerateOptions::default();

    assert_eq!(options.effective_max_tokens(128, 120), 8);
}

#[test]
fn effective_max_tokens_honors_request_limit() {
    let options = GenerateOptions {
        max_tokens: Some(4),
        ..GenerateOptions::default()
    };

    assert_eq!(options.effective_max_tokens(128, 120), 4);
}

#[test]
fn effective_max_tokens_caps_request_limit_to_remaining_context() {
    let options = GenerateOptions {
        max_tokens: Some(64),
        ..GenerateOptions::default()
    };

    assert_eq!(options.effective_max_tokens(128, 120), 8);
}

#[test]
fn normalized_temperature_ignores_non_positive_and_nan_values() {
    assert_eq!(
        GenerateOptions {
            temperature: Some(0.0),
            ..GenerateOptions::default()
        }
        .normalized_temperature(),
        None
    );
    assert_eq!(
        GenerateOptions {
            temperature: Some(-0.5),
            ..GenerateOptions::default()
        }
        .normalized_temperature(),
        None
    );
    assert_eq!(
        GenerateOptions {
            temperature: Some(f32::NAN),
            ..GenerateOptions::default()
        }
        .normalized_temperature(),
        None
    );
}

#[test]
fn normalized_temperature_preserves_positive_values() {
    assert_eq!(
        GenerateOptions {
            temperature: Some(0.7),
            ..GenerateOptions::default()
        }
        .normalized_temperature(),
        Some(0.7)
    );
}

#[test]
fn initialize_runner_parts_emits_backend_and_model_loading_progress() {
    let (callback, messages) = loading_messages();
    let config = RunnerConfig::default();

    let (backend, model) = initialize_runner_parts(
        Path::new("synthetic.gguf"),
        &config,
        Some(&callback),
        || Ok::<_, LocalModelError>("backend"),
        |backend, model_path, runner_config| {
            assert_eq!(*backend, "backend");
            assert_eq!(model_path, Path::new("synthetic.gguf"));
            assert_eq!(runner_config.context_length, config.context_length);
            Ok::<_, LocalModelError>("model")
        },
    )
    .unwrap_or_else(|err| panic!("synthetic runner parts should initialize: {err}"));

    assert_eq!(backend, "backend");
    assert_eq!(model, "model");
    assert_eq!(
        *messages.lock().unwrap_or_else(PoisonError::into_inner),
        vec![
            "initializing llama backend".to_string(),
            "loading GGUF model".to_string()
        ]
    );
}

#[test]
fn initialize_runner_parts_stops_after_backend_init_failure() {
    let (callback, messages) = loading_messages();

    let err = initialize_runner_parts::<(), (), _, _>(
        Path::new("synthetic.gguf"),
        &RunnerConfig::default(),
        Some(&callback),
        || Err(LocalModelError::loading_message("backend init failed")),
        |(), _, _| unreachable!("model load should not run"),
    )
    .unwrap_err();

    assert!(err.to_string().contains("backend init failed"));
    assert_eq!(
        *messages.lock().unwrap_or_else(PoisonError::into_inner),
        vec!["initializing llama backend".to_string()]
    );
}
