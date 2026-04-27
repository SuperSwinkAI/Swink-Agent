use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use swink_agent_eval::Score;
use crate::config::OptimizationTarget;
use crate::types::BaselineSnapshot;

/// Identifies which part of an agent config is being targeted for mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TargetComponent {
    PromptSection { index: usize, name: Option<String> },
    ToolDescription { tool_name: String },
    FullPrompt,
}

/// A single failing eval case record associated with a weak point.
#[derive(Debug, Clone)]
pub struct CaseFailure {
    pub case_id: String,
    pub evaluator_name: String,
    pub score: Score,
    pub details: Option<String>,
}

/// A ranked improvement opportunity: one mutable component with aggregated failure data.
#[derive(Debug, Clone)]
pub struct WeakPoint {
    pub component: TargetComponent,
    pub affected_cases: Vec<CaseFailure>,
    /// Mean gap between passing threshold and actual score across failing cases.
    pub mean_score_gap: f64,
    /// Severity = affected_cases.len() × mean_score_gap.
    pub severity: f64,
}

/// Analyzes a baseline and produces ranked improvement opportunities.
pub struct Diagnoser {
    pub max_weak_points: usize,
}

impl Diagnoser {
    pub fn new(max_weak_points: usize) -> Self {
        Self { max_weak_points }
    }

    /// Analyze a baseline snapshot and return ranked weak points, capped by `max_weak_points`.
    pub fn diagnose(&self, baseline: &BaselineSnapshot, _target: &OptimizationTarget) -> Vec<WeakPoint> {
        let mut groups: HashMap<String, (TargetComponent, Vec<CaseFailure>)> = HashMap::new();

        for case_result in &baseline.results {
            for metric in &case_result.metric_results {
                if metric.score.value >= metric.score.threshold {
                    continue;
                }
                let component = Self::component_for_evaluator(&metric.evaluator_name);
                let key = Self::component_key(&component);
                let failure = CaseFailure {
                    case_id: case_result.case_id.clone(),
                    evaluator_name: metric.evaluator_name.clone(),
                    score: metric.score,
                    details: metric.details.clone(),
                };
                groups.entry(key).or_insert_with(|| (component, Vec::new())).1.push(failure);
            }
        }

        let mut weak_points: Vec<WeakPoint> = groups
            .into_values()
            .map(|(component, failures)| {
                let n = failures.len() as f64;
                let mean_score_gap = failures
                    .iter()
                    .map(|f| (f.score.threshold - f.score.value).max(0.0))
                    .sum::<f64>()
                    / n;
                let severity = n * mean_score_gap;
                WeakPoint { component, affected_cases: failures, mean_score_gap, severity }
            })
            .collect();

        weak_points.sort_by(|a, b| {
            b.severity.partial_cmp(&a.severity).unwrap_or(std::cmp::Ordering::Equal)
        });
        weak_points.truncate(self.max_weak_points);
        weak_points
    }

    fn component_for_evaluator(evaluator_name: &str) -> TargetComponent {
        if let Some(tool_name) = evaluator_name.strip_prefix("tool:") {
            return TargetComponent::ToolDescription { tool_name: tool_name.to_string() };
        }
        if matches!(evaluator_name, "semantic_tool_selection" | "semantic_tool_parameter") {
            return TargetComponent::ToolDescription { tool_name: evaluator_name.to_string() };
        }
        TargetComponent::FullPrompt
    }

    fn component_key(component: &TargetComponent) -> String {
        match component {
            TargetComponent::FullPrompt => "full_prompt".to_string(),
            TargetComponent::PromptSection { index, .. } => format!("section:{index}"),
            TargetComponent::ToolDescription { tool_name } => format!("tool:{tool_name}"),
        }
    }
}
