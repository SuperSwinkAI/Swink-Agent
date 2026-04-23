//! Built-in quality-family prompt templates (T049).
//!
//! Each template produces a rubric prompt the judge renders into a structured
//! verdict. Faithfulness and hallucination rubrics are deliberately distinct
//! per spec 043 clarification Q1:
//!
//! * **Faithfulness** scores agreement with the retrieved context that was
//!   supplied to the agent (RAG-grounded).
//! * **Hallucination** scores fabrication against general model knowledge and
//!   the user prompt — no retrieved context is assumed.
//!
//! Templates deliberately keep their source small (≤ 40 lines of minijinja).
//! Every version pin is `_v0`; bumping a rubric requires registering a new
//! version (`_v1`) alongside the old one (FR-011).

use std::sync::Arc;

use crate::prompt::{JudgePromptTemplate, MinijinjaTemplate, PromptError, PromptFamily};

/// All built-in quality-family templates in registration order.
pub fn builtins() -> Result<Vec<Arc<dyn JudgePromptTemplate>>, PromptError> {
    Ok(vec![
        mk("helpfulness_v0", HELPFULNESS)?,
        mk("correctness_v0", CORRECTNESS)?,
        mk("conciseness_v0", CONCISENESS)?,
        mk("coherence_v0", COHERENCE)?,
        mk("response_relevance_v0", RESPONSE_RELEVANCE)?,
        mk("hallucination_v0", HALLUCINATION)?,
        mk("faithfulness_v0", FAITHFULNESS)?,
        mk("plan_adherence_v0", PLAN_ADHERENCE)?,
        mk("laziness_v0", LAZINESS)?,
        mk("goal_success_rate_v0", GOAL_SUCCESS_RATE)?,
    ])
}

fn mk(version: &str, source: &str) -> Result<Arc<dyn JudgePromptTemplate>, PromptError> {
    Ok(Arc::new(MinijinjaTemplate::new(
        version,
        PromptFamily::Quality,
        source,
    )?))
}

const HELPFULNESS: &str = r"You are scoring how helpful an assistant response is on a 1-7 scale.
Case: {{ case.name }}
User prompt: {{ case.user_messages[0] }}
Assistant response: {{ invocation.final_response }}
Rubric:
 1 = useless or actively harmful; 4 = partially addresses; 7 = fully addresses user's underlying need.
Return score in [0,1] as score/7 plus a one-sentence reason.";

const CORRECTNESS: &str = r"You are judging factual correctness of an assistant answer.
Question: {{ case.user_messages[0] }}
Assistant answer: {{ invocation.final_response }}
Score 1.0 if the answer is factually correct and complete, 0.0 if wrong, partial credit otherwise.
Cite the specific claim you judged if scoring < 1.0.";

const CONCISENESS: &str = r"Score conciseness on a 3-level scale.
Response: {{ invocation.final_response }}
 0.0 = padded/verbose; 0.5 = some filler; 1.0 = tight, no redundant content.
Return a numeric score in [0,1] and a short reason.";

const COHERENCE: &str = r"Score coherence on a 5-level scale (1-5 mapped to [0,1]).
Response: {{ invocation.final_response }}
Rubric:
 1 = contradictory or disjointed; 3 = generally follows but with lapses; 5 = logically connected throughout.
Return score/5 and a one-sentence reason.";

const RESPONSE_RELEVANCE: &str = r"Judge whether the response addresses the prompt.
Prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Score 1.0 if directly on-topic and answers what was asked, 0.0 if off-topic.
Penalize drift and tangential content.";

const HALLUCINATION: &str = r"Score hallucination against general world knowledge and the user prompt.
Prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Definition: a claim is a hallucination when it is not supported by widely-known facts
or by the user's own prompt. Retrieved/context documents are NOT part of this check
(see faithfulness_v0 for that).
Return 1.0 if the response is fully non-hallucinated, 0.0 if it contains a fabrication.
Identify the specific hallucinated claim when scoring below 1.0.";

const FAITHFULNESS: &str = r"Score faithfulness to the RETRIEVED context that was provided to the agent.
Prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Retrieved context (from case metadata or few-shot examples):
{% for ex in few_shot_examples %}- {{ ex.input }} -> {{ ex.expected }}
{% endfor %}
A claim is faithful iff it is supported by the supplied context. General-knowledge
correctness (see hallucination_v0) is NOT part of this check — judge only against
the retrieved context shown above.
Return 1.0 if every claim is supported, 0.0 if any claim contradicts or extends the context.";

const PLAN_ADHERENCE: &str = r"Score whether the agent followed the plan stated in its system prompt.
System prompt: {{ case.system_prompt }}
Response: {{ invocation.final_response }}
Score 1.0 when the response follows the stated plan, 0.0 when it diverges materially.";

const LAZINESS: &str = r"Detect assistant laziness (stopping short, declining solvable work, asking the user to do the task).
Prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Score 1.0 when the assistant fully attempted the task; 0.0 when it punted without cause.";

const GOAL_SUCCESS_RATE: &str = r"Score goal success against the declared expected_assertion.
Prompt: {{ case.user_messages[0] }}
Final response: {{ invocation.final_response }}
Return 1.0 if the user's underlying goal was achieved, 0.0 otherwise.
Partial credit permitted for multi-part goals.";
