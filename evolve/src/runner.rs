use crate::config::{OptimizationConfig, OptimizationTarget};
use crate::diagnose::{Diagnoser, TargetComponent};
use crate::evaluate::{CandidateResult, MutatingAgentFactory};
use crate::gate::{AcceptanceGate, AcceptanceResult};
use crate::mutate::{Candidate, MutationContext, deduplicate};
use crate::persist::CyclePersister;
use crate::types::{BaselineSnapshot, CycleResult, CycleStatus};
use std::sync::Arc;
use swink_agent::{Cost, ToolSchema};
use swink_agent_eval::{AgentFactory, EvalError, EvalRunner, EvaluatorRegistry, JudgeClient};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvolveError {
    #[error("eval error: {0}")]
    Eval(#[from] EvalError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Orchestrates the closed-loop optimization cycle.
pub struct EvolutionRunner {
    pub(crate) target: OptimizationTarget,
    pub(crate) config: OptimizationConfig,
    pub(crate) factory: Arc<dyn AgentFactory>,
    #[allow(dead_code)]
    pub(crate) judge: Option<Arc<dyn JudgeClient>>,
    pub(crate) eval_runner: EvalRunner,
    pub(crate) cycle_number: u32,
}

impl EvolutionRunner {
    pub fn new(
        target: OptimizationTarget,
        config: OptimizationConfig,
        factory: Arc<dyn AgentFactory>,
        judge: Option<Arc<dyn JudgeClient>>,
    ) -> Self {
        let parallelism = config.parallelism;
        let registry = match &judge {
            Some(j) => EvaluatorRegistry::with_defaults_and_judge(Arc::clone(j)),
            None => EvaluatorRegistry::with_defaults(),
        };
        let eval_runner = EvalRunner::new(registry).with_parallelism(parallelism);
        Self {
            target,
            config,
            factory,
            judge,
            eval_runner,
            cycle_number: 0,
        }
    }

    /// Override the internal `EvalRunner` (useful for testing with custom evaluators).
    pub fn with_eval_runner(mut self, runner: EvalRunner) -> Self {
        self.eval_runner = runner;
        self
    }

    /// Run the eval suite against the current target configuration and return a scored snapshot.
    pub async fn baseline(&self) -> Result<BaselineSnapshot, EvolveError> {
        let result = self
            .eval_runner
            .run_set(&self.config.eval_set, self.factory.as_ref())
            .await?;
        let aggregate_score = BaselineSnapshot::aggregate_from_results(&result.case_results);
        Ok(BaselineSnapshot {
            target: self.target.clone(),
            results: result.case_results,
            aggregate_score,
            cost: result.summary.total_cost,
        })
    }

    /// Execute one complete optimization cycle: baseline → diagnose → mutate → evaluate → gate → persist.
    pub async fn run_cycle(&mut self) -> Result<CycleResult, EvolveError> {
        self.cycle_number += 1;
        let cycle_number = self.cycle_number;

        // Check budget before starting.
        if self.config.budget.is_exhausted() {
            return Ok(self.early_exit(
                cycle_number,
                CycleStatus::BudgetExhausted {
                    phase: "baseline".to_string(),
                },
            ));
        }

        // Phase: baseline.
        let baseline = self.baseline().await?;
        self.config.budget.record(baseline.cost.clone());

        if self.config.budget.is_exhausted() {
            return Ok(self.early_exit_with_baseline(
                cycle_number,
                baseline,
                CycleStatus::BudgetExhausted {
                    phase: "baseline".to_string(),
                },
            ));
        }

        // Phase: diagnose.
        let diagnoser = Diagnoser::new(self.config.max_weak_points);
        let weak_points = diagnoser.diagnose(&baseline, &self.target);

        if weak_points.is_empty() {
            return Ok(self.cycle_result(
                cycle_number,
                baseline,
                vec![],
                0,
                AcceptanceResult::empty(),
                Cost::default(),
                CycleStatus::NoDiagnosis,
                None,
                vec![],
            ));
        }

        // Phase: mutate (with panic isolation).
        let mut all_candidates: Vec<Candidate> = Vec::new();
        let mut mutation_errors: Vec<(String, String)> = Vec::new();

        for weak_point in &weak_points {
            let target_value = target_component_value(&self.target, &weak_point.component);
            let context = MutationContext {
                weak_point: weak_point.clone(),
                failing_traces: failing_traces_for_weak_point(&baseline, weak_point),
                eval_criteria: "response quality".to_string(),
                seed: self.config.seed,
                max_candidates: self.config.max_candidates_per_strategy,
            };

            for strategy in &self.config.strategies {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    strategy.mutate(&target_value, &context)
                }));
                match result {
                    Ok(Ok(candidates)) => all_candidates.extend(candidates),
                    Ok(Err(e)) => {
                        mutation_errors.push((strategy.name().to_string(), e.to_string()));
                    }
                    Err(payload) => {
                        let msg = panic_message(&payload);
                        mutation_errors
                            .push((strategy.name().to_string(), format!("Panic: {msg}")));
                    }
                }
            }

            all_candidates = deduplicate(all_candidates, &target_value);
        }

        if all_candidates.is_empty() {
            return Ok(self.cycle_result(
                cycle_number,
                baseline,
                weak_points,
                0,
                AcceptanceResult::empty(),
                Cost::default(),
                CycleStatus::NoImprovements,
                None,
                mutation_errors,
            ));
        }

        // Phase: evaluate candidates.
        let mut candidate_results: Vec<CandidateResult> = Vec::new();
        let mut eval_cost = Cost::default();

        for candidate in &all_candidates {
            if self.config.budget.is_exhausted() {
                break;
            }
            let override_prompt = prompt_override(candidate);
            let wrapped_factory = Arc::new(MutatingAgentFactory::new(
                Arc::clone(&self.factory),
                override_prompt,
            ));
            let set_result = self
                .eval_runner
                .run_set(&self.config.eval_set, wrapped_factory.as_ref())
                .await?;
            let aggregate = BaselineSnapshot::aggregate_from_results(&set_result.case_results);
            let cost = set_result.summary.total_cost;
            eval_cost += cost.clone();
            self.config.budget.record(cost.clone());
            candidate_results.push(CandidateResult {
                candidate: candidate.clone(),
                results: set_result.case_results,
                aggregate_score: aggregate,
                cost,
            });
        }

        let candidates_evaluated = candidate_results.len();

        // Phase: gate.
        let gate = AcceptanceGate::new(self.config.acceptance_threshold);
        let acceptance = gate.evaluate(&baseline, &candidate_results);

        // Phase: persist.
        let persister = CyclePersister::new(&self.config.output_root);
        let output_dir = persister
            .persist(cycle_number, &acceptance, &baseline)
            .map(Some)
            .unwrap_or(None);

        let total_cost = baseline.cost.clone() + eval_cost;

        let status = if acceptance.applied.is_empty() && acceptance.accepted_not_applied.is_empty()
        {
            CycleStatus::NoImprovements
        } else {
            CycleStatus::Complete
        };

        Ok(self.cycle_result(
            cycle_number,
            baseline,
            weak_points,
            candidates_evaluated,
            acceptance,
            total_cost,
            status,
            output_dir,
            mutation_errors,
        ))
    }

    /// Run up to `max` optimization cycles, stopping early on `NoDiagnosis` or `NoImprovements`.
    ///
    /// After each successful cycle, updates the target with accepted improvements.
    pub async fn run_cycles(&mut self, max: usize) -> Result<Vec<CycleResult>, EvolveError> {
        let mut results = Vec::new();
        for _ in 0..max {
            let result = self.run_cycle().await?;
            let status = result.status.clone();

            // Apply accepted improvements to the target before next cycle.
            for (candidate, _) in &result.acceptance.applied {
                self.target = apply_candidate_to_target(&self.target, candidate);
            }

            results.push(result);

            if matches!(
                status,
                CycleStatus::NoDiagnosis
                    | CycleStatus::NoImprovements
                    | CycleStatus::BudgetExhausted { .. }
            ) {
                break;
            }
        }
        Ok(results)
    }

    // ─── helpers ───────────────────────────────────────────────────────────

    fn early_exit(&self, cycle_number: u32, status: CycleStatus) -> CycleResult {
        CycleResult {
            cycle_number,
            baseline: BaselineSnapshot {
                target: self.target.clone(),
                results: vec![],
                aggregate_score: 0.0,
                cost: Cost::default(),
            },
            weak_points: vec![],
            candidates_evaluated: 0,
            acceptance: AcceptanceResult::empty(),
            total_cost: Cost::default(),
            status,
            output_dir: None,
            mutation_errors: vec![],
        }
    }

    fn early_exit_with_baseline(
        &self,
        cycle_number: u32,
        baseline: BaselineSnapshot,
        status: CycleStatus,
    ) -> CycleResult {
        CycleResult {
            cycle_number,
            baseline,
            weak_points: vec![],
            candidates_evaluated: 0,
            acceptance: AcceptanceResult::empty(),
            total_cost: Cost::default(),
            status,
            output_dir: None,
            mutation_errors: vec![],
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cycle_result(
        &self,
        cycle_number: u32,
        baseline: BaselineSnapshot,
        weak_points: Vec<crate::diagnose::WeakPoint>,
        candidates_evaluated: usize,
        acceptance: crate::gate::AcceptanceResult,
        total_cost: Cost,
        status: CycleStatus,
        output_dir: Option<std::path::PathBuf>,
        mutation_errors: Vec<(String, String)>,
    ) -> CycleResult {
        CycleResult {
            cycle_number,
            baseline,
            weak_points,
            candidates_evaluated,
            acceptance,
            total_cost,
            status,
            output_dir,
            mutation_errors,
        }
    }
}

fn target_component_value(target: &OptimizationTarget, component: &TargetComponent) -> String {
    match component {
        TargetComponent::FullPrompt => target.system_prompt().to_string(),
        TargetComponent::PromptSection { index, .. } => target
            .sections()
            .get(*index)
            .map(|s| s.content.clone())
            .unwrap_or_default(),
        TargetComponent::ToolDescription { tool_name } => target
            .tool_schemas()
            .iter()
            .find(|t| &t.name == tool_name)
            .map(|t| t.description.clone())
            .unwrap_or_default(),
    }
}

fn prompt_override(candidate: &Candidate) -> Option<String> {
    match &candidate.component {
        TargetComponent::FullPrompt | TargetComponent::PromptSection { .. } => {
            Some(candidate.mutated_value.clone())
        }
        TargetComponent::ToolDescription { .. } => None,
    }
}

fn apply_candidate_to_target(
    target: &OptimizationTarget,
    candidate: &Candidate,
) -> OptimizationTarget {
    match &candidate.component {
        TargetComponent::FullPrompt => target.with_system_prompt(&candidate.mutated_value),
        TargetComponent::PromptSection { index, .. } => {
            target.with_replaced_section(*index, &candidate.mutated_value)
        }
        TargetComponent::ToolDescription { tool_name } => {
            let existing = target.tool_schemas().iter().find(|t| &t.name == tool_name);
            if let Some(schema) = existing {
                let new_schema = ToolSchema {
                    name: schema.name.clone(),
                    description: candidate.mutated_value.clone(),
                    parameters: schema.parameters.clone(),
                };
                target.with_replaced_tool(tool_name, new_schema)
            } else {
                target.clone()
            }
        }
    }
}

fn failing_traces_for_weak_point(
    baseline: &BaselineSnapshot,
    weak_point: &crate::diagnose::WeakPoint,
) -> Vec<swink_agent_eval::Invocation> {
    let failing_ids: std::collections::HashSet<&str> = weak_point
        .affected_cases
        .iter()
        .map(|f| f.case_id.as_str())
        .collect();
    baseline
        .results
        .iter()
        .filter(|r| failing_ids.contains(r.case_id.as_str()))
        .map(|r| r.invocation.clone())
        .collect()
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}
