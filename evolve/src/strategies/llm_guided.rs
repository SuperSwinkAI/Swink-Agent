use std::sync::Arc;
use swink_agent_eval::JudgeClient;
use crate::mutate::{Candidate, MutationContext, MutationError, MutationStrategy};

/// Mutation strategy that uses a `JudgeClient` to rewrite the target.
///
/// Constructs a prompt from the failing context, sends it to the judge, and
/// extracts the improved text from `JudgeVerdict.reason`. Bridges the async
/// `JudgeClient::judge()` to the sync `MutationStrategy::mutate()` interface
/// via `tokio::runtime::Handle::current().block_on()`.
///
/// **Runtime requirement**: callers must invoke `mutate()` from within a
/// multi-threaded Tokio runtime (i.e. `flavor = "multi_thread"`), because
/// `block_on` on the current handle requires a multi-threaded executor.
pub struct LlmGuided {
    judge: Arc<dyn JudgeClient>,
}

impl LlmGuided {
    pub fn new(judge: Arc<dyn JudgeClient>) -> Self {
        Self { judge }
    }
}

impl MutationStrategy for LlmGuided {
    fn name(&self) -> &str {
        "llm_guided"
    }

    fn mutate(
        &self,
        target: &str,
        context: &MutationContext,
    ) -> Result<Vec<Candidate>, MutationError> {
        let component = context.weak_point.component.clone();
        let prompt = build_mutation_prompt(target, context);

        let verdict = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.judge.judge(&prompt))
        })
        .map_err(|e| MutationError::JudgeUnavailable(e.to_string()))?;

        let rewrite = verdict
            .reason
            .ok_or_else(|| MutationError::InvalidResponse(
                "judge returned no rewrite in reason field".to_string(),
            ))?;

        Ok(vec![Candidate::new(
            component,
            target.to_string(),
            rewrite,
            "llm_guided".to_string(),
        )])
    }
}

fn build_mutation_prompt(target: &str, context: &MutationContext) -> String {
    format!(
        "Improve the following text to address the identified weakness.\n\n\
         Text:\n{target}\n\n\
         Weakness: mean score gap = {gap:.3}, affected cases = {n}\n\n\
         Criteria: {criteria}\n\n\
         Respond with only the improved text.",
        gap = context.weak_point.mean_score_gap,
        n = context.weak_point.affected_cases.len(),
        criteria = context.eval_criteria,
    )
}
