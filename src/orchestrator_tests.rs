//! Tests for `orchestrator`.
#![cfg(test)]

use std::panic::AssertUnwindSafe;

use super::*;

#[test]
fn add_agent_and_names() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("alpha", || panic!("not called"));
    orch.add_agent("beta", || panic!("not called"));

    let mut names = orch.names();
    names.sort_unstable();
    assert_eq!(names, vec!["alpha", "beta"]);
}

#[test]
fn contains_registered() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("a", || panic!("not called"));
    assert!(orch.contains("a"));
    assert!(!orch.contains("b"));
}

#[test]
fn parent_child_hierarchy() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("parent", || panic!("not called"));
    orch.add_child("child1", "parent", || panic!("not called"));
    orch.add_child("child2", "parent", || panic!("not called"));

    assert_eq!(orch.parent_of("child1"), Some("parent"));
    assert_eq!(orch.parent_of("child2"), Some("parent"));
    assert_eq!(orch.parent_of("parent"), None);

    let children = orch.children_of("parent").unwrap();
    assert_eq!(children, &["child1", "child2"]);
    assert!(orch.children_of("child1").unwrap().is_empty());
}

#[test]
#[should_panic(expected = "parent agent 'missing' not registered")]
fn add_child_missing_parent_panics() {
    let mut orch = AgentOrchestrator::new();
    orch.add_child("child", "missing", || panic!("not called"));
}

#[test]
#[should_panic(expected = "agent 'alpha' already registered")]
fn add_agent_duplicate_name_panics() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("alpha", || panic!("not called"));
    orch.add_agent("alpha", || panic!("not called"));
}

#[test]
fn duplicate_child_registration_preserves_existing_hierarchy() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("parent1", || panic!("not called"));
    orch.add_agent("parent2", || panic!("not called"));
    orch.add_child("child", "parent1", || panic!("not called"));

    let duplicate = std::panic::catch_unwind(AssertUnwindSafe(|| {
        orch.add_child("child", "parent2", || panic!("not called"));
    }));

    assert!(duplicate.is_err());
    assert_eq!(orch.parent_of("child"), Some("parent1"));
    assert_eq!(orch.children_of("parent1").unwrap(), &["child"]);
    assert!(orch.children_of("parent2").unwrap().is_empty());
}

#[test]
fn duplicate_top_level_registration_preserves_child_link() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("parent", || panic!("not called"));
    orch.add_child("child", "parent", || panic!("not called"));

    let duplicate = std::panic::catch_unwind(AssertUnwindSafe(|| {
        orch.add_agent("child", || panic!("not called"));
    }));

    assert!(duplicate.is_err());
    assert_eq!(orch.parent_of("child"), Some("parent"));
    assert_eq!(orch.children_of("parent").unwrap(), &["child"]);
}

#[test]
fn spawn_unregistered_agent_errors() {
    let orch = AgentOrchestrator::new();
    let result = orch.spawn("nonexistent");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(format!("{err}").contains("orchestrator"));
}

#[test]
fn send_agent_reply_reports_dropped_receiver() {
    let (reply_tx, reply_rx) = oneshot::channel();
    drop(reply_rx);

    assert!(!send_agent_reply("worker", "completed", reply_tx, "reply"));
}

#[test]
fn default_supervisor_retryable_restarts() {
    let supervisor = DefaultSupervisor::default();
    assert_eq!(supervisor.max_restarts(), 3);

    let retryable = AgentError::ModelThrottled;
    assert_eq!(
        supervisor.on_agent_error("test", &retryable),
        SupervisorAction::Restart
    );

    let non_retryable = AgentError::Aborted;
    assert_eq!(
        supervisor.on_agent_error("test", &non_retryable),
        SupervisorAction::Stop
    );
}

#[test]
fn supervisor_action_variants() {
    assert_eq!(format!("{:?}", SupervisorAction::Restart), "Restart");
    assert_eq!(format!("{:?}", SupervisorAction::Stop), "Stop");
    assert_eq!(format!("{:?}", SupervisorAction::Escalate), "Escalate");
}

#[test]
fn orchestrator_debug_format() {
    let orch = AgentOrchestrator::new();
    let debug = format!("{orch:?}");
    assert!(debug.contains("AgentOrchestrator"));
    assert!(debug.contains("channel_buffer"));
}

#[test]
fn with_supervisor_sets_policy() {
    let orch = AgentOrchestrator::new().with_supervisor(DefaultSupervisor::default());
    assert!(orch.supervisor.is_some());
}

#[test]
fn with_channel_buffer_sets_size() {
    let orch = AgentOrchestrator::new().with_channel_buffer(64);
    assert_eq!(orch.channel_buffer, 64);
}

#[test]
fn with_max_restarts_sets_default() {
    let mut orch = AgentOrchestrator::new().with_max_restarts(5);
    orch.add_agent("a", || panic!("not called"));
    assert_eq!(orch.entries["a"].max_restarts, 5);
}

#[test]
fn default_impl() {
    let orch = AgentOrchestrator::default();
    assert!(orch.entries.is_empty());
    assert!(orch.supervisor.is_none());
}

#[test]
fn custom_supervisor_policy() {
    struct AlwaysEscalate;
    impl SupervisorPolicy for AlwaysEscalate {
        fn on_agent_error(&self, _name: &str, _error: &AgentError) -> SupervisorAction {
            SupervisorAction::Escalate
        }
    }

    let supervisor = AlwaysEscalate;
    assert_eq!(
        supervisor.on_agent_error("x", &AgentError::ModelThrottled),
        SupervisorAction::Escalate
    );
}

#[test]
fn grandchild_hierarchy() {
    let mut orch = AgentOrchestrator::new();
    orch.add_agent("root", || panic!("not called"));
    orch.add_child("mid", "root", || panic!("not called"));
    orch.add_child("leaf", "mid", || panic!("not called"));

    assert_eq!(orch.parent_of("leaf"), Some("mid"));
    assert_eq!(orch.parent_of("mid"), Some("root"));
    assert_eq!(orch.parent_of("root"), None);

    assert_eq!(orch.children_of("root").unwrap(), &["mid"]);
    assert_eq!(orch.children_of("mid").unwrap(), &["leaf"]);
    assert!(orch.children_of("leaf").unwrap().is_empty());
}
