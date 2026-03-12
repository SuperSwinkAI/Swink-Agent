//! Live integration tests for embedding model.
//!
//! All tests are `#[ignore]` — they download ~200 MB on first run.
//! Run with: `cargo test -p swink-agent-local-llm --test embedding_live -- --ignored`

use std::sync::Arc;

use swink_agent_local_llm::{EmbeddingConfig, EmbeddingModel};

#[tokio::test]
#[ignore]
async fn single_embedding() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    model.ensure_ready().await.unwrap();

    let embedding = model.embed_single("Hello, world!").await.unwrap();
    assert!(!embedding.is_empty(), "embedding should not be empty");
    assert_eq!(embedding.len(), 768, "default dimensions should be 768");
}

#[tokio::test]
#[ignore]
async fn batch_embedding() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    model.ensure_ready().await.unwrap();

    let texts = &["Hello", "World", "Rust is great"];
    let embeddings = model.embed(texts).await.unwrap();
    assert_eq!(embeddings.len(), 3, "should return one vector per input");

    for (i, emb) in embeddings.iter().enumerate() {
        assert!(!emb.is_empty(), "embedding {i} should not be empty");
    }
}

#[tokio::test]
#[ignore]
async fn concurrent_embedding() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    model.ensure_ready().await.unwrap();

    let model = Arc::new(model);
    let mut handles = vec![];

    for i in 0..3 {
        let m = Arc::clone(&model);
        handles.push(tokio::spawn(async move {
            let text = format!("Concurrent embedding test {i}");
            m.embed_single(&text).await.unwrap()
        }));
    }

    for handle in handles {
        let embedding = handle.await.unwrap();
        assert!(!embedding.is_empty());
    }
}

#[tokio::test]
#[ignore]
async fn unload_and_reload() {
    let model = EmbeddingModel::new(EmbeddingConfig::default());
    model.ensure_ready().await.unwrap();
    assert!(model.is_ready().await);

    model.unload().await;
    assert!(!model.is_ready().await);

    // Re-loading should work.
    model.ensure_ready().await.unwrap();
    assert!(model.is_ready().await);

    let embedding = model.embed_single("test").await.unwrap();
    assert!(!embedding.is_empty());
}
