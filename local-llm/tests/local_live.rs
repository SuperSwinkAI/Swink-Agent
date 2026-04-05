//! Live integration tests for local model inference.
//!
//! All tests are `#[ignore]` — they download large model artifacts on first run.
//! Tests skip gracefully when the model cannot be downloaded (e.g. 404, network error).
//!
//! Run with: `cargo test -p swink-agent-local-llm --test local_live -- --ignored`
//! Gemma 4: `cargo test -p swink-agent-local-llm --features gemma4 --test local_live -- --ignored`

mod common;

use std::sync::Arc;

use futures::StreamExt;
use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{
    AgentContext, AgentMessage, ContentBlock, LlmMessage, ModelSpec, UserMessage,
};
use tokio_util::sync::CancellationToken;

use swink_agent_local_llm::{
    LocalModel, LocalModelError, LocalStreamFn, ModelConfig, ProgressEvent,
};

fn simple_context(prompt: &str) -> AgentContext {
    AgentContext {
        system_prompt: "You are a helpful assistant. Be concise.".to_string(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))],
        tools: vec![],
    }
}

/// Try to load the local model, returning `None` if the download fails
/// (404, network error, etc.).
async fn ready_model_or_skip() -> Option<Arc<LocalModel>> {
    let config = ModelConfig::default();
    let local_model = Arc::new(LocalModel::new(config));
    match local_model.ensure_ready().await {
        Ok(()) => Some(local_model),
        Err(LocalModelError::Download { ref source, .. }) => {
            eprintln!("skipping: model download failed ({source})");
            None
        }
        Err(LocalModelError::Loading { ref source, .. }) => {
            eprintln!("skipping: model loading failed ({source})");
            None
        }
        Err(e) => panic!("unexpected error loading local model: {e}"),
    }
}

#[tokio::test]
#[ignore = "downloads ~1.92 GB model artifacts"]
async fn stream_produces_valid_event_sequence() {
    let Some(local_model) = ready_model_or_skip().await else {
        return;
    };
    let stream_fn = LocalStreamFn::new(local_model);

    let model = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
    let context = simple_context("What is 2 + 2? Answer with just the number.");
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let events: Vec<AssistantMessageEvent> = stream_fn
        .stream(&model, &context, &options, token)
        .collect()
        .await;

    // Verify event sequence: Start, then content blocks, then Done.
    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "first event must be Start"
    );
    assert!(
        matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
        "last event must be Done"
    );

    // Accumulate and verify the message.
    let msg =
        swink_agent::stream::accumulate_message(events, "local", "SmolLM3-3B-Q4_K_M").unwrap();
    let text = ContentBlock::extract_text(&msg.content);
    assert!(!text.is_empty(), "response should contain text");
}

#[tokio::test]
#[ignore = "downloads ~1.92 GB model artifacts"]
async fn cancellation_stops_stream() {
    let Some(local_model) = ready_model_or_skip().await else {
        return;
    };
    let stream_fn = LocalStreamFn::new(local_model);

    let model = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
    let context = simple_context("Write a very long essay about the history of computing.");
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    // Cancel immediately.
    token.cancel();

    let events: Vec<AssistantMessageEvent> = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream_fn
            .stream(&model, &context, &options, token)
            .collect(),
    )
    .await
    .expect("stream should complete within timeout");

    // Should still have a terminal event.
    assert!(
        events.iter().any(|e| matches!(
            e,
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
        )),
        "cancelled stream should have a terminal event"
    );
}

#[tokio::test]
#[ignore = "downloads ~1.92 GB model artifacts"]
async fn concurrent_sharing() {
    let Some(local_model) = ready_model_or_skip().await else {
        return;
    };

    let mut handles = vec![];
    for i in 0..3 {
        let model = Arc::clone(&local_model);
        handles.push(tokio::spawn(async move {
            let stream_fn = LocalStreamFn::new(model);
            let spec = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
            let context = simple_context(&format!("Say the number {i}."));
            let options = StreamOptions::default();
            let token = CancellationToken::new();

            let events: Vec<AssistantMessageEvent> = stream_fn
                .stream(&spec, &context, &options, token)
                .collect()
                .await;
            assert!(matches!(
                events.last(),
                Some(AssistantMessageEvent::Done { .. })
            ));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
#[ignore = "downloads ~1.92 GB model artifacts"]
async fn progress_events_fire_during_download() {
    let config = ModelConfig::default();
    let (cb, collector) = common::progress_collector();

    let local_model = LocalModel::new(config).with_progress(cb).unwrap();
    match local_model.ensure_ready().await {
        Ok(()) => {
            let events = collector.events();
            // Should have at least one download progress and a loading complete.
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProgressEvent::DownloadComplete)),
                "should emit DownloadComplete"
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProgressEvent::LoadingComplete)),
                "should emit LoadingComplete"
            );
        }
        Err(LocalModelError::Download { ref source, .. }) => {
            eprintln!("skipping progress test: model download failed ({source})");
        }
        Err(LocalModelError::Loading { ref source, .. }) => {
            eprintln!("skipping progress test: model loading failed ({source})");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

// ─── Gemma 4 validation gate ───────────────────────────────────────────────

#[cfg(feature = "gemma4")]
mod gemma4_live {
    use super::*;
    use swink_agent_local_llm::ModelPreset;

    fn gemma4_context(system: &str, prompt: &str) -> AgentContext {
        AgentContext {
            system_prompt: system.to_string(),
            messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: prompt.to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))],
            tools: vec![],
        }
    }

    async fn ready_gemma4_or_skip() -> Option<Arc<LocalModel>> {
        let config = ModelPreset::Gemma4E2B.config();
        let local_model = Arc::new(LocalModel::new(config));
        match local_model.ensure_ready().await {
            Ok(()) => Some(local_model),
            Err(LocalModelError::Download { ref source, .. }) => {
                eprintln!("skipping: Gemma 4 E2B download failed ({source})");
                None
            }
            Err(LocalModelError::Loading { ref source, .. }) => {
                eprintln!("skipping: Gemma 4 E2B loading failed ({source})");
                None
            }
            Err(e) => panic!("unexpected error loading Gemma 4 E2B: {e}"),
        }
    }

    /// Validation gate: verify Gemma 4 E2B works on mistralrs 0.8 without
    /// NaN logits. Sends 3 prompts of increasing complexity.
    ///
    /// **STOP/GO decision point** — if this test fails (NaN, hang, garbage),
    /// pause all downstream Gemma 4 work and fall back to Ollama path.
    #[tokio::test]
    #[ignore = "downloads Gemma 4 E2B safetensors (~5 GB)"]
    async fn live_gemma4_e2b_smoke() {
        let Some(local_model) = ready_gemma4_or_skip().await else {
            return;
        };
        let stream_fn = LocalStreamFn::new(Arc::clone(&local_model));
        let model = ModelSpec::new("local", "gemma-4-E2B-it");
        let options = StreamOptions::default();

        // Prompt 1: Simple greeting
        let ctx1 = gemma4_context(
            "You are a helpful assistant.",
            "Say hello in one sentence.",
        );
        let events1: Vec<AssistantMessageEvent> = stream_fn
            .stream(&model, &ctx1, &options, CancellationToken::new())
            .collect()
            .await;
        let msg1 =
            swink_agent::stream::accumulate_message(events1, "local", "gemma-4-E2B-it").unwrap();
        let text1 = ContentBlock::extract_text(&msg1.content);
        assert!(!text1.is_empty(), "prompt 1 (simple greeting) produced empty output");
        assert!(
            !text1.contains("NaN") && !text1.contains("nan"),
            "prompt 1 output contains NaN: {text1}"
        );
        eprintln!("PASS prompt 1 (simple): {}", &text1[..text1.len().min(100)]);

        // Prompt 2: Multi-paragraph system prompt (NaN trigger in BF16)
        let ctx2 = gemma4_context(
            "You are an expert software engineer specializing in Rust programming. \
             You write clean, efficient, idiomatic code. You follow the Rust API guidelines \
             and prefer zero-copy patterns where possible. When explaining code, be thorough \
             but concise. Always consider error handling and edge cases.",
            "Explain the difference between Arc<Mutex<T>> and Arc<RwLock<T>> in Rust. \
             When should you use each? Give a brief code example.",
        );
        let events2: Vec<AssistantMessageEvent> = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            stream_fn
                .stream(&model, &ctx2, &options, CancellationToken::new())
                .collect(),
        )
        .await
        .expect("prompt 2 should complete within 120s (not hung)");
        let msg2 =
            swink_agent::stream::accumulate_message(events2, "local", "gemma-4-E2B-it").unwrap();
        let text2 = ContentBlock::extract_text(&msg2.content);
        assert!(!text2.is_empty(), "prompt 2 (complex system prompt) produced empty output");
        assert!(
            !text2.contains("NaN") && !text2.contains("nan"),
            "prompt 2 output contains NaN: {text2}"
        );
        eprintln!("PASS prompt 2 (complex): {}", &text2[..text2.len().min(100)]);

        // Prompt 3: Tool-use-style prompt (structured output request)
        let ctx3 = gemma4_context(
            "You are a helpful assistant.",
            "List 3 programming languages and their primary use cases. \
             Format as a numbered list.",
        );
        let events3: Vec<AssistantMessageEvent> = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            stream_fn
                .stream(&model, &ctx3, &options, CancellationToken::new())
                .collect(),
        )
        .await
        .expect("prompt 3 should complete within 120s (not hung)");
        let msg3 =
            swink_agent::stream::accumulate_message(events3, "local", "gemma-4-E2B-it").unwrap();
        let text3 = ContentBlock::extract_text(&msg3.content);
        assert!(!text3.is_empty(), "prompt 3 (structured) produced empty output");
        assert!(
            !text3.contains("NaN") && !text3.contains("nan"),
            "prompt 3 output contains NaN: {text3}"
        );
        eprintln!("PASS prompt 3 (structured): {}", &text3[..text3.len().min(100)]);

        eprintln!("=== VALIDATION GATE: ALL 3 PROMPTS PASSED ===");
    }
}
