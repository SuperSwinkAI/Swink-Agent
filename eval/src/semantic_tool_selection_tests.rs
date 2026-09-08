//! Tests for `semantic_tool_selection`.
#![cfg(test)]

use super::*;

use std::time::Duration as StdDuration;

use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};

use crate::testing::MockJudge;
use crate::types::{EvalCase, Invocation, TurnRecord};

fn simple_case() -> EvalCase {
    EvalCase {
        id: "c1".into(),
        name: "C1".into(),
        description: None,
        system_prompt: "be helpful".into(),
        user_messages: vec!["read the config".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: true,
        state_capture: None,
    }
}

fn invocation_with_calls(names: &[&str]) -> Invocation {
    let tool_calls: Vec<RecordedToolCall> = names
        .iter()
        .enumerate()
        .map(|(i, n)| RecordedToolCall {
            id: format!("id{i}"),
            name: (*n).to_string(),
            arguments: serde_json::json!({"k": i}),
        })
        .collect();
    Invocation {
        turns: vec![TurnRecord {
            turn_index: 0,
            assistant_message: AssistantMessage::new(
                vec![ContentBlock::Text { text: "ok".into() }],
                "p",
                "m",
            )
            .with_timestamp(0),
            tool_calls,
            tool_results: vec![],
            duration: StdDuration::from_millis(1),
        }],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: StdDuration::from_millis(1),
        final_response: Some("done".into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("p", "m"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_none_when_flag_disabled() {
    let mut case = simple_case();
    case.semantic_tool_selection = false;
    let invocation = invocation_with_calls(&["read_file"]);
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_none_when_trajectory_empty() {
    let case = simple_case();
    let mut invocation = invocation_with_calls(&[]);
    invocation.turns[0].tool_calls.clear();
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_timeout_is_five_minutes() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    assert_eq!(evaluator.timeout, Duration::from_mins(5));
}

#[test]
fn evaluates_inside_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");

    runtime.block_on(async {
        let case = simple_case();
        let invocation = invocation_with_calls(&["read_file"]);
        let judge = Arc::new(MockJudge::always_pass());
        let evaluator = SemanticToolSelectionEvaluator::new(judge.clone());

        let result = evaluator
            .evaluate(&case, &invocation)
            .expect("semantic tool selection should apply");

        assert!(result.score.verdict().is_pass());
        assert_eq!(judge.call_count(), 1);
    });
}
