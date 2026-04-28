//! Built-in code-family prompt templates (T053 — code portion).

use std::sync::Arc;

use crate::prompt::{JudgePromptTemplate, MinijinjaTemplate, PromptError, PromptFamily};

/// All built-in code-family templates in registration order.
pub fn builtins() -> Result<Vec<Arc<dyn JudgePromptTemplate>>, PromptError> {
    Ok(vec![mk("code_llm_judge_v0", CODE_LLM_JUDGE)?])
}

fn mk(version: &str, source: &str) -> Result<Arc<dyn JudgePromptTemplate>, PromptError> {
    Ok(Arc::new(MinijinjaTemplate::new(
        version,
        PromptFamily::Code,
        source,
    )?))
}

const CODE_LLM_JUDGE: &str = r"Judge the quality of a code snippet produced by the agent.
User prompt: {{ case.user_messages[0] }}
Response (containing the code): {{ invocation.final_response }}
Score criteria:
 * Correctness: does the code accomplish the stated task?
 * Safety: does it avoid obvious bugs, unsafe calls, or injection risk?
 * Style: is it readable, idiomatic, and well-named?
Return a numeric score in [0,1] blending these dimensions and a short reason
calling out the most impactful issue. Do not execute the code.";
