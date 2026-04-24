//! Extended evaluator families for advanced eval features.
//!
//! This module owns the shared [`JudgeEvaluatorConfig`] (T055) and the shared
//! [`dispatch_judge`] helper (T056) used by every judge-backed evaluator that
//! ships in spec 043.
//!
//! Concrete per-family evaluators land in follow-up tasks (T057–T086); this
//! file only provides the building blocks so those evaluators can be authored
//! independently without duplicating dispatch logic.

#![cfg(feature = "judge-core")]

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::aggregator::{Aggregator, Average};
use crate::judge::{JudgeError, JudgeRegistry, JudgeVerdict};
use crate::prompt::{JudgePromptTemplate, PromptContext, PromptError};
use crate::score::Score;
use crate::types::{AttachmentError, EvalMetricResult, MaterializedAttachment};
use crate::url_filter::UrlFilter;

// ─── US1d (deterministic) ───────────────────────────────────────────────────
//
// The module list below is owned by the US1d (deterministic / code / sandbox /
// multimodal) slice of spec 043. Keep additions inside this block so the US1c
// (judge-family) slice can land independent module declarations above without
// a mechanical merge conflict.

#[cfg(feature = "evaluator-simple")]
pub mod simple;
#[cfg(feature = "evaluator-structured")]
pub mod structured;

#[cfg(feature = "evaluator-code")]
pub mod code;

#[cfg(feature = "multimodal")]
pub mod multimodal;

// ─── Judge-family evaluator submodules (T057–T070) ──────────────────────────
//
// Each family file re-exports concrete evaluators gated behind its own cargo
// feature flag so consumers only pay for the families they opt into.
//
// Quality and Safety families land in US1c; RAG and Agent families are
// deferred to a US1c follow-up PR to keep the US1c diff reviewable.

#[cfg(feature = "evaluator-agent")]
pub mod agent;
#[cfg(feature = "evaluator-quality")]
pub mod quality;
#[cfg(feature = "evaluator-rag")]
pub mod rag;
#[cfg(feature = "evaluator-safety")]
pub mod safety;

/// Per-instance configuration shared by every judge-backed evaluator (T055).
///
/// A `None` template means "use the evaluator's built-in `_v0` template".
/// Builder methods on each concrete evaluator surface the individual knobs
/// (see data-model §3 "Base Evaluator extensions").
pub struct JudgeEvaluatorConfig {
    /// Prompt template override. When `None`, the evaluator uses its built-in
    /// `_v0` template from `PromptTemplateRegistry::builtin()`.
    pub template: Option<Arc<dyn JudgePromptTemplate>>,
    /// Few-shot examples injected ahead of the rendered prompt.
    pub few_shot_examples: Vec<crate::types::FewShotExample>,
    /// Optional system-prompt override applied ahead of the rendered prompt.
    pub system_prompt: Option<String>,
    /// Optional output-schema identifier used by structured-output evaluators.
    pub output_schema: Option<serde_json::Value>,
    /// Whether the judge should emit a reasoning field. Defaults to `true`.
    pub use_reasoning: bool,
    /// Optional feedback key forwarded to telemetry/reporter backends
    /// (e.g., LangSmith).
    pub feedback_key: Option<String>,
    /// Optional aggregator override. When `None`, callers use `Average`.
    pub aggregator: Option<Arc<dyn Aggregator>>,
    /// Required judge registry — the evaluator has no default judge model.
    pub judge_registry: Arc<JudgeRegistry>,
}

impl std::fmt::Debug for JudgeEvaluatorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JudgeEvaluatorConfig")
            .field("template", &self.template.as_ref().map(|t| t.version()))
            .field("few_shot_examples", &self.few_shot_examples.len())
            .field("system_prompt", &self.system_prompt.is_some())
            .field("output_schema", &self.output_schema.is_some())
            .field("use_reasoning", &self.use_reasoning)
            .field("feedback_key", &self.feedback_key)
            .field("aggregator", &self.aggregator.is_some())
            .field("judge_registry", &self.judge_registry)
            .finish()
    }
}

impl JudgeEvaluatorConfig {
    /// Construct a default config bound to the given judge registry (T055).
    ///
    /// Named `default_with` because [`Default`] can't take arguments; the
    /// config has no sensible default without a judge registry (FR-007/FR-010).
    #[must_use]
    pub fn default_with(judge_registry: Arc<JudgeRegistry>) -> Self {
        Self {
            template: None,
            few_shot_examples: Vec::new(),
            system_prompt: None,
            output_schema: None,
            use_reasoning: true,
            feedback_key: None,
            aggregator: None,
            judge_registry,
        }
    }

    /// Override the prompt template.
    #[must_use]
    pub fn with_prompt(mut self, template: Arc<dyn JudgePromptTemplate>) -> Self {
        self.template = Some(template);
        self
    }

    /// Backward-compatible alias for [`Self::with_prompt`].
    #[must_use]
    pub fn with_template(self, template: Arc<dyn JudgePromptTemplate>) -> Self {
        self.with_prompt(template)
    }

    /// Attach few-shot examples.
    #[must_use]
    pub fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
        self.few_shot_examples = examples;
        self
    }

    /// Override the system prompt.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Override the output schema.
    #[must_use]
    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Toggle the use-reasoning flag.
    #[must_use]
    pub const fn with_use_reasoning(mut self, flag: bool) -> Self {
        self.use_reasoning = flag;
        self
    }

    /// Override the feedback key.
    #[must_use]
    pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
        self.feedback_key = Some(key.into());
        self
    }

    /// Override the aggregator.
    #[must_use]
    pub fn with_aggregator(mut self, aggregator: Arc<dyn Aggregator>) -> Self {
        self.aggregator = Some(aggregator);
        self
    }

    /// Effective aggregator: the configured override or the default (`Average`).
    #[must_use]
    pub fn effective_aggregator(&self) -> Arc<dyn Aggregator> {
        self.aggregator.clone().unwrap_or_else(|| Arc::new(Average))
    }
}

/// Build the merged prompt context shared by every judge-backed evaluator.
///
/// The shared config can override the case's system prompt, prepend evaluator-
/// level few-shot examples, and expose additional per-dispatch metadata through
/// the `custom.*` namespace for custom templates.
#[must_use]
pub fn build_prompt_context(
    config: &JudgeEvaluatorConfig,
    case: &crate::types::EvalCase,
    invocation: &crate::types::Invocation,
) -> PromptContext {
    let mut case = case.clone();
    if let Some(system_prompt) = &config.system_prompt {
        case.system_prompt.clone_from(system_prompt);
    }
    let case_few_shot_examples = case.few_shot_examples.clone();

    let mut ctx = PromptContext::new(Arc::new(case), Arc::new(invocation.clone()));

    let mut few_shot_examples =
        Vec::with_capacity(config.few_shot_examples.len() + case_few_shot_examples.len());
    few_shot_examples.extend(config.few_shot_examples.iter().cloned());
    few_shot_examples.extend(case_few_shot_examples);
    if !few_shot_examples.is_empty() {
        ctx = ctx.with_few_shot_examples(few_shot_examples);
    }

    let mut custom = Map::new();
    custom.insert("use_reasoning".into(), Value::Bool(config.use_reasoning));
    if let Some(system_prompt) = &config.system_prompt {
        custom.insert("system_prompt".into(), Value::String(system_prompt.clone()));
    }
    if let Some(output_schema) = &config.output_schema {
        custom.insert("output_schema".into(), output_schema.clone());
    }
    if let Some(feedback_key) = &config.feedback_key {
        custom.insert("feedback_key".into(), Value::String(feedback_key.clone()));
    }
    if !custom.is_empty() {
        ctx = ctx.with_custom(custom.into_iter().collect());
    }

    ctx
}

/// Fluent builder surface exposed on every judge-backed evaluator (T105).
///
/// Complements the per-evaluator inherent `with_prompt` / `with_few_shot`
/// methods: implementors own a [`JudgeEvaluatorConfig`] and return `&mut`
/// access via [`Self::judge_config_mut`]. Default method implementations
/// route each customisation knob through the shared config so downstream
/// users can write generic code that customises any judge-backed evaluator:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use swink_agent_eval::{
///     CorrectnessEvaluator, JudgeEvaluatorBuilder, JudgeEvaluatorConfig,
///     JudgePromptTemplate,
/// };
///
/// fn customise<E: JudgeEvaluatorBuilder>(eval: E, t: Arc<dyn JudgePromptTemplate>) -> E {
///     eval.with_prompt(t).with_use_reasoning(false)
/// }
/// ```
///
/// The inherent methods on each evaluator struct shadow these defaults for
/// callers who don't need the generic trait surface — both paths route
/// through the same [`JudgeEvaluatorConfig`].
pub trait JudgeEvaluatorBuilder: Sized {
    /// Borrow the evaluator's underlying [`JudgeEvaluatorConfig`] for
    /// mutation by the default builder methods.
    fn judge_config_mut(&mut self) -> &mut JudgeEvaluatorConfig;

    /// Override the built-in prompt template.
    #[must_use]
    fn with_prompt(mut self, template: Arc<dyn JudgePromptTemplate>) -> Self {
        self.judge_config_mut().template = Some(template);
        self
    }

    /// Attach few-shot examples.
    #[must_use]
    fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
        self.judge_config_mut().few_shot_examples = examples;
        self
    }

    /// Override the system prompt applied ahead of the rendered prompt.
    #[must_use]
    fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.judge_config_mut().system_prompt = Some(prompt.into());
        self
    }

    /// Override the output-schema identifier used by structured-output
    /// evaluators.
    #[must_use]
    fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.judge_config_mut().output_schema = Some(schema);
        self
    }

    /// Toggle the `use_reasoning` flag.
    #[must_use]
    fn with_use_reasoning(mut self, flag: bool) -> Self {
        self.judge_config_mut().use_reasoning = flag;
        self
    }

    /// Override the feedback key forwarded to telemetry / reporter backends.
    #[must_use]
    fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
        self.judge_config_mut().feedback_key = Some(key.into());
        self
    }

    /// Override the aggregator applied to per-sample judge scores.
    #[must_use]
    fn with_aggregator(mut self, aggregator: Arc<dyn Aggregator>) -> Self {
        self.judge_config_mut().aggregator = Some(aggregator);
        self
    }
}

/// Convenience macro that implements [`JudgeEvaluatorBuilder`] for a struct
/// holding a `config: JudgeEvaluatorConfig` field.
#[macro_export]
macro_rules! impl_judge_evaluator_builder {
    ($ty:ty) => {
        impl $crate::evaluators::JudgeEvaluatorBuilder for $ty {
            fn judge_config_mut(&mut self) -> &mut $crate::evaluators::JudgeEvaluatorConfig {
                &mut self.config
            }
        }
    };
}

/// Structured detail record attached to [`EvalMetricResult::details`] (T056).
///
/// The existing `details: Option<String>` field retains its historical shape;
/// structured detail variants are serialized as JSON and surfaced through the
/// string. Helpers on this type render the canonical representation.
///
/// **Note**: this enum is the "Detail" surface referenced by FR-021 — the
/// `ScoreClamped` variant is authored here for the first time. PR body notes
/// that `EvalMetricResult::details` remains `Option<String>` for
/// serde-compat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Detail {
    /// Judge-returned score was outside `[0.0, 1.0]` and has been clamped.
    ScoreClamped { original: f64, clamped: f64 },
    /// The prompt template version used for this dispatch.
    PromptVersion { version: String },
    /// Feedback key consumed by downstream exporters such as LangSmith.
    FeedbackKey { key: String },
    /// Human-readable note carried verbatim.
    Note { text: String },
}

impl Detail {
    /// Render the detail as a single canonical JSON line.
    #[must_use]
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Helper that assembles structured detail lines into the free-form
/// `EvalMetricResult::details: Option<String>` field.
///
/// Each detail is serialized as one JSON line; this keeps the existing
/// `Option<String>` type shape while giving downstream consumers a
/// deterministic parse path.
#[derive(Debug, Default, Clone)]
pub struct DetailBuffer {
    entries: Vec<Detail>,
}

impl DetailBuffer {
    /// Empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a detail.
    pub fn push(&mut self, detail: Detail) {
        self.entries.push(detail);
    }

    /// Number of buffered detail entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the buffer holds no detail entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Borrow the buffered detail entries.
    #[must_use]
    pub fn entries(&self) -> &[Detail] {
        &self.entries
    }

    /// Render to the `Option<String>` shape of `EvalMetricResult::details`.
    #[must_use]
    pub fn into_details_string(self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let lines: Vec<String> = self.entries.iter().map(Detail::to_json_line).collect();
        Some(lines.join("\n"))
    }
}

/// Errors produced by [`dispatch_judge`] (T056).
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// Prompt render or template lookup failed.
    #[error("prompt: {0}")]
    Prompt(#[from] PromptError),
    /// Judge call failed.
    #[error("judge: {0}")]
    Judge(#[from] JudgeError),
    /// Attachment materialization failed.
    #[error("attachment: {0}")]
    Attachment(#[from] AttachmentError),
}

/// Structured errors surfaced by concrete evaluators in this module tree (T080–T082).
///
/// Evaluators fold these into [`EvalMetricResult`] via `Score::fail()` with the
/// error message copied into `details`; the type exists primarily so callers
/// (tests, reporters) can reason about the failure mode programmatically.
#[derive(Debug, thiserror::Error)]
pub enum EvaluatorError {
    /// The current platform cannot run this evaluator (e.g. Windows sandbox).
    #[error("evaluator unsupported on this platform: {reason}")]
    UnsupportedPlatform {
        /// Free-form explanation of the missing platform capability.
        reason: String,
    },
    /// A sandbox resource-limit cap was exceeded at evaluation time (T081).
    #[error("sandbox limit exceeded: {limit}")]
    SandboxLimitExceeded {
        /// Name of the exceeded limit (`wall_clock`, `cpu`, `memory`, `fds`, `network`).
        limit: String,
    },
    /// The evaluator could not carry out a deterministic operation.
    #[error("evaluator execution error: {reason}")]
    Execution {
        /// Human-readable explanation of the failure.
        reason: String,
    },
}

impl EvaluatorError {
    /// Convenience: render the error as the `details` string paired with `Score::fail()`.
    #[must_use]
    pub fn into_metric_details(self) -> String {
        self.to_string()
    }
}

/// Outcome of a [`dispatch_judge`] call (T056).
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    /// Clamped score in `[0.0, 1.0]`.
    pub score: Score,
    /// The judge's own pass/fail determination.
    pub pass: bool,
    /// Structured detail entries (prompt_version, optional ScoreClamped).
    pub details: DetailBuffer,
    /// Raw verdict for downstream evaluators that need label/reason.
    pub verdict: JudgeVerdict,
}

/// Shared judge-dispatch helper (T056).
///
/// Responsibilities:
///
/// * Render the supplied prompt template (or the config override) via
///   `MinijinjaTemplate`.
/// * Dispatch the rendered prompt through the config's [`JudgeRegistry`].
/// * Record `prompt_version` as a structured [`Detail::PromptVersion`] entry
///   (FR-011).
/// * Clamp the returned score to `[0.0, 1.0]` and, when the raw score was
///   outside that range, push a [`Detail::ScoreClamped { original, clamped }`]
///   entry (FR-021 extension).
///
/// `dispatch_judge` does NOT itself encode FR-020 `None`-return semantics —
/// that is the concrete evaluator's responsibility because only the evaluator
/// knows which case fields are its criterion. Callers typically short-circuit
/// before calling `dispatch_judge` and return `None` from their `evaluate`
/// implementation.
pub async fn dispatch_judge(
    config: &JudgeEvaluatorConfig,
    builtin_template: Arc<dyn JudgePromptTemplate>,
    context: &PromptContext,
) -> Result<DispatchOutcome, DispatchError> {
    let template = config.template.clone().unwrap_or(builtin_template);
    let prompt_version = template.version().to_string();

    // Per `build_prompt_context` the incoming `context` already has every
    // config-level customisation merged in (system prompt override,
    // config-level few-shot examples prepended to case-level ones, and the
    // `custom.*` namespace populated). Render verbatim.
    let rendered = template.render(context)?;
    let verdict = config.judge_registry.client().judge(&rendered).await?;

    let mut details = DetailBuffer::new();
    details.push(Detail::PromptVersion {
        version: prompt_version,
    });
    if let Some(feedback_key) = config.feedback_key.clone() {
        details.push(Detail::FeedbackKey { key: feedback_key });
    }

    let raw = verdict.score;
    let clamped = raw.clamp(0.0, 1.0);
    if (raw - clamped).abs() > f64::EPSILON {
        details.push(Detail::ScoreClamped {
            original: raw,
            clamped,
        });
    }

    let score = Score::new(clamped, 0.5);

    Ok(DispatchOutcome {
        score,
        pass: verdict.pass,
        details,
        verdict,
    })
}

/// Drive an async future to completion from the sync `Evaluator::evaluate`
/// entry point, regardless of the caller's Tokio runtime state.
///
/// Multi-thread runtime active → `block_in_place` + the ambient
/// `Handle::block_on` so the host runtime keeps scheduling other tasks.
/// Otherwise → build an ephemeral current-thread runtime and `block_on` it.
///
/// ## Known limitation
/// Running from *inside* a single-threaded Tokio runtime will panic with
/// "Cannot start a runtime from within a runtime". This is an inherent
/// Tokio constraint — use a multi-thread runtime or call from sync context.
pub fn block_on<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    use tokio::runtime::{Handle, RuntimeFlavor};

    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build ephemeral current-thread runtime");
    rt.block_on(future)
}

/// Materialize every attachment on the case through the shared attachment
/// pipeline (T086).
///
/// This is the narrow wiring point for FR-019: any judge-backed evaluator can
/// call [`materialize_case_attachments`] to get a `Vec<MaterializedAttachment>`
/// without re-implementing path resolution, base64 handling, or SSRF-filtered
/// URL fetching. The helper lives next to [`dispatch_judge`] so every caller
/// sees the same wiring.
///
/// Returns an empty vector when the case has no attachments; the
/// [`PromptContext`] passed downstream remains cheap to clone.
pub async fn materialize_case_attachments(
    case: &crate::types::EvalCase,
    eval_set_root: &Path,
    filter: &dyn UrlFilter,
) -> Result<Vec<MaterializedAttachment>, AttachmentError> {
    let mut out = Vec::with_capacity(case.attachments.len());
    for attachment in &case.attachments {
        let materialized = attachment.materialize(eval_set_root, filter).await?;
        out.push(materialized);
    }
    Ok(out)
}

/// Convenience: finalize a [`DispatchOutcome`] (plus optional judge reason)
/// into an [`EvalMetricResult`], preserving the `Option<String>` shape of
/// `details`.
#[must_use]
pub fn finish_metric_result(
    evaluator_name: impl Into<String>,
    outcome: DispatchOutcome,
) -> EvalMetricResult {
    let mut buffer = outcome.details;
    if let Some(reason) = outcome.verdict.reason.as_ref() {
        buffer.push(Detail::Note {
            text: reason.clone(),
        });
    }
    EvalMetricResult {
        evaluator_name: evaluator_name.into(),
        score: outcome.score,
        details: buffer.into_details_string(),
    }
}

/// Drive an async workload to completion from the sync [`crate::Evaluator::evaluate`]
/// entry point, regardless of the caller's Tokio runtime state.
///
/// Mirrors the pattern documented on
/// [`crate::SemanticToolSelectionEvaluator`] (spec 023): when a multi-thread
/// Tokio runtime is active we use `block_in_place` + the ambient
/// `Handle::block_on`; otherwise we build an ephemeral current-thread runtime.
/// Calling this from inside a single-threaded runtime will panic — an
/// inherent Tokio constraint, not a bug.
pub fn drive_judge_call<F, Fut, T>(make_future: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    use tokio::runtime::{Handle, RuntimeFlavor};

    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(make_future()));
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime for judge calls");
    rt.block_on(make_future())
}

/// Sync helper for judge-backed evaluators.
///
/// Locates the built-in template by version, dispatches via
/// [`dispatch_judge`], and finalises the [`EvalMetricResult`] via
/// [`finish_metric_result`]. Dispatch errors map to `Score::fail()` with the
/// error captured in `details` (FR-014 / FR-021).
///
/// Concrete evaluators are responsible for deciding whether their criterion
/// is set before calling this helper (FR-020). The helper itself never
/// returns `None`; once invoked, it always produces a metric result.
#[must_use]
pub fn evaluate_with_builtin(
    evaluator_name: &'static str,
    template_version: &'static str,
    config: &JudgeEvaluatorConfig,
    context: &PromptContext,
) -> EvalMetricResult {
    let builtin = crate::prompt::PromptTemplateRegistry::builtin()
        .get(template_version)
        .unwrap_or_else(|| panic!("built-in template {template_version} is missing"));

    let dispatch = drive_judge_call(|| async { dispatch_judge(config, builtin, context).await });

    match dispatch {
        Ok(outcome) => finish_metric_result(evaluator_name.to_string(), outcome),
        Err(err) => EvalMetricResult {
            evaluator_name: evaluator_name.to_string(),
            score: Score::fail(),
            details: Some(format!("{evaluator_name}: dispatch error — {err}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::{JudgeClient, JudgeRegistry};
    use crate::prompt::{MinijinjaTemplate, PromptContext, PromptFamily};
    use crate::types::{EvalCase, Invocation};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    use swink_agent::{Cost, ModelSpec, StopReason, Usage};

    struct FixedJudge {
        score: f64,
        reason: Option<String>,
        last_prompt: Mutex<Option<String>>,
    }

    #[async_trait]
    impl JudgeClient for FixedJudge {
        async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
            *self.last_prompt.lock().unwrap() = Some(prompt.to_string());
            Ok(JudgeVerdict {
                score: self.score,
                pass: (0.5..=1.0).contains(&self.score),
                reason: self.reason.clone(),
                label: None,
            })
        }
    }

    fn make_case() -> EvalCase {
        EvalCase {
            id: "case-1".into(),
            name: "Case One".into(),
            description: None,
            system_prompt: "answer".into(),
            user_messages: vec!["hi".into()],
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

    fn make_invocation() -> Invocation {
        Invocation {
            turns: vec![],
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: Duration::from_millis(1),
            final_response: Some("42".into()),
            stop_reason: StopReason::Stop,
            model: ModelSpec::new("test", "judge-target"),
        }
    }

    fn make_registry(score: f64) -> (Arc<JudgeRegistry>, Arc<FixedJudge>) {
        let judge = Arc::new(FixedJudge {
            score,
            reason: Some("ok".into()),
            last_prompt: Mutex::new(None),
        });
        let registry = JudgeRegistry::builder(judge.clone() as Arc<dyn JudgeClient>, "mock-model")
            .build()
            .expect("registry builds");
        (Arc::new(registry), judge)
    }

    fn make_template() -> Arc<dyn JudgePromptTemplate> {
        Arc::new(
            MinijinjaTemplate::new(
                "mock_v0",
                PromptFamily::Quality,
                "Case={{ case.name }} Actual={{ invocation.final_response }}",
            )
            .expect("template compiles"),
        )
    }

    fn make_context(case: &EvalCase, invocation: &Invocation) -> PromptContext {
        PromptContext::new(Arc::new(case.clone()), Arc::new(invocation.clone()))
    }

    #[tokio::test]
    async fn dispatch_records_prompt_version() {
        let (registry, _) = make_registry(0.8);
        let config = JudgeEvaluatorConfig::default_with(registry);
        let template = make_template();
        let case = make_case();
        let invocation = make_invocation();
        let ctx = make_context(&case, &invocation);

        let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

        assert!(
            outcome
                .details
                .entries()
                .iter()
                .any(|d| matches!(d, Detail::PromptVersion { version } if version == "mock_v0"))
        );
        assert!(
            !outcome
                .details
                .entries()
                .iter()
                .any(|d| matches!(d, Detail::ScoreClamped { .. }))
        );
        assert!((outcome.score.value - 0.8).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn dispatch_clamps_out_of_range_scores() {
        let (registry, _) = make_registry(1.3);
        let config = JudgeEvaluatorConfig::default_with(registry);
        let template = make_template();
        let case = make_case();
        let invocation = make_invocation();
        let ctx = make_context(&case, &invocation);

        let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

        // Score clamped to 1.0.
        assert!((outcome.score.value - 1.0).abs() < f64::EPSILON);
        // ScoreClamped detail present with original 1.3 and clamped 1.0.
        let clamp = outcome
            .details
            .entries()
            .iter()
            .find_map(|d| match d {
                Detail::ScoreClamped { original, clamped } => Some((*original, *clamped)),
                _ => None,
            })
            .expect("ScoreClamped detail present");
        assert!((clamp.0 - 1.3).abs() < f64::EPSILON);
        assert!((clamp.1 - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn dispatch_clamps_negative_scores() {
        let (registry, _) = make_registry(-0.2);
        let config = JudgeEvaluatorConfig::default_with(registry);
        let template = make_template();
        let case = make_case();
        let invocation = make_invocation();
        let ctx = make_context(&case, &invocation);

        let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

        assert!((outcome.score.value - 0.0).abs() < f64::EPSILON);
        assert!(
            outcome
                .details
                .entries()
                .iter()
                .any(|d| matches!(d, Detail::ScoreClamped { .. }))
        );
    }

    #[tokio::test]
    async fn dispatch_uses_config_override_when_present() {
        let (registry, judge) = make_registry(0.5);
        let custom: Arc<dyn JudgePromptTemplate> = Arc::new(
            MinijinjaTemplate::new(
                "mock_v1",
                PromptFamily::Quality,
                "override Case={{ case.id }}",
            )
            .unwrap(),
        );
        let config = JudgeEvaluatorConfig::default_with(registry).with_template(custom);
        let builtin = make_template(); // would render "mock_v0" but override wins
        let case = make_case();
        let invocation = make_invocation();
        let ctx = make_context(&case, &invocation);

        let outcome = dispatch_judge(&config, builtin, &ctx).await.unwrap();

        // The recorded prompt_version must come from the override, not the builtin.
        let recorded_version = outcome
            .details
            .entries()
            .iter()
            .find_map(|d| match d {
                Detail::PromptVersion { version } => Some(version.clone()),
                _ => None,
            })
            .expect("prompt version recorded");
        assert_eq!(recorded_version, "mock_v1");

        // The judge must have seen the override prompt.
        let seen = judge.last_prompt.lock().unwrap().clone().unwrap();
        assert!(seen.starts_with("override Case=case-1"));
    }

    #[test]
    fn detail_buffer_round_trips_through_details_string() {
        let mut buffer = DetailBuffer::new();
        buffer.push(Detail::PromptVersion {
            version: "v0".into(),
        });
        buffer.push(Detail::ScoreClamped {
            original: 1.2,
            clamped: 1.0,
        });
        let rendered = buffer.into_details_string().expect("some");
        // Two JSON lines, parseable.
        let parsed: Vec<Detail> = rendered
            .lines()
            .map(|line| serde_json::from_str::<Detail>(line).unwrap())
            .collect();
        assert_eq!(parsed.len(), 2);
        assert!(matches!(parsed[0], Detail::PromptVersion { .. }));
        assert!(matches!(parsed[1], Detail::ScoreClamped { .. }));
    }

    #[test]
    fn empty_detail_buffer_renders_none() {
        assert!(DetailBuffer::new().into_details_string().is_none());
    }

    #[test]
    fn config_builder_surface() {
        let (registry, _) = make_registry(0.5);
        let config = JudgeEvaluatorConfig::default_with(registry)
            .with_system_prompt("sys")
            .with_use_reasoning(false)
            .with_feedback_key("fb");
        assert_eq!(config.system_prompt.as_deref(), Some("sys"));
        assert!(!config.use_reasoning);
        assert_eq!(config.feedback_key.as_deref(), Some("fb"));
    }

    #[tokio::test]
    async fn dispatch_records_feedback_key_when_configured() {
        let (registry, _) = make_registry(0.8);
        let config =
            JudgeEvaluatorConfig::default_with(registry).with_feedback_key("quality.score");
        let template = make_template();
        let case = make_case();
        let invocation = make_invocation();
        let ctx = make_context(&case, &invocation);

        let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

        assert!(
            outcome
                .details
                .entries()
                .iter()
                .any(|d| matches!(d, Detail::FeedbackKey { key } if key == "quality.score"))
        );
    }
}
