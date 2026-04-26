//! US4 end-to-end regression (T114).
//!
//! The end-to-end scenario scores a simulated conversation through
//! `GoalSuccessRateEvaluator` and compares against an equivalent real
//! invocation.

#![cfg(all(feature = "simulation", feature = "evaluator-quality"))]

use std::sync::Arc;

use swink_agent::testing::{MockStreamFn, default_convert, default_model, text_only_events};
use swink_agent::{Agent, AgentOptions, DefaultRetryStrategy};
use swink_agent_eval::simulation::{
    ActorProfile, ActorSimulator, SimulationOutcome, run_multiturn_simulation,
};
use swink_agent_eval::{
    Assertion, AssertionKind, Evaluator, GoalSuccessRateEvaluator, JudgeClient,
    JudgeEvaluatorConfig, JudgeRegistry, JudgeVerdict, MockJudge, ResponseCriteria,
};
use tokio_util::sync::CancellationToken;

mod common;

fn verdict(reason: &str, label: Option<&str>) -> JudgeVerdict {
    JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some(reason.into()),
        label: label.map(str::to_string),
    }
}

fn config(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    JudgeEvaluatorConfig::default_with(Arc::new(
        JudgeRegistry::builder(judge, "mock-judge")
            .build()
            .expect("registry builds"),
    ))
}

fn goal_case() -> swink_agent_eval::EvalCase {
    let mut case = common::case_with_response(ResponseCriteria::Contains {
        substring: "refund is complete".into(),
    });
    case.expected_assertion = Some(Assertion {
        description: "The user received a completed refund resolution.".into(),
        kind: AssertionKind::GoalCompleted,
    });
    case
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simulated_conversation_scores_equivalent_to_real_invocation() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("I can help with that refund."),
        text_only_events("I found the order and started the refund."),
        text_only_events("The refund is complete."),
    ]));
    let mut agent = Agent::new(
        AgentOptions::new(
            "Resolve refund requests.",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(std::time::Duration::from_millis(1)),
        )),
    );

    let actor_judge = Arc::new(MockJudge::with_verdicts(vec![
        verdict("My order number is A-123.", None),
        verdict("Please confirm the refund finished.", None),
        verdict("Thanks, that solves it.", Some("goal_complete")),
    ]));
    let actor = ActorSimulator::new(
        ActorProfile::new("Pat", "complete a refund request"),
        actor_judge,
        "mock-actor",
    )
    .with_goal_completion_signal("goal_complete");

    let (simulated, outcome) =
        run_multiturn_simulation(&mut agent, &actor, None, 5, CancellationToken::new())
            .await
            .expect("simulation completes");
    assert_eq!(outcome, SimulationOutcome::GoalCompleted);
    assert_eq!(simulated.turns.len(), 3);
    assert_eq!(
        simulated.final_response.as_deref(),
        Some("The refund is complete.")
    );

    let case = goal_case();
    let real = common::mock_invocation_with_response(&[], "The refund is complete.");
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![
        verdict("goal met", None),
        verdict("goal met", None),
    ]));
    let evaluator = GoalSuccessRateEvaluator::new(config(judge));

    let simulated_metric = evaluator
        .evaluate(&case, &simulated)
        .expect("simulated invocation is scorable");
    let real_metric = evaluator
        .evaluate(&case, &real)
        .expect("real invocation is scorable");

    assert_eq!(simulated_metric.evaluator_name, real_metric.evaluator_name);
    assert_eq!(simulated_metric.score.value, real_metric.score.value);
    assert_eq!(
        simulated_metric.score.verdict(),
        real_metric.score.verdict()
    );
    assert_eq!(simulated_metric.details, real_metric.details);
}

#[tokio::test]
async fn simulation_surfaces_are_importable() {
    // Smoke check asserts the re-exports resolve so downstream users can wire
    // simulation into their own test harnesses without pulling private paths.
    use swink_agent_eval::simulation::{
        ActorProfile, ActorSimulator, SimulationOutcome, StateRegistry, ToolSchema, ToolSimulator,
    };
    let _ = ActorProfile::new("x", "y");
    let _ = StateRegistry::new();
    let _ = ToolSchema::new("noop", serde_json::json!({"type": "object"}));
    let outcome = SimulationOutcome::MaxTurnsReached;
    assert_eq!(outcome, SimulationOutcome::MaxTurnsReached);

    // Confirm the ctor shape compiles; we do not actually invoke the judge.
    use std::sync::Arc;
    use swink_agent_eval::testing::MockJudge;
    let judge = Arc::new(MockJudge::with_verdicts(vec![]));
    let _actor = ActorSimulator::new(ActorProfile::new("a", "b"), judge.clone(), "m");
    let _tool_sim = ToolSimulator::new(vec![], judge, "m");
}
