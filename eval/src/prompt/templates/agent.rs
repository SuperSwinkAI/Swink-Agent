//! Built-in agent-family prompt templates (T052).

use std::sync::Arc;

use crate::prompt::{JudgePromptTemplate, MinijinjaTemplate, PromptError, PromptFamily};

/// All built-in agent-family templates in registration order.
pub fn builtins() -> Result<Vec<Arc<dyn JudgePromptTemplate>>, PromptError> {
    Ok(vec![
        mk("trajectory_accuracy_v0", TRAJECTORY_ACCURACY)?,
        mk(
            "trajectory_accuracy_with_ref_v0",
            TRAJECTORY_ACCURACY_WITH_REF,
        )?,
        mk("task_completion_v0", TASK_COMPLETION)?,
        mk("user_satisfaction_v0", USER_SATISFACTION)?,
        mk("agent_tone_v0", AGENT_TONE)?,
        mk("knowledge_retention_v0", KNOWLEDGE_RETENTION)?,
        mk("language_detection_v0", LANGUAGE_DETECTION)?,
        mk("perceived_error_v0", PERCEIVED_ERROR)?,
        mk("interactions_v0", INTERACTIONS)?,
    ])
}

fn mk(version: &str, source: &str) -> Result<Arc<dyn JudgePromptTemplate>, PromptError> {
    Ok(Arc::new(MinijinjaTemplate::new(
        version,
        PromptFamily::Agent,
        source,
    )?))
}

const TRAJECTORY_ACCURACY: &str = r"Score whether the agent's trajectory was appropriate for the user's goal.
User prompt: {{ case.user_messages[0] }}
Final response: {{ invocation.final_response }}
Judge without a reference trajectory — base the score on whether the tool calls
visible in the invocation record are a reasonable path toward the goal.
Return 1.0 when the trajectory is appropriate, 0.0 when it is wasteful or wrong.";

const TRAJECTORY_ACCURACY_WITH_REF: &str = r"Score trajectory accuracy against the declared reference trajectory.
User prompt: {{ case.user_messages[0] }}
Final response: {{ invocation.final_response }}
Compare the agent's actual tool calls against the expected_trajectory declared
on the case. Return 1.0 when the agent's trajectory matches semantically,
0.0 when it diverges materially. Partial credit permitted for extra or reordered
steps that still reach the same goal.";

const TASK_COMPLETION: &str = r"Score whether the declared task was completed.
User prompt: {{ case.user_messages[0] }}
Final response: {{ invocation.final_response }}
Consumes expected_assertion from the case. Return 1.0 when the task is complete,
0.0 when it is not. Partial credit permitted for multi-part tasks.";

const USER_SATISFACTION: &str = r"Score projected user satisfaction with the response.
Prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Return a score in [0,1] reflecting whether a reasonable user would be satisfied.
Penalize friction (apologies without recovery, unnecessary clarifying questions).";

const AGENT_TONE: &str = r"Score agent tone — does the response match a professional, helpful register?
Response: {{ invocation.final_response }}
Return 1.0 for clear, professional, respectful tone; 0.0 for dismissive, rude,
or flippant tone. Cite the offending phrasing when scoring below 1.0.";

const KNOWLEDGE_RETENTION: &str = r"Score knowledge retention across the conversation.
User prompts seen: {{ case.user_messages }}
Final response: {{ invocation.final_response }}
Return 1.0 when earlier context is correctly referenced in the final response,
0.0 when the agent forgets or contradicts prior context.";

const LANGUAGE_DETECTION: &str = r"Detect the primary language of the response and score language-match with the prompt.
Prompt: {{ case.user_messages[0] }}
Response: {{ invocation.final_response }}
Return 1.0 when the response is in the same language as the prompt, 0.0
otherwise. In the reason, include the detected language code (e.g., `en`, `es`).";

const PERCEIVED_ERROR: &str = r"Detect perceived error — user-visible failure signals in the response.
Response: {{ invocation.final_response }}
Look for unresolved exceptions, apology loops, refusal-without-recovery, or
explicit error messages surfaced to the user. Return 1.0 when no perceived
error, 0.0 when one is present. Cite the signal when scoring 0.0.";

const INTERACTIONS: &str = r"Score multi-agent interactions against the declared expected_interactions.
User prompt: {{ case.user_messages[0] }}
Final response: {{ invocation.final_response }}
Consumes expected_interactions from the case (from -> to -> description). Return
1.0 when the interaction topology was respected, 0.0 when hand-offs were skipped
or routed incorrectly. Partial credit for mostly-correct topology.";
