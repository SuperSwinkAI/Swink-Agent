//! Tests for `checkpoint`.
#![cfg(test)]

use super::*;
use std::collections::HashMap;

use swink_agent::{AgentMessage, AssistantMessage, Cost, ModelSpec, StopReason, Usage};

/// Minimal in-memory checkpoint store for testing.
struct MockCheckpointStore {
    data: std::sync::Mutex<HashMap<String, String>>,
    saved: tokio::sync::Notify,
}

impl MockCheckpointStore {
    fn new() -> Self {
        Self {
            data: std::sync::Mutex::new(HashMap::new()),
            saved: tokio::sync::Notify::new(),
        }
    }

    fn get(&self, id: &str) -> Option<Checkpoint> {
        let guard = self.data.lock().unwrap();
        guard.get(id).map(|s| serde_json::from_str(s).unwrap())
    }

    async fn wait_for_checkpoint(&self, id: &str) -> Checkpoint {
        loop {
            if let Some(checkpoint) = self.get(id) {
                return checkpoint;
            }

            self.saved.notified().await;
        }
    }
}

impl CheckpointStore for MockCheckpointStore {
    fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()> {
        let json = serde_json::to_string(&checkpoint).unwrap();
        let id = checkpoint.id;
        Box::pin(async move {
            self.data.lock().unwrap().insert(id, json);
            self.saved.notify_waiters();
            Ok(())
        })
    }

    fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>> {
        let id = id.to_string();
        Box::pin(async move {
            let guard = self.data.lock().unwrap();
            Ok(guard.get(&id).map(|s| serde_json::from_str(s).unwrap()))
        })
    }

    fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>> {
        Box::pin(async move { Ok(self.data.lock().unwrap().keys().cloned().collect()) })
    }

    fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()> {
        let id = id.to_string();
        Box::pin(async move {
            self.data.lock().unwrap().remove(&id);
            Ok(())
        })
    }
}

fn sample_model_spec() -> ModelSpec {
    ModelSpec::new("anthropic", "claude-sonnet-4-20250514")
}

fn sample_assistant_message() -> AssistantMessage {
    AssistantMessage::new(
        vec![swink_agent::ContentBlock::Text {
            text: "Hello!".to_string(),
        }],
        "anthropic",
        "claude-sonnet-4-20250514",
    )
    .with_timestamp(0)
}

fn sample_messages() -> Vec<AgentMessage> {
    use swink_agent::{ContentBlock, LlmMessage, UserMessage};
    vec![
        AgentMessage::Llm(LlmMessage::User(
            UserMessage::new(vec![ContentBlock::Text {
                text: "What is 2+2?".to_string(),
            }])
            .with_timestamp(100),
        )),
        AgentMessage::Llm(LlmMessage::Assistant(sample_assistant_message())),
    ]
}

/// Shared `PolicyContext` builder for tests. `overflow_signal` is always
/// `false` and `new_messages` is always empty across every call site in
/// this file, so those two fields are fixed here rather than threaded
/// through as parameters.
fn make_policy_ctx<'a>(
    turn_index: usize,
    message_count: usize,
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a swink_agent::SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(turn_index, usage, cost, message_count, false, &[], state)
}

/// Shared `TurnPolicyContext` builder for tests. `tool_results` is always
/// empty and `stop_reason` is always `StopReason::Stop` across every call
/// site in this file, so those two fields are fixed here rather than
/// threaded through as parameters.
fn make_turn_ctx<'a>(
    assistant_message: &'a AssistantMessage,
    system_prompt: &'a str,
    model_spec: &'a ModelSpec,
    context_messages: &'a [AgentMessage],
) -> TurnPolicyContext<'a> {
    TurnPolicyContext::new(
        assistant_message,
        &[],
        StopReason::Stop,
        system_prompt,
        model_spec,
        context_messages,
    )
}

#[test]
fn name_returns_checkpoint() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store);
    assert_eq!(policy.name(), "checkpoint");
}

#[test]
fn evaluate_returns_continue() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store);

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(0, 0, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "Be helpful.", &model, &messages);

    let result = policy.evaluate(&ctx, &turn);
    assert!(matches!(result, PolicyVerdict::Continue));
}

#[tokio::test]
async fn checkpoint_contains_system_prompt() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(0, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "You are a helpful math tutor.", &model, &messages);

    policy.evaluate(&ctx, &turn);

    let cp = store.wait_for_checkpoint("turn-0").await;
    assert_eq!(cp.system_prompt, "You are a helpful math tutor.");
}

#[tokio::test]
async fn checkpoint_contains_model_identity() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(1, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "prompt", &model, &messages);

    policy.evaluate(&ctx, &turn);

    let cp = store.wait_for_checkpoint("turn-1").await;
    assert_eq!(cp.provider, "anthropic");
    assert_eq!(cp.model_id, "claude-sonnet-4-20250514");
}

#[tokio::test]
async fn checkpoint_contains_message_history() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(0, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "prompt", &model, &messages);

    policy.evaluate(&ctx, &turn);

    let cp = store.wait_for_checkpoint("turn-0").await;
    assert_eq!(
        cp.messages.len(),
        2,
        "should contain both user and assistant messages"
    );
}

#[tokio::test]
async fn checkpoint_roundtrip_save_load() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

    let usage = Usage::default().with_input(100).with_output(50);
    let cost = Cost::default().with_input(0.01).with_output(0.005);
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(3, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "You are a math tutor.", &model, &messages);

    policy.evaluate(&ctx, &turn);
    store.wait_for_checkpoint("turn-3").await;

    // Load via the CheckpointStore trait
    let loaded = store
        .load_checkpoint("turn-3")
        .await
        .expect("load should succeed")
        .expect("checkpoint should exist");

    assert_eq!(loaded.system_prompt, "You are a math tutor.");
    assert_eq!(loaded.provider, "anthropic");
    assert_eq!(loaded.model_id, "claude-sonnet-4-20250514");
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.turn_count, 3);
    assert_eq!(loaded.usage.input, 100);
    assert_eq!(loaded.usage.output, 50);

    // Restore messages and verify content
    let restored = loaded.restore_messages(None);
    assert_eq!(restored.len(), 2);
}

#[test]
fn session_id_scopes_checkpoint_ids() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store).with_session_id("sess-a");
    assert_eq!(policy.checkpoint_id(0), "sess-a-turn-0");
    assert_eq!(policy.checkpoint_id(7), "sess-a-turn-7");
}

#[test]
fn default_checkpoint_ids_keep_legacy_format() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store);
    assert_eq!(policy.checkpoint_id(0), "turn-0");
}

#[tokio::test]
async fn session_scoped_runs_do_not_collide() {
    // Two "runs" (turn_index restarts at 0 in each) against one store,
    // each with its own session id: both turn-0 checkpoints survive.
    let store = Arc::new(MockCheckpointStore::new());
    let run1 =
        CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>).with_session_id("run1");
    let run2 =
        CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>).with_session_id("run2");

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(0, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn1 = make_turn_ctx(&msg, "first run", &model, &messages);
    run1.evaluate(&ctx, &turn1);

    let turn2 = make_turn_ctx(&msg, "second run", &model, &messages);
    run2.evaluate(&ctx, &turn2);

    let cp1 = store.wait_for_checkpoint("run1-turn-0").await;
    let cp2 = store.wait_for_checkpoint("run2-turn-0").await;
    assert_eq!(cp1.system_prompt, "first run");
    assert_eq!(cp2.system_prompt, "second run");
}

#[tokio::test]
async fn default_ids_collide_across_runs() {
    // Documents the CURRENT DEFAULT behavior (kept for backward compat):
    // without a session id, turn_index restarting at 0 in a second run
    // reuses "turn-0" and silently overwrites the first run's checkpoint.
    // This is the stale-restore hazard `with_session_id` exists to prevent.
    let store = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(0, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn1 = make_turn_ctx(&msg, "first run", &model, &messages);

    policy.evaluate(&ctx, &turn1);
    let cp = store.wait_for_checkpoint("turn-0").await;
    assert_eq!(cp.system_prompt, "first run");

    // "Second run": turn_index is 0 again.
    let turn2 = make_turn_ctx(&msg, "second run", &model, &messages);
    policy.evaluate(&ctx, &turn2);

    loop {
        let cp = store.get("turn-0").unwrap();
        if cp.system_prompt == "second run" {
            break; // run 1's checkpoint was silently overwritten
        }
        store.saved.notified().await;
    }
}

#[tokio::test]
async fn rolling_policy_overwrites_single_id_across_turns() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = RollingCheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);
    assert_eq!(policy.name(), "rolling-checkpoint");

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();

    for turn_index in 0..3 {
        let ctx = make_policy_ctx(turn_index, 2, &usage, &cost, &state);
        let turn = make_turn_ctx(&msg, "rolling prompt", &model, &messages);
        let verdict = policy.evaluate(&ctx, &turn);
        assert!(matches!(verdict, PolicyVerdict::Continue));

        // Wait until this turn's save lands before evaluating the next,
        // so the final content deterministically reflects the last turn.
        loop {
            if let Some(cp) = store.get("rolling")
                && cp.turn_count == turn_index
            {
                break;
            }
            store.saved.notified().await;
        }
    }

    // Exactly one checkpoint ID exists, and it matches the latest turn.
    let guard = store.data.lock().unwrap();
    assert_eq!(guard.len(), 1, "rolling policy must keep a single ID");
    let cp: Checkpoint = serde_json::from_str(guard.get("rolling").unwrap()).unwrap();
    assert_eq!(cp.turn_count, 2);
    assert_eq!(cp.system_prompt, "rolling prompt");
}

#[tokio::test]
async fn rolling_policy_session_id_scopes_the_single_id() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = RollingCheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>)
        .with_session_id("sess-a");

    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(0, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "prompt", &model, &messages);

    policy.evaluate(&ctx, &turn);
    let cp = store.wait_for_checkpoint("sess-a-rolling").await;
    assert_eq!(cp.id, "sess-a-rolling");
}

#[tokio::test]
async fn checkpoint_contains_restorable_session_state() {
    let store = Arc::new(MockCheckpointStore::new());
    let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

    let usage = Usage::default();
    let cost = Cost::default();
    let mut state = swink_agent::SessionState::new();
    state.set("workflow_id", "wf-123").unwrap();
    state
        .set("profile", serde_json::json!({"tier": "pro", "score": 42}))
        .unwrap();
    let ctx = make_policy_ctx(4, 2, &usage, &cost, &state);
    let msg = sample_assistant_message();
    let model = sample_model_spec();
    let messages = sample_messages();
    let turn = make_turn_ctx(&msg, "Track session state.", &model, &messages);

    policy.evaluate(&ctx, &turn);
    store.wait_for_checkpoint("turn-4").await;

    let loaded = store
        .load_checkpoint("turn-4")
        .await
        .expect("load should succeed")
        .expect("checkpoint should exist");
    let restored = swink_agent::SessionState::restore_from_snapshot(
        loaded
            .state
            .expect("checkpoint should include session state"),
    )
    .expect("state snapshot should restore");

    assert_eq!(restored.get::<String>("workflow_id"), Some("wf-123".into()));
    assert_eq!(
        restored.get_raw("profile"),
        Some(&serde_json::json!({"tier": "pro", "score": 42}))
    );
}
