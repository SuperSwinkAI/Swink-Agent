//! Data types for evaluation cases, invocations, and results.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, ToolResultMessage, Usage};
use swink_agent_policies::{BudgetPolicy, MaxTurnsPolicy};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::error::EvalError;
use crate::score::{Score, Verdict};
use crate::url_filter::UrlFilter;

// ─── Recorded Data ──────────────────────────────────────────────────────────

/// A tool call as captured from the agent event stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedToolCall {
    /// Provider-assigned tool call ID.
    pub id: String,
    /// Name of the tool that was invoked.
    pub name: String,
    /// Parsed JSON arguments passed to the tool.
    pub arguments: serde_json::Value,
}

/// A single recorded turn from an agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    /// Zero-based index of this turn within the run.
    pub turn_index: usize,
    /// The assistant message produced during this turn.
    pub assistant_message: AssistantMessage,
    /// Tool calls made during this turn (in execution order).
    pub tool_calls: Vec<RecordedToolCall>,
    /// Tool results returned during this turn.
    pub tool_results: Vec<ToolResultMessage>,
    /// Wall-clock duration of this turn.
    pub duration: Duration,
}

/// Complete trace of an agent run, built by [`TrajectoryCollector`](crate::TrajectoryCollector).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    /// All turns in execution order.
    pub turns: Vec<TurnRecord>,
    /// Aggregated token usage across all turns.
    pub total_usage: Usage,
    /// Aggregated cost across all turns.
    pub total_cost: Cost,
    /// Wall-clock duration of the entire run.
    pub total_duration: Duration,
    /// Extracted text from the final assistant message, if any.
    pub final_response: Option<String>,
    /// Stop reason from the final turn.
    pub stop_reason: StopReason,
    /// Model used for this run.
    pub model: ModelSpec,
}

// ─── Expected Data ──────────────────────────────────────────────────────────

/// A single expected tool invocation in a golden path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedToolCall {
    /// The tool name that should be called.
    pub tool_name: String,
    /// If present, the arguments must match exactly (JSON equality).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Criteria for matching the final response text.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ResponseCriteria {
    /// Response must match exactly.
    Exact { expected: String },
    /// Response must contain the given substring.
    Contains { substring: String },
    /// Response must match the given regex pattern.
    Regex { pattern: String },
    /// Custom scoring function (not serializable — set programmatically).
    #[serde(skip)]
    Custom(#[serde(skip)] Arc<dyn Fn(&str) -> Score + Send + Sync>),
}

impl std::fmt::Debug for ResponseCriteria {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact { expected } => {
                f.debug_struct("Exact").field("expected", expected).finish()
            }
            Self::Contains { substring } => f
                .debug_struct("Contains")
                .field("substring", substring)
                .finish(),
            Self::Regex { pattern } => f.debug_struct("Regex").field("pattern", pattern).finish(),
            Self::Custom(_) => f.debug_tuple("Custom").field(&"<fn>").finish(),
        }
    }
}

/// Named snapshot of an environment state produced by a [`StateCapture`].
///
/// Used with `EvalCase::expected_environment_state` to assert that after the
/// agent completes, the captured environment matches the expected values via
/// full JSON equality (FR-013, FR-015).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentState {
    /// Identifier for this state entry. Duplicate names within a single
    /// `expected_environment_state` are rejected at case-load time
    /// (FR-015, SC-009).
    pub name: String,
    /// Expected (or captured) JSON value; compared for full JSON equality.
    pub state: serde_json::Value,
}

/// Expected semantic tool intent used by the tool-parameter semantic evaluator.
///
/// When `tool_name` is `Some`, only tool calls whose name matches are judged;
/// other calls are skipped (not Pass, not Fail).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIntent {
    /// Natural-language description of what the tool call should accomplish.
    pub intent: String,
    /// When `Some`, restrict judging to tool calls with this exact name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Callback that captures the environment state after an agent run completes.
///
/// Registered programmatically on an [`EvalCase`] (or supplied by the
/// `AgentFactory`). The callback is invoked once after the agent finishes; its
/// output populates the "actual" side for the `EnvironmentStateEvaluator`.
///
/// Panics are caught by the evaluator and surfaced as `Score::fail()` with the
/// panic message (FR-014).
pub type StateCapture = Arc<dyn Fn(&Invocation) -> Vec<EnvironmentState> + Send + Sync>;

/// Judge-evaluated assertion expected to hold after an agent invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assertion {
    /// Natural-language assertion description.
    pub description: String,
    /// Machine-readable assertion category.
    pub kind: AssertionKind,
}

/// Assertion categories used by judge-backed evaluators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionKind {
    /// The user's goal was completed.
    GoalCompleted,
    /// The user appears satisfied with the outcome.
    UserSatisfied,
    /// A named tool must be invoked.
    ToolInvoked(String),
    /// Free-form predicate evaluated by a judge-backed evaluator.
    Custom { predicate: String },
}

/// Expected interaction between agents, tools, or hand-off participants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionExpectation {
    /// Source participant or component.
    pub from: String,
    /// Target participant or component.
    pub to: String,
    /// Expected interaction description.
    pub description: String,
}

/// Example shown to a judge prompt before the case being evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FewShotExample {
    /// Example input.
    pub input: String,
    /// Expected output or verdict.
    pub expected: String,
    /// Optional reasoning to include with the example.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// Multimodal attachment reference attached to an evaluation case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attachment {
    /// File path resolved relative to the eval-set root at materialization time.
    Path(PathBuf),
    /// Self-contained bytes with an explicit MIME type.
    Base64 { mime: String, bytes: Vec<u8> },
    /// Remote HTTPS resource guarded by a [`UrlFilter`].
    Url(String),
}

/// Bytes ready for judge-client payload construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Structured attachment materialization errors.
#[derive(Debug, Error)]
pub enum AttachmentError {
    #[error("attachment path not found: {0}")]
    PathNotFound(PathBuf),
    #[error("attachment decode failed: {0}")]
    DecodeError(String),
    #[error("attachment URL blocked: {url}: {reason}")]
    UrlBlocked { url: String, reason: String },
    #[error("attachment fetch failed: {url}: status {status}")]
    FetchFailed { url: String, status: u16 },
    #[error("unsupported attachment MIME type: {mime}")]
    UnsupportedMime { mime: String },
}

impl Attachment {
    /// Materialize an attachment into bytes suitable for judge dispatch.
    ///
    /// URL fetching is available when the `multimodal` feature is enabled.
    pub async fn materialize(
        &self,
        eval_set_root: &Path,
        filter: &dyn UrlFilter,
    ) -> Result<MaterializedAttachment, AttachmentError> {
        match self {
            Self::Path(path) => materialize_path(eval_set_root, path).await,
            Self::Base64 { mime, bytes } => {
                validate_attachment_mime(mime)?;
                Ok(MaterializedAttachment {
                    mime: normalize_mime(mime),
                    bytes: bytes.clone(),
                })
            }
            Self::Url(url) => materialize_url(url, filter).await,
        }
    }
}

async fn materialize_path(
    eval_set_root: &Path,
    path: &Path,
) -> Result<MaterializedAttachment, AttachmentError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(AttachmentError::PathNotFound(path.to_path_buf()));
    }

    let full_path = eval_set_root.join(path);
    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|_| AttachmentError::PathNotFound(path.to_path_buf()))?;
    let mime = mime_from_path(path)?;

    Ok(MaterializedAttachment { mime, bytes })
}

async fn materialize_url(
    url: &str,
    filter: &dyn UrlFilter,
) -> Result<MaterializedAttachment, AttachmentError> {
    let parsed = Url::parse(url).map_err(|err| AttachmentError::UrlBlocked {
        url: url.to_string(),
        reason: err.to_string(),
    })?;

    validate_remote_url(&parsed, filter)?;

    materialize_checked_url(parsed, filter).await
}

#[cfg(feature = "multimodal")]
async fn materialize_checked_url(
    parsed: Url,
    filter: &dyn UrlFilter,
) -> Result<MaterializedAttachment, AttachmentError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AttachmentError::FetchFailed {
            url: parsed.as_str().to_string(),
            status: 0,
        })?;
    let mut current = parsed;

    for _ in 0..10 {
        let url = current.as_str().to_string();
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(|_| AttachmentError::FetchFailed {
                url: url.clone(),
                status: 0,
            })?;
        let status = response.status();

        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| AttachmentError::FetchFailed {
                    url: url.clone(),
                    status: status.as_u16(),
                })?;
            current = resolve_redirect_target(&current, location, filter)?;
            continue;
        }

        if !status.is_success() {
            return Err(AttachmentError::FetchFailed {
                url,
                status: status.as_u16(),
            });
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(normalize_mime);
        let mime = match content_type {
            Some(mime) => {
                validate_attachment_mime(&mime)?;
                mime
            }
            None => mime_from_url_path(&url)?,
        };
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AttachmentError::FetchFailed { url, status: 0 })?
            .to_vec();

        return Ok(MaterializedAttachment { mime, bytes });
    }

    Err(AttachmentError::FetchFailed {
        url: current.as_str().to_string(),
        status: 0,
    })
}

#[cfg(not(feature = "multimodal"))]
#[allow(clippy::unused_async)]
async fn materialize_checked_url(
    parsed: Url,
    _filter: &dyn UrlFilter,
) -> Result<MaterializedAttachment, AttachmentError> {
    Err(AttachmentError::FetchFailed {
        url: parsed.as_str().to_string(),
        status: 0,
    })
}

fn validate_remote_url(url: &Url, filter: &dyn UrlFilter) -> Result<(), AttachmentError> {
    if url.scheme() != "https" {
        return Err(AttachmentError::UrlBlocked {
            url: url.as_str().to_string(),
            reason: "only https URLs are supported".to_string(),
        });
    }

    if !filter.allows(url) {
        return Err(AttachmentError::UrlBlocked {
            url: url.as_str().to_string(),
            reason: "blocked by URL filter".to_string(),
        });
    }

    Ok(())
}

#[cfg(feature = "multimodal")]
fn resolve_redirect_target(
    current: &Url,
    location: &str,
    filter: &dyn UrlFilter,
) -> Result<Url, AttachmentError> {
    let redirected = current
        .join(location)
        .map_err(|err| AttachmentError::UrlBlocked {
            url: current.as_str().to_string(),
            reason: format!("invalid redirect target: {err}"),
        })?;
    validate_remote_url(&redirected, filter)?;
    Ok(redirected)
}

fn mime_from_path(path: &Path) -> Result<String, AttachmentError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => {
            return Err(AttachmentError::UnsupportedMime {
                mime: "application/octet-stream".to_string(),
            });
        }
    };
    Ok(mime.to_string())
}

#[cfg(feature = "multimodal")]
fn mime_from_url_path(url: &str) -> Result<String, AttachmentError> {
    let parsed = Url::parse(url).map_err(|_| AttachmentError::UnsupportedMime {
        mime: "application/octet-stream".to_string(),
    })?;
    mime_from_path(Path::new(parsed.path()))
}

fn normalize_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
}

fn validate_attachment_mime(mime: &str) -> Result<(), AttachmentError> {
    let mime = normalize_mime(mime);
    match mime.as_str() {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => Ok(()),
        _ => Err(AttachmentError::UnsupportedMime { mime }),
    }
}

/// Stable namespace for deterministic case-derived session IDs.
///
/// Pinned to `Uuid::new_v5(&Uuid::NAMESPACE_OID, b"swink-agent-eval.case")`
/// per spec 043 research R-014.
pub const CASE_NAMESPACE: Uuid = Uuid::from_bytes([
    37, 101, 28, 203, 118, 231, 87, 244, 147, 248, 152, 59, 222, 174, 80, 226,
]);

/// Canonical serializable projection of an [`EvalCase`] used for deterministic
/// session IDs and future cache keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaseFingerprint {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub system_prompt: String,
    pub user_messages: Vec<String>,
    pub expected_trajectory: Option<Vec<ExpectedToolCallFingerprint>>,
    pub expected_response: Option<ResponseCriteriaFingerprint>,
    pub expected_assertion: Option<Assertion>,
    pub expected_interactions: Option<Vec<InteractionExpectation>>,
    pub few_shot_examples: Vec<FewShotExample>,
    pub budget: Option<BudgetConstraintsFingerprint>,
    pub evaluators: Vec<String>,
    pub metadata: CanonicalJsonValue,
    pub attachments: Vec<AttachmentFingerprint>,
    pub expected_environment_state: Option<Vec<EnvironmentStateFingerprint>>,
    pub expected_tool_intent: Option<ToolIntentFingerprint>,
    pub semantic_tool_selection: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectedToolCallFingerprint {
    pub tool_name: String,
    pub arguments: Option<CanonicalJsonValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ResponseCriteriaFingerprint {
    Exact { expected: String },
    Contains { substring: String },
    Regex { pattern: String },
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetConstraintsFingerprint {
    pub cost_limit_bits: Option<u64>,
    pub input_limit: Option<u64>,
    pub output_limit: Option<u64>,
    pub turn_limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvironmentStateFingerprint {
    pub name: String,
    pub state: CanonicalJsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolIntentFingerprint {
    pub intent: String,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum AttachmentFingerprint {
    Path(String),
    Base64 { mime: String, sha256: String },
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CanonicalJsonValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

impl From<&serde_json::Value> for CanonicalJsonValue {
    fn from(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(*value),
            serde_json::Value::Number(value) => Self::Number(value.to_string()),
            serde_json::Value::String(value) => Self::String(value.clone()),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from).collect())
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from(value)))
                    .collect(),
            ),
        }
    }
}

impl From<&ExpectedToolCall> for ExpectedToolCallFingerprint {
    fn from(call: &ExpectedToolCall) -> Self {
        Self {
            tool_name: call.tool_name.clone(),
            arguments: call.arguments.as_ref().map(CanonicalJsonValue::from),
        }
    }
}

impl From<&ResponseCriteria> for ResponseCriteriaFingerprint {
    fn from(criteria: &ResponseCriteria) -> Self {
        match criteria {
            ResponseCriteria::Exact { expected } => Self::Exact {
                expected: expected.clone(),
            },
            ResponseCriteria::Contains { substring } => Self::Contains {
                substring: substring.clone(),
            },
            ResponseCriteria::Regex { pattern } => Self::Regex {
                pattern: pattern.clone(),
            },
            ResponseCriteria::Custom(_) => Self::Custom,
        }
    }
}

impl From<&BudgetConstraints> for BudgetConstraintsFingerprint {
    fn from(budget: &BudgetConstraints) -> Self {
        Self {
            cost_limit_bits: budget.max_cost.map(f64::to_bits),
            input_limit: budget.max_input,
            output_limit: budget.max_output,
            turn_limit: budget.max_turns,
        }
    }
}

impl From<&EnvironmentState> for EnvironmentStateFingerprint {
    fn from(state: &EnvironmentState) -> Self {
        Self {
            name: state.name.clone(),
            state: CanonicalJsonValue::from(&state.state),
        }
    }
}

impl From<&ToolIntent> for ToolIntentFingerprint {
    fn from(intent: &ToolIntent) -> Self {
        Self {
            intent: intent.intent.clone(),
            tool_name: intent.tool_name.clone(),
        }
    }
}

impl From<&Attachment> for AttachmentFingerprint {
    fn from(attachment: &Attachment) -> Self {
        match attachment {
            Attachment::Path(path) => Self::Path(path.to_string_lossy().replace('\\', "/")),
            Attachment::Base64 { mime, bytes } => {
                let digest = Sha256::digest(bytes);
                Self::Base64 {
                    mime: normalize_mime(mime),
                    sha256: hex_lower(&digest),
                }
            }
            Attachment::Url(url) => Self::Url(url.clone()),
        }
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Budget constraints for cost and latency governance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConstraints {
    /// Maximum allowed cost in dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<f64>,
    /// Maximum allowed input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input: Option<u64>,
    /// Maximum allowed output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    /// Maximum allowed number of turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

impl BudgetConstraints {
    /// Convert budget constraints into loop policies for agent construction.
    #[must_use]
    pub fn to_policies(&self) -> (Option<BudgetPolicy>, Option<MaxTurnsPolicy>) {
        let budget_policy =
            if self.max_cost.is_none() && self.max_input.is_none() && self.max_output.is_none() {
                None
            } else {
                let mut policy = BudgetPolicy::new();
                if let Some(max_cost) = self.max_cost {
                    policy = policy.max_cost(max_cost);
                }
                if let Some(max_input) = self.max_input {
                    policy = policy.max_input(max_input);
                }
                if let Some(max_output) = self.max_output {
                    policy = policy.max_output(max_output);
                }
                Some(policy)
            };

        let max_turns_policy = self.max_turns.map(MaxTurnsPolicy::new);

        (budget_policy, max_turns_policy)
    }
}

// ─── Eval Case & Set ────────────────────────────────────────────────────────

/// A single evaluation scenario.
///
/// Defines the agent prompt, expected outcomes, and which evaluators to run.
#[derive(Clone, Serialize, Deserialize)]
pub struct EvalCase {
    /// Unique identifier for this case.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description of what this case tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// System prompt for the agent.
    pub system_prompt: String,
    /// Initial user messages (the prompt).
    pub user_messages: Vec<String>,
    /// Expected tool call trajectory (golden path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_trajectory: Option<Vec<ExpectedToolCall>>,
    /// Expected final response criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_response: Option<ResponseCriteria>,
    /// Judge-evaluated assertion expected to hold after the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_assertion: Option<Assertion>,
    /// Expected interactions or hand-offs within the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_interactions: Option<Vec<InteractionExpectation>>,
    /// Prompt examples injected ahead of judge-backed evaluations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub few_shot_examples: Vec<FewShotExample>,
    /// Cost/budget governance constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetConstraints>,
    /// Names of evaluators to run. Empty means all registered evaluators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluators: Vec<String>,
    /// Arbitrary metadata for user-defined extensions and filtering.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
    /// Multimodal data references consumed by multimodal evaluators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Stable case/session identifier. When absent, callers may derive one
    /// deterministically via [`Self::default_session_id`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_uuid",
        deserialize_with = "deserialize_optional_uuid"
    )]
    pub session_id: Option<Uuid>,
    /// Expected environment-state snapshots keyed by name (FR-013).
    ///
    /// Compared against the output of `state_capture` via full JSON equality.
    /// Duplicate names are rejected at case-load time (FR-015, SC-009).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_environment_state: Option<Vec<EnvironmentState>>,
    /// Expected semantic tool intent for the tool-parameter evaluator (FR-012).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_tool_intent: Option<ToolIntent>,
    /// Enable semantic tool-selection scoring for this case (FR-011).
    #[serde(default, skip_serializing_if = "is_false")]
    pub semantic_tool_selection: bool,
    /// Callback that produces the actual environment state after the agent
    /// completes. Programmatic only — mirrors `ResponseCriteria::Custom`.
    #[serde(skip)]
    pub state_capture: Option<StateCapture>,
}

impl std::fmt::Debug for EvalCase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvalCase")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("system_prompt", &self.system_prompt)
            .field("user_messages", &self.user_messages)
            .field("expected_trajectory", &self.expected_trajectory)
            .field("expected_response", &self.expected_response)
            .field("expected_assertion", &self.expected_assertion)
            .field("expected_interactions", &self.expected_interactions)
            .field("few_shot_examples", &self.few_shot_examples)
            .field("budget", &self.budget)
            .field("evaluators", &self.evaluators)
            .field("metadata", &self.metadata)
            .field("attachments", &self.attachments)
            .field("session_id", &self.session_id)
            .field(
                "expected_environment_state",
                &self.expected_environment_state,
            )
            .field("expected_tool_intent", &self.expected_tool_intent)
            .field("semantic_tool_selection", &self.semantic_tool_selection)
            .field(
                "state_capture",
                &self.state_capture.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
}

impl From<&EvalCase> for CaseFingerprint {
    fn from(case: &EvalCase) -> Self {
        Self {
            id: case.id.clone(),
            name: case.name.clone(),
            description: case.description.clone(),
            system_prompt: case.system_prompt.clone(),
            user_messages: case.user_messages.clone(),
            expected_trajectory: case.expected_trajectory.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(ExpectedToolCallFingerprint::from)
                    .collect()
            }),
            expected_response: case
                .expected_response
                .as_ref()
                .map(ResponseCriteriaFingerprint::from),
            expected_assertion: case.expected_assertion.clone(),
            expected_interactions: case.expected_interactions.clone(),
            few_shot_examples: case.few_shot_examples.clone(),
            budget: case.budget.as_ref().map(BudgetConstraintsFingerprint::from),
            evaluators: case.evaluators.clone(),
            metadata: CanonicalJsonValue::from(&case.metadata),
            attachments: case
                .attachments
                .iter()
                .map(AttachmentFingerprint::from)
                .collect(),
            expected_environment_state: case.expected_environment_state.as_ref().map(|states| {
                states
                    .iter()
                    .map(EnvironmentStateFingerprint::from)
                    .collect()
            }),
            expected_tool_intent: case
                .expected_tool_intent
                .as_ref()
                .map(ToolIntentFingerprint::from),
            semantic_tool_selection: case.semantic_tool_selection,
        }
    }
}

impl EvalCase {
    /// Canonical serializable projection used by deterministic ID and cache-key
    /// derivation.
    #[must_use]
    pub fn content_fingerprint(&self) -> CaseFingerprint {
        CaseFingerprint::from(self)
    }

    /// Deterministically derive the default session ID for this case.
    ///
    /// Programmatic-only closures such as `state_capture` and
    /// `ResponseCriteria::Custom` bodies are never serialized directly.
    /// Instead, this hashes a stable canonical fingerprint that preserves the
    /// presence of custom criteria while avoiding pointer-address instability.
    #[must_use]
    pub fn default_session_id(&self) -> Uuid {
        let canonical =
            serde_json::to_vec(&self.content_fingerprint()).expect("case fingerprint serializes");
        let digest = Sha256::digest(canonical);
        Uuid::new_v5(&CASE_NAMESPACE, digest.as_slice())
    }

    /// Validate this case's static configuration.
    pub fn validate(&self) -> Result<(), EvalError> {
        if let Some(assertion) = &self.expected_assertion {
            validate_non_empty_field(
                &self.id,
                "expected_assertion.description",
                &assertion.description,
            )?;
            match &assertion.kind {
                AssertionKind::GoalCompleted | AssertionKind::UserSatisfied => {}
                AssertionKind::ToolInvoked(tool_name) => {
                    validate_non_empty_field(
                        &self.id,
                        "expected_assertion.kind.tool_name",
                        tool_name,
                    )?;
                }
                AssertionKind::Custom { predicate } => {
                    validate_non_empty_field(
                        &self.id,
                        "expected_assertion.kind.predicate",
                        predicate,
                    )?;
                }
            }
        }

        if let Some(interactions) = &self.expected_interactions {
            for (index, interaction) in interactions.iter().enumerate() {
                let field_prefix = format!("expected_interactions[{index}]");
                validate_non_empty_field(
                    &self.id,
                    &format!("{field_prefix}.from"),
                    &interaction.from,
                )?;
                validate_non_empty_field(&self.id, &format!("{field_prefix}.to"), &interaction.to)?;
                validate_non_empty_field(
                    &self.id,
                    &format!("{field_prefix}.description"),
                    &interaction.description,
                )?;
            }
        }

        for (index, example) in self.few_shot_examples.iter().enumerate() {
            let field_prefix = format!("few_shot_examples[{index}]");
            validate_non_empty_field(&self.id, &format!("{field_prefix}.input"), &example.input)?;
            validate_non_empty_field(
                &self.id,
                &format!("{field_prefix}.expected"),
                &example.expected,
            )?;
            if let Some(reasoning) = &example.reasoning {
                validate_non_empty_field(
                    &self.id,
                    &format!("{field_prefix}.reasoning"),
                    reasoning,
                )?;
            }
        }

        for (index, attachment) in self.attachments.iter().enumerate() {
            validate_attachment_declaration(&self.id, index, attachment)?;
        }

        if let Some(states) = &self.expected_environment_state {
            let mut seen: HashSet<&str> = HashSet::with_capacity(states.len());
            for state in states {
                if !seen.insert(state.name.as_str()) {
                    return Err(EvalError::invalid_case(format!(
                        "case `{case_id}`: duplicate expected_environment_state name `{name}`",
                        case_id = self.id,
                        name = state.name,
                    )));
                }
            }
        }

        Ok(())
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(b: &bool) -> bool {
    !*b
}

/// A named collection of evaluation cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSet {
    /// Unique identifier for this set.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The cases in this set.
    pub cases: Vec<EvalCase>,
}

// ─── Results ────────────────────────────────────────────────────────────────

/// Per-evaluator result for a single case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalMetricResult {
    /// Name of the evaluator that produced this result.
    pub evaluator_name: String,
    /// The numeric score.
    pub score: Score,
    /// Optional human-readable details about the scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Result of evaluating a single case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCaseResult {
    /// The case ID that was evaluated.
    pub case_id: String,
    /// The captured invocation trace.
    pub invocation: Invocation,
    /// Per-evaluator metric results.
    pub metric_results: Vec<EvalMetricResult>,
    /// Overall verdict (all metrics must pass).
    pub verdict: Verdict,
}

/// Result of evaluating an entire eval set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSetResult {
    /// The eval set ID.
    pub eval_set_id: String,
    /// Per-case results.
    pub case_results: Vec<EvalCaseResult>,
    /// Aggregated summary statistics.
    pub summary: EvalSummary,
    /// Unix timestamp when this result was produced.
    pub timestamp: u64,
}

/// Aggregated statistics for an eval set run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    /// Total number of cases evaluated.
    pub total_cases: usize,
    /// Number of cases that passed all metrics.
    pub passed: usize,
    /// Number of cases that failed at least one metric.
    pub failed: usize,
    /// Aggregated cost across all cases.
    pub total_cost: Cost,
    /// Aggregated token usage across all cases.
    pub total_usage: Usage,
    /// Total wall-clock duration across all cases.
    pub total_duration: Duration,
}

// ─── Case-load Validation (FR-015, SC-009) ──────────────────────────────────

/// Validate a single [`EvalCase`] against the case-load rules.
///
/// Currently enforces:
///
/// * `expected_environment_state` — names MUST be unique. Duplicates are
///   rejected with [`EvalError::InvalidCase`] pointing at the offending name
///   (FR-015, SC-009).
///
/// This check is shared by [`validate_eval_set`] and the YAML loader so
/// programmatic constructors get the same guarantees as on-disk configs.
pub fn validate_eval_case(case: &EvalCase) -> Result<(), EvalError> {
    case.validate()
}

/// Validate an entire [`EvalSet`], short-circuiting on the first invalid case.
pub fn validate_eval_set(set: &EvalSet) -> Result<(), EvalError> {
    let mut seen_case_ids: HashSet<&str> = HashSet::with_capacity(set.cases.len());
    for case in &set.cases {
        if !seen_case_ids.insert(case.id.as_str()) {
            return Err(EvalError::invalid_case(format!(
                "eval set `{set_id}`: duplicate case id `{case_id}`",
                set_id = set.id,
                case_id = case.id,
            )));
        }
        case.validate()?;
    }
    Ok(())
}

fn validate_non_empty_field(case_id: &str, field: &str, value: &str) -> Result<(), EvalError> {
    if value.trim().is_empty() {
        return Err(EvalError::invalid_case(format!(
            "case `{case_id}`: `{field}` must not be blank"
        )));
    }
    Ok(())
}

fn validate_attachment_declaration(
    case_id: &str,
    index: usize,
    attachment: &Attachment,
) -> Result<(), EvalError> {
    match attachment {
        Attachment::Path(path) => {
            if path.as_os_str().is_empty()
                || path.is_absolute()
                || path
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                return Err(EvalError::invalid_case(format!(
                    "case `{case_id}`: attachments[{index}] path must stay relative to the eval-set root"
                )));
            }
        }
        Attachment::Base64 { mime, .. } => {
            validate_attachment_mime(mime).map_err(|err| {
                EvalError::invalid_case(format!(
                    "case `{case_id}`: attachments[{index}] invalid MIME: {err}"
                ))
            })?;
        }
        Attachment::Url(url) => {
            let parsed = Url::parse(url).map_err(|err| {
                EvalError::invalid_case(format!(
                    "case `{case_id}`: attachments[{index}] invalid URL: {err}"
                ))
            })?;
            if parsed.scheme() != "https" {
                return Err(EvalError::invalid_case(format!(
                    "case `{case_id}`: attachments[{index}] URL must use https"
                )));
            }
        }
    }

    Ok(())
}

#[allow(clippy::ref_option)]
fn serialize_optional_uuid<S>(value: &Option<Uuid>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(uuid) => serializer.serialize_some(&uuid.to_string()),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_uuid<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|value| {
            Uuid::parse_str(&value).map_err(|err| serde::de::Error::custom(err.to_string()))
        })
        .transpose()
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn base_case(id: &str) -> EvalCase {
        EvalCase {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            system_prompt: String::new(),
            user_messages: vec!["hi".to_string()],
            expected_trajectory: None,
            expected_response: None,
            expected_assertion: None,
            expected_interactions: None,
            few_shot_examples: vec![],
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
            attachments: vec![],
            session_id: None,
            expected_environment_state: None,
            expected_tool_intent: None,
            semantic_tool_selection: false,
            state_capture: None,
        }
    }

    #[test]
    fn validate_accepts_unique_environment_state_names() {
        let mut case = base_case("c1");
        case.expected_environment_state = Some(vec![
            EnvironmentState {
                name: "alpha".into(),
                state: serde_json::json!({"v": 1}),
            },
            EnvironmentState {
                name: "beta".into(),
                state: serde_json::json!({"v": 2}),
            },
        ]);
        assert!(validate_eval_case(&case).is_ok());
    }

    #[test]
    fn validate_rejects_duplicate_environment_state_names() {
        let mut case = base_case("dup");
        case.expected_environment_state = Some(vec![
            EnvironmentState {
                name: "alpha".into(),
                state: serde_json::json!({"v": 1}),
            },
            EnvironmentState {
                name: "alpha".into(),
                state: serde_json::json!({"v": 2}),
            },
        ]);
        let err = validate_eval_case(&case).expect_err("duplicate should be rejected");
        match err {
            EvalError::InvalidCase { reason } => {
                assert!(reason.contains("alpha"), "reason: {reason}");
                assert!(reason.contains("dup"), "reason mentions case id: {reason}");
            }
            other => panic!("expected InvalidCase, got {other:?}"),
        }
    }

    #[test]
    fn validate_none_environment_state_is_ok() {
        let case = base_case("none");
        assert!(validate_eval_case(&case).is_ok());
    }

    #[test]
    fn validate_eval_set_propagates_case_errors() {
        let mut case = base_case("bad");
        case.expected_environment_state = Some(vec![
            EnvironmentState {
                name: "x".into(),
                state: serde_json::Value::Null,
            },
            EnvironmentState {
                name: "x".into(),
                state: serde_json::Value::Null,
            },
        ]);
        let set = EvalSet {
            id: "set".into(),
            name: "Set".into(),
            description: None,
            cases: vec![case],
        };
        assert!(validate_eval_set(&set).is_err());
    }

    #[test]
    fn environment_state_serde_round_trip() {
        let state = EnvironmentState {
            name: "db".into(),
            state: serde_json::json!({"rows": 3, "schema": "public"}),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: EnvironmentState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, state.name);
        assert_eq!(back.state, state.state);
    }

    #[test]
    fn eval_case_serde_round_trip_with_v2_fields() {
        let mut case = base_case("v2");
        case.expected_environment_state = Some(vec![EnvironmentState {
            name: "alpha".into(),
            state: serde_json::json!({"n": 1}),
        }]);
        case.expected_tool_intent = Some(ToolIntent {
            intent: "read config".into(),
            tool_name: Some("read_file".into()),
        });
        case.expected_assertion = Some(Assertion {
            description: "goal completed".into(),
            kind: AssertionKind::GoalCompleted,
        });
        case.expected_interactions = Some(vec![InteractionExpectation {
            from: "planner".into(),
            to: "worker".into(),
            description: "delegates the task".into(),
        }]);
        case.few_shot_examples = vec![FewShotExample {
            input: "hello".into(),
            expected: "world".into(),
            reasoning: Some("example".into()),
        }];
        case.session_id = Some(Uuid::nil());
        case.semantic_tool_selection = true;
        let yaml_like = serde_json::to_string(&case).unwrap();
        let back: EvalCase = serde_json::from_str(&yaml_like).unwrap();
        assert_eq!(back.expected_environment_state.as_ref().unwrap().len(), 1);
        assert_eq!(
            back.expected_tool_intent.as_ref().unwrap().intent,
            "read config"
        );
        assert_eq!(
            back.expected_assertion.as_ref().unwrap().description,
            "goal completed"
        );
        assert_eq!(back.expected_interactions.as_ref().unwrap().len(), 1);
        assert_eq!(back.few_shot_examples.len(), 1);
        assert_eq!(back.session_id, Some(Uuid::nil()));
        assert!(back.semantic_tool_selection);
        assert!(back.attachments.is_empty());
        assert!(back.state_capture.is_none());
    }

    #[test]
    fn case_namespace_matches_oid_derived_value() {
        assert_eq!(
            CASE_NAMESPACE,
            Uuid::new_v5(&Uuid::NAMESPACE_OID, b"swink-agent-eval.case")
        );
    }

    #[test]
    fn default_session_id_is_deterministic_for_same_case() {
        let mut case = base_case("stable");
        case.metadata = serde_json::json!({
            "beta": [2, {"y": true, "x": false}],
            "alpha": {"nested_b": 2, "nested_a": 1}
        });
        case.expected_response = Some(ResponseCriteria::Contains {
            substring: "ok".into(),
        });
        case.expected_trajectory = Some(vec![ExpectedToolCall {
            tool_name: "read_file".into(),
            arguments: Some(serde_json::json!({"path": "./project-alpha/config.toml"})),
        }]);

        let first = case.default_session_id();
        let second = case.default_session_id();
        assert_eq!(first, second);
    }

    #[test]
    fn default_session_id_is_stable_across_json_key_order() {
        let mut left = base_case("ordered");
        left.metadata = serde_json::json!({
            "alpha": {"x": 1, "y": 2},
            "beta": [3, 4]
        });
        left.expected_environment_state = Some(vec![EnvironmentState {
            name: "workspace".into(),
            state: serde_json::json!({"files": {"b": 2, "a": 1}}),
        }]);

        let mut right = left.clone();
        right.metadata = serde_json::from_str(r#"{"beta":[3,4],"alpha":{"y":2,"x":1}}"#)
            .expect("valid metadata json");
        right.expected_environment_state = Some(vec![EnvironmentState {
            name: "workspace".into(),
            state: serde_json::from_str(r#"{"files":{"a":1,"b":2}}"#).expect("valid state json"),
        }]);

        assert_eq!(left.default_session_id(), right.default_session_id());
    }

    #[test]
    fn default_session_id_changes_when_case_content_changes() {
        let mut case = base_case("mutates");
        let original = case.default_session_id();
        case.user_messages.push("follow-up".into());
        assert_ne!(original, case.default_session_id());
    }
}

#[cfg(test)]
mod budget_policy_tests {
    use super::*;
    use swink_agent::{Cost, PolicyContext, PolicyVerdict, PreTurnPolicy, SessionState, Usage};

    fn make_ctx<'a>(turn_index: usize, usage: &'a Usage, cost: &'a Cost) -> PolicyContext<'a> {
        let state = Box::leak(Box::new(SessionState::new()));
        PolicyContext {
            turn_index,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    #[test]
    fn budget_constraints_to_policies_none_when_unset() {
        let constraints = BudgetConstraints {
            max_cost: None,
            max_input: None,
            max_output: None,
            max_turns: None,
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();

        assert!(budget_policy.is_none());
        assert!(max_turns_policy.is_none());
    }

    #[test]
    fn budget_constraints_to_policies_builds_budget_only_for_cost() {
        let constraints = BudgetConstraints {
            max_cost: Some(1.0),
            max_input: None,
            max_output: None,
            max_turns: None,
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();
        let usage = Usage::default();
        let cost = Cost {
            total: 1.0,
            ..Default::default()
        };
        let ctx = make_ctx(0, &usage, &cost);

        assert!(matches!(
            PreTurnPolicy::evaluate(&budget_policy.unwrap(), &ctx),
            PolicyVerdict::Stop(_)
        ));
        assert!(max_turns_policy.is_none());
    }

    #[test]
    fn budget_constraints_to_policies_builds_budget_only_for_input_output() {
        let constraints = BudgetConstraints {
            max_cost: None,
            max_input: Some(10),
            max_output: Some(20),
            max_turns: None,
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();
        let usage = Usage {
            input: 10,
            output: 20,
            total: 30,
            ..Default::default()
        };
        let cost = Cost::default();
        let ctx = make_ctx(0, &usage, &cost);

        assert!(matches!(
            PreTurnPolicy::evaluate(&budget_policy.unwrap(), &ctx),
            PolicyVerdict::Stop(_)
        ));
        assert!(max_turns_policy.is_none());
    }

    #[test]
    fn budget_constraints_to_policies_builds_both_policies_when_needed() {
        let constraints = BudgetConstraints {
            max_cost: Some(2.0),
            max_input: None,
            max_output: None,
            max_turns: Some(3),
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();
        let usage = Usage::default();
        let cost = Cost {
            total: 2.0,
            ..Default::default()
        };
        let budget_ctx = make_ctx(0, &usage, &cost);
        let turn_cost = Cost::default();
        let turn_ctx = make_ctx(3, &usage, &turn_cost);

        assert!(matches!(
            PreTurnPolicy::evaluate(&budget_policy.unwrap(), &budget_ctx),
            PolicyVerdict::Stop(_)
        ));
        assert!(matches!(
            PreTurnPolicy::evaluate(&max_turns_policy.unwrap(), &turn_ctx),
            PolicyVerdict::Stop(_)
        ));
    }
}

#[cfg(all(test, feature = "multimodal"))]
mod attachment_url_tests {
    use super::*;

    struct AllowListedFilter;

    impl UrlFilter for AllowListedFilter {
        fn allows(&self, url: &Url) -> bool {
            matches!(url.host_str(), Some("assets.example.com" | "cdn.example.com"))
        }
    }

    #[test]
    fn resolve_redirect_target_revalidates_each_hop_against_filter() {
        let current = Url::parse("https://assets.example.com/image.png").unwrap();
        let err = resolve_redirect_target(
            &current,
            "https://169.254.169.254/latest/meta-data",
            &AllowListedFilter,
        )
        .expect_err("redirect target should be revalidated");

        match err {
            AttachmentError::UrlBlocked { url, reason } => {
                assert_eq!(url, "https://169.254.169.254/latest/meta-data");
                assert!(reason.contains("blocked by URL filter"));
            }
            other => panic!("expected UrlBlocked, got {other:?}"),
        }
    }

    #[test]
    fn resolve_redirect_target_rejects_http_downgrades() {
        let current = Url::parse("https://assets.example.com/image.png").unwrap();
        let err = resolve_redirect_target(&current, "http://cdn.example.com/image.png", &AllowListedFilter)
            .expect_err("http redirect should be rejected");

        match err {
            AttachmentError::UrlBlocked { url, reason } => {
                assert_eq!(url, "http://cdn.example.com/image.png");
                assert!(reason.contains("only https URLs are supported"));
            }
            other => panic!("expected UrlBlocked, got {other:?}"),
        }
    }

    #[test]
    fn resolve_redirect_target_allows_relative_https_redirects_when_filter_passes() {
        let current = Url::parse("https://assets.example.com/path/start.png").unwrap();
        let redirected =
            resolve_redirect_target(&current, "../final.webp", &AllowListedFilter).unwrap();

        assert_eq!(redirected.as_str(), "https://assets.example.com/final.webp");
    }
}
