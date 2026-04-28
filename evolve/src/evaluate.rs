use crate::mutate::Candidate;
use std::sync::Arc;
use swink_agent::Cost;
use swink_agent_eval::EvalCaseResult;
use swink_agent_eval::{AgentFactory, EvalCase, EvalError};
use tokio_util::sync::CancellationToken;

/// Evaluation result for a single candidate mutation.
#[derive(Debug, Clone)]
pub struct CandidateResult {
    pub candidate: Candidate,
    pub results: Vec<EvalCaseResult>,
    pub aggregate_score: f64,
    pub cost: Cost,
}

/// Wraps an inner `AgentFactory`, intercepting `create_agent` to inject
/// a mutated system prompt for candidate evaluation.
///
/// The modified system prompt is stored on construction. For each eval case,
/// we clone the case and replace `case.system_prompt` before delegating to
/// the inner factory. This lets the inner factory handle all provider-specific
/// agent construction while the wrapper injects only the mutation.
pub struct MutatingAgentFactory {
    inner: Arc<dyn AgentFactory>,
    override_prompt: Option<String>,
}

impl MutatingAgentFactory {
    pub fn new(inner: Arc<dyn AgentFactory>, override_prompt: Option<String>) -> Self {
        Self {
            inner,
            override_prompt,
        }
    }
}

impl AgentFactory for MutatingAgentFactory {
    fn create_agent(
        &self,
        case: &EvalCase,
    ) -> Result<(swink_agent::Agent, CancellationToken), EvalError> {
        if let Some(ref prompt) = self.override_prompt {
            let mut modified = case.clone();
            modified.system_prompt = prompt.clone();
            self.inner.create_agent(&modified)
        } else {
            self.inner.create_agent(case)
        }
    }
}
