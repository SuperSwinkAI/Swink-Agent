# Data Model: Evals: Advanced Features

**Feature**: 043-evals-adv-features | **Date**: 2026-04-21

Entities grouped by scope item. Types use Rust-like pseudocode; trait methods show async where asyncified. All new types live in `swink-agent-eval` unless marked `[eval-judges]`.

---

## 1. Judge infrastructure

### `JudgeRegistry`

Top-level configuration binding a judge model, retry policy, batch size, and cache to an `EvaluatorRegistry`. Constructor refuses to build without an explicit `model_id` (per Q9 clarification).

```rust
pub struct JudgeRegistry {
    client: Arc<dyn JudgeClient>,     // concrete impl from eval-judges
    model_id: String,                  // REQUIRED — no default ships
    retry_policy: RetryPolicy,
    batch_size: usize,
    cache: Arc<JudgeCache>,
    url_filter: Arc<dyn UrlFilter>,    // for attachments
}

pub struct RetryPolicy {
    max_attempts: u32,                 // default 6
    max_delay: Duration,               // default 4 min
    jitter: bool,                      // default true
}

impl JudgeRegistry {
    pub fn builder(client: Arc<dyn JudgeClient>, model_id: impl Into<String>) -> JudgeRegistryBuilder;
    // Builder methods return Self; build() returns Result to surface validation
    // (e.g., batch_size == 0 is rejected).
}
```

**Validation rules**:
- `model_id` is non-empty.
- `batch_size` ∈ [1, 128] (upper bound pinned per FR-005 "bounded upper limit").
- `retry_policy.max_attempts` ≤ 16 (ceiling — anything higher is almost certainly a misconfiguration).

### `JudgePromptTemplate`

Versioned, variable-substituting prompt template. `render` validates variable presence at construction; missing variables surface as `PromptError::MissingVariable { name }`.

```rust
pub trait JudgePromptTemplate: Send + Sync {
    fn version(&self) -> &str;                // e.g., "correctness_v0"
    fn render(&self, ctx: &PromptContext) -> Result<String, PromptError>;
    fn family(&self) -> PromptFamily;         // Quality | Safety | RAG | Agent | Structured | Code | Multimodal
                                              // (No `Simple` — simple-family evaluators are deterministic and register no templates.)
}

pub struct PromptContext {
    pub case: Arc<EvalCase>,
    pub invocation: Arc<Invocation>,
    pub few_shot_examples: Vec<FewShotExample>,
    pub custom: HashMap<String, serde_json::Value>,
}

pub enum PromptError {
    MissingVariable { name: String },
    RenderError(String),
}
```

**Built-in templates** (one per evaluator, versioned `_v0`):
- Quality: `helpfulness_v0`, `correctness_v0`, `conciseness_v0`, `coherence_v0`, `response_relevance_v0`, `hallucination_v0`, `faithfulness_v0`, `plan_adherence_v0`, `laziness_v0`, `goal_success_rate_v0`
- Safety: `harmfulness_v0`, `toxicity_v0`, `fairness_v0`, `pii_leakage_v0`, `prompt_injection_v0`, `code_injection_v0`
- RAG: `rag_groundedness_v0`, `rag_retrieval_relevance_v0`, `rag_helpfulness_v0`
- Agent: `trajectory_accuracy_v0`, `trajectory_accuracy_with_ref_v0`, `task_completion_v0`, `user_satisfaction_v0`, `agent_tone_v0`, `knowledge_retention_v0`, `language_detection_v0`, `perceived_error_v0`, `interactions_v0`
- Code: `code_llm_judge_v0`
- Multimodal: `image_safety_v0`

### `PromptTemplateRegistry`

```rust
pub struct PromptTemplateRegistry {
    templates: HashMap<String, Arc<dyn JudgePromptTemplate>>,   // key = version identifier
}

impl PromptTemplateRegistry {
    pub fn builtin() -> Self;                                    // ships all _v0 templates
    pub fn get(&self, version: &str) -> Option<Arc<dyn JudgePromptTemplate>>;
    pub fn register(&mut self, template: Arc<dyn JudgePromptTemplate>) -> Result<(), PromptError>;
    // Rejects duplicate version keys.
}
```

### `JudgeCache`

LRU cache of prompt + model → verdict. Default in-memory; optional disk-backed.

```rust
pub struct JudgeCache {
    entries: Mutex<LruMap<CacheKey, CachedVerdict>>,
    capacity: usize,                          // default 1024
    disk_path: Option<PathBuf>,
}

pub struct CacheKey {
    prompt_hash: [u8; 32],   // SHA-256 of rendered prompt
    model_id: String,
}

pub struct CachedVerdict {
    verdict: JudgeVerdict,    // from 023
    recorded_at: SystemTime,
}
```

**Lifecycle**:
- Insertion updates LRU ordering.
- Lookup updates LRU ordering.
- Disk-backed variant flushes dirty entries on `Drop` and warm-loads on construction.

---

## 2. Prompt-template registry *(covered under §1)*

## 3. LLM judge evaluator family

### Base `Evaluator` extensions

Each new evaluator implements the 023 `Evaluator` trait. The signature is unchanged; what's new is that every implementation now supports per-instance overrides via a common `JudgeEvaluatorConfig`:

```rust
pub struct JudgeEvaluatorConfig {
    pub template: Option<Arc<dyn JudgePromptTemplate>>,   // None → use builtin _v0
    pub few_shot_examples: Vec<FewShotExample>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<JsonSchema>,
    pub use_reasoning: bool,                               // default true
    pub feedback_key: Option<String>,                      // for LangSmith export
    pub aggregator: Option<Box<dyn Aggregator>>,           // None → Average
    pub judge_registry: Arc<JudgeRegistry>,
}
```

Every judge-backed evaluator holds one `JudgeEvaluatorConfig`.

### Quality family (10 evaluators)

```rust
pub struct HelpfulnessEvaluator { config: JudgeEvaluatorConfig }   // 7-level scale
pub struct CorrectnessEvaluator { config: JudgeEvaluatorConfig }   // against expected_output
pub struct ConcisenessEvaluator { config: JudgeEvaluatorConfig }   // 3-level
pub struct CoherenceEvaluator   { config: JudgeEvaluatorConfig }   // 5-level
pub struct ResponseRelevanceEvaluator { config: JudgeEvaluatorConfig }
pub struct HallucinationEvaluator { config: JudgeEvaluatorConfig } // general, against model knowledge
pub struct FaithfulnessEvaluator  { config: JudgeEvaluatorConfig } // RAG-grounded, against retrieved context
pub struct PlanAdherenceEvaluator { config: JudgeEvaluatorConfig }
pub struct LazinessEvaluator { config: JudgeEvaluatorConfig }
pub struct GoalSuccessRateEvaluator { config: JudgeEvaluatorConfig } // consumes expected_assertion
```

### Safety family (6 evaluators; default aggregator `AllPass`)

```rust
pub struct HarmfulnessEvaluator { config: JudgeEvaluatorConfig }   // binary, broad spectrum
pub struct ToxicityEvaluator    { config: JudgeEvaluatorConfig }   // binary, narrow (hate/harassment/slurs)
pub struct FairnessEvaluator    { config: JudgeEvaluatorConfig }
pub struct PIILeakageEvaluator  {
    config: JudgeEvaluatorConfig,
    entity_classes: Vec<PIIClass>,  // default: all built-in
}
pub struct PromptInjectionEvaluator { config: JudgeEvaluatorConfig }
pub struct CodeInjectionEvaluator   { config: JudgeEvaluatorConfig }

pub enum PIIClass { Email, Phone, SSN, CreditCard, IpAddress, ApiKey, PersonalName, Address, Other(String) }
```

### RAG family (4 evaluators)

```rust
pub struct RAGGroundednessEvaluator      { config: JudgeEvaluatorConfig }
pub struct RAGRetrievalRelevanceEvaluator { config: JudgeEvaluatorConfig }
pub struct RAGHelpfulnessEvaluator        { config: JudgeEvaluatorConfig }
pub struct EmbeddingSimilarityEvaluator {
    embedder: Arc<dyn Embedder>,  // consumer-supplied, no default
    threshold: f64,               // default 0.8
    aggregator: Option<Box<dyn Aggregator>>,
}
```

`EmbeddingSimilarityEvaluator` is the sole deterministic member of this family (no `JudgeClient` use).

### Agent / trajectory family (8 evaluators)

```rust
pub struct TrajectoryAccuracyEvaluator        { config: JudgeEvaluatorConfig }  // without ref
pub struct TrajectoryAccuracyWithRefEvaluator { config: JudgeEvaluatorConfig }  // with expected_trajectory
pub struct TaskCompletionEvaluator            { config: JudgeEvaluatorConfig }
pub struct UserSatisfactionEvaluator          { config: JudgeEvaluatorConfig }
pub struct AgentToneEvaluator                 { config: JudgeEvaluatorConfig }
pub struct KnowledgeRetentionEvaluator        { config: JudgeEvaluatorConfig }
pub struct LanguageDetectionEvaluator         { config: JudgeEvaluatorConfig }
pub struct PerceivedErrorEvaluator            { config: JudgeEvaluatorConfig }
pub struct InteractionsEvaluator {
    config: JudgeEvaluatorConfig,
    // expected_interactions consumed from case; scores multi-agent hand-off topology
}
```

### Structured-output family (2 evaluators)

```rust
pub struct JsonMatchEvaluator {
    config: JudgeEvaluatorConfig,
    per_key_strategy: HashMap<String, KeyStrategy>,
    exclude_keys: HashSet<String>,
}

pub enum KeyStrategy {
    Average,
    All,
    None,
    Rubric(Arc<dyn JudgePromptTemplate>),
}

pub struct JsonSchemaEvaluator {
    schema: jsonschema::JSONSchema,    // compiled ahead of time
    // deterministic; no JudgeClient use
}
```

### Simple family (2 evaluators, always-on deterministic)

```rust
pub struct ExactMatchEvaluator {
    case_sensitive: bool,       // default true
    trim: bool,                 // default false
}

pub struct LevenshteinDistanceEvaluator {
    threshold: f64,             // default 0.8 (normalized similarity)
}
```

### Code family (5 evaluators, behind `evaluator-code`)

```rust
pub struct CargoCheckEvaluator { ... }          // deterministic — spawns `cargo check`
pub struct ClippyEvaluator { ... }              // deterministic — spawns `cargo clippy`
pub struct CodeExtractor {
    strategy: CodeExtractorStrategy,
}
pub enum CodeExtractorStrategy {
    MarkdownFence,
    Regex(Regex),
    Llm(Arc<dyn JudgeClient>),
}
pub struct CodeLlmJudgeEvaluator { config: JudgeEvaluatorConfig }
pub struct SandboxedExecutionEvaluator {
    limits: SandboxLimits,      // defaults per FR-017: 120s wall, 60s CPU, 1GB RSS, 256 FDs, no network
}

pub struct SandboxLimits {
    pub wall_clock: Duration,    // default 120s
    pub cpu_seconds: Duration,   // default 60s
    pub memory_bytes: u64,       // default 1 GiB
    pub file_descriptors: u32,   // default 256
    pub allow_network: bool,     // default false
}
```

### Multimodal family (behind `multimodal`)

```rust
pub struct ImageSafetyEvaluator { config: JudgeEvaluatorConfig }
// Audio evaluators out of scope per FR-019.
```

### Aggregators (FR-022/023)

```rust
pub trait Aggregator: Send + Sync {
    fn aggregate(&self, samples: &[Score]) -> Score;
}

pub struct Average;   // default per Q6
pub struct AllPass;
pub struct AnyPass;
pub struct Weighted { weights: Vec<f64> }
```

---

## 5. Multiturn simulation *(behind `simulation` feature)*

### `ActorSimulator`

```rust
pub struct ActorSimulator {
    profile: ActorProfile,
    judge: Arc<dyn JudgeClient>,           // drives the simulated user
    model_id: String,
    greeting_pool: Vec<String>,
    max_turns: u32,                        // default 10
    goal_completion_signal: Option<ToolDef>,
}

pub struct ActorProfile {
    pub name: String,
    pub traits: Vec<String>,                // e.g., ["frustrated", "terse"]
    pub context: String,                    // paragraph describing background
    pub goal: String,                       // natural-language goal statement
}
```

### `ToolSimulator`

```rust
pub struct ToolSimulator {
    tools: HashMap<String, ToolSchema>,     // tool name → schema
    judge: Arc<dyn JudgeClient>,
    model_id: String,
    state_registry: Arc<StateRegistry>,
    history_cap: usize,                     // default 32
}

pub struct StateRegistry {
    buckets: Mutex<HashMap<String, StateBucket>>,
}

pub struct StateBucket {
    shared_state: serde_json::Value,        // arbitrary state blob per key
    history: VecDeque<ToolCallRecord>,      // bounded by history_cap
}

pub struct ToolCallRecord {
    tool: String,
    args: serde_json::Value,
    result: serde_json::Value,
    timestamp: SystemTime,
}
```

### `run_multiturn_simulation`

```rust
pub async fn run_multiturn_simulation(
    agent: &dyn Agent,
    actor: &ActorSimulator,
    tool_sim: Option<&ToolSimulator>,
    max_turns: u32,
    cancel: CancellationToken,
) -> Result<Invocation, SimulationError>;
```

Returns a complete `Invocation` scorable by any registered evaluator. Honors cancellation cooperatively.

---

## 6. Experiment generation *(behind `generation` feature)*

### `ExperimentGenerator`

```rust
pub struct ExperimentGenerator {
    judge: Arc<dyn JudgeClient>,
    model_id: String,
    planner: Arc<TopicPlanner>,
    retry_cap: u32,                         // bounded retries on malformed output; default 3
    validate: bool,                         // default true — always validate before emit
}

pub struct GenerationRequest {
    pub context: String,
    pub task: String,
    pub desired_count: u32,
    pub num_topics: u32,                    // how many diverse topics to plan
    pub include_expected_output: bool,
    pub include_expected_trajectory: bool,
    pub include_expected_interactions: bool,
    pub include_metadata: bool,
    pub agent_tools: Option<Vec<ToolDef>>,   // when provided, trajectories reference only these
}

impl ExperimentGenerator {
    pub async fn generate(&self, req: GenerationRequest) -> Result<EvalSet, GenerationError>;
}
```

### `TopicPlanner`

```rust
pub struct TopicPlanner { judge: Arc<dyn JudgeClient>, model_id: String }

impl TopicPlanner {
    pub async fn plan(&self, context: &str, task: &str, num_topics: u32)
        -> Result<Vec<TopicSlot>, GenerationError>;
}

pub struct TopicSlot {
    pub topic: String,
    pub case_count: u32,
}
```

**Validation**: Every emitted `EvalCase` passes `EvalCase::validate()` before being added to the `EvalSet`. Failures trigger retry up to `retry_cap`; unrecoverable slots are omitted with a warning.

---

## 7. Observability / trace ingestion *(behind `trace-ingest` feature)*

### `TraceProvider`

```rust
#[async_trait]
pub trait TraceProvider: Send + Sync {
    async fn fetch_session(&self, session_id: &str)
        -> Result<RawSession, TraceProviderError>;
}

pub struct OtelInMemoryTraceProvider {
    exporter: Arc<InMemorySpanExporter>,
}  // always-on

// Feature-gated concrete providers:
pub struct OtlpHttpTraceProvider { /* ... */ }        // trace-otlp
pub struct LangfuseTraceProvider { /* ... */ }        // trace-langfuse
pub struct OpenSearchTraceProvider { /* ... */ }      // trace-opensearch
pub struct CloudWatchTraceProvider { /* ... */ }      // trace-cloudwatch
```

### `SessionMapper`

```rust
pub trait SessionMapper: Send + Sync {
    fn map(&self, raw: &RawSession) -> Result<Invocation, MappingError>;
}

pub struct OpenInferenceSessionMapper;

pub struct LangChainSessionMapper;

pub struct OtelGenAiSessionMapper {
    pub version: GenAIConventionVersion,
}

pub enum GenAIConventionVersion {
    V1_27,
    V1_30,
    Experimental,
}
```

**Error model**: `MappingError::MissingAttribute { name }` never panics, per spec edge case.

### `EvaluationLevel` and `TraceExtractor`

```rust
pub enum EvaluationLevel { Tool, Trace, Session }

pub trait TraceExtractor: Send + Sync {
    fn extract(&self, inv: &Invocation, level: EvaluationLevel) -> Vec<EvaluatorInput>;
}

// Adapters for multi-agent frameworks (specs 040 and 039):
pub struct SwarmExtractor;
pub struct GraphExtractor;
```

### `EvalsTelemetry` *(behind `telemetry` feature)*

```rust
pub struct EvalsTelemetry {
    tracer: Arc<opentelemetry::Tracer>,
    attributes: Vec<KeyValue>,                // always-on attributes
}

impl EvalsTelemetry {
    pub fn builder() -> EvalsTelemetryBuilder;
    pub fn instrument_runner(self, runner: &mut EvalRunner);
}
```

Span tree per FR-035:
- `swink.eval.run_set` — root; attrs: `eval_set.id`, `eval_set.name`, `case_count`.
- `swink.eval.case` — child; attrs: `case.id`, `case.name`, `verdict`, `duration_ms`, `session_id`.
- `swink.eval.evaluator` — grandchild; attrs: `evaluator.name`, `prompt.version`, `score.value`, `score.threshold`, `verdict`.

---

## 8. Runner upgrades

### Extended `EvalRunner`

```rust
pub struct EvalRunner {
    // Existing fields from 023/024...
    parallelism: usize,                     // default 1
    num_runs: u32,                          // default 1
    cache: Option<Arc<dyn EvaluationDataStore>>,
    initial_session_file: Option<PathBuf>,
    telemetry: Option<Arc<EvalsTelemetry>>,
    cancel: Option<CancellationToken>,
}

impl EvalRunner {
    pub fn with_parallelism(self, n: usize) -> Self;       // panics if n == 0
    pub fn with_num_runs(self, n: u32) -> Self;             // panics if n == 0
    pub fn with_cache(self, store: Arc<dyn EvaluationDataStore>) -> Self;
    pub fn with_initial_session_file(self, path: PathBuf) -> Self;
    pub fn with_telemetry(self, tel: Arc<EvalsTelemetry>) -> Self;
    pub fn with_cancellation(self, tok: CancellationToken) -> Self;
}
```

### `EvaluationDataStore`

```rust
#[async_trait]
pub trait EvaluationDataStore: Send + Sync {
    async fn get(&self, key: &CacheKey) -> Result<Option<Invocation>, StoreError>;
    async fn put(&self, key: CacheKey, inv: Invocation) -> Result<(), StoreError>;
}

pub struct LocalFileTaskResultStore {
    root: PathBuf,
}
```

Cache key format per R-020:
```rust
pub struct CacheKey(pub [u8; 32]);  // SHA-256 of canonical CaseFingerprint bincode
```

### Runner iteration shape (pseudocode)

```rust
for case in eval_set.cases {
    let permit = semaphore.acquire().await;
    let fingerprint = CaseFingerprint::from(&case, &initial_session, &agent_tools, model_id);
    let key = CacheKey::from(&fingerprint);

    let invocation = match cache.as_ref().and_then(|c| c.get(&key).await?) {
        Some(cached) => cached,
        None => {
            let fresh = agent.run(&case).await?;
            if let Some(c) = cache.as_ref() { c.put(key, fresh.clone()).await?; }
            fresh
        }
    };

    let samples: Vec<Score> = (0..num_runs).map(|run_idx| {
        telemetry.span("swink.eval.case", || registry.evaluate(&case, &invocation).await)
    }).collect();

    let composite = aggregator.aggregate(&samples);   // Average by default
    let variance = samples.std_dev();
}
```

---

## 9. Reporting

```rust
pub trait Reporter: Send + Sync {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError>;
}

pub enum ReporterOutput {
    Stdout(String),                          // console / markdown
    Artifact { path: PathBuf, bytes: Vec<u8> },
    Remote { backend: String, identifier: String },   // LangSmith run URL
}

pub struct ConsoleReporter;                    // always-on, plain-text
pub struct JsonReporter;                        // always-on, self-contained JSON + published schema
pub struct MarkdownReporter;                    // always-on, PR-comment-ready
pub struct HtmlReporter { /* ... */ }           // html-report feature
pub struct LangSmithExporter { api_token: String, endpoint: Url }  // langsmith feature
```

**JSON schema**: Published at `specs/043-evals-adv-features/contracts/eval-result.schema.json` (added during implementation). `JsonReporter` output validates against it.

---

## 10. Case-model extensions

### Extended `EvalCase`

```rust
pub struct EvalCase {
    // Existing fields from 023...
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
    pub session_id: Uuid,                       // auto-default UUID v5, see below
    pub metadata: HashMap<String, serde_json::Value>,
}

impl EvalCase {
    pub fn default_session_id(&self) -> Uuid {
        // CASE_NAMESPACE is a project-specific namespace UUID minted once and pinned
        // in eval/src/types.rs. It is itself Uuid::new_v5(&NAMESPACE_OID, b"swink-agent-eval.case").
        let canonical = bincode::serialize(&self.content_fingerprint()).unwrap();
        let digest = Sha256::digest(&canonical);
        Uuid::new_v5(&CASE_NAMESPACE, digest.as_slice())
    }
}
```

### `Attachment` (per Q10 clarification)

```rust
pub enum Attachment {
    Path(PathBuf),
    Base64 { mime: String, bytes: Vec<u8> },
    Url(String),
}

pub struct MaterializedAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub enum AttachmentError {
    PathNotFound(PathBuf),
    DecodeError(String),
    UrlBlocked { url: String, reason: String },  // SSRF filter
    FetchFailed { url: String, status: u16 },
    UnsupportedMime { mime: String },
}

impl Attachment {
    pub async fn materialize(&self, eval_set_root: &Path, filter: &dyn UrlFilter)
        -> Result<MaterializedAttachment, AttachmentError>;
}
```

### `FewShotExample`

```rust
pub struct FewShotExample {
    pub input: String,
    pub expected: String,
    pub reasoning: Option<String>,
}
```

### `Assertion`

```rust
pub struct Assertion {
    pub description: String,
    pub kind: AssertionKind,
}

pub enum AssertionKind {
    GoalCompleted,
    UserSatisfied,
    ToolInvoked(String),
    Custom { predicate: String },     // free-form; judge-evaluated
}
```

---

## 11. CI and DX scaffolding

Static YAML templates shipped in `eval/src/ci/templates/` (not runtime data). See R-018 for filenames and R-016 for the CLI subcommand shape.

---

## Relationships

```text
EvalSet ──owns*── EvalCase ──references── Attachment
                              ──refs?──── expected_assertion: Assertion
                              ──refs?──── expected_interactions: [InteractionExpectation]
                              ──refs*──── few_shot_examples: [FewShotExample]

EvalRunner ──uses──── EvaluatorRegistry ──owns*── Evaluator
           ──uses──── JudgeRegistry    ──owns──── JudgeClient (from eval-judges)
                                       ──owns──── PromptTemplateRegistry
                                       ──owns──── JudgeCache
           ──uses?─── EvaluationDataStore
           ──uses?─── EvalsTelemetry
           ──uses?─── CancellationToken

ActorSimulator ──drives── Agent ──via── ToolSimulator(s)
run_multiturn_simulation ──produces── Invocation

ExperimentGenerator ──owns── TopicPlanner ──produces── [TopicSlot]
                    ──produces── EvalSet

TraceProvider ──produces── RawSession ──mapped-by── SessionMapper ──produces── Invocation
                                      ──extracted-by── TraceExtractor ──produces── [EvaluatorInput]

EvalSetResult ──rendered-by── Reporter (Console | Json | Markdown | Html | LangSmith)
```

---

## Validation rules summary

| Entity | Rule | Surface |
|---|---|---|
| `JudgeRegistry` | `model_id` non-empty | Construction-time error |
| `JudgeRegistry` | `batch_size` ∈ [1, 128] | Construction-time error |
| `JudgePromptTemplate::render` | All named variables populated | Construction or evaluator-construction time |
| `EvalCase` | `case_id` unique within `EvalSet` | Validation on `EvalSet::validate()` |
| `EvalCase` | `session_id` is valid UUID (v5 when auto) | Serde parse time |
| `Attachment::Path` | Path exists relative to eval-set root at materialization | `AttachmentError::PathNotFound` at runtime (not parse) |
| `Attachment::Url` | Passes `UrlFilter::check` | `AttachmentError::UrlBlocked` at materialization |
| `SandboxedExecutionEvaluator` | Not invoked on Windows | `EvaluatorError::UnsupportedPlatform` at `evaluate_async` |
| `ExperimentGenerator` | Every emitted case passes `EvalCase::validate` | Before adding to returned `EvalSet` |
| `ToolSimulator` | Generated response validates against tool schema | `SimulationError::SchemaValidation` surfaced to evaluator |
| `EvalRunner::with_parallelism` | `n > 0` | Panics on 0 (misconfiguration signal) |
| `EvalRunner::with_num_runs` | `n > 0` | Panics on 0 |

---

## State transitions

### `EvalRunner` execution state (per case)

```text
Queued → AcquiringPermit → RunningAgent (or CacheHit) → Evaluating[run=1..N] → Aggregating → Reporting
                                                                      │
                                                                      └─(cancel)→ PartialCompleted
```

### `ActorSimulator` turn loop

```text
Initialized → Greeted → Turn(i) → Turn(i+1) → ... → { GoalCompleted | MaxTurnsReached | Cancelled }
```

---

## Notes on panic isolation (FR-021)

Every `Evaluator`, `SessionMapper`, `TraceExtractor`, `Simulator`, and `Reporter` runs through a `tokio::spawn(...).await` wrapper at the registry/orchestrator boundary. `JoinError::is_panic()` converts to `Score::fail().with_detail(PanicDetail { location, message })` — see R-021. No panic propagates to abort the run.
