//! Async integration tests for session store operations (US3).

mod common;

use swink_agent_memory::{AsyncSessionStore, BlockingSessionStore, JsonlSessionStore};

use common::{sample_meta, user_message};

#[tokio::test]
async fn async_save_and_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();
    let store = BlockingSessionStore::new(jsonl);

    let meta = sample_meta("async_rt", "Async roundtrip");
    let messages = vec![user_message("hello async")];

    store.save("async_rt", &meta, &messages).await.unwrap();

    let (loaded_meta, loaded_msgs) = store.load("async_rt").await.unwrap();
    assert_eq!(loaded_meta.id, meta.id);
    assert_eq!(loaded_meta.title, meta.title);
    assert_eq!(loaded_meta.sequence, 1); // incremented on save
    assert_eq!(loaded_msgs.len(), 1);
}

#[tokio::test]
async fn concurrent_async_operations_on_different_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();
    let store = std::sync::Arc::new(BlockingSessionStore::new(jsonl));

    let mut handles = Vec::new();
    for i in 0..5 {
        let store = store.clone();
        let handle = tokio::spawn(async move {
            let id = format!("concurrent_{i}");
            let meta = sample_meta(&id, &format!("Session {i}"));
            let messages = vec![user_message(&format!("message from {i}"))];
            store.save(&id, &meta, &messages).await.unwrap();
            let (loaded_meta, loaded_msgs) = store.load(&id).await.unwrap();
            assert_eq!(loaded_meta.id, id);
            assert_eq!(loaded_msgs.len(), 1);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let sessions = store.list().await.unwrap();
    assert_eq!(sessions.len(), 5);
}

#[tokio::test]
async fn async_list_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();
    let store = BlockingSessionStore::new(jsonl);

    // Save multiple sessions
    for id in &["del_a", "del_b", "del_c"] {
        let meta = sample_meta(id, &format!("Session {id}"));
        store.save(id, &meta, &[]).await.unwrap();
    }

    let sessions = store.list().await.unwrap();
    assert_eq!(sessions.len(), 3);

    // Delete one
    store.delete("del_b").await.unwrap();

    let sessions = store.list().await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(!sessions.iter().map(|s| s.id.as_str()).any(|x| x == "del_b"));
}
