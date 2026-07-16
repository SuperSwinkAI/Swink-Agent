#![cfg(feature = "checkpoint")]
//! Integration tests for checkpoint hardening (#1070): session-scoped
//! checkpoint IDs, store retention, and `RollingCheckpointPolicy` — driven
//! through real `Agent::prompt_text` runs against a real
//! [`FileCheckpointStore`].

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{Agent, AgentOptions, Checkpoint, CheckpointStore, StreamFn};
use swink_agent_memory::FileCheckpointStore;
use swink_agent_policies::{CheckpointPolicy, RollingCheckpointPolicy};

use common::{MockStreamFn, MockTool, default_model, text_only_events, tool_call_events};

/// Poll until `pred` holds; checkpoint saves are fire-and-forget spawned
/// tasks, so they can land after `prompt_text` returns.
async fn wait_for(what: &str, pred: impl Fn() -> bool) {
    for _ in 0..500 {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn read_checkpoint(path: &Path) -> Option<Checkpoint> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Run a single-turn prompt with the given post-turn checkpoint policy.
async fn run_one_turn(system_prompt: &str, policy: CheckpointPolicy) {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events("done")]));
    let options = AgentOptions::new_simple(system_prompt, default_model(), stream_fn)
        .with_post_turn_policy(policy);
    let mut agent = Agent::new(options);
    agent
        .prompt_text("go")
        .await
        .expect("prompt should succeed");
}

#[tokio::test]
async fn prompt_runs_with_session_ids_do_not_collide() {
    let dir = tempfile::tempdir().unwrap();

    for (session, prompt) in [("run1", "first run"), ("run2", "second run")] {
        let store: Arc<dyn CheckpointStore> =
            Arc::new(FileCheckpointStore::new(dir.path().to_path_buf()).unwrap());
        run_one_turn(
            prompt,
            CheckpointPolicy::new(store).with_session_id(session),
        )
        .await;
    }

    let run1 = dir.path().join("run1-turn-0.json");
    let run2 = dir.path().join("run2-turn-0.json");
    wait_for("both session-scoped checkpoints", || {
        run1.exists() && run2.exists()
    })
    .await;

    // Each run's turn-0 checkpoint survives with its own content.
    assert_eq!(read_checkpoint(&run1).unwrap().system_prompt, "first run");
    assert_eq!(read_checkpoint(&run2).unwrap().system_prompt, "second run");
}

#[tokio::test]
async fn prompt_runs_without_session_ids_collide() {
    // Documents the CURRENT DEFAULT behavior (kept for backward compat):
    // without session ids, checkpoint IDs are "turn-{n}" and the turn index
    // resets per prompt() run, so a second run overwrites the first run's
    // IDs. If the second run is shorter, the store ends up with a mix of
    // fresh and stale checkpoints — "restore the highest turn" would restore
    // STALE history. This is the hazard `with_session_id` exists to prevent.
    let dir = tempfile::tempdir().unwrap();
    let turn0 = dir.path().join("turn-0.json");
    let turn1 = dir.path().join("turn-1.json");

    // Run 1: two turns (tool call, then final text) -> turn-0, turn-1.
    {
        let store: Arc<dyn CheckpointStore> =
            Arc::new(FileCheckpointStore::new(dir.path().to_path_buf()).unwrap());
        let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![
            tool_call_events("call-1", "mock_tool", "{}"),
            text_only_events("done"),
        ]));
        let options = AgentOptions::new_simple("first run", default_model(), stream_fn)
            .with_tools(vec![Arc::new(MockTool::new("mock_tool"))])
            .with_post_turn_policy(CheckpointPolicy::new(store));
        let mut agent = Agent::new(options);
        agent.prompt_text("go").await.expect("run 1 should succeed");
    }
    wait_for("run 1 checkpoints", || {
        read_checkpoint(&turn0).is_some_and(|cp| cp.system_prompt == "first run") && turn1.exists()
    })
    .await;

    // Run 2: one turn, fresh agent, same store -> overwrites turn-0 only.
    {
        let store: Arc<dyn CheckpointStore> =
            Arc::new(FileCheckpointStore::new(dir.path().to_path_buf()).unwrap());
        run_one_turn("second run", CheckpointPolicy::new(store)).await;
    }
    wait_for("run 2 overwrote turn-0", || {
        read_checkpoint(&turn0).is_some_and(|cp| cp.system_prompt == "second run")
    })
    .await;

    // The collision, pinned: turn-0 is fresh (run 2), but the "highest turn"
    // checkpoint is stale leftover history from run 1.
    assert_eq!(read_checkpoint(&turn1).unwrap().system_prompt, "first run");
}

#[tokio::test]
async fn rolling_policy_with_retention_composes() {
    let dir = tempfile::tempdir().unwrap();

    // Seed two stale per-turn checkpoints from an "old run".
    {
        let seed = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();
        let mut old = Checkpoint::new("stale-old", "old", "provider", "model", &[]);
        old.created_at = 1;
        seed.save_checkpoint(old).await.unwrap();
        let mut newer = Checkpoint::new("stale-new", "old", "provider", "model", &[]);
        newer.created_at = 2;
        seed.save_checkpoint(newer).await.unwrap();
    }

    // A rolling policy writing through a retention-capped store: two turns.
    let store: Arc<dyn CheckpointStore> = Arc::new(
        FileCheckpointStore::new(dir.path().to_path_buf())
            .unwrap()
            .with_max_checkpoints(2),
    );
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call-1", "mock_tool", "{}"),
        text_only_events("done"),
    ]));
    let options = AgentOptions::new_simple("rolling run", default_model(), stream_fn)
        .with_tools(vec![Arc::new(MockTool::new("mock_tool"))])
        .with_post_turn_policy(RollingCheckpointPolicy::new(store).with_session_id("sess"));
    let mut agent = Agent::new(options);
    agent
        .prompt_text("go")
        .await
        .expect("prompt should succeed");

    // Both turns roll the same checkpoint; wait for the second (turn index 1).
    let rolling = dir.path().join("sess-rolling.json");
    wait_for("rolling checkpoint reflects the latest turn", || {
        read_checkpoint(&rolling).is_some_and(|cp| cp.turn_count == 1)
    })
    .await;

    // Retention pruned the oldest stale checkpoint; the rolling ID never
    // multiplied (same ID overwritten in place), so exactly two remain.
    assert!(!dir.path().join("stale-old.json").exists());
    assert!(dir.path().join("stale-new.json").exists());
    let cp = read_checkpoint(&rolling).unwrap();
    assert_eq!(cp.system_prompt, "rolling run");
    assert_eq!(cp.turn_count, 1);
}
