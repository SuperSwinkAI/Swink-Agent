# Public API Contracts: Evals: Advanced Features

**Feature**: 043-evals-adv-features | **Date**: 2026-04-21

Authoritative list of public items the two crates expose once 043 ships. Items are grouped by crate and module. Every listed item appears in `pub` re-exports from the crate root.

---

## Crate: `swink-agent-eval` (extended from 023/024)

### Module `swink_agent_eval::prompt`

```rust
pub trait JudgePromptTemplate: Send + Sync {
    fn version(&self) -> &str;
    fn family(&self) -> PromptFamily;
    fn render(&self, ctx: &PromptContext) -> Result<String, PromptError>;
}

pub struct PromptTemplateRegistry { /* ... */ }
impl PromptTemplateRegistry {
    pub fn builtin() -> Self;
    pub fn get(&self, version: &str) -> Option<Arc<dyn JudgePromptTemplate>>;
    pub fn register(&mut self, template: Arc<dyn JudgePromptTemplate>) -> Result<(), PromptError>;
}

pub struct PromptContext { /* fields: case, invocation, few_shot_examples, custom */ }

pub enum PromptFamily {
    Quality, Safety, RAG, Agent, Structured, Code, Multimodal,
}
// No `Simple` — simple-family evaluators are deterministic and register no templates
// (matches FR-009's 7-family list).

pub enum PromptError {
    MissingVariable { name: String },
    RenderError(String),
    DuplicateVersion(String),
}

pub struct FewShotExample {
    pub input: String,
    pub expected: String,
    pub reasoning: Option<String>,
}
```

### Module `swink_agent_eval::judge` *(feature `judge-core`)*

```rust
pub struct JudgeRegistry { /* ... */ }
pub struct JudgeRegistryBuilder { /* ... */ }
impl JudgeRegistry {
    pub fn builder(client: Arc<dyn JudgeClient>, model_id: impl Into<String>) -> JudgeRegistryBuilder;
}
impl JudgeRegistryBuilder {
    pub fn retry_policy(self, p: RetryPolicy) -> Self;
    pub fn batch_size(self, n: usize) -> Self;                 // [1, 128]
    pub fn cache(self, c: Arc<JudgeCache>) -> Self;
    pub fn url_filter(self, f: Arc<dyn UrlFilter>) -> Self;
    pub fn build(self) -> Result<JudgeRegistry, JudgeRegistryError>;
}

pub struct RetryPolicy { pub max_attempts: u32, pub max_delay: Duration, pub jitter: bool }
impl Default for RetryPolicy { /* 6 attempts, 4 min, jitter=true */ }

pub struct JudgeCache { /* ... */ }
impl JudgeCache {
    pub fn in_memory(capacity: usize) -> Arc<Self>;
    pub fn with_disk(capacity: usize, path: PathBuf) -> Arc<Self>;
    pub fn get(&self, key: &CacheKey) -> Option<JudgeVerdict>;
    pub fn put(&self, key: CacheKey, verdict: JudgeVerdict);
}

pub struct CacheKey { pub prompt_hash: [u8; 32], pub model_id: String }

// UrlFilter actually lives at `swink_agent_eval::url_filter` — always-on top-level module,
// not `swink_agent_eval::judge::url_filter` — so attachment materialization can use it
// without enabling the `judge-core` feature. Re-exported from both locations for ergonomic use.
pub trait UrlFilter: Send + Sync {
    fn check(&self, url: &Url) -> Result<(), AttachmentError>;
}
pub struct DefaultUrlFilter;   // denies RFC 1918, loopback, metadata endpoints; requires HTTPS
```

### Module `swink_agent_eval::aggregator`

```rust
pub trait Aggregator: Send + Sync {
    fn aggregate(&self, samples: &[Score]) -> Score;
}
pub struct Average;    // default
pub struct AllPass;
pub struct AnyPass;
pub struct Weighted { pub weights: Vec<f64> }
```

### Module `swink_agent_eval::evaluators` *(each family behind its own feature)*

```rust
// feature evaluator-quality
pub struct HelpfulnessEvaluator { /* ... */ }
pub struct CorrectnessEvaluator { /* ... */ }
pub struct ConcisenessEvaluator { /* ... */ }
pub struct CoherenceEvaluator { /* ... */ }
pub struct ResponseRelevanceEvaluator { /* ... */ }
pub struct HallucinationEvaluator { /* ... */ }
pub struct FaithfulnessEvaluator { /* ... */ }
pub struct PlanAdherenceEvaluator { /* ... */ }
pub struct LazinessEvaluator { /* ... */ }
pub struct GoalSuccessRateEvaluator { /* ... */ }

// feature evaluator-safety
pub struct HarmfulnessEvaluator { /* ... */ }
pub struct ToxicityEvaluator { /* ... */ }
pub struct FairnessEvaluator { /* ... */ }
pub struct PIILeakageEvaluator { /* ... */ }
pub struct PromptInjectionEvaluator { /* ... */ }
pub struct CodeInjectionEvaluator { /* ... */ }
pub enum PIIClass { Email, Phone, SSN, CreditCard, IpAddress, ApiKey, PersonalName, Address, Other(String) }

// feature evaluator-rag
pub struct RAGGroundednessEvaluator { /* ... */ }
pub struct RAGRetrievalRelevanceEvaluator { /* ... */ }
pub struct RAGHelpfulnessEvaluator { /* ... */ }
pub struct EmbeddingSimilarityEvaluator { /* ... */ }
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError>;
    fn dimension(&self) -> usize;
}
pub enum EmbedderError { /* ... */ }

// feature evaluator-agent
pub struct TrajectoryAccuracyEvaluator { /* ... */ }
pub struct TrajectoryAccuracyWithRefEvaluator { /* ... */ }
pub struct TaskCompletionEvaluator { /* ... */ }
pub struct UserSatisfactionEvaluator { /* ... */ }
pub struct AgentToneEvaluator { /* ... */ }
pub struct KnowledgeRetentionEvaluator { /* ... */ }
pub struct LanguageDetectionEvaluator { /* ... */ }
pub struct PerceivedErrorEvaluator { /* ... */ }
pub struct InteractionsEvaluator { /* ... */ }

// feature evaluator-structured
pub struct JsonMatchEvaluator { /* ... */ }
pub enum KeyStrategy { Average, All, None, Rubric(Arc<dyn JudgePromptTemplate>) }
pub struct JsonSchemaEvaluator { /* ... */ }

// feature evaluator-simple (always-on deterministic; `strsim` dep only behind feature)
pub struct ExactMatchEvaluator { /* ... */ }
pub struct LevenshteinDistanceEvaluator { /* ... */ }

// feature evaluator-code
pub struct CargoCheckEvaluator { /* ... */ }
pub struct ClippyEvaluator { /* ... */ }
pub struct CodeExtractor { /* ... */ }
pub enum CodeExtractorStrategy { MarkdownFence, Regex(Regex), Llm(Arc<dyn JudgeClient>) }
pub struct CodeLlmJudgeEvaluator { /* ... */ }

// feature evaluator-sandbox (Unix only; Windows: stub returning EvaluatorError::UnsupportedPlatform)
pub struct SandboxedExecutionEvaluator { /* ... */ }
pub struct SandboxLimits {
    pub wall_clock: Duration,
    pub cpu_seconds: Duration,
    pub memory_bytes: u64,
    pub file_descriptors: u32,
    pub allow_network: bool,
}
impl Default for SandboxLimits {
    // 120 s wall, 60 s CPU, 1 GiB RSS, 256 FDs, no network (FR-017)
}

// feature multimodal
pub struct ImageSafetyEvaluator { /* ... */ }

// Common per-evaluator configuration (every judge-backed evaluator takes this via builder)
pub struct JudgeEvaluatorConfig {
    pub template: Option<Arc<dyn JudgePromptTemplate>>,
    pub few_shot_examples: Vec<FewShotExample>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<JsonSchema>,
    pub use_reasoning: bool,
    pub feedback_key: Option<String>,
    pub aggregator: Option<Box<dyn Aggregator>>,
    pub judge_registry: Arc<JudgeRegistry>,
}
```

### Module `swink_agent_eval::simulation` *(feature `simulation`)*

```rust
pub struct ActorSimulator { /* ... */ }
pub struct ActorProfile { /* ... */ }
pub struct ToolSimulator { /* ... */ }
pub struct StateRegistry { /* ... */ }
pub struct StateBucket { /* ... */ }

pub async fn run_multiturn_simulation(
    agent: &dyn Agent,
    actor: &ActorSimulator,
    tool_sim: Option<&ToolSimulator>,
    max_turns: u32,
    cancel: CancellationToken,
) -> Result<Invocation, SimulationError>;

pub enum SimulationError {
    MaxTurnsReached,
    SchemaValidation { tool: String, detail: String },
    Cancelled,
    Judge(JudgeError),
}
```

### Module `swink_agent_eval::generation` *(feature `generation`)*

```rust
pub struct ExperimentGenerator { /* ... */ }
pub struct GenerationRequest { /* ... */ }
pub struct TopicPlanner { /* ... */ }
pub struct TopicSlot { pub topic: String, pub case_count: u32 }

pub enum GenerationError {
    MaxRetriesExceeded,
    SchemaValidation(String),
    Judge(JudgeError),
}
```

### Module `swink_agent_eval::trace` *(feature `trace-ingest`)*

```rust
#[async_trait]
pub trait TraceProvider: Send + Sync {
    async fn fetch_session(&self, session_id: &str) -> Result<RawSession, TraceProviderError>;
}

pub struct OtelInMemoryTraceProvider { /* always-on once trace-ingest feature is enabled */ }

// Per-backend, each behind its own sub-feature:
pub struct OtlpHttpTraceProvider { /* feature trace-otlp */ }
pub struct LangfuseTraceProvider { /* feature trace-langfuse */ }
pub struct OpenSearchTraceProvider { /* feature trace-opensearch */ }
pub struct CloudWatchTraceProvider { /* feature trace-cloudwatch */ }

pub trait SessionMapper: Send + Sync {
    fn map(&self, raw: &RawSession) -> Result<Invocation, MappingError>;
}

pub struct OpenInferenceSessionMapper;
pub struct LangChainSessionMapper;
pub struct OtelGenAiSessionMapper { pub version: GenAIConventionVersion }

pub enum GenAIConventionVersion { V1_27, V1_30, Experimental }

pub enum EvaluationLevel { Tool, Trace, Session }

pub trait TraceExtractor: Send + Sync {
    fn extract(&self, inv: &Invocation, level: EvaluationLevel) -> Vec<EvaluatorInput>;
}

pub struct SwarmExtractor;                  // consumes spec 040 types
pub struct GraphExtractor;                  // consumes spec 039 types

pub enum MappingError {
    MissingAttribute { name: String },
    UnknownConventionVersion,
    SchemaMismatch(String),
}

pub enum TraceProviderError {
    SessionNotFound,
    SessionInProgress,
    Backend(String),
}
```

### Module `swink_agent_eval::telemetry` *(feature `telemetry`)*

```rust
pub struct EvalsTelemetry { /* ... */ }
pub struct EvalsTelemetryBuilder { /* ... */ }

impl EvalsTelemetry {
    pub fn builder() -> EvalsTelemetryBuilder;
}
impl EvalsTelemetryBuilder {
    pub fn tracer(self, t: Arc<opentelemetry::Tracer>) -> Self;
    pub fn attributes(self, kvs: Vec<KeyValue>) -> Self;
    pub fn build(self) -> EvalsTelemetry;
}
```

Emitted span names (stable):
- `swink.eval.run_set`
- `swink.eval.case`
- `swink.eval.evaluator`

### Module `swink_agent_eval::cache`

```rust
#[async_trait]
pub trait EvaluationDataStore: Send + Sync {
    async fn get(&self, key: &CacheKey) -> Result<Option<Invocation>, StoreError>;
    async fn put(&self, key: CacheKey, inv: Invocation) -> Result<(), StoreError>;
}

pub struct LocalFileTaskResultStore { /* always-on */ }
impl LocalFileTaskResultStore {
    pub fn new(root: PathBuf) -> Self;
}

pub enum StoreError { Io(std::io::Error), Serde(String) }

pub struct CaseFingerprint<'a> { /* see data-model.md §R-020 */ }
```

### Module `swink_agent_eval::report`

```rust
pub trait Reporter: Send + Sync {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError>;
}

pub enum ReporterOutput {
    Stdout(String),
    Artifact { path: PathBuf, bytes: Vec<u8> },
    Remote { backend: String, identifier: String },
}

pub enum ReporterError { Io(std::io::Error), Format(String), Network(String) }

// Always-on:
pub struct ConsoleReporter;
pub struct JsonReporter;
pub struct MarkdownReporter;

// feature html-report
pub struct HtmlReporter { /* ... */ }

// feature langsmith
pub struct LangSmithExporter {
    pub api_token: String,
    pub endpoint: Url,           // default https://api.smith.langchain.com
}

pub enum LangSmithExportError {
    Push { pushed: u32, failed: u32, first_error: String },
    Auth,
    Network(String),
}
```

### Module `swink_agent_eval::types` *(extended)*

```rust
// Extended EvalCase per FR-043:
pub struct EvalCase {
    pub case_id: String,
    pub case_name: String,
    pub system_prompt: Option<String>,
    pub user_messages: Vec<ChatMessage>,
    pub expected_output: Option<String>,
    pub expected_trajectory: Option<Vec<ToolCallExpectation>>,

    // New in 043:
    pub expected_assertion: Option<Assertion>,
    pub expected_interactions: Option<Vec<InteractionExpectation>>,
    pub few_shot_examples: Vec<FewShotExample>,
    pub attachments: Vec<Attachment>,
    pub session_id: Uuid,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl EvalCase {
    pub fn default_session_id(&self) -> Uuid;     // deterministic UUID v5 per R-014
    pub fn validate(&self) -> Result<(), ValidationError>;
}

pub enum Attachment {
    Path(PathBuf),
    Base64 { mime: String, bytes: Vec<u8> },
    Url(String),
}

pub struct MaterializedAttachment { pub mime: String, pub bytes: Vec<u8> }

pub enum AttachmentError {
    PathNotFound(PathBuf),
    DecodeError(String),
    UrlBlocked { url: String, reason: String },
    FetchFailed { url: String, status: u16 },
    UnsupportedMime { mime: String },
}

impl Attachment {
    pub async fn materialize(&self, eval_set_root: &Path, filter: &dyn UrlFilter)
        -> Result<MaterializedAttachment, AttachmentError>;
}

pub struct Assertion { pub description: String, pub kind: AssertionKind }
pub enum AssertionKind {
    GoalCompleted,
    UserSatisfied,
    ToolInvoked(String),
    Custom { predicate: String },
}

pub struct InteractionExpectation {
    pub from_agent: String,
    pub to_agent: String,
    pub expected_topic: Option<String>,
}
```

### Extended `EvalRunner` (backwards-compatible additions)

```rust
impl EvalRunner {
    // Existing constructors unchanged.
    pub fn with_parallelism(self, n: usize) -> Self;
    pub fn with_num_runs(self, n: u32) -> Self;
    pub fn with_cache(self, store: Arc<dyn EvaluationDataStore>) -> Self;
    pub fn with_initial_session_file(self, path: PathBuf) -> Self;
    pub fn with_telemetry(self, tel: Arc<EvalsTelemetry>) -> Self;
    pub fn with_cancellation(self, tok: CancellationToken) -> Self;
}
```

### Binary target `swink-eval` *(feature `cli`)*

```text
swink-eval run --set <path> [--out <path>] [--parallelism <n>] [--reporter console|json|md|html]
swink-eval report <result.json> --format <console|json|md|html>
swink-eval gate <result.json> --gate-config <path>
```

Exit codes:
- `0` — success (run passed; gate passed; report rendered)
- `1` — eval run completed but gate failed
- `2` — configuration error (missing file, invalid args)
- `3` — runtime error (cancelled, IO error)

---

## Crate: `swink-agent-eval-judges` (NEW)

### Root `src/lib.rs`

```rust
#[cfg(feature = "anthropic")] pub use self::anthropic::AnthropicJudgeClient;
#[cfg(feature = "openai")]    pub use self::openai::OpenAIJudgeClient;
#[cfg(feature = "bedrock")]   pub use self::bedrock::BedrockJudgeClient;
#[cfg(feature = "gemini")]    pub use self::gemini::GeminiJudgeClient;
#[cfg(feature = "mistral")]   pub use self::mistral::MistralJudgeClient;
#[cfg(feature = "azure")]     pub use self::azure::AzureJudgeClient;
#[cfg(feature = "xai")]       pub use self::xai::XaiJudgeClient;
#[cfg(feature = "ollama")]    pub use self::ollama::OllamaJudgeClient;
#[cfg(feature = "proxy")]     pub use self::proxy::ProxyJudgeClient;
```

### Per-provider client shape (uniform across all 9)

```rust
pub struct AnthropicJudgeClient {
    adapter: Arc<swink_agent_adapters_anthropic::AnthropicAdapter>,
    retry_policy: RetryPolicy,
    batch_size: usize,
}

impl AnthropicJudgeClient {
    pub fn new(adapter: Arc<swink_agent_adapters_anthropic::AnthropicAdapter>) -> Self;
    pub fn with_retry_policy(self, p: RetryPolicy) -> Self;
    pub fn with_batch_size(self, n: usize) -> Self;
    pub fn blocking(&self) -> BlockingAnthropicJudgeClient;   // sync wrapper per FR-003
}

#[async_trait]
impl JudgeClient for AnthropicJudgeClient {
    async fn judge(&self, request: JudgeRequest) -> Result<JudgeVerdict, JudgeError>;
}

pub struct BlockingAnthropicJudgeClient { inner: AnthropicJudgeClient }
// Synchronous wrapper that does the right thing inside or outside a Tokio runtime (FR-048).
```

Each of the 9 providers follows this exact shape: `<Provider>JudgeClient`, `Blocking<Provider>JudgeClient`, with `new`, `with_retry_policy`, `with_batch_size`, `blocking()`, and the `JudgeClient` trait impl.

---

## Invariants across both crates

1. **FR-048 — async + blocking**: Every public `Evaluator`, `JudgeClient`, and `Reporter` exposes both `evaluate_async` (async) and `evaluate` (blocking). The blocking wrapper uses `tokio::runtime::Handle::try_current()` + `block_in_place` or `futures::executor::block_on` as appropriate so it works inside and outside a Tokio runtime.
2. **FR-049 — no unsafe**: Both crates compile under `#![forbid(unsafe_code)]` except `eval/src/evaluators/code/sandbox.rs` which relaxes to `#![allow(unsafe_code)]` at module scope (Unix only). This is the single FFI surface; see R-006.
3. **FR-047 / SC-009 — opt-in only**: Default build of both crates adds no new mandatory deps. Every new surface lives behind a feature flag.
4. **FR-050 — mock-only default tests**: `cargo test -p swink-agent-eval` and `cargo test -p swink-agent-eval-judges` make zero live LLM calls. Provider canary tests gate behind `live-judges` and env-var-sourced credentials.
5. **Stable public API**: Every item listed here is part of the 0.x → 1.0 surface. Breaking changes require a semver-major bump of the affected crate.

---

## JSON wire schemas

### `EvalSetResult` JSON schema

Path: `specs/043-evals-adv-features/contracts/eval-result.schema.json` (to be authored during implementation). `JsonReporter` output validates against it in the reporter test suite.

Top-level structure:
```json
{
  "schema_version": "043",
  "eval_set": { "id": "...", "name": "...", "case_count": N },
  "cases": [
    {
      "case_id": "...",
      "session_id": "uuid-v5",
      "verdict": "Pass|Fail|Partial",
      "duration_ms": N,
      "metrics": [
        { "evaluator": "...", "prompt_version": "...", "score": 0.82,
          "threshold": 0.7, "verdict": "Pass", "reason": "...",
          "samples": [0.81, 0.83, 0.82], "variance": 0.00011 }
      ]
    }
  ],
  "gate": { "passed": true, "violations": [] }
}
```

### `GateConfig` JSON schema

Extends spec 024's existing `GateConfig`. No new fields in 043.

### LangSmith push payload

Documented in R-015; mirrors LangSmith's `/runs` and `/feedback` endpoints. Keyed by evaluator `feedback_key` where set, else evaluator `name()`.
