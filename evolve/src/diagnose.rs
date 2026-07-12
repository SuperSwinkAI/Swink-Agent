use crate::config::OptimizationTarget;
use crate::types::BaselineSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use swink_agent_eval::Score;
use tracing::debug;

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
    pub fn diagnose(
        &self,
        baseline: &BaselineSnapshot,
        _target: &OptimizationTarget,
    ) -> Vec<WeakPoint> {
        let mut groups: HashMap<String, (TargetComponent, Vec<CaseFailure>)> = HashMap::new();

        for case_result in &baseline.results {
            for metric in &case_result.metric_results {
                if metric.score.value >= metric.score.threshold {
                    continue;
                }
                let component = Self::component_for_evaluator(
                    &metric.evaluator_name,
                    metric.details.as_deref(),
                );
                let key = Self::component_key(&component);
                let failure = CaseFailure {
                    case_id: case_result.case_id.clone(),
                    evaluator_name: metric.evaluator_name.clone(),
                    score: metric.score,
                    details: metric.details.clone(),
                };
                groups
                    .entry(key)
                    .or_insert_with(|| (component, Vec::new()))
                    .1
                    .push(failure);
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
                WeakPoint {
                    component,
                    affected_cases: failures,
                    mean_score_gap,
                    severity,
                }
            })
            .collect();

        weak_points.sort_by(|a, b| {
            b.severity
                .partial_cmp(&a.severity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        weak_points.truncate(self.max_weak_points);
        weak_points
    }

    /// Map a failing metric to the component responsible for it.
    ///
    /// `semantic_tool_selection` and `semantic_tool_parameter` are per-invocation
    /// aggregate evaluators: `evaluator_name` never identifies a specific tool
    /// (it's always the evaluator's own static name), so the only place a real
    /// tool name can come from is the evaluator's `details` string, which both
    /// evaluators format as `"{tool}: {pass|fail} (...)"` segments joined by
    /// `"; "` (see `eval::semantic_tool_selection::aggregate` /
    /// `eval::semantic_tool_parameter`). We parse the first `fail` segment for
    /// its tool name; when that format isn't present (or changes), we fall
    /// back to `FullPrompt` rather than fabricate a `tool_name` that matches no
    /// real tool and would silently no-op in `with_replaced_tool`.
    fn component_for_evaluator(evaluator_name: &str, details: Option<&str>) -> TargetComponent {
        if matches!(
            evaluator_name,
            "semantic_tool_selection" | "semantic_tool_parameter"
        ) {
            if let Some(tool_name) = Self::failing_tool_name_from_details(details) {
                return TargetComponent::ToolDescription { tool_name };
            }
            debug!(
                evaluator_name,
                ?details,
                "could not parse a failing tool name from evaluator details; \
                 attributing weak point to FullPrompt instead of fabricating a tool_name"
            );
            return TargetComponent::FullPrompt;
        }
        TargetComponent::FullPrompt
    }

    /// Extract the tool name of the first `fail`-marked segment in a
    /// semantic-tool-* evaluator's `details` string.
    fn failing_tool_name_from_details(details: Option<&str>) -> Option<String> {
        let details = details?;
        details.split("; ").find_map(|segment| {
            let (name, rest) = segment.split_once(": ")?;
            if rest.trim_start().starts_with("fail") {
                Some(name.trim().to_string())
            } else {
                None
            }
        })
    }

    fn component_key(component: &TargetComponent) -> String {
        match component {
            TargetComponent::FullPrompt => "full_prompt".to_string(),
            TargetComponent::PromptSection { index, .. } => format!("section:{index}"),
            TargetComponent::ToolDescription { tool_name } => format!("tool:{tool_name}"),
        }
    }
}
