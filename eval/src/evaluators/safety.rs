//! Safety-family judge-backed evaluators (T062, T063, T064).
//!
//! Every evaluator in this module produces a binary pass/fail score and
//! explicitly sets the default aggregator to [`crate::AllPass`] in its
//! constructor (data-model §3 safety family).
//!
//! The [`HarmfulnessEvaluator`] and [`ToxicityEvaluator`] ship distinct
//! rubrics (spec 043 clarification Q1):
//!
//! * `HarmfulnessEvaluator` is the broad rubric — self-harm, weapons,
//!   illegal-activity, large-scale societal risk.
//! * `ToxicityEvaluator` is the narrow rubric — hate speech, harassment,
//!   slurs, directed insults.

#![forbid(unsafe_code)]
#![cfg(feature = "evaluator-safety")]

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::aggregator::AllPass;
use crate::evaluator::Evaluator;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

use super::{JudgeEvaluatorConfig, build_prompt_context, evaluate_with_builtin};

fn has_final_response(_case: &EvalCase, invocation: &Invocation) -> bool {
    invocation
        .final_response
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

fn has_user_prompt(case: &EvalCase, _invocation: &Invocation) -> bool {
    !case.user_messages.is_empty()
}

/// Set the AllPass aggregator on a config unless the caller already picked one.
fn with_safety_default(config: JudgeEvaluatorConfig) -> JudgeEvaluatorConfig {
    if config.aggregator.is_some() {
        config
    } else {
        config.with_aggregator(Arc::new(AllPass))
    }
}

macro_rules! safety_evaluator {
    (
        $(#[$meta:meta])*
        $name:ident, $eval_name:literal, $template:literal, $criterion:expr
    ) => {
        $(#[$meta])*
        pub struct $name {
            config: JudgeEvaluatorConfig,
        }

        impl $name {
            /// Construct with the supplied judge config, explicitly
            /// defaulting the aggregator to [`crate::AllPass`] when no
            /// override is configured (data-model §3 safety family).
            #[must_use]
            pub fn new(config: JudgeEvaluatorConfig) -> Self {
                Self {
                    config: with_safety_default(config),
                }
            }

            /// Override the prompt template used by this evaluator.
            #[must_use]
            pub fn with_prompt(mut self, template: Arc<dyn crate::prompt::JudgePromptTemplate>) -> Self {
                self.config = self.config.with_prompt(template);
                self
            }

            /// Attach evaluator-level few-shot examples that render before any
            /// case-level examples.
            #[must_use]
            pub fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
                self.config = self.config.with_few_shot(examples);
                self
            }

            /// Override the system prompt visible to the template render.
            #[must_use]
            pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
                self.config = self.config.with_system_prompt(prompt);
                self
            }

            /// Attach an output schema for custom prompt templates.
            #[must_use]
            pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
                self.config = self.config.with_output_schema(schema);
                self
            }

            /// Toggle judge reasoning capture.
            #[must_use]
            pub fn with_use_reasoning(mut self, flag: bool) -> Self {
                self.config = self.config.with_use_reasoning(flag);
                self
            }

            /// Override the feedback key used by downstream exporters.
            #[must_use]
            pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
                self.config = self.config.with_feedback_key(key);
                self
            }

            /// Borrow the underlying config.
            #[must_use]
            pub const fn config(&self) -> &JudgeEvaluatorConfig {
                &self.config
            }
        }

        impl $crate::evaluators::JudgeEvaluatorBuilder for $name {
            fn judge_config_mut(&mut self) -> &mut JudgeEvaluatorConfig {
                &mut self.config
            }
        }

        impl Evaluator for $name {
            fn name(&self) -> &'static str {
                $eval_name
            }

            fn evaluate(
                &self,
                case: &EvalCase,
                invocation: &Invocation,
            ) -> Option<EvalMetricResult> {
                let criterion: fn(&EvalCase, &Invocation) -> bool = $criterion;
                if !criterion(case, invocation) {
                    return None;
                }

                Some(evaluate_with_builtin(
                    $eval_name,
                    $template,
                    &self.config,
                    &build_prompt_context(&self.config, case, invocation),
                ))
            }
        }
    };
}

safety_evaluator! {
    /// Broad-spectrum harmful-content check (prompt: `harmfulness_v0`).
    HarmfulnessEvaluator,
    "harmfulness",
    "harmfulness_v0",
    has_final_response
}

safety_evaluator! {
    /// Narrow rubric targeting hate speech, harassment, and slurs (prompt:
    /// `toxicity_v0`). Distinct from [`HarmfulnessEvaluator`].
    ToxicityEvaluator,
    "toxicity",
    "toxicity_v0",
    has_final_response
}

safety_evaluator! {
    /// Fairness / disparate-treatment check (prompt: `fairness_v0`).
    FairnessEvaluator,
    "fairness",
    "fairness_v0",
    has_final_response
}

safety_evaluator! {
    /// Prompt-injection detector evaluated against the user prompt (prompt:
    /// `prompt_injection_v0`). Criterion: the case must include at least one
    /// user message.
    PromptInjectionEvaluator,
    "prompt_injection",
    "prompt_injection_v0",
    has_user_prompt
}

safety_evaluator! {
    /// Code-injection detector evaluated against the user prompt (prompt:
    /// `code_injection_v0`).
    CodeInjectionEvaluator,
    "code_injection",
    "code_injection_v0",
    has_user_prompt
}

/// PII categories recognised by [`PIILeakageEvaluator`].
///
/// `Other(String)` lets consumers add custom entity classes (e.g.,
/// `"MedicalRecordNumber"`) without forking the evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PIIClass {
    Email,
    Phone,
    /// Social Security Number (US).
    Ssn,
    CreditCard,
    IpAddress,
    ApiKey,
    PersonalName,
    Address,
    /// Free-form class label; callers supply the class name.
    Other(String),
}

impl PIIClass {
    /// Canonical name used in prompt rendering and telemetry.
    #[must_use]
    pub fn canonical_name(&self) -> String {
        match self {
            Self::Email => "email".into(),
            Self::Phone => "phone".into(),
            Self::Ssn => "ssn".into(),
            Self::CreditCard => "credit_card".into(),
            Self::IpAddress => "ip_address".into(),
            Self::ApiKey => "api_key".into(),
            Self::PersonalName => "personal_name".into(),
            Self::Address => "address".into(),
            Self::Other(name) => name.clone(),
        }
    }

    /// All built-in PII classes in stable registration order. `Other` is
    /// intentionally excluded — it is a user-supplied extension.
    #[must_use]
    pub fn all_builtin() -> Vec<Self> {
        vec![
            Self::Email,
            Self::Phone,
            Self::Ssn,
            Self::CreditCard,
            Self::IpAddress,
            Self::ApiKey,
            Self::PersonalName,
            Self::Address,
        ]
    }
}

/// PII-leakage detector (prompt: `pii_leakage_v0`).
///
/// Consumers pick which [`PIIClass`] variants participate in detection.
/// The default constructor enables every built-in class.
pub struct PIILeakageEvaluator {
    config: JudgeEvaluatorConfig,
    entity_classes: Vec<PIIClass>,
}

impl PIILeakageEvaluator {
    /// Construct with every built-in PII class enabled.
    #[must_use]
    pub fn new(config: JudgeEvaluatorConfig) -> Self {
        Self {
            config: with_safety_default(config),
            entity_classes: PIIClass::all_builtin(),
        }
    }

    /// Construct with an explicit subset of classes. An empty `entity_classes`
    /// is accepted but will always return a passing score (the evaluator has
    /// nothing to look for).
    #[must_use]
    pub fn with_classes(config: JudgeEvaluatorConfig, entity_classes: Vec<PIIClass>) -> Self {
        Self {
            config: with_safety_default(config),
            entity_classes,
        }
    }

    /// Override the prompt template used by this evaluator.
    #[must_use]
    pub fn with_prompt(mut self, template: Arc<dyn crate::prompt::JudgePromptTemplate>) -> Self {
        self.config = self.config.with_prompt(template);
        self
    }

    /// Attach evaluator-level few-shot examples that render before any
    /// case-level examples.
    #[must_use]
    pub fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
        self.config = self.config.with_few_shot(examples);
        self
    }

    /// Override the system prompt visible to the template render.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config = self.config.with_system_prompt(prompt);
        self
    }

    /// Attach an output schema for custom prompt templates.
    #[must_use]
    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.config = self.config.with_output_schema(schema);
        self
    }

    /// Toggle judge reasoning capture.
    #[must_use]
    pub fn with_use_reasoning(mut self, flag: bool) -> Self {
        self.config = self.config.with_use_reasoning(flag);
        self
    }

    /// Override the feedback key used by downstream exporters.
    #[must_use]
    pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
        self.config = self.config.with_feedback_key(key);
        self
    }

    /// Borrow the configured class list.
    #[must_use]
    pub fn entity_classes(&self) -> &[PIIClass] {
        &self.entity_classes
    }

    /// Borrow the underlying config.
    #[must_use]
    pub const fn config(&self) -> &JudgeEvaluatorConfig {
        &self.config
    }
}

impl crate::evaluators::JudgeEvaluatorBuilder for PIILeakageEvaluator {
    fn judge_config_mut(&mut self) -> &mut JudgeEvaluatorConfig {
        &mut self.config
    }
}

impl Evaluator for PIILeakageEvaluator {
    fn name(&self) -> &'static str {
        "pii_leakage"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        if !has_final_response(case, invocation) {
            return None;
        }

        // Render the active class list into the prompt's custom namespace so
        // the `pii_leakage_v0` template can surface it if consumers override
        // the rubric. Built-in template ignores the custom field today.
        let mut ctx = build_prompt_context(&self.config, case, invocation);
        let classes: Vec<serde_json::Value> = self
            .entity_classes
            .iter()
            .map(|c| serde_json::Value::String(c.canonical_name()))
            .collect();
        ctx = ctx.with_custom(std::collections::HashMap::from([(
            "pii_entity_classes".to_string(),
            serde_json::Value::Array(classes),
        )]));

        Some(evaluate_with_builtin(
            "pii_leakage",
            "pii_leakage_v0",
            &self.config,
            &ctx,
        ))
    }
}
