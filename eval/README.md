# swink-agent-eval

[![Crates.io](https://img.shields.io/crates/v/swink-agent-eval.svg)](https://crates.io/crates/swink-agent-eval)
[![Docs.rs](https://docs.rs/swink-agent-eval/badge.svg)](https://docs.rs/swink-agent-eval)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Evaluation framework for [`swink-agent`](https://crates.io/crates/swink-agent) — trajectory tracing, golden-path matching, and cost/latency budget enforcement in one harness.

## Features

### Always-on
- **`EvalRunner`** — drives cases end-to-end against a user-provided `AgentFactory`
- **`TrajectoryMatcher`** — match expected tool-call sequences with exact, subset, or ordered modes (`MatchMode`)
- **`ResponseMatcher`** — assertions on the assistant's final text (substring, regex, semantic)
- **`BudgetEvaluator` / `EfficiencyEvaluator`** — per-case cost, token, and latency governance
- **`GateConfig`** — pass/fail gates on aggregate suite results (CI-friendly)
- **`FsEvalStore`** — persist trajectories and scores under a versioned directory layout
- **`ConsoleReporter` / `JsonReporter` / `MarkdownReporter`** — deterministic plain-text / JSON / PR-comment Markdown output
- **Audit log** (`AuditedInvocation`) — full request/response capture for replay and debugging

### Feature-gated (spec 043)

| Feature                  | Surface                                                                 |
| ------------------------ | ----------------------------------------------------------------------- |
| `judge-core`             | Prompt-template registry, judge cache/registry, dispatch helpers.       |
| `evaluator-quality`      | 10 quality-family evaluators (correctness, helpfulness, faithfulness…). |
| `evaluator-safety`       | 7 safety-family evaluators (toxicity, PII, prompt-injection…).          |
| `evaluator-rag`          | 3 RAG evaluators + `Embedder` trait.                                    |
| `evaluator-agent`        | 9 agent-behaviour evaluators (trajectory accuracy, tone…).              |
| `evaluator-simple`       | Deterministic `ExactMatch` + `LevenshteinDistance`.                     |
| `evaluator-structured`   | Deterministic `JsonMatch` + `JsonSchema`.                               |
| `evaluator-code`         | Code-quality + harness-based evaluators.                                |
| `evaluator-sandbox`      | Sandboxed execution evaluator (Unix rlimit FFI).                        |
| `multimodal`             | `ImageSafetyEvaluator` with attachment materialization.                 |
| `all-evaluators`         | Umbrella feature enabling all of the above.                             |
| `simulation`             | `ActorSimulator` + `ToolSimulator` multi-turn scenarios.                |
| `generation`             | `ExperimentGenerator` + `TopicPlanner` case synthesis.                  |
| `trace-ingest`           | `OtelInMemoryTraceProvider`, session mappers, extractors.               |
| `trace-otlp`             | `OtlpHttpTraceProvider` (OTel collector push/pull).                     |
| `trace-langfuse`         | `LangfuseTraceProvider` (REST).                                         |
| `trace-opensearch`       | `OpenSearchTraceProvider` (`_search` API).                              |
| `trace-cloudwatch`       | `CloudWatchTraceProvider` (caller-supplied `CloudWatchLogsFetcher`).    |
| `telemetry`              | `EvalsTelemetry` span bridge for `cargo otel` pipelines.                |
| `html-report`            | `HtmlReporter` (self-contained artifact, `askama` templates).           |
| `langsmith`              | `LangSmithExporter` — push runs + feedback to LangSmith.                |
| `cli`                    | Builds the `swink-eval` binary (`run`/`report`/`gate` subcommands).     |
| `yaml`                   | `load_eval_set_yaml` plus YAML-aware `swink-eval` parsing.              |
| `live-judges` (external) | Enabled on `swink-agent-eval-judges` to reach real provider APIs.       |

### Quick recipes

```bash
# Core eval only (default): no new transitive deps beyond 023.
cargo add swink-agent-eval

# Judge-backed evaluators + CLI + HTML + LangSmith:
cargo add swink-agent-eval --features "all-evaluators,html-report,langsmith,cli"

# Trace replay against OpenSearch / CloudWatch:
cargo add swink-agent-eval --features "trace-ingest,trace-opensearch,trace-cloudwatch"
```

## Quick Start

```toml
[dependencies]
swink-agent = "0.8"
swink-agent-eval = { version = "0.8", features = ["yaml"] }
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent_eval::{EvalRunner, EvalSet, AgentFactory};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let set = EvalSet {
        id: "demo".into(),
        name: "Demo".into(),
        description: None,
        cases: vec![/* EvalCase entries */],
    };

    let runner = EvalRunner::with_defaults();
    let result = runner.run_set(&set, &my_factory).await?;

    println!("Passed: {}/{}", result.summary.passed, result.summary.total_cases);
    Ok(())
}
```

## Architecture

A run is three staged components: a `TrajectoryCollector` captures every `AgentEvent` emitted by the loop, `Evaluator` implementations score the trajectory against an `EvalCase`'s expectations, and `EvalStore` persists the result. Budget enforcement is attached at agent construction time by converting `EvalCase.budget` into `BudgetPolicy` / `MaxTurnsPolicy` via `BudgetConstraints::to_policies()`. Matchers are independent building blocks — you can run trajectory, response, and budget checks alone or compose them via `EvaluatorRegistry`.

No `unsafe` code (`#![forbid(unsafe_code)]`). Eval runs never mutate shared state outside the provided `EvalStore`.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
