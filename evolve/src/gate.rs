use crate::diagnose::TargetComponent;
use crate::evaluate::CandidateResult;
use crate::mutate::Candidate;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use swink_agent_eval::Verdict;

/// The outcome of evaluating a single candidate against the acceptance criteria.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcceptanceVerdict {
    /// Meets threshold, no P1 regressions, top-ranked for its component.
    Accepted,
    /// Met acceptance criteria but outranked by a better candidate for the same component.
    AcceptedNotApplied,
    /// Score improvement was positive but below the configured threshold.
    BelowThreshold { improvement: f64, threshold: f64 },
    /// Regressed a P1-priority eval case from pass to fail.
    P1Regression { case_id: String },
    /// Score equal to or worse than baseline.
    NoImprovement,
}

/// The full output of running the acceptance gate over all evaluated candidates.
#[derive(Debug)]
pub struct AcceptanceResult {
    /// Top-ranked accepted candidate per component — persisted.
    pub applied: Vec<(Candidate, CandidateResult)>,
    /// Accepted candidates that were outranked — recorded in manifest only.
    pub accepted_not_applied: Vec<(Candidate, CandidateResult)>,
    /// All rejected candidates with their rejection reasons.
    pub rejected: Vec<(Candidate, CandidateResult, AcceptanceVerdict)>,
}

impl AcceptanceResult {
    pub fn empty() -> Self {
        Self {
            applied: Vec::new(),
            accepted_not_applied: Vec::new(),
            rejected: Vec::new(),
        }
    }
}

/// Evaluates candidates against a baseline and returns accept/reject decisions.
pub struct AcceptanceGate {
    pub threshold: f64,
    /// Maps case_id → metadata JSON (used for priority lookups).
    case_metadata: HashMap<String, JsonValue>,
}

impl AcceptanceGate {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            case_metadata: HashMap::new(),
        }
    }

    /// Inject per-case metadata (used to distinguish P1 from P2/P3 cases).
    pub fn with_case_metadata(mut self, metadata: HashMap<String, JsonValue>) -> Self {
        self.case_metadata = metadata;
        self
    }

    /// Evaluate candidates against baseline. Returns ranked accept/reject results.
    pub fn evaluate(
        &self,
        baseline: &crate::types::BaselineSnapshot,
        candidates: &[CandidateResult],
    ) -> AcceptanceResult {
        // Build baseline per-case verdict map.
        let baseline_pass: HashMap<&str, bool> = baseline
            .results
            .iter()
            .map(|r| (r.case_id.as_str(), r.verdict == Verdict::Pass))
            .collect();

        let mut tentatively_accepted: Vec<(Candidate, CandidateResult, f64)> = Vec::new();
        let mut rejected: Vec<(Candidate, CandidateResult, AcceptanceVerdict)> = Vec::new();

        for cr in candidates {
            let improvement = cr.aggregate_score - baseline.aggregate_score;

            if improvement <= 0.0 {
                rejected.push((
                    cr.candidate.clone(),
                    cr.clone(),
                    AcceptanceVerdict::NoImprovement,
                ));
                continue;
            }

            if improvement < self.threshold {
                rejected.push((
                    cr.candidate.clone(),
                    cr.clone(),
                    AcceptanceVerdict::BelowThreshold {
                        improvement,
                        threshold: self.threshold,
                    },
                ));
                continue;
            }

            // Check P1 regressions.
            let mut regression_case_id: Option<String> = None;
            for case_result in &cr.results {
                if !self.is_p1(&case_result.case_id) {
                    continue;
                }
                let was_passing = baseline_pass
                    .get(case_result.case_id.as_str())
                    .copied()
                    .unwrap_or(false);
                let now_failing = case_result.verdict != Verdict::Pass;
                if was_passing && now_failing {
                    regression_case_id = Some(case_result.case_id.clone());
                    break;
                }
            }

            if let Some(case_id) = regression_case_id {
                rejected.push((
                    cr.candidate.clone(),
                    cr.clone(),
                    AcceptanceVerdict::P1Regression { case_id },
                ));
                continue;
            }

            tentatively_accepted.push((cr.candidate.clone(), cr.clone(), improvement));
        }

        // Among accepted: rank by improvement desc, apply per-component top-only rule.
        tentatively_accepted
            .sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        let mut seen_components: HashSet<String> = HashSet::new();
        let mut applied = Vec::new();
        let mut accepted_not_applied = Vec::new();

        for (candidate, result, _) in tentatively_accepted {
            let key = component_key(&candidate.component);
            if seen_components.insert(key) {
                applied.push((candidate, result));
            } else {
                accepted_not_applied.push((candidate, result));
            }
        }

        AcceptanceResult {
            applied,
            accepted_not_applied,
            rejected,
        }
    }

    /// Returns `true` if the case should be treated as P1 (default when no metadata).
    fn is_p1(&self, case_id: &str) -> bool {
        if let Some(priority) = self
            .case_metadata
            .get(case_id)
            .and_then(|meta| meta.get("priority"))
            .and_then(|v| v.as_str())
        {
            let lower = priority.to_lowercase();
            if lower == "p2" || lower == "p3" {
                return false;
            }
        }
        true
    }
}

fn component_key(component: &TargetComponent) -> String {
    match component {
        TargetComponent::FullPrompt => "full_prompt".to_string(),
        TargetComponent::PromptSection { index, .. } => format!("section:{index}"),
        TargetComponent::ToolDescription { tool_name } => format!("tool:{tool_name}"),
    }
}
