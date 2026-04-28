# Public API Contract: swink-agent-evolve

## Re-exports from lib.rs

```rust
// Core runner
pub use runner::EvolutionRunner;

// Configuration
pub use config::{OptimizationTarget, OptimizationConfig, CycleBudget, PromptSection};

// Diagnosis
pub use diagnose::{Diagnoser, WeakPoint, TargetComponent, CaseFailure};

// Mutation
pub use mutate::{MutationStrategy, MutationContext, MutationError, Candidate};
pub use strategies::{LlmGuided, TemplateBased, Ablation};

// Evaluation & gating
pub use evaluate::CandidateResult;
pub use gate::{AcceptanceGate, AcceptanceResult, AcceptanceVerdict};

// Results
pub use types::{BaselineSnapshot, CycleResult, CycleStatus, ManifestEntry};
```

## EvolutionRunner

```rust
pub struct EvolutionRunner { /* private */ }

impl EvolutionRunner {
    /// Create a runner with the given target and config.
    pub fn new(
        target: OptimizationTarget,
        config: OptimizationConfig,
        factory: Arc<dyn AgentFactory>,
        judge: Option<Arc<dyn JudgeClient>>,
    ) -> Self;

    /// Run the eval suite against the current target configuration.
    pub async fn baseline(&self) -> Result<BaselineSnapshot, EvolveError>;

    /// Execute one full optimization cycle: baseline → diagnose → mutate → evaluate → gate → persist.
    pub async fn run_cycle(&mut self) -> Result<CycleResult, EvolveError>;

    /// Run up to `max` cycles, stopping early if no improvements found.
    pub async fn run_cycles(&mut self, max: usize) -> Result<Vec<CycleResult>, EvolveError>;
}
```

## OptimizationTarget

```rust
pub struct OptimizationTarget { /* private */ }

impl OptimizationTarget {
    /// Create from a system prompt and tool schemas. Sections auto-parsed from markdown headers.
    pub fn new(system_prompt: impl Into<String>, tool_schemas: Vec<ToolSchema>) -> Self;

    /// Override the section delimiter regex (default: markdown `## ` headers).
    pub fn with_section_delimiter(self, delimiter: Regex) -> Self;

    /// Access parsed sections.
    pub fn sections(&self) -> &[PromptSection];

    /// Access the full system prompt.
    pub fn system_prompt(&self) -> &str;

    /// Access tool schemas.
    pub fn tool_schemas(&self) -> &[ToolSchema];

    /// Produce a new target with a section replaced.
    pub fn with_replaced_section(&self, index: usize, content: &str) -> Self;

    /// Produce a new target with a tool schema replaced.
    pub fn with_replaced_tool(&self, tool_name: &str, schema: ToolSchema) -> Self;
}
```

## OptimizationConfig

```rust
pub struct OptimizationConfig { /* private */ }

impl OptimizationConfig {
    pub fn new(eval_set: EvalSet, output_root: impl Into<PathBuf>) -> Self;
    pub fn with_strategies(self, strategies: Vec<Box<dyn MutationStrategy>>) -> Self;
    pub fn with_acceptance_threshold(self, threshold: f64) -> Self;
    pub fn with_budget(self, budget: CycleBudget) -> Self;
    pub fn with_parallelism(self, parallelism: usize) -> Self;
    pub fn with_seed(self, seed: u64) -> Self;
    pub fn with_max_weak_points(self, max: usize) -> Self;
    pub fn with_max_candidates_per_strategy(self, max: usize) -> Self;
}
```

## MutationStrategy

```rust
pub trait MutationStrategy: Send + Sync {
    /// Human-readable strategy name (e.g., "llm-guided", "template", "ablation").
    fn name(&self) -> &str;

    /// Generate candidates for the given target text and context.
    fn mutate(&self, target: &str, context: &MutationContext) -> Result<Vec<Candidate>, MutationError>;
}
```

## Built-in Strategies

```rust
pub struct LlmGuided { /* private */ }

impl LlmGuided {
    /// Create with a judge client for LLM-powered rewrites.
    pub fn new(judge: Arc<dyn JudgeClient>) -> Self;
}

pub struct TemplateBased { /* private */ }

impl TemplateBased {
    /// Create with default transformation library.
    pub fn new() -> Self;

    /// Add a custom find-replace template.
    pub fn with_template(self, find: &str, replace: &str) -> Result<Self, regex::Error>;
}

pub struct Ablation { /* private */ }

impl Ablation {
    /// Create with default ablation rules (remove section, simplify to first sentence).
    pub fn new() -> Self;
}
```

## AcceptanceGate

```rust
pub struct AcceptanceGate { /* private */ }

impl AcceptanceGate {
    pub fn new(threshold: f64) -> Self;

    /// Evaluate candidates against baseline. Returns ranked results.
    pub fn evaluate(
        &self,
        baseline: &BaselineSnapshot,
        candidates: &[CandidateResult],
    ) -> AcceptanceResult;
}
```

## CycleBudget

```rust
pub struct CycleBudget { /* private */ }

impl CycleBudget {
    pub fn new(max_cost: Cost) -> Self;
    pub fn record(&self, cost: Cost);
    pub fn remaining(&self) -> Cost;
    pub fn is_exhausted(&self) -> bool;
}
```
