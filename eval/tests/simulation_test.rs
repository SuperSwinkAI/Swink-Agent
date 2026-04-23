//! US4 simulation regression tests (T108).

#![cfg(feature = "simulation")]

use std::sync::Arc;

use swink_agent_eval::JudgeVerdict;
use swink_agent_eval::simulation::{
    ActorProfile, ActorSimulator, StateBucket, StateRegistry, ToolCallRecord,
};
use swink_agent_eval::testing::MockJudge;

fn verdict(reason: &str, label: Option<&str>) -> JudgeVerdict {
    JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some(reason.into()),
        label: label.map(str::to_string),
    }
}

#[tokio::test]
async fn actor_runs_to_five_turns_and_fires_goal_signal_on_turn_three() {
    let judge = Arc::new(MockJudge::with_verdicts(vec![
        verdict("Tell me more.", None),
        verdict("I need help with X.", None),
        verdict("Thanks, that's it.", Some("goal_complete")),
        verdict("Follow up.", None),
        verdict("Anything else?", None),
    ]));
    let actor = ActorSimulator::new(
        ActorProfile::new("Pat", "resolve issue"),
        judge,
        "test-model",
    )
    .with_max_turns(5)
    .with_goal_completion_signal("goal_complete");

    assert_eq!(actor.max_turns(), 5);
    assert!(!actor.greeting().message.is_empty());
    assert!(
        actor
            .next_turn("Hello")
            .await
            .unwrap()
            .goal_completed
            .is_none()
    );
    assert!(
        actor
            .next_turn("Clarify")
            .await
            .unwrap()
            .goal_completed
            .is_none()
    );
    let t3 = actor.next_turn("Resolved.").await.unwrap();
    assert_eq!(t3.goal_completed.as_deref(), Some("goal_complete"));
}

#[tokio::test]
async fn max_turns_reached_without_goal_terminates_gracefully() {
    let judge = Arc::new(MockJudge::with_verdicts(vec![
        verdict("hi", None),
        verdict("ok", None),
    ]));
    let actor =
        ActorSimulator::new(ActorProfile::new("Pat", "x"), judge, "test-model").with_max_turns(2);

    assert_eq!(actor.max_turns(), 2);
    assert!(actor.next_turn("m").await.unwrap().goal_completed.is_none());
    assert!(actor.next_turn("m").await.unwrap().goal_completed.is_none());
}

#[test]
fn state_bucket_fifo_enforces_history_cap() {
    let mut bucket = StateBucket::with_capacity(3);
    for i in 0..5 {
        bucket.record(ToolCallRecord {
            tool: format!("tool{i}"),
            args: serde_json::json!({"i": i}),
            result: serde_json::Value::Null,
            timestamp: std::time::SystemTime::now(),
        });
    }
    assert_eq!(bucket.history.len(), 3);
    assert_eq!(bucket.history.front().unwrap().tool, "tool2");
    assert_eq!(bucket.history.back().unwrap().tool, "tool4");
}

#[test]
fn state_registry_defaults_to_history_cap_32() {
    assert_eq!(StateRegistry::new().history_cap(), 32);
}
