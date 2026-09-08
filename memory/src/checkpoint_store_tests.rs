//! Tests for `checkpoint_store`.
#![cfg(test)]

use std::io;

use super::{FileCheckpointStore, validate_checkpoint_id};
use swink_agent::{Checkpoint, CheckpointStore};

#[tokio::test]
async fn file_checkpoint_store_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();
    let checkpoint =
        Checkpoint::new("cp-file", "prompt", "provider", "model", &[]).with_turn_count(3);

    store.save_checkpoint(checkpoint).await.unwrap();

    let loaded = store.load_checkpoint("cp-file").await.unwrap().unwrap();
    assert_eq!(loaded.id, "cp-file");
    assert_eq!(loaded.turn_count, 3);
}

#[tokio::test]
async fn file_checkpoint_store_lists_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

    let mut older = Checkpoint::new("older", "prompt", "provider", "model", &[]);
    older.created_at = 10;
    let mut newer = Checkpoint::new("newer", "prompt", "provider", "model", &[]);
    newer.created_at = 20;

    store.save_checkpoint(older).await.unwrap();
    store.save_checkpoint(newer).await.unwrap();

    let ids = store.list_checkpoints().await.unwrap();
    assert_eq!(ids, vec!["newer".to_string(), "older".to_string()]);
}

#[tokio::test]
async fn file_checkpoint_store_list_skips_unrelated_and_invalid_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

    let mut checkpoint = Checkpoint::new("valid", "prompt", "provider", "model", &[]);
    checkpoint.created_at = 10;
    store.save_checkpoint(checkpoint).await.unwrap();

    std::fs::write(dir.path().join("scratch.tmp"), "not a checkpoint").unwrap();
    std::fs::write(dir.path().join("broken.json"), "{not valid json").unwrap();
    std::fs::write(
        dir.path().join("wrong-shape.json"),
        serde_json::json!({"id": "wrong-shape"}).to_string(),
    )
    .unwrap();

    let ids = store.list_checkpoints().await.unwrap();

    assert_eq!(ids, vec!["valid".to_string()]);
}

#[tokio::test]
async fn retention_prunes_oldest_to_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf())
        .unwrap()
        .with_max_checkpoints(2);

    for (id, created_at) in [("cp-old", 10), ("cp-mid", 20), ("cp-new", 30)] {
        let mut checkpoint = Checkpoint::new(id, "prompt", "provider", "model", &[]);
        checkpoint.created_at = created_at;
        store.save_checkpoint(checkpoint).await.unwrap();
    }

    // Oldest pruned first: only the two newest remain.
    let ids = store.list_checkpoints().await.unwrap();
    assert_eq!(ids, vec!["cp-new".to_string(), "cp-mid".to_string()]);
    assert!(!dir.path().join("cp-old.json").exists());
    assert!(dir.path().join("cp-mid.json").exists());
    assert!(dir.path().join("cp-new.json").exists());
}

#[tokio::test]
async fn retention_defaults_to_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

    let total = FileCheckpointStore::DEFAULT_MAX_CHECKPOINTS + 5;
    for i in 0..total {
        let mut checkpoint =
            Checkpoint::new(format!("cp-{i:03}"), "prompt", "provider", "model", &[]);
        checkpoint.created_at = u64::try_from(i).unwrap();
        store.save_checkpoint(checkpoint).await.unwrap();
    }

    let ids = store.list_checkpoints().await.unwrap();
    assert_eq!(
        ids.len(),
        FileCheckpointStore::DEFAULT_MAX_CHECKPOINTS,
        "default retention must cap the checkpoint count"
    );
    assert_eq!(
        ids[0],
        format!("cp-{:03}", total - 1),
        "newest checkpoint must survive pruning"
    );
    assert!(
        !ids.contains(&"cp-000".to_string()),
        "oldest checkpoint must be pruned"
    );
}

#[tokio::test]
async fn retention_unbounded_opt_out_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf())
        .unwrap()
        .unbounded();

    let total = FileCheckpointStore::DEFAULT_MAX_CHECKPOINTS + 5;
    for i in 0..total {
        let mut checkpoint =
            Checkpoint::new(format!("cp-{i:03}"), "prompt", "provider", "model", &[]);
        checkpoint.created_at = u64::try_from(i).unwrap();
        store.save_checkpoint(checkpoint).await.unwrap();
    }

    assert_eq!(
        store.list_checkpoints().await.unwrap().len(),
        total,
        "unbounded store must retain every checkpoint"
    );
}

#[tokio::test]
async fn retention_ignores_foreign_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf())
        .unwrap()
        .with_max_checkpoints(1);

    // Foreign files that must never be deleted, even under pruning
    // pressure: wrong extension, invalid JSON, and JSON that is not a
    // checkpoint.
    std::fs::write(dir.path().join("scratch.tmp"), "not a checkpoint").unwrap();
    std::fs::write(dir.path().join("broken.json"), "{not valid json").unwrap();
    std::fs::write(
        dir.path().join("wrong-shape.json"),
        serde_json::json!({"id": "wrong-shape"}).to_string(),
    )
    .unwrap();

    let mut older = Checkpoint::new("older", "prompt", "provider", "model", &[]);
    older.created_at = 10;
    store.save_checkpoint(older).await.unwrap();
    let mut newer = Checkpoint::new("newer", "prompt", "provider", "model", &[]);
    newer.created_at = 20;
    store.save_checkpoint(newer).await.unwrap();

    let ids = store.list_checkpoints().await.unwrap();
    assert_eq!(ids, vec!["newer".to_string()]);
    assert!(dir.path().join("scratch.tmp").exists());
    assert!(dir.path().join("broken.json").exists());
    assert!(dir.path().join("wrong-shape.json").exists());
}

#[tokio::test]
async fn retention_same_id_overwrite_does_not_prune_survivors() {
    // Rolling-style usage: repeatedly saving the SAME id under retention
    // must not inflate the count or evict unrelated newer checkpoints.
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf())
        .unwrap()
        .with_max_checkpoints(2);

    let mut other = Checkpoint::new("other", "prompt", "provider", "model", &[]);
    other.created_at = 100;
    store.save_checkpoint(other).await.unwrap();

    for (turn, created_at) in [(1, 50), (2, 60), (3, 70)] {
        let mut rolling =
            Checkpoint::new("rolling", "prompt", "provider", "model", &[]).with_turn_count(turn);
        rolling.created_at = created_at;
        store.save_checkpoint(rolling).await.unwrap();
    }

    let ids = store.list_checkpoints().await.unwrap();
    assert_eq!(ids, vec!["other".to_string(), "rolling".to_string()]);
    let rolling = store.load_checkpoint("rolling").await.unwrap().unwrap();
    assert_eq!(rolling.turn_count, 3, "content must match the latest save");
}

#[tokio::test]
async fn file_checkpoint_store_rejects_unsafe_checkpoint_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

    let err = store.load_checkpoint("../escape").await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_checkpoint_id_rejects_colon() {
    let err = validate_checkpoint_id("C:drive").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_checkpoint_id_rejects_control_chars() {
    let err = validate_checkpoint_id("checkpoint\nid").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}
