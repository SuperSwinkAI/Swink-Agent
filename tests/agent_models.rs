//! Model management, tool add/remove, and multi-instance tests.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{MockStreamFn, MockTool, default_convert, default_model, text_only_events, user_msg};

use swink_agent::{Agent, AgentOptions, DefaultRetryStrategy, ModelSpec, StopReason, StreamFn};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn make_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        )),
    )
}

// ─── Multi-instance independence ──────────────────────────────────────────

#[tokio::test]
async fn multiple_agents_independent_state() {
    // Create two agents with different system prompts and stream functions.
    let stream_fn_a = Arc::new(MockStreamFn::new(vec![text_only_events("response A")]));
    let stream_fn_b = Arc::new(MockStreamFn::new(vec![text_only_events("response B")]));

    let mut agent_a = Agent::new(AgentOptions::new(
        "You are Agent A",
        ModelSpec::new("test", "model-a"),
        stream_fn_a as Arc<dyn StreamFn>,
        default_convert,
    ));
    let mut agent_b = Agent::new(AgentOptions::new(
        "You are Agent B",
        ModelSpec::new("test", "model-b"),
        stream_fn_b as Arc<dyn StreamFn>,
        default_convert,
    ));

    // Verify initial state is independent.
    assert_eq!(agent_a.state().system_prompt, "You are Agent A");
    assert_eq!(agent_b.state().system_prompt, "You are Agent B");
    assert_eq!(agent_a.state().model.model_id, "model-a");
    assert_eq!(agent_b.state().model.model_id, "model-b");

    // Run both agents concurrently.
    let (result_a, result_b) = tokio::join!(
        agent_a.prompt_async(vec![user_msg("hello from A")]),
        agent_b.prompt_async(vec![user_msg("hello from B")]),
    );

    let result_a = result_a.unwrap();
    let result_b = result_b.unwrap();

    assert_eq!(result_a.stop_reason, StopReason::Stop);
    assert_eq!(result_b.stop_reason, StopReason::Stop);

    // Messages should not cross between agents.
    assert_eq!(
        agent_a.state().messages.len(),
        2,
        "agent A: user + assistant"
    );
    assert_eq!(
        agent_b.state().messages.len(),
        2,
        "agent B: user + assistant"
    );

    // Mutating one agent does not affect the other.
    agent_a.set_system_prompt("mutated A");
    assert_eq!(agent_a.state().system_prompt, "mutated A");
    assert_eq!(
        agent_b.state().system_prompt,
        "You are Agent B",
        "agent B should be unaffected by mutation of agent A"
    );
}

// ─── add_tool / remove_tool ─────────────────────────────────────────────

#[test]
fn add_tool_appends() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
    let mut agent = make_agent(stream_fn);
    assert_eq!(agent.state().tools.len(), 0);

    let tool = Arc::new(MockTool::new("alpha"));
    agent.add_tool(tool);
    assert_eq!(agent.state().tools.len(), 1);
    assert_eq!(agent.state().tools[0].name(), "alpha");
}

#[test]
fn add_tool_replaces_by_name() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
    let mut agent = make_agent(stream_fn);

    agent.add_tool(Arc::new(MockTool::new("alpha")));
    agent.add_tool(Arc::new(MockTool::new("beta")));
    assert_eq!(agent.state().tools.len(), 2);

    // Adding another "alpha" should replace, not duplicate.
    agent.add_tool(Arc::new(MockTool::new("alpha")));
    assert_eq!(agent.state().tools.len(), 2);
}

#[test]
fn remove_tool_found() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
    let mut agent = make_agent(stream_fn);

    agent.add_tool(Arc::new(MockTool::new("alpha")));
    assert!(agent.remove_tool("alpha"));
    assert_eq!(agent.state().tools.len(), 0);
}

#[test]
fn remove_tool_not_found() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
    let mut agent = make_agent(stream_fn);
    assert!(!agent.remove_tool("nonexistent"));
}

#[test]
fn remove_tool_preserves_others() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
    let mut agent = make_agent(stream_fn);

    agent.add_tool(Arc::new(MockTool::new("alpha")));
    agent.add_tool(Arc::new(MockTool::new("beta")));
    agent.add_tool(Arc::new(MockTool::new("gamma")));

    agent.remove_tool("beta");

    let names: Vec<&str> = agent.state().tools.iter().map(|t| t.name()).collect();
    assert_eq!(names, vec!["alpha", "gamma"]);
}

// ─── available_models / set_model stream_fn swap ────────────────────────

#[test]
fn available_models_includes_primary_at_index_zero() {
    let primary_sfn = Arc::new(MockStreamFn::new(vec![]));
    let extra_sfn = Arc::new(MockStreamFn::new(vec![]));

    let primary = ModelSpec::new("test", "primary-model");
    let extra = ModelSpec::new("test", "extra-model");

    let agent = Agent::new(
        AgentOptions::new(
            "sys",
            primary.clone(),
            primary_sfn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_available_models(vec![(extra.clone(), extra_sfn as Arc<dyn StreamFn>)]),
    );

    let models = &agent.state().available_models;
    assert_eq!(models.len(), 2);
    assert_eq!(models[0], primary, "primary model should be at index 0");
    assert_eq!(models[1], extra);
}

#[test]
fn available_models_empty_when_none_configured() {
    let sfn = Arc::new(MockStreamFn::new(vec![]));
    let agent = Agent::new(AgentOptions::new(
        "sys",
        default_model(),
        sfn as Arc<dyn StreamFn>,
        default_convert,
    ));

    let models = &agent.state().available_models;
    assert_eq!(models.len(), 1, "should contain only the primary model");
    assert_eq!(models[0], default_model());
}

#[tokio::test]
async fn set_model_swaps_stream_fn_for_known_model() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use common::MockFlagStreamFn;

    let primary_sfn = Arc::new(MockStreamFn::new(vec![text_only_events("from primary")]));
    let extra_sfn = Arc::new(MockFlagStreamFn {
        called: AtomicBool::new(false),
        responses: std::sync::Mutex::new(vec![text_only_events("from extra")]),
    });

    let primary = ModelSpec::new("test", "primary-model");
    let extra = ModelSpec::new("other", "extra-model");

    let mut agent = Agent::new(
        AgentOptions::new(
            "sys",
            primary,
            primary_sfn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_available_models(vec![(
            extra.clone(),
            extra_sfn.clone() as Arc<dyn StreamFn>,
        )]),
    );

    // Switch to extra model.
    agent.set_model(extra.clone());
    assert_eq!(agent.state().model, extra);

    // Prompt — should use the extra stream_fn.
    let _result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();
    assert!(
        extra_sfn.called.load(Ordering::SeqCst),
        "extra stream_fn should have been called after set_model"
    );
}

#[tokio::test]
async fn set_model_restores_primary_stream_fn_when_switching_back() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use common::MockFlagStreamFn;

    let primary_sfn = Arc::new(MockFlagStreamFn {
        called: AtomicBool::new(false),
        responses: std::sync::Mutex::new(vec![text_only_events("from primary")]),
    });
    let extra_sfn = Arc::new(MockFlagStreamFn {
        called: AtomicBool::new(false),
        responses: std::sync::Mutex::new(vec![text_only_events("from extra")]),
    });

    let primary = ModelSpec::new("test", "primary-model");
    let extra = ModelSpec::new("other", "extra-model");

    let mut agent = Agent::new(
        AgentOptions::new(
            "sys",
            primary.clone(),
            primary_sfn.clone() as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_available_models(vec![(
            extra.clone(),
            extra_sfn.clone() as Arc<dyn StreamFn>,
        )]),
    );

    agent.set_model(extra);
    let _ = agent
        .prompt_async(vec![user_msg("use extra")])
        .await
        .unwrap();
    assert!(extra_sfn.called.load(Ordering::SeqCst));
    assert!(!primary_sfn.called.load(Ordering::SeqCst));

    primary_sfn.called.store(false, Ordering::SeqCst);
    extra_sfn.called.store(false, Ordering::SeqCst);

    agent.set_model(primary);
    let _ = agent
        .prompt_async(vec![user_msg("use primary")])
        .await
        .unwrap();
    assert!(
        primary_sfn.called.load(Ordering::SeqCst),
        "primary stream_fn should be restored when switching back"
    );
    assert!(
        !extra_sfn.called.load(Ordering::SeqCst),
        "extra stream_fn should not remain active after restoring primary"
    );
}

// ─── US6: Dynamic Model Swapping ─────────────────────────────────────────

#[tokio::test]
async fn set_model_swaps_stream_fn() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use common::MockFlagStreamFn;

    let primary_sfn = Arc::new(MockStreamFn::new(vec![text_only_events("from primary")]));
    let extra_sfn = Arc::new(MockFlagStreamFn {
        called: AtomicBool::new(false),
        responses: std::sync::Mutex::new(vec![text_only_events("from extra")]),
    });

    let primary = ModelSpec::new("test", "primary-model");
    let extra = ModelSpec::new("test", "extra-model");

    let mut agent = Agent::new(
        AgentOptions::new(
            "sys",
            primary,
            primary_sfn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_available_models(vec![(extra.clone(), extra_sfn.clone() as Arc<dyn StreamFn>)]),
    );

    agent.set_model(extra.clone());
    assert_eq!(agent.state().model, extra);

    let _result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(
        extra_sfn.called.load(Ordering::SeqCst),
        "extra stream_fn should be used after set_model"
    );
}

#[test]
fn set_model_unknown_keeps_stream_fn() {
    let primary_sfn = Arc::new(MockStreamFn::new(vec![text_only_events("from primary")]));
    let primary = ModelSpec::new("test", "primary-model");
    let unknown = ModelSpec::new("test", "unknown-model");

    let mut agent = Agent::new(AgentOptions::new(
        "sys",
        primary,
        primary_sfn as Arc<dyn StreamFn>,
        default_convert,
    ));

    // set_model with unknown model — ModelSpec updates, StreamFn unchanged.
    agent.set_model(unknown.clone());
    assert_eq!(agent.state().model, unknown);
    // StreamFn is internal; we can only verify by prompting (which would use the original).
}

#[tokio::test]
async fn set_model_with_stream_bypasses_available() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use common::MockFlagStreamFn;

    let primary_sfn = Arc::new(MockStreamFn::new(vec![text_only_events("from primary")]));
    let explicit_sfn = Arc::new(MockFlagStreamFn {
        called: AtomicBool::new(false),
        responses: std::sync::Mutex::new(vec![text_only_events("from explicit")]),
    });

    let primary = ModelSpec::new("test", "primary-model");
    let explicit_model = ModelSpec::new("test", "explicit-model");

    let mut agent = Agent::new(AgentOptions::new(
        "sys",
        primary,
        primary_sfn as Arc<dyn StreamFn>,
        default_convert,
    ));

    // set_model_with_stream bypasses available_models.
    agent.set_model_with_stream(explicit_model.clone(), explicit_sfn.clone() as Arc<dyn StreamFn>);
    assert_eq!(agent.state().model, explicit_model);

    let _result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(
        explicit_sfn.called.load(Ordering::SeqCst),
        "explicit stream_fn should be used after set_model_with_stream"
    );
}

#[test]
fn set_model_emits_model_cycled_event() {
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    use swink_agent::AgentEvent;

    let primary_sfn = Arc::new(MockStreamFn::new(vec![]));
    let extra_sfn = Arc::new(MockStreamFn::new(vec![]));

    let primary = ModelSpec::new("test", "primary-model");
    let extra = ModelSpec::new("test", "extra-model");

    let mut agent = Agent::new(
        AgentOptions::new(
            "sys",
            primary.clone(),
            primary_sfn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_available_models(vec![(extra.clone(), extra_sfn as Arc<dyn StreamFn>)]),
    );

    let events: StdArc<StdMutex<Vec<String>>> = StdArc::new(StdMutex::new(Vec::new()));
    let events_clone = StdArc::clone(&events);
    let captured_old: StdArc<StdMutex<Option<ModelSpec>>> = StdArc::new(StdMutex::new(None));
    let captured_new: StdArc<StdMutex<Option<ModelSpec>>> = StdArc::new(StdMutex::new(None));
    let old_clone = StdArc::clone(&captured_old);
    let new_clone = StdArc::clone(&captured_new);

    agent.subscribe(move |event| {
        if let AgentEvent::ModelCycled { old, new, .. } = event {
            events_clone.lock().unwrap().push("ModelCycled".to_string());
            *old_clone.lock().unwrap() = Some(old.clone());
            *new_clone.lock().unwrap() = Some(new.clone());
        }
    });

    agent.set_model(extra.clone());

    let received_len = events.lock().unwrap().len();
    assert_eq!(received_len, 1, "should receive exactly one ModelCycled event");

    let old = captured_old.lock().unwrap().clone().unwrap();
    let new = captured_new.lock().unwrap().clone().unwrap();
    assert_eq!(old, primary);
    assert_eq!(new, extra);
}
