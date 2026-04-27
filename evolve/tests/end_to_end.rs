//! US7: Full optimization cycle integration tests.

use std::sync::Arc;

use swink_agent::{Agent, AgentOptions, Cost, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalSet, ResponseCriteria, Score};
use tokio_util::sync::CancellationToken;

use swink_agent_evolve::{
    Candidate, CycleBudget, CycleStatus, MutationContext, MutationError, MutationStrategy,
    OptimizationConfig, OptimizationTarget,
};
use swink_agent_evolve::runner::EvolutionRunner;

// ─── Factories ─────────────────────────────────────────────────────────────

struct EchoFactory;

impl AgentFactory for EchoFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let stream_fn = Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()]));
        let model = ModelSpec::new("test", "test-model");
        let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn);
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

/// Returns the case's system_prompt as the agent response — lets scoring be
/// prompt-aware when combined with a matching `ResponseCriteria::Custom`.
struct SystemPromptEchoFactory;

impl AgentFactory for SystemPromptEchoFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let response = case.system_prompt.clone();
        let stream_fn = Arc::new(SimpleMockStreamFn::new(vec![response]));
        let model = ModelSpec::new("test", "test-model");
        let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn);
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

// ─── Strategies ─────────────────────────────────────────────────────────────

struct PanickingStrategy;

impl MutationStrategy for PanickingStrategy {
    fn name(&self) -> &str { "panicking" }
    fn mutate(&self, _target: &str, _context: &MutationContext) -> Result<Vec<Candidate>, MutationError> {
        panic!("intentional test panic")
    }
}

/// Replaces "helpful" with "useful" in the target; returns an empty vec if
/// "helpful" is not present (idempotent on the second cycle).
struct HelpfulToUsefulStrategy;

impl MutationStrategy for HelpfulToUsefulStrategy {
    fn name(&self) -> &str { "helpful_to_useful" }
    fn mutate(&self, target: &str, context: &MutationContext) -> Result<Vec<Candidate>, MutationError> {
        let mutated = target.replace("helpful", "useful");
        if mutated == target {
            return Ok(vec![]);
        }
        Ok(vec![Candidate::new(
            context.weak_point.component.clone(),
            target.to_string(),
            mutated,
            self.name().to_string(),
        )])
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn low_score_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "You are a helpful assistant.".to_string(),
        user_messages: vec!["hello".to_string()],
        expected_response: Some(ResponseCriteria::Custom(Arc::new(|_: &str| Score {
            value: 0.3,
            threshold: 0.5,
        }))),
        expected_trajectory: None,
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

/// Case scored 0.9 when the response contains "useful", 0.3 otherwise.
/// Used with `SystemPromptEchoFactory` so the score reflects the active prompt.
fn content_scored_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "You are a helpful assistant.".to_string(),
        user_messages: vec!["hello".to_string()],
        expected_response: Some(ResponseCriteria::Custom(Arc::new(|response: &str| Score {
            value: if response.contains("useful") { 0.9 } else { 0.3 },
            threshold: 0.5,
        }))),
        expected_trajectory: None,
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

fn make_eval_set(cases: Vec<EvalCase>) -> EvalSet {
    EvalSet { id: "e2e".into(), name: "E2E Test".into(), description: None, cases }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn run_cycle_executes_all_phases() {
    let tmp = tempfile::tempdir().unwrap();
    let target = OptimizationTarget::new("You are a helpful assistant.", vec![]);
    let set = make_eval_set(vec![low_score_case("c1"), low_score_case("c2")]);
    let config = OptimizationConfig::new(set, tmp.path())
        .with_strategies(vec![Box::new(HelpfulToUsefulStrategy)])
        .with_acceptance_threshold(0.01);
    let mut runner = EvolutionRunner::new(target, config, Arc::new(EchoFactory), None);

    let result = runner.run_cycle().await.unwrap();

    assert_eq!(result.cycle_number, 1);
    assert!(!result.weak_points.is_empty(), "low-scoring cases should produce weak points");
    assert!(result.candidates_evaluated >= 1, "at least one candidate should be evaluated");
    assert_eq!(result.status, CycleStatus::NoImprovements,
        "EchoFactory always returns 0.3, so no improvement over baseline");
}

#[tokio::test]
async fn run_cycles_stops_on_no_improvement() {
    let tmp = tempfile::tempdir().unwrap();
    let target = OptimizationTarget::new("You are a helpful assistant.", vec![]);
    let set = make_eval_set(vec![low_score_case("c1")]);
    let config = OptimizationConfig::new(set, tmp.path())
        .with_strategies(vec![Box::new(HelpfulToUsefulStrategy)])
        .with_acceptance_threshold(0.01);
    let mut runner = EvolutionRunner::new(target, config, Arc::new(EchoFactory), None);

    let results = runner.run_cycles(5).await.unwrap();

    assert_eq!(results.len(), 1, "run_cycles should stop after the first NoImprovements cycle");
    assert_eq!(results[0].status, CycleStatus::NoImprovements);
}

#[tokio::test]
async fn budget_exhaustion_returns_partial_result() {
    let tmp = tempfile::tempdir().unwrap();
    let target = OptimizationTarget::new("You are a helpful assistant.", vec![]);
    let set = make_eval_set(vec![low_score_case("c1")]);
    let config = OptimizationConfig::new(set, tmp.path())
        .with_budget(CycleBudget::new(Cost { total: 0.0, ..Cost::default() }));
    let mut runner = EvolutionRunner::new(target, config, Arc::new(EchoFactory), None);

    let result = runner.run_cycle().await.unwrap();

    assert!(
        matches!(result.status, CycleStatus::BudgetExhausted { .. }),
        "expected BudgetExhausted, got {:?}", result.status,
    );
    assert_eq!(result.candidates_evaluated, 0);
}

/// Verifies that `run_cycles` applies accepted improvements to the target
/// before each subsequent cycle, allowing the strategy to converge.
#[tokio::test]
async fn consecutive_cycles_chain_improvements() {
    let tmp = tempfile::tempdir().unwrap();
    let target = OptimizationTarget::new("You are a helpful assistant.", vec![]);
    let set = make_eval_set(vec![content_scored_case("c1"), content_scored_case("c2")]);
    let config = OptimizationConfig::new(set, tmp.path())
        .with_strategies(vec![Box::new(HelpfulToUsefulStrategy)])
        .with_acceptance_threshold(0.1);
    let mut runner = EvolutionRunner::new(
        target, config, Arc::new(SystemPromptEchoFactory), None,
    );

    let results = runner.run_cycles(5).await.unwrap();

    // Cycle 1: baseline 0.3, candidate (prompt has "useful") → 0.9; accepted.
    // Cycle 2: target now "…useful…"; HelpfulToUseful finds nothing → NoImprovements; stops.
    assert_eq!(results.len(), 2, "expected exactly 2 cycles before convergence");
    assert_eq!(results[0].status, CycleStatus::Complete);
    assert!(
        !results[0].acceptance.applied.is_empty(),
        "cycle 1 should apply at least one improvement",
    );
    assert_eq!(results[1].status, CycleStatus::NoImprovements);
}

#[tokio::test]
async fn panic_strategy_caught_and_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let target = OptimizationTarget::new("You are a helpful assistant.", vec![]);
    let set = make_eval_set(vec![low_score_case("c1")]);
    let config = OptimizationConfig::new(set, tmp.path())
        .with_strategies(vec![Box::new(PanickingStrategy)]);
    let mut runner = EvolutionRunner::new(target, config, Arc::new(EchoFactory), None);

    let result = runner.run_cycle().await.unwrap();

    assert!(!result.mutation_errors.is_empty(), "panicking strategy should be recorded as error");
    let (strategy_name, msg) = &result.mutation_errors[0];
    assert_eq!(strategy_name, "panicking");
    assert!(msg.starts_with("Panic:"), "error message should start with 'Panic:', got: {msg}");
    assert_eq!(result.candidates_evaluated, 0);
    assert_eq!(result.status, CycleStatus::NoImprovements);
}

#[tokio::test]
async fn cycle_cost_matches_eval_costs() {
    let tmp = tempfile::tempdir().unwrap();
    let target = OptimizationTarget::new("You are a helpful assistant.", vec![]);
    let set = make_eval_set(vec![low_score_case("c1"), low_score_case("c2")]);
    let config = OptimizationConfig::new(set, tmp.path())
        .with_strategies(vec![Box::new(HelpfulToUsefulStrategy)])
        .with_acceptance_threshold(0.01);
    let mut runner = EvolutionRunner::new(target, config, Arc::new(EchoFactory), None);

    let result = runner.run_cycle().await.unwrap();

    let candidate_cost: f64 = result.acceptance.applied.iter().map(|(_, cr)| cr.cost.total)
        .chain(result.acceptance.accepted_not_applied.iter().map(|(_, cr)| cr.cost.total))
        .chain(result.acceptance.rejected.iter().map(|(_, cr, _)| cr.cost.total))
        .sum();
    let expected = result.baseline.cost.total + candidate_cost;
    assert!(
        (result.total_cost.total - expected).abs() < 1e-10,
        "total_cost ({}) must equal baseline ({}) + candidate costs ({})",
        result.total_cost.total, result.baseline.cost.total, candidate_cost,
    );
}
