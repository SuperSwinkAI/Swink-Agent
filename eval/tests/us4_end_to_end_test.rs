//! US4 end-to-end regression (T114).
//!
//! The real end-to-end scenario scores a simulated conversation through
//! `GoalSuccessRateEvaluator` and compares against an equivalent real
//! invocation. `GoalSuccessRateEvaluator` lands in PR #750 (spec 043 US1
//! evaluator-quality family), so until that merges we `#[ignore]` the real
//! scenario and keep a structural smoke check in its place.

#![cfg(feature = "simulation")]

// TODO: unignore when GoalSuccessRateEvaluator lands (PR #750)
#[ignore = "GoalSuccessRateEvaluator ships in PR #750"]
#[tokio::test]
async fn simulated_conversation_scores_equivalent_to_real_invocation() {
    // Placeholder — real assertion arrives with PR #750:
    //   1. Build a 5-turn simulated invocation via run_multiturn_simulation.
    //   2. Score with GoalSuccessRateEvaluator.
    //   3. Build an equivalent hand-crafted Invocation.
    //   4. Score.
    //   5. Assert identical per-metric score / pass / reason.
    unreachable!("gated behind ignore until PR #750 lands");
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
