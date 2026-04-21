# Research: Evals: Advanced Features

**Feature**: 043-evals-adv-features | **Date**: 2026-04-21

Phase 0 resolves architectural decisions, dependency choices, and integration patterns for the advanced evals surface. Each entry follows the `Decision / Rationale / Alternatives` format.

---

## R-001: Prompt-Templating Engine

**Decision**: `minijinja` (latest 2.x).

**Rationale**: Jinja2-compatible, zero-unsafe, actively maintained, ~2M downloads. Supports named variables, conditionals, loops, and custom filters — everything the judge prompts need. Compile-time validation via `Environment::add_template()` surfaces missing-variable errors at construction time per FR-008. Small enough (~100 KB stripped) to sit behind the `judge-core` feature without bloating default builds of consumers who only want deterministic evaluators.

**Alternatives considered**:
- `handlebars`: Heavier, mustache-style; awkward for the conditional logic judge prompts need.
- `tera`: Also Jinja-compatible, but larger dep graph (pulls in `chrono`, `unic-segment`).
- Format strings (`format!` / custom recursive-replace): Hand-rolled alternative violates "Leverage the Ecosystem" and would re-implement variable-error reporting that `minijinja` already does correctly.

**API surface** (from `eval/src/prompt/mod.rs`):
```rust
use minijinja::{Environment, context};

pub trait JudgePromptTemplate: Send + Sync {
    fn version(&self) -> &str;
    fn render(&self, ctx: &PromptContext) -> Result<String, PromptError>;
}
```

---

## R-002: Retry / Backoff Library

**Decision**: `backon` (v1.x).

**Rationale**: Async-first, supports `ExponentialBuilder` with jitter and max-elapsed caps, no thread-pool dependency. Used by several major Rust projects (OpenDAL). ~1M downloads. Supports cancellation via `futures`, integrating cleanly with `tokio_util::sync::CancellationToken` per FR-040. Cooperative retry honoring 6-attempts-max and 4-minute-max-backoff (FR-004) is a two-line builder call.

**Alternatives considered**:
- `backoff` (v0.4): Older crate, unmaintained since 2023, async support via `tokio::time::sleep`.
- Hand-rolled retry: Subtle to get right for cancellation + jitter + header-driven backoff (provider `Retry-After`). Violates "Leverage the Ecosystem."

**Retry policy** (pinned):
```rust
ExponentialBuilder::default()
    .with_max_times(6)
    .with_max_delay(Duration::from_secs(240))
    .with_jitter()
```

---

## R-003: String Similarity (Levenshtein)

**Decision**: `strsim` (v0.12).

**Rationale**: Pure-Rust, zero deps, workspace-maintained. `strsim::levenshtein(a, b)` returns edit distance; normalized score is `1.0 - (dist as f64 / max(a.len, b.len))`. ~30M downloads.

**Alternatives considered**:
- `edit-distance` (smaller but fewer algorithms).
- Hand-rolled DP: Well-trodden problem; no benefit.

---

## R-004: JSON Schema Validation

**Decision**: `jsonschema` (v0.30).

**Rationale**: Draft 2020-12 support, pure-Rust, compiles schemas ahead-of-time. Needed by `JsonSchemaEvaluator` (FR-016) for deterministic schema-validation scoring — no judge call required. Also used by `ToolSimulator` to validate simulator-generated tool responses against tool schemas per FR-025.

**Alternatives considered**:
- `valico` (older, draft-07 only).
- Hand-rolled validation: Rejected.

---

## R-005: OpenTelemetry Integration

**Decision**: `opentelemetry` 0.31 + `opentelemetry-sdk` 0.31, gated behind `telemetry` feature. Per-backend exporters (`opentelemetry-otlp`, Langfuse-specific HTTP, OpenSearch via OTLP-HTTP + CloudWatch via AWS SDK) are separately feature-gated.

**Rationale**: OTel is the canonical multi-vendor trace substrate. `opentelemetry-sdk` provides the in-memory exporter (`opentelemetry-sdk::testing::trace::InMemorySpanExporter`) that backs `OtelInMemoryTraceProvider` without requiring any provider configuration — satisfying the "always available" guarantee of FR-031.

**Span structure** (per FR-035, US7):
- Root: `swink.eval.run_set` — attributes: `eval_set.id`, `eval_set.name`, `case_count`.
- Per case: `swink.eval.case` — attributes: `case.id`, `case.name`, `verdict`, `duration_ms`.
- Per evaluator: `swink.eval.evaluator` — attributes: `evaluator.name`, `prompt.version`, `score.value`, `score.threshold`, `verdict`.

**GenAI semantic-convention version matrix** (per FR-032):
- v1.27: `gen_ai.system`, `gen_ai.request.model`, `gen_ai.response.finish_reasons`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`.
- v1.30: Adds `gen_ai.operation.name`, renames some; mapper adapts in a lookup table.
- "experimental": Forward-compatible; mapper tolerates unknown attributes.

**Alternatives considered**:
- Raw `tracing` spans: Doesn't give us the vendor-agnostic wire format; loses interop with downstream consumers.
- Langfuse-SDK-first: Vendor lock-in.

---

## R-006: Sandboxing for Code Execution

**Decision**: POSIX rlimit-based sandbox using `libc` directly (Unix only; Windows fails fast). A `SandboxedExecutionEvaluator` spawns a child `cargo` process in a temporary directory, sets `setrlimit(RLIMIT_CPU)`, `RLIMIT_AS`, `RLIMIT_NOFILE`, and wall-clock via `tokio::time::timeout`. Network isolation via unsharing network namespace on Linux (`unshare(CLONE_NEWNET)`); on macOS, we rely on the process not opening sockets (`RLIMIT_NOFILE` bound) and document the limitation.

**Rationale**: Container isolation (e.g., Docker, Firecracker) adds infrastructure dependencies the spec explicitly avoids (sandbox must work in a normal `cargo test` run). rlimits are universally available on Unix, don't require root, and enforce the default caps pinned in FR-017 (120 s wall, 60 s CPU, 1 GiB RSS, 256 FDs, no network).

**The `libc` exception to unsafe-free**: `libc::setrlimit`, `libc::unshare`, and `libc::prlimit` are `unsafe extern "C"` calls. They are wrapped in a safe `evaluators::code::sandbox::posix` submodule whose internal blocks use `// SAFETY:` comments explaining the invariants. This is the only FFI surface in either crate; the module compiles only on `cfg(target_family = "unix")`, and `#![forbid(unsafe_code)]` is relaxed at that submodule to `#![allow(unsafe_code)]` with a module-level explanation. The forbid attribute remains at both crate roots. This exception is explicitly authorized by FR-049 in spec.md (see the exception clause pinning the module path, the `cfg` gate, and the `// SAFETY:` requirement).

**Windows behavior**: The entire `sandbox.rs` module compiles under `cfg(target_family = "unix")`. The Windows build ships a stub that constructs `EvaluatorError::UnsupportedPlatform` at evaluation time — no silent fallback, no panics.

**Alternatives considered**:
- `nix` crate (safe rlimit wrappers): Usable and would eliminate the unsafe exception, but `nix` is heavy (pulls in many syscall wrappers). Given we only need three syscalls, direct `libc` is simpler.
- Pure wall-clock timeout: Doesn't protect against CPU burners, memory bombs, or FD exhaustion.
- Docker-in-docker: Infrastructure surface expansion rejected.

---

## R-007: Embedding Provider (for `EmbeddingSimilarityEvaluator`)

**Decision**: Ship an `Embedder` trait with **no default implementation**. Consumers supply their own embedder: a local model (`fastembed`-style crate), a provider embedding API, or an in-memory test fixture.

**Rationale**: The spec's Assumptions call this out explicitly ("we don't bundle a default embedding provider"). Embedding providers have wildly different payload shapes, rate limits, and costs; baking one in would constrain users and pull in a mandatory dep. The trait gives users a clean injection point.

**API**:
```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError>;
    fn dimension(&self) -> usize;
}
```

---

## R-008: Attachment Handling (per Q10 clarification)

**Decision**: `Attachment` is an enum with three variants:
```rust
pub enum Attachment {
    Path(PathBuf),                          // resolved relative to eval-set root
    Base64 { mime: String, bytes: Vec<u8> }, // self-contained
    Url(String),                            // remote
}
```

**Materialization pipeline** (uniform across all variants before judge dispatch):
1. `Attachment::Path` → resolve against `EvalSet::root_path` → read bytes → detect MIME.
2. `Attachment::Base64` → direct.
3. `Attachment::Url` → SSRF-filtered `reqwest::get()` using the same domain-filter infrastructure already in `plugins/web` (R-011).

All three produce a `MaterializedAttachment { mime, bytes }` that judge clients consume uniformly. `AttachmentError` is a structured error type; no panics, no silent failures.

**SSRF protection**: URL attachments are filtered through a default deny-list (RFC 1918 ranges, localhost, metadata endpoints). Consumers may configure an allow-list. This mirrors the `plugins/web` domain-filter policy pattern so that the filter can be shared via the `policies` crate in a future refactor.

**Alternatives considered**: Separate fields per variant (rejected as less ergonomic); URL-only (rejected due to SSRF surface and deterministic-replay issues); base64-only (rejected due to file bloat — verbally the driver of Q10).

---

## R-009: Parallelism Model for `EvalRunner`

**Decision**: `tokio::sync::Semaphore` with a bounded permit count (configurable via `with_parallelism(n)`; default = 1 for backwards compatibility with current sequential behavior). Each case acquires a permit before its agent call + evaluator dispatch; permits release after the full case completes. Judge calls within a case share the case's permit — they don't contend for a separate pool.

**Rationale**: `Semaphore` is the canonical Tokio primitive for bounded concurrency. Sharing the permit across judge calls avoids unbounded fan-out (if every case fans out to 7 evaluators and parallelism is 4, naive impl = 28 concurrent provider calls); the per-provider rate-limit-aware retry absorbs transient throttling but catastrophic over-fan-out is prevented here.

**Cancellation**: `CancellationToken` is woven through the permit-acquisition loop. In-flight cases observe the token cooperatively via `tokio::select!` at await points; on cancellation, in-flight cases return a partial result with a cancellation indicator (FR-040).

**SC-002 calibration**: Default `parallelism = 1`. Recommended values in docs: 4–8 for fast providers (Anthropic/OpenAI), 2–4 for slower ones (Bedrock, Gemini). Parallelism is a knob, not something we auto-tune.

**Alternatives considered**:
- `tokio::task::JoinSet` with unbounded spawn: Doesn't bound concurrency, overwhelms providers.
- `futures::stream::buffer_unordered`: Equivalent to Semaphore but less explicit about permit lifecycle across nested fan-out (evaluator-within-case).

---

## R-010: Feature-Flag Naming Convention

**Decision**: Verb-first hierarchical naming on `eval`: `evaluator-<family>`, `trace-<backend>`, `judge-core`, `telemetry`, `simulation`, `generation`, `multimodal`, `html-report`, `langsmith`, `cli`, `live-judges`. On `eval-judges`: bare provider name (`anthropic`, `openai`, etc.) — parallel to the adapter crate name.

**Rationale**: The hierarchical prefix (`evaluator-`, `trace-`) groups related features alphabetically in `cargo tree --features` output and in `docs.rs`. Bare provider names on `eval-judges` match user expectation from the adapter crates (`swink-agent-adapters-anthropic` already uses bare names). Meta-feature `all-judges` = every provider feature; `all-evaluators` on `eval` = every evaluator family.

**Default feature set on `eval`**: `[]` (empty). Per FR-047, the default build adds no new mandatory dependencies. `judge-core` is opt-in because prompt templating (`minijinja`) and retry (`backon`) are only needed when a user wants LLM judges.

**Alternatives considered**:
- Flat names (`quality-evaluators`, `safety-evaluators`): Harder to group in UI listings.
- Single mega-feature `full-eval`: Defeats the purpose of feature-gating.

---

## R-011: SSRF Protection for `Attachment::Url` and Trace URLs

**Decision**: Reuse the domain-filter pattern from `plugins/web/src/policy/domain_filter.rs` (spec 042). `UrlFilter` lives in `eval/src/url_filter.rs` — an **always-on** top-level module (not under `judge/`, not feature-gated) because attachment materialization on `EvalCase` happens even when `judge-core` is disabled:

```rust
pub trait UrlFilter: Send + Sync {
    fn check(&self, url: &Url) -> Result<(), AttachmentError>;
}
```

A default `DefaultUrlFilter` denies RFC 1918 ranges (10/8, 172.16/12, 192.168/16), loopback (127/8, ::1), link-local (169.254/16, fe80::/10), cloud-metadata endpoints (169.254.169.254), and requires HTTPS by default. Consumers can install a permissive filter for test or dev.

**Rationale**: DRY with plugins/web; a future refactor could extract this into a `swink-agent-url-filter` helper, but for now both crates carry a small copy — the alternative (eval depending on plugins/web) introduces a layering violation. Placing `UrlFilter` at `eval/src/url_filter.rs` rather than under `eval/src/judge/` prevents an implicit dependency from always-on `Attachment::materialize` onto the `judge-core` feature.

---

## R-012: Judge Cache Eviction Policy

**Decision**: LRU, bounded by entry count (not memory). Default cap: 1024 entries. Implemented via `std::collections::HashMap` plus a VecDeque for recency — no external `lru` crate needed (the `lru` crate pulls in `hashbrown` which is already transitive via `indexmap`, but we avoid the dep by hand-rolling the 50 LoC).

**Rationale**: Prompts are short (<2 KB typically); entry-count bounding is simpler and more predictable than byte bounding. LRU matches user intent: the most recently seen prompt+model is the one being iterated on.

**Disk-backed variant**: Optional `disk_cache_path: Option<PathBuf>` — serializes entries as JSON in a directory, one file per cache key (SHA-256 of prompt+model). On startup, the cache warm-loads from disk up to its capacity. Disk-backed is opt-in; in-memory is default.

**Alternatives considered**:
- Time-based TTL: Judge responses don't expire; model identity is the invalidation signal.
- `lru` crate: Small, clean dep, but we already have the pattern in `swink-agent/src/util/` from the session cache — reuse reduces dep count.

---

## R-013: `num_runs` × Cache Interaction (per Q2 clarification)

**Decision**: A single cached `Invocation` serves all N runs of a case. The judge-side scoring loop runs N times with the same `Invocation` as input, and the variance diagnostic measures judge non-determinism. Agent non-determinism is out of scope.

**Implementation**: `EvalRunner` materializes `Invocation` once (from cache or by calling the agent), then loops the evaluator dispatch N times. Each iteration records its score in a `RunnerMetricSample { run: u32, score: Score }`; variance is `std_dev` computed across the N samples.

**Rationale**: Spec clarification Q2 pinned this explicitly. Matches SC-003's promise of no agent re-invocation on prompt-only iteration.

---

## R-014: Session ID Determinism (per Q7 clarification)

**Decision**: UUID v5 with a **project-specific namespace UUID** minted once and hard-coded as `CASE_NAMESPACE` in `eval/src/types.rs`. The namespace is itself a UUID v5 derived from the OID namespace over the ASCII bytes `"swink-agent-eval.case"` — this produces a stable UUID that semantically represents "the case-identity namespace of the swink-agent-eval library" rather than reusing an unrelated reserved namespace (the URL namespace is reserved for actual URLs). The name being hashed is the hex-encoded SHA-256 of the case's canonical bincode serialization (same basis as the cache key in FR-038).

**Rationale**: UUID v5 is deterministic, well-specified, and gives identical IDs across re-runs and machines for identical case inputs — satisfying trace-correlation (FR-035) and cache-key semantics (FR-038). Minting a project-specific namespace rather than reusing `Uuid::NAMESPACE_URL` avoids semantic pollution and prevents accidental collision with any other system that also happens to v5-hash case-shaped strings under the URL namespace.

**Implementation**:
```rust
use uuid::Uuid;

// Computed once at crate build time:
// CASE_NAMESPACE = uuid_v5(NAMESPACE_OID, "swink-agent-eval.case")
//                = 8d1f5e84-...-...-...-...............  (exact bytes pinned in types.rs)
pub const CASE_NAMESPACE: Uuid = Uuid::from_bytes([/* 16 bytes — pinned in types.rs */]);

let name = Sha256::digest(canonical_bincode(&case));
let session_id = Uuid::new_v5(&CASE_NAMESPACE, name.as_slice());
```

Task T016 materializes the namespace constant; the exact 16-byte value is pinned in the crate and covered by a unit test verifying `Uuid::new_v5(&Uuid::NAMESPACE_OID, b"swink-agent-eval.case") == CASE_NAMESPACE`.

---

## R-015: LangSmith Export Format

**Decision**: Push an `EvalSetResult` as a LangSmith "run" with each evaluator's `Score` attached as feedback under its configured feedback-key. Auth via `LANGSMITH_API_KEY` env var or constructor arg. Endpoint: `https://api.smith.langchain.com/runs` (POST) and `/feedback` (POST).

**Rationale**: Matches the LangSmith data model as of 2026-04. Every case maps to a LangSmith run; every evaluator's score becomes a feedback entry attached to that run under a key derived from `evaluator.feedback_key().unwrap_or(evaluator.name())`.

**Error handling** (per spec edge case): Partial push failures surface as a structured `LangSmithExportError { pushed: u32, failed: u32, first_error: ... }` — no local partial-state persistence.

**Alternatives considered**: Their OpenTelemetry-compatible ingest is newer and incomplete as of 2026-04; we stick with the stable REST path.

---

## R-016: CLI Argument Parser

**Decision**: `clap` (v4.x) with derive macros, feature-gated behind `cli`.

**Rationale**: De facto standard. Three subcommands (`run`, `report`, `gate`) per FR-046 clarification; each has a tight argument surface. `clap`'s `#[derive(Parser)]` keeps the binary source under 200 LoC.

**Subcommand structure**:
```rust
#[derive(Parser)]
enum Cmd {
    Run { #[arg(long)] set: PathBuf, #[arg(long)] out: Option<PathBuf>, #[arg(long)] parallelism: Option<usize> },
    Report { #[arg(long)] result: PathBuf, #[arg(long, value_enum)] format: ReportFormat },
    Gate { #[arg(long)] result: PathBuf, #[arg(long)] gate_config: PathBuf },
}
```

---

## R-017: HTML Reporter Templating

**Decision**: `askama` (v0.13+). Compile-time-checked templates, derives `Template` on Rust structs, zero runtime template parsing cost. Feature-gated behind `html-report`.

**Rationale**: HTML reporter must produce a single self-contained file (per FR-041 and US8). `askama` lets us embed templates at compile time, inline CSS/JS, and validate placeholder correctness at `cargo build`. Output is the only reporter supporting interactivity (per Q8 clarification — terminal/JSON/Markdown are plain, HTML is the rich tier).

**Implementation notes**: One master template `report.html.j2` that includes CSS and JS as inline `<style>`/`<script>` blocks. Collapsible-per-case behavior uses `<details>`/`<summary>` — no JavaScript required. A small amount of JS only if we add filtering/sorting (future; not in this spec).

**Alternatives considered**:
- `maud`: Compile-time-safe HTML-as-Rust, but reads as Rust code, harder for non-Rustaceans to tweak the output.
- `handlebars`: Runtime template validation.
- Hand-rolled `format!`: Intricate for collapsible nesting.

---

## R-018: CI Workflow Templates

**Decision**: Ship four YAML files in `eval/src/ci/templates/`:
1. `pr-eval.yml` — runs on PR, runs a declared eval set, comments the Markdown summary on the PR, sets status based on gate.
2. `nightly-eval.yml` — scheduled workflow, produces HTML report, uploads as a GitHub Actions artifact.
3. `release-eval.yml` — runs on tag push, produces JSON result, attaches to release.
4. `pre-commit-hook.yml` — a `.pre-commit-config.yaml` fragment that runs `swink-eval run` on a local eval set.

**Distribution**: These templates ship as static string constants compiled into `swink-eval init-ci` (a possible future subcommand, not in this spec), and also as files in the repo that users can copy manually. For this spec, only the static files are in scope — the `init-ci` subcommand can come later.

**Rationale**: GitHub Actions is the de-facto CI substrate; per spec Assumptions, we target it explicitly and leave other CI systems for third-party templates.

---

## R-019: Default Aggregator (per Q6 clarification)

**Decision**: `Average` (arithmetic mean of sub-scores). Binary-verdict evaluators (safety family) opt into `AllPass` explicitly in their constructors.

**Rationale**: Clarification Q6 pinned this. Implementation-wise, `Evaluator` default method returns `Box::new(Average)`; constructors on binary evaluators override via `.aggregator(AllPass)` at registration time.

---

## R-020: `EvaluationDataStore` Cache Key Format

**Decision**: Cache key is `sha256(bincode(CaseFingerprint))` where:
```rust
struct CaseFingerprint<'a> {
    case_id: &'a str,
    system_prompt: &'a str,
    user_messages: &'a [ChatMessage],
    initial_session: Option<&'a SessionState>,
    tool_set_hash: [u8; 32],  // hash of tool names + schemas
    agent_model: &'a str,
}
```

Any change to these fields invalidates the cache per spec edge case "cache keyed by a content hash of case input." Tool-set changes invalidate because adding/removing a tool is a behavior change even if inputs are otherwise identical.

**Rationale**: `bincode` gives a stable, canonical byte sequence from Rust structs; SHA-256 gives a collision-resistant key. Using `bincode` over `serde_json` for hashing avoids key-ordering ambiguity in nested maps.

**Disk layout** (`LocalFileTaskResultStore`):
```
<root>/<eval_set_id>/<case_id>/<fingerprint_hex>.json  # the cached Invocation
```

---

## R-021: Panic Isolation Strategy

**Decision**: Every new evaluator, simulator, generator, and reporter wraps its hot path in `tokio::task::spawn_blocking` (for sync panics) or a `tokio::spawn` + `JoinHandle.await` pair (for async panics) at the registry / orchestrator boundary. Panics surface as `JoinError::is_panic()` and convert to `Score::fail()` with `PanicDetail { location, message }` in details.

**Rationale**: Spec 023 already established this pattern at the `EvaluatorRegistry` boundary. Extending it to simulators and generators (which weren't in 023's scope) maintains a consistent contract: a rogue evaluator/simulator/generator/reporter never aborts a run.

**Implementation**:
```rust
let result = tokio::spawn(async move {
    evaluator.evaluate_async(&case, &invocation).await
}).await;

match result {
    Ok(r) => r,
    Err(e) if e.is_panic() => Score::fail().with_detail(panic_detail(e)),
    Err(e) => Err(e.into()),
}
```

**Note**: `tokio::spawn` moves the future to another task, which already catches panics. We don't need `std::panic::catch_unwind` here.

---

## R-022: "No Backwards Compat" Principle Application

**Decision**: FR-044 removed during clarification. No legacy-format converter ships. Per the workspace principle saved to memory, this workspace does not maintain compatibility with external eval tooling unless adopting a specific open standard.

**Consequence for the runner**: `EvalSet` loading accepts only the current-version `swink-agent-eval` schema. A top-level `"version": "043"` field in `EvalSet` JSON gates compatibility — unknown versions fail with a clear error rather than best-effort parsing.

---

## R-023: `initial_session_file` Format

**Decision**: JSON file matching `SessionState`'s `serde` representation from spec 034 (`session-state-store`). Lives on disk as `initial_session.json` by convention.

**Rationale**: `SessionState` is already serializable via `serde`. Reusing that format avoids introducing a new schema. TOML was considered but `SessionState` contains arbitrary message content that encodes poorly in TOML.

**Implementation**:
```rust
let initial: SessionState = serde_json::from_reader(fs::File::open(path)?)?;
```

---

## R-024: `live-judges` Canary Suite Scope

**Decision**: The `live-judges` feature enables a small canary suite (~3 test cases) per provider. Each canary:
1. Constructs a `<Provider>JudgeClient` with env-var-sourced credentials.
2. Runs a single `CorrectnessEvaluator` against a trivial case.
3. Asserts the result has a valid score and non-empty reason.

**Rationale**: Provides end-to-end confidence without drowning CI in live API calls. Canary suite runs only when `SWINK_EVAL_LIVE_JUDGES=1` and the relevant provider env vars are set. Default CI (including this feature's in-tree tests) does not run canaries.

**Skip policy**: Canaries skip with a diagnostic message (not fail) when credentials are absent; this keeps the binary honest about what it ran.

---

## R-025: Workspace-Level Cargo Dependencies

New workspace-level entries in root `Cargo.toml`'s `[workspace.dependencies]`:

```toml
minijinja = { version = "2", default-features = false, features = ["builtins", "loader"] }
backon = { version = "1", default-features = false, features = ["tokio-sleep"] }
strsim = "0.12"
jsonschema = { version = "0.30", default-features = false }
opentelemetry = "0.31"
opentelemetry-sdk = "0.31"
opentelemetry-otlp = { version = "0.31", features = ["http-proto", "reqwest-client"] }
askama = { version = "0.13", default-features = false }
clap = { version = "4", features = ["derive"] }
libc = "0.2"
```

**Rationale**: Centralizing versions in the workspace manifest matches the existing pattern and simplifies future bumps.

---

## Summary of Phase 0 decisions

| Concern | Decision |
|---|---|
| Template engine | `minijinja` (behind `judge-core`) |
| Retry | `backon` (behind `judge-core`) |
| Levenshtein | `strsim` (behind `evaluator-simple`) |
| JSON Schema | `jsonschema` (behind `evaluator-structured`) |
| OTel | `opentelemetry` 0.31 + SDK (behind `telemetry`) |
| Sandboxing | `libc` rlimits on Unix, stub on Windows (behind `evaluator-sandbox`) |
| HTML templating | `askama` compile-time templates (behind `html-report`) |
| CLI | `clap` 4 derive macros (behind `cli`) |
| Parallelism | `tokio::Semaphore` |
| Cache policy | LRU by entry count (default 1024) |
| Session ID | UUID v5 from SHA-256 of canonical case bytes |
| Default aggregator | `Average` |
| SSRF filter | Reused domain-filter pattern from plugins/web |
| Workspace layout | `eval` extended; new `eval-judges` crate; CLI binary in `eval/src/bin/` |
