//! Experiment-generator: produces validated `EvalSet`s from a context (US5).
//!
//! [`ExperimentGenerator`] plans diverse topics via a [`TopicPlanner`] and
//! produces one or more [`EvalCase`]s per topic. Every emitted case passes
//! [`EvalCase::validate`]; malformed JSON is retried up to a bounded cap and
//! unrecoverable slots are dropped with a warning (FR-030).

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use super::topic::{TopicPlanner, TopicSlot};
use crate::judge::{JudgeClient, JudgeError};
use crate::types::{
    Assertion, AssertionKind, EvalCase, EvalSet, ExpectedToolCall, InteractionExpectation,
    ResponseCriteria,
};

/// Default bounded retry cap when judge output is malformed.
pub const DEFAULT_RETRY_CAP: u32 = 3;

/// Description of a tool available to the generated agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
}

impl ToolDef {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Request sent to [`ExperimentGenerator::generate`].
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default)]
pub struct GenerationRequest {
    pub context: String,
    pub task: String,
    pub desired_count: u32,
    pub num_topics: u32,
    pub include_expected_output: bool,
    pub include_expected_trajectory: bool,
    pub include_expected_interactions: bool,
    pub include_metadata: bool,
    /// When `Some`, trajectories reference only these tool names (FR-029).
    pub agent_tools: Option<Vec<ToolDef>>,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("judge error: {0}")]
    Judge(#[source] JudgeError),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

pub struct ExperimentGenerator {
    judge: Arc<dyn JudgeClient>,
    model_id: String,
    planner: Arc<TopicPlanner>,
    retry_cap: u32,
    validate: bool,
}

impl ExperimentGenerator {
    #[must_use]
    pub fn new(
        judge: Arc<dyn JudgeClient>,
        model_id: impl Into<String>,
        planner: Arc<TopicPlanner>,
    ) -> Self {
        Self {
            judge,
            model_id: model_id.into(),
            planner,
            retry_cap: DEFAULT_RETRY_CAP,
            validate: true,
        }
    }

    #[must_use]
    pub const fn with_retry_cap(mut self, retry_cap: u32) -> Self {
        self.retry_cap = if retry_cap == 0 { 1 } else { retry_cap };
        self
    }

    #[must_use]
    pub const fn with_validation(mut self, validate: bool) -> Self {
        self.validate = validate;
        self
    }

    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    #[must_use]
    pub const fn retry_cap(&self) -> u32 {
        self.retry_cap
    }

    pub async fn generate(&self, request: GenerationRequest) -> Result<EvalSet, GenerationError> {
        if request.desired_count == 0 || request.num_topics == 0 {
            return Err(GenerationError::InvalidRequest(
                "desired_count and num_topics must be > 0".into(),
            ));
        }
        let slots = self
            .planner
            .plan(
                &request.context,
                &request.task,
                request.num_topics,
                request.desired_count,
            )
            .await
            .map_err(GenerationError::Judge)?;

        let mut cases = Vec::with_capacity(request.desired_count as usize);
        for slot in &slots {
            for local_idx in 0..slot.case_count {
                match self.generate_case(slot, local_idx, &request).await {
                    Ok(case) => cases.push(case),
                    Err(reason) => warn!(
                        topic = %slot.topic, reason = %reason,
                        "dropping malformed generated case after exhausting retries",
                    ),
                }
            }
        }
        Ok(EvalSet {
            id: format!("generated-{}", cases.len()),
            name: "Generated Experiment".into(),
            description: Some(format!(
                "Auto-generated eval set ({} topics, {} cases requested)",
                slots.len(),
                request.desired_count
            )),
            cases,
        })
    }

    async fn generate_case(
        &self,
        slot: &TopicSlot,
        local_idx: u32,
        req: &GenerationRequest,
    ) -> Result<EvalCase, String> {
        let mut last_error = String::from("no attempts");
        for attempt in 0..self.retry_cap.max(1) {
            let prompt = render_prompt(slot, local_idx, req, attempt);
            let verdict = match self.judge.judge(&prompt).await {
                Ok(v) => v,
                Err(err) => {
                    last_error = format!("judge error: {err}");
                    continue;
                }
            };
            let Some(body) = verdict.reason.as_deref() else {
                last_error = "empty judge body".into();
                continue;
            };
            let draft: CaseDraft = match serde_json::from_str(body.trim()) {
                Ok(d) => d,
                Err(err) => {
                    last_error = format!("parse error: {err}");
                    continue;
                }
            };
            let case = match draft_to_case(draft, slot, local_idx, req) {
                Ok(c) => c,
                Err(err) => {
                    last_error = err;
                    continue;
                }
            };
            if self.validate
                && let Err(err) = case.validate()
            {
                last_error = format!("validation error: {err}");
                continue;
            }
            return Ok(case);
        }
        Err(last_error)
    }
}

impl std::fmt::Debug for ExperimentGenerator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExperimentGenerator")
            .field("model_id", &self.model_id)
            .field("retry_cap", &self.retry_cap)
            .field("validate", &self.validate)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
struct CaseDraft {
    name: Option<String>,
    description: Option<String>,
    system_prompt: Option<String>,
    user_messages: Option<Vec<String>>,
    expected_response: Option<String>,
    expected_assertion: Option<String>,
    expected_trajectory: Option<Vec<ToolCallDraft>>,
    expected_interactions: Option<Vec<InteractionDraft>>,
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDraft {
    tool_name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct InteractionDraft {
    from: String,
    to: String,
    description: String,
}

fn draft_to_case(
    draft: CaseDraft,
    slot: &TopicSlot,
    local_idx: u32,
    req: &GenerationRequest,
) -> Result<EvalCase, String> {
    let user_messages = draft
        .user_messages
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "missing user_messages".to_string())?;
    let id = format!(
        "{}::{}::{}",
        slug(&slot.topic),
        local_idx,
        draft.name.as_deref().unwrap_or("case")
    );
    let name = draft
        .name
        .clone()
        .unwrap_or_else(|| format!("{}-case-{}", slot.topic, local_idx));

    let expected_response = req
        .include_expected_output
        .then(|| {
            draft
                .expected_response
                .map(|substring| ResponseCriteria::Contains { substring })
        })
        .flatten();

    let allowed: Option<HashSet<&str>> = req
        .agent_tools
        .as_ref()
        .map(|tools| tools.iter().map(|tool| tool.name.as_str()).collect());
    let expected_trajectory = req
        .include_expected_trajectory
        .then(|| {
            draft.expected_trajectory.map(|calls| {
                calls
                    .into_iter()
                    .filter(|call| {
                        allowed
                            .as_ref()
                            .is_none_or(|set| set.contains(call.tool_name.as_str()))
                    })
                    .map(|call| ExpectedToolCall {
                        tool_name: call.tool_name,
                        arguments: call.arguments,
                    })
                    .collect()
            })
        })
        .flatten();

    let expected_assertion = draft.expected_assertion.map(|description| Assertion {
        description,
        kind: AssertionKind::GoalCompleted,
    });

    let expected_interactions = req
        .include_expected_interactions
        .then(|| {
            draft.expected_interactions.map(|interactions| {
                interactions
                    .into_iter()
                    .map(|i| InteractionExpectation {
                        from: i.from,
                        to: i.to,
                        description: i.description,
                    })
                    .collect()
            })
        })
        .flatten();

    let metadata = if req.include_metadata {
        draft.metadata.unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    Ok(EvalCase {
        id,
        name,
        description: draft.description,
        system_prompt: draft.system_prompt.unwrap_or_default(),
        user_messages,
        expected_trajectory,
        expected_response,
        expected_assertion,
        expected_interactions,
        few_shot_examples: Vec::new(),
        budget: None,
        evaluators: Vec::new(),
        metadata,
        attachments: Vec::new(),
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    })
}

fn render_prompt(
    slot: &TopicSlot,
    local_idx: u32,
    req: &GenerationRequest,
    attempt: u32,
) -> String {
    let tool_list = req
        .agent_tools
        .as_ref()
        .map(|tools| {
            let joined: Vec<String> = tools
                .iter()
                .map(|t| format!("{} — {}", t.name, t.description))
                .collect();
            format!("\nAvailable tools:\n{}", joined.join("\n"))
        })
        .unwrap_or_default();
    format!(
        "Generate one eval case for topic `{}` (case #{local_idx}, attempt {attempt}).\n\
Context: {}\nTask: {}{tool_list}\n\
Respond with a JSON object with keys: name, description, system_prompt, \
user_messages (non-empty array), expected_response, expected_assertion, \
expected_trajectory ([{{tool_name,arguments}}]), \
expected_interactions ([{{from,to,description}}]), metadata.",
        slot.topic, req.context, req.task,
    )
}

fn slug(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c.is_ascii_whitespace() || c == '_' || c == '-' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
