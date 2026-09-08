//! Tests for `semantic_tool_parameter`.
#![cfg(test)]

use super::*;

use std::time::Duration as StdDuration;

use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};

use crate::testing::MockJudge;
use crate::types::{EvalCase, Invocation, ToolIntent, TurnRecord};

fn simple_case(intent: Option<ToolIntent>) -> EvalCase {
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
        expected_tool_intent: intent,
        semantic_tool_selection: false,
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
async fn returns_none_when_intent_missing() {
    let case = simple_case(None);
    let invocation = invocation_with_calls(&["read_file"]);
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolParameterEvaluator::new(judge);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_none_when_filter_has_no_match() {
    let case = simple_case(Some(ToolIntent {
        intent: "read config for project-alpha".into(),
        tool_name: Some("read_file".into()),
    }));
    // Agent only calls a different tool.
    let invocation = invocation_with_calls(&["list_dir"]);
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolParameterEvaluator::new(judge);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_timeout_is_five_minutes() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolParameterEvaluator::new(judge);
    assert_eq!(evaluator.timeout, Duration::from_mins(5));
}

#[test]
fn evaluates_inside_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");

    runtime.block_on(async {
        let case = simple_case(Some(ToolIntent {
            intent: "read config for project-alpha".into(),
            tool_name: Some("read_file".into()),
        }));
        let invocation = invocation_with_calls(&["read_file"]);
        let judge = Arc::new(MockJudge::always_pass());
        let evaluator = SemanticToolParameterEvaluator::new(judge.clone());

        let result = evaluator
            .evaluate(&case, &invocation)
            .expect("semantic tool parameter should apply");

        assert!(result.score.verdict().is_pass());
        assert_eq!(judge.call_count(), 1);
    });
}
