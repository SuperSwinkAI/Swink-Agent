//! Regression tests for the Code family evaluators (T076).
//!
//! These tests focus on the deterministic pieces (extractor strategies and
//! the `cargo_check` / `clippy` path under a no-op harness). Running real
//! `cargo check` is an orchestrator concern; this file only exercises the
//! shape of the extraction + scoring pipeline.

#![cfg(feature = "evaluator-code")]

use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    CodeExtractor, CodeExtractorStrategy, EvalCase, Evaluator, Invocation, JudgeClient, JudgeError,
    JudgeVerdict,
};

fn make_case() -> EvalCase {
    EvalCase {
        id: "case".into(),
        name: "Case".into(),
        description: None,
        system_prompt: "s".into(),
        user_messages: vec!["write fn add".into()],
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
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn make_invocation(response: Option<&str>) -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: response.map(str::to_string),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "m"),
    }
}

#[tokio::test]
async fn extractor_markdown_fence_returns_first_rust_block() {
    let extractor = CodeExtractor::new(CodeExtractorStrategy::MarkdownFence {
        language: Some("rust".into()),
    });
    let response = "Here is some rust:\n\n```rust\nfn a() {}\n```\n";
    let extracted = extractor.extract(response).await;
    assert_eq!(extracted.as_deref(), Some("fn a() {}"));
}

#[tokio::test]
async fn extractor_markdown_fence_without_language_matches_any_fence() {
    let extractor = CodeExtractor::markdown_fence();
    let response = "```\nhello\n```";
    assert_eq!(extractor.extract(response).await.as_deref(), Some("hello"));
}

#[tokio::test]
async fn extractor_regex_uses_first_capture_group() {
    let pattern = Regex::new(r"code:\s*(?P<body>[^\n]+)").unwrap();
    let extractor = CodeExtractor::new(CodeExtractorStrategy::Regex { pattern });
    let response = "code: let x = 42;";
    assert_eq!(
        extractor.extract(response).await.as_deref(),
        Some("let x = 42;")
    );
}

struct StaticJudge(Option<String>);

impl JudgeClient for StaticJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> swink_agent_eval::JudgeFuture<'a> {
        Box::pin(async move {
            Ok(JudgeVerdict {
                score: 1.0,
                pass: true,
                reason: self.0.clone(),
                label: None,
            })
        })
    }
}

#[tokio::test]
async fn extractor_llm_uses_judge_reason_field() {
    let judge: Arc<dyn JudgeClient> = Arc::new(StaticJudge(Some("fn llm_body() {}".into())));
    let extractor = CodeExtractor::new(CodeExtractorStrategy::Llm {
        prompt: "Extract the code:".into(),
        judge,
    });
    let response = "boilerplate ... ```fn llm_body() {}```";
    assert_eq!(
        extractor.extract(response).await.as_deref(),
        Some("fn llm_body() {}")
    );
}

// The cargo shell-outs are verified by asserting that the evaluator returns
// `None` when no code is extractable — we do not want to run `cargo` from a
// unit test.

#[test]
fn cargo_check_returns_none_without_response() {
    use swink_agent_eval::CargoCheckEvaluator;
    let extractor = Arc::new(CodeExtractor::markdown_fence());
    let evaluator = CargoCheckEvaluator::new(extractor);
    assert!(
        evaluator
            .evaluate(&make_case(), &make_invocation(None))
            .is_none()
    );
}

#[test]
fn clippy_returns_none_when_extractor_produces_nothing() {
    use swink_agent_eval::ClippyEvaluator;
    let extractor = Arc::new(CodeExtractor::markdown_fence());
    let evaluator = ClippyEvaluator::new(extractor);
    // Response has no fenced code block → extractor yields None.
    let invocation = make_invocation(Some("no fences here"));
    assert!(evaluator.evaluate(&make_case(), &invocation).is_none());
}
