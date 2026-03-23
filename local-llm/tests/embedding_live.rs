//! Live integration tests for embedding model.
//!
//! All tests are `#[ignore]` — they download ~200 MB on first run.
//! The model (`google/gemma-embedding-300m`) is gated and requires `HF_TOKEN`.
//! Tests skip gracefully when the token is missing or invalid.
//!
//! Run with: `cargo test -p swink-agent-local-llm --test embedding_live -- --ignored`

use std::sync::Arc;

use swink_agent_local_llm::{EmbeddingConfig, EmbeddingModel, LocalModelError};

/// Try to load the embedding model, returning `None` if authentication fails
/// (missing or invalid `HF_TOKEN` for the gated model).
async fn ready_model_or_skip() -> Option<EmbeddingModel> {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    match model.ensure_ready().await {
        Ok(()) => Some(model),
        Err(
            LocalModelError::Loading { ref source, .. }
            | LocalModelError::Download { ref source, .. },
        ) if source.to_string().contains("401") => {
            eprintln!("skipping: HF_TOKEN not set or invalid (HTTP 401)");
            None
        }
        Err(e) => panic!("unexpected error loading embedding model: {e}"),
    }
}

#[tokio::test]
#[ignore = "downloads gated embedding model artifacts"]
async fn single_embedding() {
    let Some(model) = ready_model_or_skip().await else {
        return;
    };

    let embedding = model.embed("Hello, world!").await.unwrap();
    assert!(!embedding.is_empty(), "embedding should not be empty");
    assert_eq!(embedding.len(), 768, "default dimensions should be 768");
}

#[tokio::test]
#[ignore = "downloads gated embedding model artifacts"]
async fn batch_embedding() {
    let Some(model) = ready_model_or_skip().await else {
        return;
    };

    let texts = &["Hello", "World", "Rust is great"];
    let embeddings = model.embed_batch(texts).await.unwrap();
    assert_eq!(embeddings.len(), 3, "should return one vector per input");

    for (i, emb) in embeddings.iter().enumerate() {
        assert!(!emb.is_empty(), "embedding {i} should not be empty");
    }
}

#[tokio::test]
#[ignore = "downloads gated embedding model artifacts"]
async fn concurrent_embedding() {
    let Some(model) = ready_model_or_skip().await else {
        return;
    };

    let model = Arc::new(model);
    let mut handles = vec![];

    for i in 0..3 {
        let m = Arc::clone(&model);
        handles.push(tokio::spawn(async move {
            let text = format!("Concurrent embedding test {i}");
            m.embed(&text).await.unwrap()
        }));
    }

    for handle in handles {
        let embedding = handle.await.unwrap();
        assert!(!embedding.is_empty());
    }
}

#[tokio::test]
#[ignore = "downloads gated embedding model artifacts"]
async fn unload_and_reload() {
    let Some(model) = ready_model_or_skip().await else {
        return;
    };
    assert!(model.is_ready().await);

    model.unload().await;
    assert!(!model.is_ready().await);

    // Re-loading should work.
    model.ensure_ready().await.unwrap();
    assert!(model.is_ready().await);

    let embedding = model.embed("test").await.unwrap();
    assert!(!embedding.is_empty());
}

#[tokio::test]
#[ignore = "downloads gated embedding model artifacts"]
async fn empty_input_returns_valid_vector() {
    let Some(model) = ready_model_or_skip().await else {
        return;
    };

    let embedding = model.embed("").await.unwrap();
    assert!(
        !embedding.is_empty(),
        "empty input should still return a valid vector"
    );
}
