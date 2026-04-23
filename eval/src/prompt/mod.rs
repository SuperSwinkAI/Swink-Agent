//! Prompt templates and rendering infrastructure for judge-backed evaluators.

pub mod templates;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use minijinja::{Environment, ErrorKind, UndefinedBehavior};
use serde::Serialize;
use thiserror::Error;

use crate::types::{EvalCase, FewShotExample, Invocation};

/// Stable list of every built-in template version registered by [`PromptTemplateRegistry::builtin`].
///
/// Exposed so downstream tests (and users) can assert the built-in surface
/// without hard-coding the list in multiple places (T054).
pub const BUILTIN_TEMPLATE_VERSIONS: &[&str] = &[
    // Quality family (T049)
    "helpfulness_v0",
    "correctness_v0",
    "conciseness_v0",
    "coherence_v0",
    "response_relevance_v0",
    "hallucination_v0",
    "faithfulness_v0",
    "plan_adherence_v0",
    "laziness_v0",
    "goal_success_rate_v0",
    // Safety family (T050)
    "harmfulness_v0",
    "toxicity_v0",
    "fairness_v0",
    "pii_leakage_v0",
    "prompt_injection_v0",
    "code_injection_v0",
    // RAG family (T051)
    "rag_groundedness_v0",
    "rag_retrieval_relevance_v0",
    "rag_helpfulness_v0",
    // Agent family (T052)
    "trajectory_accuracy_v0",
    "trajectory_accuracy_with_ref_v0",
    "task_completion_v0",
    "user_satisfaction_v0",
    "agent_tone_v0",
    "knowledge_retention_v0",
    "language_detection_v0",
    "perceived_error_v0",
    "interactions_v0",
    // Code family (T053)
    "code_llm_judge_v0",
    // Multimodal family (T053)
    "image_safety_v0",
];

/// Versioned prompt template consumed by judge-backed evaluators.
pub trait JudgePromptTemplate: Send + Sync {
    /// Stable version identifier, for example `correctness_v0`.
    fn version(&self) -> &str;

    /// Render the prompt for a single evaluator dispatch.
    fn render(&self, ctx: &PromptContext) -> Result<String, PromptError>;

    /// Evaluator family this template belongs to.
    fn family(&self) -> PromptFamily;
}

/// Prompt families with judge-backed templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptFamily {
    Quality,
    Safety,
    Rag,
    Agent,
    Structured,
    Code,
    Multimodal,
}

/// Data made available to prompt templates.
#[derive(Debug, Clone, Serialize)]
pub struct PromptContext {
    pub case: Arc<EvalCase>,
    pub invocation: Arc<Invocation>,
    pub few_shot_examples: Vec<FewShotExample>,
    pub custom: HashMap<String, serde_json::Value>,
}

impl PromptContext {
    /// Construct a context with no few-shot examples or custom variables.
    pub fn new(case: Arc<EvalCase>, invocation: Arc<Invocation>) -> Self {
        Self {
            case,
            invocation,
            few_shot_examples: Vec::new(),
            custom: HashMap::new(),
        }
    }

    /// Add few-shot examples to the render context.
    #[must_use]
    pub fn with_few_shot_examples(mut self, examples: Vec<FewShotExample>) -> Self {
        self.few_shot_examples = examples;
        self
    }

    /// Add custom template variables under the `custom` namespace.
    #[must_use]
    pub fn with_custom(mut self, custom: HashMap<String, serde_json::Value>) -> Self {
        self.custom = custom;
        self
    }
}

/// Prompt-template construction and rendering errors.
#[derive(Debug, Error)]
pub enum PromptError {
    #[error("missing prompt variable: {name}")]
    MissingVariable { name: String },
    #[error("duplicate prompt template version: {version}")]
    DuplicateTemplate { version: String },
    #[error("prompt render error: {0}")]
    RenderError(String),
}

/// Registry keyed by template version.
#[derive(Clone, Default)]
pub struct PromptTemplateRegistry {
    templates: HashMap<String, Arc<dyn JudgePromptTemplate>>,
}

impl PromptTemplateRegistry {
    /// Registry seeded with every built-in template authored under
    /// `prompt::templates::{quality,safety,rag,agent,code,multimodal}` (T054).
    ///
    /// Built-in sources are validated at development time; any registration
    /// failure here is a programming error and surfaces as a panic rather
    /// than being silently swallowed.
    pub fn builtin() -> Self {
        let mut registry = Self::default();
        for family in [
            templates::quality::builtins,
            templates::safety::builtins,
            templates::rag::builtins,
            templates::agent::builtins,
            templates::code::builtins,
            templates::multimodal::builtins,
        ] {
            let entries = family().expect("built-in prompt template failed to compile");
            for template in entries {
                registry
                    .register(template)
                    .expect("duplicate built-in prompt template version");
            }
        }
        registry
    }

    /// Iterate over every registered template version identifier.
    pub fn versions(&self) -> impl Iterator<Item = &str> {
        self.templates.keys().map(String::as_str)
    }

    /// Current number of registered templates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether the registry currently holds zero templates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Look up a template by version identifier.
    pub fn get(&self, version: &str) -> Option<Arc<dyn JudgePromptTemplate>> {
        self.templates.get(version).cloned()
    }

    /// Register a template, rejecting duplicate version identifiers.
    pub fn register(&mut self, template: Arc<dyn JudgePromptTemplate>) -> Result<(), PromptError> {
        let version = template.version().to_string();
        if self.templates.contains_key(&version) {
            return Err(PromptError::DuplicateTemplate { version });
        }
        self.templates.insert(version, template);
        Ok(())
    }
}

/// MiniJinja-backed implementation of [`JudgePromptTemplate`].
#[derive(Debug, Clone)]
pub struct MinijinjaTemplate {
    version: String,
    family: PromptFamily,
    source: String,
    undeclared: HashSet<String>,
}

impl MinijinjaTemplate {
    /// Compile and validate a MiniJinja prompt template.
    ///
    /// Templates may reference the root variables `case`, `invocation`,
    /// `few_shot_examples`, and `custom`. Any other root variable is rejected
    /// at construction time.
    pub fn new(
        version: impl Into<String>,
        family: PromptFamily,
        source: impl Into<String>,
    ) -> Result<Self, PromptError> {
        let version = version.into();
        let source = source.into();
        let mut env = strict_environment();
        env.add_template_owned(version.clone(), source.clone())
            .map_err(|err| PromptError::RenderError(err.to_string()))?;
        let template = env
            .get_template(&version)
            .map_err(|err| PromptError::RenderError(err.to_string()))?;
        let undeclared = template.undeclared_variables(false);
        if let Some(name) = undeclared
            .iter()
            .find(|name| !ALLOWED_ROOT_VARIABLES.contains(&name.as_str()))
        {
            return Err(PromptError::MissingVariable { name: name.clone() });
        }

        Ok(Self {
            version,
            family,
            source,
            undeclared,
        })
    }

    /// Root variables discovered while compiling the template.
    pub fn variables(&self) -> &HashSet<String> {
        &self.undeclared
    }
}

impl JudgePromptTemplate for MinijinjaTemplate {
    fn version(&self) -> &str {
        &self.version
    }

    fn render(&self, ctx: &PromptContext) -> Result<String, PromptError> {
        let mut env = strict_environment();
        env.add_template_owned(self.version.clone(), self.source.clone())
            .map_err(|err| render_error(&err))?;
        let template = env
            .get_template(&self.version)
            .map_err(|err| render_error(&err))?;
        template.render(ctx).map_err(|err| render_error(&err))
    }

    fn family(&self) -> PromptFamily {
        self.family
    }
}

const ALLOWED_ROOT_VARIABLES: &[&str] = &["case", "invocation", "few_shot_examples", "custom"];

fn strict_environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env
}

fn render_error(err: &minijinja::Error) -> PromptError {
    if err.kind() == ErrorKind::UndefinedError {
        return PromptError::MissingVariable {
            name: err.to_string(),
        };
    }
    PromptError::RenderError(err.to_string())
}
