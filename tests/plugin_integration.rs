#![cfg(feature = "plugins")]

//! Integration tests for plugin contribution merge in Agent::new().

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::plugin::Plugin;
use swink_agent::policy::{PolicyContext, PolicyVerdict, PostTurnPolicy, TurnPolicyContext};
use swink_agent::tool::{AgentTool, AgentToolResult, ToolFuture};
use swink_agent::{Agent, AgentOptions};

mod common;
use common::*;

// ─── Test Helpers ──────────────────────────────────────────────────────────

/// A plugin that contributes a post-turn policy which records when it fires.
struct PolicyPlugin {
    name: String,
    fired: Arc<AtomicBool>,
}

impl PolicyPlugin {
    fn new(name: &str, fired: Arc<AtomicBool>) -> Self {
        Self {
            name: name.to_owned(),
            fired,
        }
    }
}

impl Plugin for PolicyPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
        let fired = Arc::clone(&self.fired);
        vec![Arc::new(RecordingPostTurnPolicy { fired })]
    }
}

struct RecordingPostTurnPolicy {
    fired: Arc<AtomicBool>,
}

impl PostTurnPolicy for RecordingPostTurnPolicy {
    fn name(&self) -> &str {
        "recording-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        self.fired.store(true, Ordering::SeqCst);
        PolicyVerdict::Continue
    }
}

/// A simple tool for plugin contribution tests.
struct PluginStubTool {
    name: String,
    schema: Value,
}

impl PluginStubTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            schema: json!({"type": "object"}),
        }
    }
}

impl AgentTool for PluginStubTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "plugin stub tool"
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

/// A plugin that contributes tools.
struct ToolPlugin {
    name: String,
    tool_names: Vec<String>,
}

impl ToolPlugin {
    fn new(name: &str, tool_names: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            tool_names: tool_names.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Plugin for ToolPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tool_names
            .iter()
            .map(|n| Arc::new(PluginStubTool::new(n)) as Arc<dyn AgentTool>)
            .collect()
    }
}

/// A plugin that tracks event observer calls.
struct EventPlugin {
    name: String,
    event_count: Arc<AtomicUsize>,
}

impl EventPlugin {
    fn new(name: &str, event_count: Arc<AtomicUsize>) -> Self {
        Self {
            name: name.to_owned(),
            event_count,
        }
    }
}

impl Plugin for EventPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_event(&self, _event: &swink_agent::AgentEvent) {
        self.event_count.fetch_add(1, Ordering::SeqCst);
    }
}

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
    let plugin: Arc<dyn Plugin> = Arc::new(PolicyPlugin::new("test-policy", Arc::clone(&fired)));

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
    let plugin: Arc<dyn Plugin> = Arc::new(ToolPlugin::new("myplugin", &["save", "load"]));

    let agent = make_agent_with_plugins(vec![plugin]);

    let tool_names: Vec<&str> = agent.state().tools.iter().map(|t| t.name()).collect();
    assert!(
        tool_names.contains(&"myplugin.save"),
        "expected namespaced tool 'myplugin.save', got: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"myplugin.load"),
        "expected namespaced tool 'myplugin.load', got: {tool_names:?}"
    );
}

// ─── T011: Plugin event observer fires for AgentStart ──────────────────────

#[tokio::test]
async fn plugin_event_observer_called_for_agent_start() {
    let event_count = Arc::new(AtomicUsize::new(0));
    let plugin: Arc<dyn Plugin> = Arc::new(EventPlugin::new("observer", Arc::clone(&event_count)));

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
