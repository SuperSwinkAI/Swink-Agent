//! Built-in RAG-family prompt templates (T051).

use std::sync::Arc;

use crate::prompt::{JudgePromptTemplate, MinijinjaTemplate, PromptError, PromptFamily};

/// All built-in RAG-family templates in registration order.
pub fn builtins() -> Result<Vec<Arc<dyn JudgePromptTemplate>>, PromptError> {
    Ok(vec![
        mk("rag_groundedness_v0", GROUNDEDNESS)?,
        mk("rag_retrieval_relevance_v0", RETRIEVAL_RELEVANCE)?,
        mk("rag_helpfulness_v0", HELPFULNESS)?,
    ])
}

fn mk(version: &str, source: &str) -> Result<Arc<dyn JudgePromptTemplate>, PromptError> {
    Ok(Arc::new(MinijinjaTemplate::new(
        version,
        PromptFamily::Rag,
        source,
    )?))
}

const GROUNDEDNESS: &str = r"Judge whether every claim in the response is grounded in the retrieved context.
User prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Retrieved context:
{% for ex in few_shot_examples %}- {{ ex.input }} -> {{ ex.expected }}
{% endfor %}
A response is grounded iff every factual claim is supported by a passage above.
Return 1.0 when fully grounded, 0.0 when any claim lacks support.
When scoring < 1.0, identify the unsupported claim.";

const RETRIEVAL_RELEVANCE: &str = r"Score how relevant the retrieved context is to the user's prompt.
User prompt: {{ case.user_messages[0] }}
Retrieved context:
{% for ex in few_shot_examples %}- {{ ex.input }} -> {{ ex.expected }}
{% endfor %}
Return 1.0 if the retrieved passages are directly pertinent; 0.0 if the
retrieval is off-topic. Partial credit when some passages are relevant.";

const HELPFULNESS: &str = r"Score RAG helpfulness — does the response leverage the retrieved context to help the user?
User prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Retrieved context:
{% for ex in few_shot_examples %}- {{ ex.input }} -> {{ ex.expected }}
{% endfor %}
Return 1.0 when the response uses the context to fully address the user's need;
0.0 when the context is ignored or misused.";
