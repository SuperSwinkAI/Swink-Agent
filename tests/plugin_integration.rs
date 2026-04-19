#![cfg(all(feature = "plugins", feature = "testkit"))]

//! Integration tests for plugin contribution merge in Agent::new().

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use swink_agent::{
    Agent, AgentOptions, Plugin, PolicyContext, PolicyVerdict, PostTurnPolicy, PreTurnPolicy,
};

mod common;
use common::{
    MOCK_PLUGIN_GLOBAL_ORDER, MockPlugin, MockStreamFn, OrderRecordingPreTurnPolicy,
    RecordingPostTurnPolicy, default_convert, default_model, text_only_events, user_msg,
};

fn make_agent_with_plugins(plugins: Vec<Arc<dyn Plugin>>) -> Agent {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugins(plugins);
    Agent::new(options)
}

// ─── T009: Plugin contributing a post-turn policy ──────────────────────────

#[tokio::test]
async fn plugin_post_turn_policy_evaluates_during_loop() {
    let fired = Arc::new(AtomicBool::new(false));
    let plugin: Arc<dyn Plugin> =
        Arc::new(MockPlugin::new("test-policy").with_post_turn_tracker(Arc::clone(&fired)));

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options =
        AgentOptions::new("test", default_model(), stream_fn, default_convert).with_plugin(plugin);

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    assert!(
        fired.load(Ordering::SeqCst),
        "post-turn policy should have fired during the loop"
    );
}

// ─── T010: Plugin contributing tools appear namespaced ─────────────────────

#[test]
fn plugin_tools_appear_namespaced_in_agent_tool_list() {
    let plugin: Arc<dyn Plugin> =
        Arc::new(MockPlugin::new("myplugin").with_tools(&["save", "load"]));

    let agent = make_agent_with_plugins(vec![plugin]);

    let tool_names: Vec<&str> = agent.state().tools.iter().map(|t| t.name()).collect();
    assert!(
        tool_names.contains(&"myplugin_save"),
        "expected namespaced tool 'myplugin_save', got: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"myplugin_load"),
        "expected namespaced tool 'myplugin_load', got: {tool_names:?}"
    );
}

// ─── T011: Plugin event observer fires for AgentStart ──────────────────────

#[tokio::test]
async fn plugin_event_observer_called_for_agent_start() {
    let event_count = Arc::new(AtomicUsize::new(0));
    let plugin: Arc<dyn Plugin> =
        Arc::new(MockPlugin::new("observer").with_event_counter(Arc::clone(&event_count)));

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options =
        AgentOptions::new("test", default_model(), stream_fn, default_convert).with_plugin(plugin);

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    let count = event_count.load(Ordering::SeqCst);
    assert!(
        count > 0,
        "event observer should have been called at least once, got {count}"
    );
}

// ─── Phase 4 Helpers: Priority-Based Execution Order ─────────────────────

// ─── T017: Two plugins with different priorities ─────────────────────────

#[tokio::test]
async fn higher_priority_plugin_policy_runs_first() {
    // Reset global counter for this test.
    MOCK_PLUGIN_GLOBAL_ORDER.store(0, Ordering::SeqCst);

    let low_order = Arc::new(AtomicUsize::new(usize::MAX));
    let high_order = Arc::new(AtomicUsize::new(usize::MAX));

    let low_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("low")
            .with_priority(1)
            .with_pre_turn_order(Arc::clone(&low_order)),
    );
    let high_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("high")
            .with_priority(10)
            .with_pre_turn_order(Arc::clone(&high_order)),
    );

    // Register low first, high second — priority should override insertion order.
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugins(vec![low_plugin, high_plugin]);

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    let high_seq = high_order.load(Ordering::SeqCst);
    let low_seq = low_order.load(Ordering::SeqCst);

    assert!(
        high_seq < low_seq,
        "high-priority plugin should run first: high={high_seq}, low={low_seq}"
    );
}

// ─── T018: Two plugins with same priority — insertion order preserved ────

#[tokio::test]
async fn same_priority_plugins_preserve_insertion_order() {
    MOCK_PLUGIN_GLOBAL_ORDER.store(0, Ordering::SeqCst);

    let first_order = Arc::new(AtomicUsize::new(usize::MAX));
    let second_order = Arc::new(AtomicUsize::new(usize::MAX));

    let first_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("first")
            .with_priority(0)
            .with_pre_turn_order(Arc::clone(&first_order)),
    );
    let second_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("second")
            .with_priority(0)
            .with_pre_turn_order(Arc::clone(&second_order)),
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugins(vec![first_plugin, second_plugin]);

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    let first_seq = first_order.load(Ordering::SeqCst);
    let second_seq = second_order.load(Ordering::SeqCst);

    assert!(
        first_seq < second_seq,
        "first-registered plugin should run first when priorities are equal: first={first_seq}, second={second_seq}"
    );
}

// ─── T019: Higher-priority Stop prevents lower-priority evaluation ───────

#[tokio::test]
async fn higher_priority_stop_short_circuits_lower_priority() {
    MOCK_PLUGIN_GLOBAL_ORDER.store(0, Ordering::SeqCst);

    let low_order = Arc::new(AtomicUsize::new(usize::MAX));

    // High-priority plugin returns Stop.
    let high_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("blocker")
            .with_priority(10)
            .with_stopping_pre_turn(),
    );
    // Low-priority plugin should never run.
    let low_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("victim")
            .with_priority(1)
            .with_pre_turn_order(Arc::clone(&low_order)),
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugins(vec![low_plugin, high_plugin]);

    let mut agent = Agent::new(options);
    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    // The low-priority plugin's policy should not have been evaluated.
    let low_seq = low_order.load(Ordering::SeqCst);
    assert_eq!(
        low_seq,
        usize::MAX,
        "low-priority plugin policy should not have run after Stop, but got order={low_seq}"
    );

    // Agent should have produced no assistant messages (stopped before LLM call).
    assert!(
        result.messages.is_empty(),
        "expected no messages after pre-turn Stop, got {}",
        result.messages.len()
    );
}

// ─── T021: Short-circuit across merged list (plugin + direct policies) ───
// (Phase 4: US2 — verifies short-circuit semantics across merged policy list)

#[tokio::test]
async fn plugin_stop_prevents_direct_policy_evaluation() {
    let direct_fired = Arc::new(AtomicBool::new(false));
    let direct_fired_clone = Arc::clone(&direct_fired);

    // Direct pre-turn policy that records whether it fired.
    struct DirectPreTurnPolicy {
        fired: Arc<AtomicBool>,
    }
    impl PreTurnPolicy for DirectPreTurnPolicy {
        fn name(&self) -> &str {
            "direct-pre-turn"
        }
        fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
            self.fired.store(true, Ordering::SeqCst);
            PolicyVerdict::Continue
        }
    }

    // Plugin with Stop policy (priority 10 — runs before direct policies).
    let stopping_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("blocker")
            .with_priority(10)
            .with_stopping_pre_turn(),
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugin(stopping_plugin)
        .with_pre_turn_policy(DirectPreTurnPolicy {
            fired: direct_fired_clone,
        });

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    assert!(
        !direct_fired.load(Ordering::SeqCst),
        "direct pre-turn policy should not fire after plugin Stop verdict"
    );
}

// ─── Phase 5: User Story 3 — Backward-Compatible Composition ───────────

// ─── T022: Plugin policy runs before direct policy ─────────────────────

#[tokio::test]
async fn plugin_policy_runs_before_direct_policy() {
    MOCK_PLUGIN_GLOBAL_ORDER.store(0, Ordering::SeqCst);

    let plugin_order = Arc::new(AtomicUsize::new(usize::MAX));
    let direct_order = Arc::new(AtomicUsize::new(usize::MAX));

    // Plugin with a pre-turn policy (priority 0 — default).
    let plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("myplugin")
            .with_priority(0)
            .with_pre_turn_order(Arc::clone(&plugin_order)),
    );

    // Direct pre-turn policy.
    let direct_order_clone = Arc::clone(&direct_order);

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugin(plugin)
        .with_pre_turn_policy(OrderRecordingPreTurnPolicy {
            label: "direct-pre-turn".to_owned(),
            order: direct_order_clone,
        });

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    let plugin_seq = plugin_order.load(Ordering::SeqCst);
    let direct_seq = direct_order.load(Ordering::SeqCst);

    assert!(
        plugin_seq < direct_seq,
        "plugin policy should run before direct policy: plugin={plugin_seq}, direct={direct_seq}"
    );
}

// ─── T023: No plugins — direct policies behave identically ────────────

#[tokio::test]
async fn no_plugins_direct_policies_behave_identically() {
    MOCK_PLUGIN_GLOBAL_ORDER.store(0, Ordering::SeqCst);

    let first_order = Arc::new(AtomicUsize::new(usize::MAX));
    let second_order = Arc::new(AtomicUsize::new(usize::MAX));

    let first_order_clone = Arc::clone(&first_order);
    let second_order_clone = Arc::clone(&second_order);

    // Agent with two direct pre-turn policies and NO plugins.
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_pre_turn_policy(OrderRecordingPreTurnPolicy {
            label: "first-direct".to_owned(),
            order: first_order_clone,
        })
        .with_pre_turn_policy(OrderRecordingPreTurnPolicy {
            label: "second-direct".to_owned(),
            order: second_order_clone,
        });

    let mut agent = Agent::new(options);
    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    let first_seq = first_order.load(Ordering::SeqCst);
    let second_seq = second_order.load(Ordering::SeqCst);

    // Both should have fired.
    assert_ne!(
        first_seq,
        usize::MAX,
        "first direct policy should have fired"
    );
    assert_ne!(
        second_seq,
        usize::MAX,
        "second direct policy should have fired"
    );

    // Insertion order preserved.
    assert!(
        first_seq < second_seq,
        "direct policies should preserve insertion order: first={first_seq}, second={second_seq}"
    );

    // Agent should produce a normal response.
    assert!(
        !result.messages.is_empty(),
        "agent should produce messages when policies all Continue"
    );
}

// ─── Phase 6: User Story 4 — Registry Introspection ──────────────────────

// ─── T026: agent.plugins() returns all plugins in priority order ─────────

#[test]
fn agent_plugins_returns_all_in_priority_order() {
    let p_low: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("low")
            .with_priority(1)
            .with_pre_turn_order(Arc::new(AtomicUsize::new(0))),
    );
    let p_mid: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("mid")
            .with_priority(5)
            .with_pre_turn_order(Arc::new(AtomicUsize::new(0))),
    );
    let p_high: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("high")
            .with_priority(10)
            .with_pre_turn_order(Arc::new(AtomicUsize::new(0))),
    );

    // Register in low→mid→high order; plugins() should return high→mid→low.
    let agent = make_agent_with_plugins(vec![p_low, p_mid, p_high]);

    let names: Vec<&str> = agent.plugins().iter().map(|p| p.name()).collect();
    assert_eq!(
        names,
        vec!["high", "mid", "low"],
        "plugins() should return all plugins sorted by priority descending"
    );
}

// ─── T027: agent.plugin("name") returns correct plugin reference ─────────

#[test]
fn agent_plugin_by_name_returns_correct_reference() {
    let p_alpha: Arc<dyn Plugin> = Arc::new(MockPlugin::new("alpha").with_tools(&["save"]));
    let p_beta: Arc<dyn Plugin> = Arc::new(MockPlugin::new("beta").with_tools(&["load"]));

    let agent = make_agent_with_plugins(vec![p_alpha, p_beta]);

    let found = agent.plugin("beta");
    assert!(found.is_some(), "plugin 'beta' should be found");
    assert_eq!(found.unwrap().name(), "beta");

    let found_alpha = agent.plugin("alpha");
    assert!(found_alpha.is_some(), "plugin 'alpha' should be found");
    assert_eq!(found_alpha.unwrap().name(), "alpha");
}

// ─── T028: agent.plugin("nonexistent") returns None ──────────────────────

#[test]
fn agent_plugin_nonexistent_returns_none() {
    let p: Arc<dyn Plugin> = Arc::new(MockPlugin::new("existing").with_tools(&["tool1"]));
    let agent = make_agent_with_plugins(vec![p]);

    assert!(
        agent.plugin("nonexistent").is_none(),
        "looking up a nonexistent plugin should return None"
    );
}

// ─── Phase 7: User Story 5 — Initialization Callback ───────────────────

/// A plugin that tracks on_init calls with ordering.
struct InitPlugin {
    name: String,
    priority: i32,
    init_called: Arc<AtomicBool>,
    init_order: Arc<AtomicUsize>,
}

/// Shared counter to track init order across plugins.
static GLOBAL_INIT_ORDER: AtomicUsize = AtomicUsize::new(0);

impl InitPlugin {
    fn new(name: &str, init_called: Arc<AtomicBool>) -> Self {
        Self {
            name: name.to_owned(),
            priority: 0,
            init_called,
            init_order: Arc::new(AtomicUsize::new(usize::MAX)),
        }
    }

    fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    fn with_order(mut self, init_order: Arc<AtomicUsize>) -> Self {
        self.init_order = init_order;
        self
    }
}

impl Plugin for InitPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn on_init(&self, _agent: &Agent) {
        self.init_called.store(true, Ordering::SeqCst);
        let seq = GLOBAL_INIT_ORDER.fetch_add(1, Ordering::SeqCst);
        self.init_order.store(seq, Ordering::SeqCst);
    }
}

/// A plugin whose on_init panics, but still contributes a post-turn policy.
struct PanickingInitPlugin {
    name: String,
    fired: Arc<AtomicBool>,
}

impl PanickingInitPlugin {
    fn new(name: &str, fired: Arc<AtomicBool>) -> Self {
        Self {
            name: name.to_owned(),
            fired,
        }
    }
}

impl Plugin for PanickingInitPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_init(&self, _agent: &Agent) {
        panic!("intentional panic in on_init");
    }

    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
        let fired = Arc::clone(&self.fired);
        vec![Arc::new(RecordingPostTurnPolicy { fired })]
    }
}

// ─── T031: Plugin on_init is called once during Agent::new() ───────────

#[test]
fn plugin_on_init_called_once_during_agent_new() {
    let init_called = Arc::new(AtomicBool::new(false));
    let plugin: Arc<dyn Plugin> = Arc::new(InitPlugin::new("init-test", Arc::clone(&init_called)));

    let _agent = make_agent_with_plugins(vec![plugin]);

    assert!(
        init_called.load(Ordering::SeqCst),
        "on_init should have been called during Agent::new()"
    );
}

// ─── T032: Multiple plugins — on_init fires in priority order ──────────

#[test]
fn plugin_on_init_fires_in_priority_order() {
    GLOBAL_INIT_ORDER.store(0, Ordering::SeqCst);

    let low_called = Arc::new(AtomicBool::new(false));
    let high_called = Arc::new(AtomicBool::new(false));
    let low_order = Arc::new(AtomicUsize::new(usize::MAX));
    let high_order = Arc::new(AtomicUsize::new(usize::MAX));

    let low_plugin: Arc<dyn Plugin> = Arc::new(
        InitPlugin::new("low-init", Arc::clone(&low_called))
            .with_priority(1)
            .with_order(Arc::clone(&low_order)),
    );
    let high_plugin: Arc<dyn Plugin> = Arc::new(
        InitPlugin::new("high-init", Arc::clone(&high_called))
            .with_priority(10)
            .with_order(Arc::clone(&high_order)),
    );

    // Register low first, high second — priority should determine init order.
    let _agent = make_agent_with_plugins(vec![low_plugin, high_plugin]);

    assert!(
        low_called.load(Ordering::SeqCst),
        "low plugin on_init should have been called"
    );
    assert!(
        high_called.load(Ordering::SeqCst),
        "high plugin on_init should have been called"
    );

    let high_seq = high_order.load(Ordering::SeqCst);
    let low_seq = low_order.load(Ordering::SeqCst);

    assert!(
        high_seq < low_seq,
        "high-priority plugin on_init should fire first: high={high_seq}, low={low_seq}"
    );
}

// ─── T033: Panicking on_init is caught, agent continues, policies active ─

#[tokio::test]
async fn panicking_on_init_caught_agent_continues() {
    let policy_fired = Arc::new(AtomicBool::new(false));

    // Plugin whose on_init panics but contributes a post-turn policy.
    let panicking: Arc<dyn Plugin> = Arc::new(PanickingInitPlugin::new(
        "panicker",
        Arc::clone(&policy_fired),
    ));

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugin(panicking);

    // Agent construction should NOT panic.
    let mut agent = Agent::new(options);

    // Run a conversation — the panicking plugin's policy should still be active.
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    assert!(
        policy_fired.load(Ordering::SeqCst),
        "panicking plugin's post-turn policy should still fire after on_init panic"
    );
}

// ─── T024: Plugin Stop prevents ALL direct policies from evaluating ────

#[tokio::test]
async fn plugin_stop_prevents_all_direct_policies() {
    let direct_a_fired = Arc::new(AtomicBool::new(false));
    let direct_b_fired = Arc::new(AtomicBool::new(false));

    struct TrackingPreTurnPolicy {
        name: String,
        fired: Arc<AtomicBool>,
    }
    impl PreTurnPolicy for TrackingPreTurnPolicy {
        fn name(&self) -> &str {
            &self.name
        }
        fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
            self.fired.store(true, Ordering::SeqCst);
            PolicyVerdict::Continue
        }
    }

    // Plugin with Stop policy.
    let stopping_plugin: Arc<dyn Plugin> = Arc::new(
        MockPlugin::new("blocker")
            .with_priority(0)
            .with_stopping_pre_turn(),
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugin(stopping_plugin)
        .with_pre_turn_policy(TrackingPreTurnPolicy {
            name: "direct-a".to_owned(),
            fired: Arc::clone(&direct_a_fired),
        })
        .with_pre_turn_policy(TrackingPreTurnPolicy {
            name: "direct-b".to_owned(),
            fired: Arc::clone(&direct_b_fired),
        });

    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    assert!(
        !direct_a_fired.load(Ordering::SeqCst),
        "direct policy A should not fire after plugin Stop verdict"
    );
    assert!(
        !direct_b_fired.load(Ordering::SeqCst),
        "direct policy B should not fire after plugin Stop verdict"
    );
}

// ─── Phase 8: User Story 7 — Plugin Tool Contribution ──────────────────

// ─── T035: Plugin tools appear as "{plugin_name}_{tool_name}" ───────────

#[test]
fn plugin_tools_namespaced_format() {
    let plugin: Arc<dyn Plugin> =
        Arc::new(MockPlugin::new("analyzer").with_tools(&["scan", "report"]));

    let agent = make_agent_with_plugins(vec![plugin]);

    let tool_names: Vec<&str> = agent.state().tools.iter().map(|t| t.name()).collect();
    assert!(
        tool_names.contains(&"analyzer_scan"),
        "expected 'analyzer_scan', got: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"analyzer_report"),
        "expected 'analyzer_report', got: {tool_names:?}"
    );
}

// ─── T036: Two plugins with same-named tools — distinct namespaces ──────

#[test]
fn two_plugins_same_tool_names_distinct_namespaces() {
    let plugin_a: Arc<dyn Plugin> = Arc::new(MockPlugin::new("alpha").with_tools(&["run"]));
    let plugin_b: Arc<dyn Plugin> = Arc::new(MockPlugin::new("beta").with_tools(&["run"]));

    let agent = make_agent_with_plugins(vec![plugin_a, plugin_b]);

    let tool_names: Vec<&str> = agent.state().tools.iter().map(|t| t.name()).collect();
    assert!(
        tool_names.contains(&"alpha_run"),
        "expected 'alpha_run', got: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"beta_run"),
        "expected 'beta_run', got: {tool_names:?}"
    );
    // Both should coexist — no collision.
    assert_eq!(
        tool_names.iter().filter(|&&n| n == "alpha_run").count(),
        1,
        "alpha_run should appear exactly once"
    );
    assert_eq!(
        tool_names.iter().filter(|&&n| n == "beta_run").count(),
        1,
        "beta_run should appear exactly once"
    );
}

// ─── T037: Duplicate composed tool names are rejected ───────────────────

#[test]
fn duplicate_composed_tool_names_are_rejected() {
    use swink_agent::AgentTool;

    // Create a direct tool named "myns_fetch" and a plugin named "myns" contributing "fetch".
    // Both resolve to the same public name, so construction must fail fast.
    let direct_tool: Arc<dyn AgentTool> =
        Arc::new(swink_agent::testing::MockTool::new("myns_fetch"));
    let plugin: Arc<dyn Plugin> = Arc::new(MockPlugin::new("myns").with_tools(&["fetch"]));

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_tools(vec![direct_tool])
        .with_plugin(plugin);

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _agent = Agent::new(options);
    }))
    .expect_err("duplicate composed tool names should be rejected");

    let panic_message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|message| (*message).to_owned()))
        .expect("panic should carry a message");
    assert!(
        panic_message.contains("duplicate tool names are not allowed after composition: myns_fetch"),
        "unexpected panic message: {panic_message}"
    );
}

// ─── T038: Verify tool merge order — direct first, then plugin ──────────

// ─── Phase 9: User Story 6 — Plugin Removal ────────────────────────────

// ─── T039: Unregister plugin — contributions absent after Agent::new() ──

#[test]
fn unregistered_plugin_contributions_absent() {
    use swink_agent::PluginRegistry;

    let mut registry = PluginRegistry::new();
    let plugin_a: Arc<dyn Plugin> = Arc::new(MockPlugin::new("keep").with_tools(&["tool_a"]));
    let plugin_b: Arc<dyn Plugin> = Arc::new(MockPlugin::new("remove").with_tools(&["tool_b"]));
    registry.register(plugin_a);
    registry.register(plugin_b);

    // Unregister "remove".
    registry.unregister("remove");

    // Build agent with remaining plugins only.
    let plugins: Vec<Arc<dyn Plugin>> = registry.list().into_iter().cloned().collect();
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_plugins(plugins);
    let agent = Agent::new(options);

    let tool_names: Vec<&str> = agent.state().tools.iter().map(|t| t.name()).collect();
    assert!(
        tool_names.contains(&"keep_tool_a"),
        "kept plugin's tools should be present: {tool_names:?}"
    );
    assert!(
        !tool_names.iter().any(|&n| n.contains("remove")),
        "removed plugin's tools should be absent: {tool_names:?}"
    );
}

// ─── T040: Unregister nonexistent name — succeeds silently ──────────────
// (Already covered in tests/plugin_registry.rs: `registry_unregister_nonexistent_is_noop`)
