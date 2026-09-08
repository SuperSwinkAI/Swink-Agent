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
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedToolCall {
    /// Provider-assigned tool call ID.
    pub id: String,
    /// Name of the tool that was invoked.
    pub name: String,
    /// Parsed JSON arguments passed to the tool.
    pub arguments: serde_json::Value,
}

impl RecordedToolCall {
    /// Create a recorded tool call with the given ID, name, and arguments.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// A single recorded turn from an agent run.
#[non_exhaustive]
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

impl TurnRecord {
    /// Create a turn record with empty tool calls/results and zero duration.
    #[must_use]
    pub fn new(turn_index: usize, assistant_message: AssistantMessage) -> Self {
        Self {
            turn_index,
            assistant_message,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            duration: Duration::ZERO,
        }
    }

    /// Set the tool calls made during this turn.
    #[must_use]
    pub fn with_tool_calls(mut self, tool_calls: Vec<RecordedToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Set the tool results returned during this turn.
    #[must_use]
    pub fn with_tool_results(mut self, tool_results: Vec<ToolResultMessage>) -> Self {
        self.tool_results = tool_results;
        self
    }

    /// Set the wall-clock duration of this turn.
    #[must_use]
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
}

/// Complete trace of an agent run, built by [`TrajectoryCollector`](crate::TrajectoryCollector).
#[non_exhaustive]
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

impl Invocation {
    /// Create an invocation with empty turns, default usage/cost, zero duration, and no
    /// final response.
    #[must_use]
    pub fn new(stop_reason: StopReason, model: ModelSpec) -> Self {
        Self {
            turns: Vec::new(),
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: Duration::ZERO,
            final_response: None,
            stop_reason,
            model,
        }
    }

    /// Set all turns in execution order.
    #[must_use]
    pub fn with_turns(mut self, turns: Vec<TurnRecord>) -> Self {
        self.turns = turns;
        self
    }

    /// Set the aggregated token usage across all turns.
    #[must_use]
    pub fn with_total_usage(mut self, total_usage: Usage) -> Self {
        self.total_usage = total_usage;
        self
    }

    /// Set the aggregated cost across all turns.
    #[must_use]
    pub fn with_total_cost(mut self, total_cost: Cost) -> Self {
        self.total_cost = total_cost;
        self
    }

    /// Set the wall-clock duration of the entire run.
    #[must_use]
    pub fn with_total_duration(mut self, total_duration: Duration) -> Self {
        self.total_duration = total_duration;
        self
    }

    /// Set the extracted text from the final assistant message.
    #[must_use]
    pub fn with_final_response(mut self, final_response: impl Into<String>) -> Self {
        self.final_response = Some(final_response.into());
        self
    }
}

// ─── Expected Data ──────────────────────────────────────────────────────────

/// A single expected tool invocation in a golden path.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedToolCall {
    /// The tool name that should be called.
    pub tool_name: String,
    /// If present, the arguments must match exactly (JSON equality).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

impl ExpectedToolCall {
    /// Create an expected tool call with no argument constraint.
    #[must_use]
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            arguments: None,
        }
    }

    /// Set the expected arguments (compared via exact JSON equality).
    #[must_use]
    pub fn with_arguments(mut self, arguments: serde_json::Value) -> Self {
        self.arguments = Some(arguments);
        self
    }
}

/// Criteria for matching the final response text.
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentState {
    /// Identifier for this state entry. Duplicate names within a single
    /// `expected_environment_state` are rejected at case-load time
    /// (FR-015, SC-009).
    pub name: String,
    /// Expected (or captured) JSON value; compared for full JSON equality.
    pub state: serde_json::Value,
}

impl EnvironmentState {
    /// Create an environment state entry with the given name and value.
    #[must_use]
    pub fn new(name: impl Into<String>, state: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            state,
        }
    }
}

/// Expected semantic tool intent used by the tool-parameter semantic evaluator.
///
/// When `tool_name` is `Some`, only tool calls whose name matches are judged;
/// other calls are skipped (not Pass, not Fail).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIntent {
    /// Natural-language description of what the tool call should accomplish.
    pub intent: String,
    /// When `Some`, restrict judging to tool calls with this exact name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ToolIntent {
    /// Create a tool intent that judges every tool call.
    #[must_use]
    pub fn new(intent: impl Into<String>) -> Self {
        Self {
            intent: intent.into(),
            tool_name: None,
        }
    }

    /// Set the tool name that restricts which calls are judged.
    #[must_use]
    pub fn with_tool_name(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_name = Some(tool_name.into());
        self
    }
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
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assertion {
    /// Natural-language assertion description.
    pub description: String,
    /// Machine-readable assertion category.
    pub kind: AssertionKind,
}

impl Assertion {
    /// Create an assertion with the given description and kind.
    #[must_use]
    pub fn new(description: impl Into<String>, kind: AssertionKind) -> Self {
        Self {
            description: description.into(),
            kind,
        }
    }
}

/// Assertion categories used by judge-backed evaluators.
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionExpectation {
    /// Source participant or component.
    pub from: String,
    /// Target participant or component.
    pub to: String,
    /// Expected interaction description.
    pub description: String,
}

impl InteractionExpectation {
    /// Create an expected interaction between the given participants.
    #[must_use]
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            description: description.into(),
        }
    }
}

/// Example shown to a judge prompt before the case being evaluated.
#[non_exhaustive]
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

impl FewShotExample {
    /// Create a few-shot example with no accompanying reasoning.
    #[must_use]
    pub fn new(input: impl Into<String>, expected: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            expected: expected.into(),
            reasoning: None,
        }
    }

    /// Set the reasoning to include with the example.
    #[must_use]
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }
}

/// Multimodal attachment reference attached to an evaluation case.
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl MaterializedAttachment {
    /// Create a materialized attachment from its MIME type and bytes.
    #[must_use]
    pub fn new(mime: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            mime: mime.into(),
            bytes,
        }
    }
}

/// Structured attachment materialization errors.
#[non_exhaustive]
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
    crate::ensure_default_crypto_provider();
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
        let response =
            client
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
///
/// Construct via [`EvalCase::content_fingerprint`].
#[non_exhaustive]
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

/// Narrow, cache-key-only projection of an [`EvalCase`] (spec 043 FR-038).
///
/// Unlike [`CaseFingerprint`] — which feeds [`EvalCase::default_session_id`]
/// and deliberately captures every case field so distinct cases never share a
/// session ID — this type hashes only the case-derived fields FR-038 lists as
/// part of the agent-invocation cache key: `case_id`, `system_prompt`, and
/// `user_messages`. The remaining three fields FR-038 names
/// (`initial_session`, tool-set hash, agent model) live on
/// [`crate::cache::FingerprintContext`], which the caller combines with this
/// type via [`crate::cache::CacheKey::from_fingerprint`].
///
/// Fields outside this set (expected criteria, budget, evaluators, metadata,
/// attachments, etc.) affect *scoring*, not what the agent actually sees, so
/// they intentionally do NOT invalidate the agent-invocation cache — the same
/// cached invocation remains valid for a case whose only change is, say, an
/// added assertion or a new evaluator filter.
///
/// Construct via [`EvalCase::cache_fingerprint`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheFingerprint {
    pub case_id: String,
    pub system_prompt: String,
    pub user_messages: Vec<String>,
}

impl From<&EvalCase> for CacheFingerprint {
    fn from(case: &EvalCase) -> Self {
        Self {
            case_id: case.id.clone(),
            system_prompt: case.system_prompt.clone(),
            user_messages: case.user_messages.clone(),
        }
    }
}

/// Canonical fingerprint projection of an [`ExpectedToolCall`], constructed via `From`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectedToolCallFingerprint {
    pub tool_name: String,
    pub arguments: Option<CanonicalJsonValue>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ResponseCriteriaFingerprint {
    Exact { expected: String },
    Contains { substring: String },
    Regex { pattern: String },
    Custom,
}

/// Canonical fingerprint projection of [`BudgetConstraints`], constructed via `From`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetConstraintsFingerprint {
    pub cost_limit_bits: Option<u64>,
    pub input_limit: Option<u64>,
    pub output_limit: Option<u64>,
    pub turn_limit: Option<usize>,
}

/// Canonical fingerprint projection of an [`EnvironmentState`], constructed via `From`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvironmentStateFingerprint {
    pub name: String,
    pub state: CanonicalJsonValue,
}

/// Canonical fingerprint projection of a [`ToolIntent`], constructed via `From`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolIntentFingerprint {
    pub intent: String,
    pub tool_name: Option<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum AttachmentFingerprint {
    Path(String),
    Base64 { mime: String, sha256: String },
    Url(String),
}

/// Frozen by design: a complete classification of the JSON value domain
/// (RFC 8259) — a new variant is impossible without JSON itself changing.
#[allow(clippy::exhaustive_enums)]
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
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Set the maximum allowed cost in dollars.
    #[must_use]
    pub fn with_max_cost(mut self, max_cost: f64) -> Self {
        self.max_cost = Some(max_cost);
        self
    }

    /// Set the maximum allowed input tokens.
    #[must_use]
    pub fn with_max_input(mut self, max_input: u64) -> Self {
        self.max_input = Some(max_input);
        self
    }

    /// Set the maximum allowed output tokens.
    #[must_use]
    pub fn with_max_output(mut self, max_output: u64) -> Self {
        self.max_output = Some(max_output);
        self
    }

    /// Set the maximum allowed number of turns.
    #[must_use]
    pub fn with_max_turns(mut self, max_turns: usize) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Convert budget constraints into loop policies for agent construction.
    #[must_use]
    pub fn to_policies(&self) -> (Option<BudgetPolicy>, Option<MaxTurnsPolicy>) {
        let budget_policy =
            if self.max_cost.is_none() && self.max_input.is_none() && self.max_output.is_none() {
                None
            } else {
                let mut policy = BudgetPolicy::new();
                if let Some(max_cost) = self.max_cost {
                    policy = policy.with_max_cost(max_cost);
                }
                if let Some(max_input) = self.max_input {
                    policy = policy.with_max_input(max_input);
                }
                if let Some(max_output) = self.max_output {
                    policy = policy.with_max_output(max_output);
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
#[non_exhaustive]
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
    /// Create an eval case with no expected criteria, budget, evaluators, or attachments.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        system_prompt: impl Into<String>,
        user_messages: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            system_prompt: system_prompt.into(),
            user_messages,
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

    /// Set the description of what this case tests.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the expected tool call trajectory (golden path).
    #[must_use]
    pub fn with_expected_trajectory(mut self, expected_trajectory: Vec<ExpectedToolCall>) -> Self {
        self.expected_trajectory = Some(expected_trajectory);
        self
    }

    /// Set the expected final response criteria.
    #[must_use]
    pub fn with_expected_response(mut self, expected_response: ResponseCriteria) -> Self {
        self.expected_response = Some(expected_response);
        self
    }

    /// Set the judge-evaluated assertion expected to hold after the run.
    #[must_use]
    pub fn with_expected_assertion(mut self, expected_assertion: Assertion) -> Self {
        self.expected_assertion = Some(expected_assertion);
        self
    }

    /// Set the expected interactions or hand-offs within the run.
    #[must_use]
    pub fn with_expected_interactions(
        mut self,
        expected_interactions: Vec<InteractionExpectation>,
    ) -> Self {
        self.expected_interactions = Some(expected_interactions);
        self
    }

    /// Set the prompt examples injected ahead of judge-backed evaluations.
    #[must_use]
    pub fn with_few_shot_examples(mut self, few_shot_examples: Vec<FewShotExample>) -> Self {
        self.few_shot_examples = few_shot_examples;
        self
    }

    /// Set the cost/budget governance constraints.
    #[must_use]
    pub fn with_budget(mut self, budget: BudgetConstraints) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Set the names of evaluators to run.
    #[must_use]
    pub fn with_evaluators(mut self, evaluators: Vec<String>) -> Self {
        self.evaluators = evaluators;
        self
    }

    /// Set arbitrary metadata for user-defined extensions and filtering.
    #[must_use]
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set multimodal data references consumed by multimodal evaluators.
    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<Attachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Set the stable case/session identifier.
    #[must_use]
    pub fn with_session_id(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set the expected environment-state snapshots keyed by name.
    #[must_use]
    pub fn with_expected_environment_state(
        mut self,
        expected_environment_state: Vec<EnvironmentState>,
    ) -> Self {
        self.expected_environment_state = Some(expected_environment_state);
        self
    }

    /// Set the expected semantic tool intent for the tool-parameter evaluator.
    #[must_use]
    pub fn with_expected_tool_intent(mut self, expected_tool_intent: ToolIntent) -> Self {
        self.expected_tool_intent = Some(expected_tool_intent);
        self
    }

    /// Set whether semantic tool-selection scoring is enabled for this case.
    #[must_use]
    pub fn with_semantic_tool_selection(mut self, semantic_tool_selection: bool) -> Self {
        self.semantic_tool_selection = semantic_tool_selection;
        self
    }

    /// Set the callback that produces the actual environment state after the agent completes.
    #[must_use]
    pub fn with_state_capture(mut self, state_capture: StateCapture) -> Self {
        self.state_capture = Some(state_capture);
        self
    }

    /// Canonical serializable projection used by deterministic ID and cache-key
    /// derivation.
    #[must_use]
    pub fn content_fingerprint(&self) -> CaseFingerprint {
        CaseFingerprint::from(self)
    }

    /// Narrow cache-key projection used by the runner's invocation cache
    /// (FR-038). See [`CacheFingerprint`] for why this is deliberately
    /// smaller than [`Self::content_fingerprint`].
    #[must_use]
    pub fn cache_fingerprint(&self) -> CacheFingerprint {
        CacheFingerprint::from(self)
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
#[non_exhaustive]
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

impl EvalSet {
    /// Create an eval set with no description.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, cases: Vec<EvalCase>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            cases,
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

// ─── Results ────────────────────────────────────────────────────────────────

/// Per-evaluator result for a single case.
#[non_exhaustive]
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

impl EvalMetricResult {
    /// Create a metric result with no additional details.
    #[must_use]
    pub fn new(evaluator_name: impl Into<String>, score: Score) -> Self {
        Self {
            evaluator_name: evaluator_name.into(),
            score,
            details: None,
        }
    }

    /// Set human-readable details about the scoring.
    #[must_use]
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// Result of evaluating a single case.
#[non_exhaustive]
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

impl EvalCaseResult {
    /// Create a case result with no per-evaluator metric results.
    #[must_use]
    pub fn new(case_id: impl Into<String>, invocation: Invocation, verdict: Verdict) -> Self {
        Self {
            case_id: case_id.into(),
            invocation,
            metric_results: Vec::new(),
            verdict,
        }
    }

    /// Set the per-evaluator metric results.
    #[must_use]
    pub fn with_metric_results(mut self, metric_results: Vec<EvalMetricResult>) -> Self {
        self.metric_results = metric_results;
        self
    }
}

/// Result of evaluating an entire eval set.
#[non_exhaustive]
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

impl EvalSetResult {
    /// Create a result for an entire eval set run.
    #[must_use]
    pub fn new(
        eval_set_id: impl Into<String>,
        case_results: Vec<EvalCaseResult>,
        summary: EvalSummary,
        timestamp: u64,
    ) -> Self {
        Self {
            eval_set_id: eval_set_id.into(),
            case_results,
            summary,
            timestamp,
        }
    }
}

/// Aggregated statistics for an eval set run.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

impl EvalSummary {
    /// Set the total number of cases evaluated.
    #[must_use]
    pub fn with_total_cases(mut self, total_cases: usize) -> Self {
        self.total_cases = total_cases;
        self
    }

    /// Set the number of cases that passed all metrics.
    #[must_use]
    pub fn with_passed(mut self, passed: usize) -> Self {
        self.passed = passed;
        self
    }

    /// Set the number of cases that failed at least one metric.
    #[must_use]
    pub fn with_failed(mut self, failed: usize) -> Self {
        self.failed = failed;
        self
    }

    /// Set the aggregated cost across all cases.
    #[must_use]
    pub fn with_total_cost(mut self, total_cost: Cost) -> Self {
        self.total_cost = total_cost;
        self
    }

    /// Set the aggregated token usage across all cases.
    #[must_use]
    pub fn with_total_usage(mut self, total_usage: Usage) -> Self {
        self.total_usage = total_usage;
        self
    }

    /// Set the total wall-clock duration across all cases.
    #[must_use]
    pub fn with_total_duration(mut self, total_duration: Duration) -> Self {
        self.total_duration = total_duration;
        self
    }
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
#[path = "types_validation_tests.rs"]
mod validation_tests;

#[cfg(test)]
#[path = "types_budget_policy_tests.rs"]
mod budget_policy_tests;

#[cfg(all(test, feature = "multimodal"))]
#[path = "types_attachment_url_tests.rs"]
mod attachment_url_tests;
