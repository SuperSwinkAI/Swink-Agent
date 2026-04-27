use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use swink_agent_eval::Invocation;
use crate::diagnose::WeakPoint;
use crate::diagnose::TargetComponent;

/// Context passed to each mutation strategy.
#[derive(Debug, Clone)]
pub struct MutationContext {
    pub weak_point: WeakPoint,
    /// Failing trajectory traces from baseline evaluation.
    pub failing_traces: Vec<Invocation>,
    /// Eval criteria description from the failing case.
    pub eval_criteria: String,
    /// Seed for deterministic strategies.
    pub seed: Option<u64>,
    pub max_candidates: usize,
}

/// Errors that a mutation strategy can return.
#[derive(Debug, Clone, Error)]
pub enum MutationError {
    #[error("judge unavailable: {0}")]
    JudgeUnavailable(String),
    #[error("budget exhausted")]
    BudgetExhausted,
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("panic in strategy: {0}")]
    Panic(String),
    #[error("{0}")]
    Other(String),
}

/// A candidate mutation: original → mutated text, tagged with its component and strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// SHA-256 of `mutated_value` (hex string) — used for deduplication.
    pub id: String,
    pub component: TargetComponent,
    pub original_value: String,
    pub mutated_value: String,
    pub strategy: String,
}

impl Candidate {
    pub fn new(
        component: TargetComponent,
        original_value: String,
        mutated_value: String,
        strategy: String,
    ) -> Self {
        let hash = Sha256::digest(mutated_value.as_bytes());
        let hash_bytes: &[u8] = hash.as_ref();
        let id: String = hash_bytes.iter().map(|b| format!("{:02x}", b)).collect();
        Self { id, component, original_value, mutated_value, strategy }
    }
}

/// Trait implemented by all mutation strategies.
pub trait MutationStrategy: Send + Sync {
    fn name(&self) -> &str;
    fn mutate(
        &self,
        target: &str,
        context: &MutationContext,
    ) -> Result<Vec<Candidate>, MutationError>;
}

/// Remove candidates that duplicate existing ones or are identical to the original.
pub fn deduplicate(candidates: Vec<Candidate>, original: &str) -> Vec<Candidate> {
    let mut seen_ids = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|c| c.mutated_value != original && seen_ids.insert(c.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_id_is_deterministic() {
        let c1 = Candidate::new(
            TargetComponent::FullPrompt,
            "original".to_string(),
            "mutated text".to_string(),
            "test".to_string(),
        );
        let c2 = Candidate::new(
            TargetComponent::FullPrompt,
            "different original".to_string(),
            "mutated text".to_string(),
            "other".to_string(),
        );
        // Same mutated value → same id regardless of original or strategy
        assert_eq!(c1.id, c2.id);

        let c3 = Candidate::new(
            TargetComponent::FullPrompt,
            "original".to_string(),
            "different text".to_string(),
            "test".to_string(),
        );
        assert_ne!(c1.id, c3.id);
    }

    #[test]
    fn deduplicate_removes_identity_and_duplicates() {
        let original = "original text";
        let candidates = vec![
            Candidate::new(TargetComponent::FullPrompt, original.into(), "mutated".into(), "a".into()),
            Candidate::new(TargetComponent::FullPrompt, original.into(), "mutated".into(), "b".into()),
            Candidate::new(TargetComponent::FullPrompt, original.into(), original.into(), "c".into()),
        ];
        let result = deduplicate(candidates, original);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].mutated_value, "mutated");
    }
}
