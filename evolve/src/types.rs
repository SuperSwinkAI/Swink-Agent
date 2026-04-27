use std::path::PathBuf;
use swink_agent::Cost;
use swink_agent_eval::EvalCaseResult;
use crate::config::OptimizationTarget;
use crate::diagnose::WeakPoint;
use crate::gate::AcceptanceResult;

/// Scored snapshot of the current agent config before any mutations.
pub struct BaselineSnapshot {
    pub target: OptimizationTarget,
    pub results: Vec<EvalCaseResult>,
    /// Arithmetic mean of per-case scores (equal weight per case).
    pub aggregate_score: f64,
    pub cost: Cost,
}

impl BaselineSnapshot {
    /// Compute aggregate score as the arithmetic mean of per-case means.
    pub fn aggregate_from_results(results: &[EvalCaseResult]) -> f64 {
        if results.is_empty() {
            return 0.0;
        }
        let sum: f64 = results
            .iter()
            .map(|r| {
                let metrics = &r.metric_results;
                if metrics.is_empty() {
                    0.0
                } else {
                    metrics.iter().map(|m| m.score.value).sum::<f64>() / metrics.len() as f64
                }
            })
            .sum();
        sum / results.len() as f64
    }
}

/// Summary status of a completed optimization cycle.
#[derive(Debug, Clone, PartialEq)]
pub enum CycleStatus {
    /// All phases completed normally.
    Complete,
    /// A phase exhausted the cost budget before finishing.
    BudgetExhausted { phase: String },
    /// Diagnose and mutate phases ran but no candidate improved the baseline.
    NoImprovements,
    /// Diagnose phase found no weak points (baseline already passing).
    NoDiagnosis,
}

/// Full result of one optimization cycle.
pub struct CycleResult {
    pub cycle_number: u32,
    pub baseline: BaselineSnapshot,
    pub weak_points: Vec<WeakPoint>,
    pub candidates_evaluated: usize,
    pub acceptance: AcceptanceResult,
    pub total_cost: Cost,
    pub status: CycleStatus,
    pub output_dir: Option<PathBuf>,
    /// Mutation errors recorded during the mutation phase (strategy_name, error_message).
    pub mutation_errors: Vec<(String, String)>,
}
