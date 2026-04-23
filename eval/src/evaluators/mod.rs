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

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::aggregator::{Aggregator, Average};
use crate::judge::{JudgeError, JudgeRegistry, JudgeVerdict};
use crate::prompt::{JudgePromptTemplate, PromptContext, PromptError};
use crate::score::Score;
use crate::types::EvalMetricResult;

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
    pub fn with_template(mut self, template: Arc<dyn JudgePromptTemplate>) -> Self {
        self.template = Some(template);
        self
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

    let rendered = template.render(context)?;
    let verdict = config.judge_registry.client().judge(&rendered).await?;

    let mut details = DetailBuffer::new();
    details.push(Detail::PromptVersion {
        version: prompt_version,
    });

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
}
