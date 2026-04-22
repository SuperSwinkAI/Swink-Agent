# Quickstart: Evals: Advanced Features

**Feature**: 043-evals-adv-features | **Date**: 2026-04-21

End-to-end walkthroughs for the advanced evals surface. Assumes the user has spec 023's baseline `swink-agent-eval` working and an `EvalSet` authored.

---

## 1. Score an eval set with a production judge

**Goal**: Run 20 cases through `CorrectnessEvaluator` and `HelpfulnessEvaluator` backed by Anthropic.

```toml
# Cargo.toml
[dependencies]
swink-agent = "0.9"
swink-agent-eval = { version = "0.9", features = ["judge-core", "evaluator-quality"] }
swink-agent-eval-judges = { version = "0.9", features = ["anthropic"] }
swink-agent-adapters-anthropic = "0.9"
```

```rust
use std::sync::Arc;
use swink_agent_eval::{
    judge::JudgeRegistry,
    evaluators::{CorrectnessEvaluator, HelpfulnessEvaluator, JudgeEvaluatorConfig},
    EvaluatorRegistry, EvalRunner, EvalSet,
};
use swink_agent_eval_judges::AnthropicJudgeClient;
use swink_agent_adapters_anthropic::AnthropicAdapter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let adapter = Arc::new(AnthropicAdapter::from_env()?);
    let judge_client = Arc::new(AnthropicJudgeClient::new(adapter));
    let judge_registry = Arc::new(
        JudgeRegistry::builder(judge_client, "claude-sonnet-4-6").build()?
    );

    let mut registry = EvaluatorRegistry::new();
    registry.add(Box::new(CorrectnessEvaluator::new(
        JudgeEvaluatorConfig::default_with(judge_registry.clone())
    )));
    registry.add(Box::new(HelpfulnessEvaluator::new(
        JudgeEvaluatorConfig::default_with(judge_registry.clone())
    )));

    let eval_set = EvalSet::from_file("cases.json")?;
    let agent = build_agent().await?;
    let result = EvalRunner::new(agent, registry).run_set(&eval_set).await?;

    println!("Verdict: {:?}", result.verdict());
    Ok(())
}
```

That's US1 (score runs) + the 10-line happy path promised by SC-001.

---

## 2. Run 200 cases fast with parallelism + caching

**Goal**: Iterate on a judge prompt across 200 cases without re-calling the agent.

```rust
use swink_agent_eval::cache::LocalFileTaskResultStore;

let cache = Arc::new(LocalFileTaskResultStore::new("./.swink-eval-cache".into()));

let runner = EvalRunner::new(agent, registry)
    .with_parallelism(8)
    .with_num_runs(3)                     // judge-variance diagnostic
    .with_cache(cache);

let result = runner.run_set(&eval_set).await?;

// result.cases[0].metrics[0].samples == [0.81, 0.83, 0.82]
// result.cases[0].metrics[0].variance ≈ 0.00011
```

**First run**: every case calls the agent; invocations are cached by content-hashed key.
**Second run** (after tweaking `HelpfulnessEvaluator`'s prompt template): cache hits, agent not invoked, only the judge loop re-executes (per Q2/R-013 clarification). This is SC-003's promise.

---

## 3. Override a built-in judge prompt

**Goal**: Customize `CorrectnessEvaluator`'s rubric with a few-shot example.

```rust
use swink_agent_eval::prompt::{JudgePromptTemplate, FewShotExample};
use swink_agent_eval::evaluators::JudgeEvaluatorConfig;

struct MyCorrectnessTemplate;
impl JudgePromptTemplate for MyCorrectnessTemplate {
    fn version(&self) -> &str { "my_correctness_v0" }
    fn family(&self) -> PromptFamily { PromptFamily::Quality }
    fn render(&self, ctx: &PromptContext) -> Result<String, PromptError> {
        // return rendered prompt string
    }
}

let config = JudgeEvaluatorConfig {
    template: Some(Arc::new(MyCorrectnessTemplate)),
    few_shot_examples: vec![FewShotExample {
        input: "What is 2+2?".into(),
        expected: "4".into(),
        reasoning: Some("Simple arithmetic".into()),
    }],
    use_reasoning: true,
    ..JudgeEvaluatorConfig::default_with(judge_registry.clone())
};

let evaluator = CorrectnessEvaluator::new(config);
```

Each case's result records `prompt_version: "my_correctness_v0"`, making score drift across versions deterministic to trace (SC-005).

---

## 4. Multi-turn simulation with a simulated user and tools

**Goal**: Test an agent's behavior across a 5-turn dialogue without wiring real tools.

```toml
[dependencies]
swink-agent-eval = { version = "0.9", features = ["judge-core", "simulation", "evaluator-agent"] }
```

```rust
use swink_agent_eval::simulation::{
    ActorSimulator, ActorProfile, ToolSimulator, run_multiturn_simulation,
};
use swink_agent_eval::evaluators::GoalSuccessRateEvaluator;

let profile = ActorProfile {
    name: "frustrated_customer".into(),
    traits: vec!["frustrated".into(), "terse".into()],
    context: "Their shipment is 2 weeks late; they've already emailed twice.".into(),
    goal: "Get a firm delivery date or a refund.".into(),
};

let actor = ActorSimulator::new(profile, judge_client.clone(), "claude-sonnet-4-6")
    .with_max_turns(10);

let tool_sim = ToolSimulator::new(judge_client.clone(), "claude-sonnet-4-6")
    .register_tool("lookup_order", order_schema)
    .register_tool("issue_refund", refund_schema);

let invocation = run_multiturn_simulation(
    &agent, &actor, Some(&tool_sim), 10, CancellationToken::new()
).await?;

let eval = GoalSuccessRateEvaluator::new(JudgeEvaluatorConfig::default_with(judge_registry));
let score = eval.evaluate_async(&case, &invocation).await?;
```

---

## 5. Auto-generate 20 diverse test cases

**Goal**: Seed an eval set from a context paragraph.

```toml
[dependencies]
swink-agent-eval = { version = "0.9", features = ["judge-core", "generation"] }
```

```rust
use swink_agent_eval::generation::{ExperimentGenerator, GenerationRequest};

let gen = ExperimentGenerator::new(judge_client.clone(), "claude-sonnet-4-6");

let req = GenerationRequest {
    context: "Our agent handles refunds, shipping questions, and product recommendations \
              for an online outdoor-gear retailer.".into(),
    task: "Verify the agent resolves the user's issue politely and cites \
           company policy correctly.".into(),
    desired_count: 20,
    num_topics: 5,
    include_expected_output: false,
    include_expected_trajectory: true,
    include_expected_interactions: false,
    include_metadata: true,
    agent_tools: Some(agent.tool_defs()),
};

let eval_set = gen.generate(req).await?;
eval_set.save_to_file("generated-cases.json")?;
```

Every emitted case validates (SC-007); trajectory expectations reference only tools the agent has.

---

## 6. Score an OTel trace fetched from Langfuse

**Goal**: Evaluate a production trace offline without re-running the agent.

```toml
[dependencies]
swink-agent-eval = { version = "0.9", features = [
    "judge-core", "evaluator-quality", "trace-ingest", "trace-langfuse"
]}
```

```rust
use swink_agent_eval::trace::{LangfuseTraceProvider, OpenInferenceSessionMapper, TraceProvider};

let provider = LangfuseTraceProvider::from_env()?;  // LANGFUSE_HOST, LANGFUSE_KEY
let raw = provider.fetch_session("session-id-123").await?;

let mapper = OpenInferenceSessionMapper;
let invocation = mapper.map(&raw)?;

// Score with the same evaluators as in-process runs:
let case = EvalCase::from_invocation(&invocation);  // synthesizes minimal case
let score = registry.evaluate_async(&case, &invocation).await?;
```

---

## 7. Emit OTel spans for eval runs

```rust
use swink_agent_eval::telemetry::EvalsTelemetry;
use opentelemetry::global;

let tracer = global::tracer("swink-eval");
let telemetry = EvalsTelemetry::builder()
    .tracer(Arc::new(tracer))
    .build();

let runner = EvalRunner::new(agent, registry)
    .with_telemetry(Arc::new(telemetry));

runner.run_set(&eval_set).await?;
// Produces: swink.eval.run_set → swink.eval.case → swink.eval.evaluator spans
```

---

## 8. Generate reports

```rust
use swink_agent_eval::report::{ConsoleReporter, JsonReporter, MarkdownReporter, HtmlReporter};

// Plain-text for terminals:
print!("{}", ConsoleReporter.render(&result)?);

// Machine-readable for CI:
std::fs::write("result.json", JsonReporter.render(&result)?.as_stdout().as_bytes())?;

// PR comment:
let md = MarkdownReporter.render(&result)?;

// Rich/interactive (html-report feature):
HtmlReporter::new()
    .write_to("report.html", &result)?;
```

---

## 9. Use the `swink-eval` CLI

```bash
cargo install swink-agent-eval --features cli

# Run and gate:
swink-eval run --set cases.json --out result.json --parallelism 4 --reporter md

# Re-render a previous run without re-executing:
swink-eval report result.json --format html > report.html

# Gate-only check (exit 0 pass / non-zero fail, no stdout):
swink-eval gate result.json --gate-config gate.yaml
echo $?   # 0 or 1
```

---

## 10. Wire CI

Copy `eval/src/ci/templates/pr-eval.yml` into your repo at `.github/workflows/pr-eval.yml`:

```yaml
# Simplified — see eval/src/ci/templates/pr-eval.yml for the full template
name: PR Eval
on: pull_request
jobs:
  eval:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install swink-agent-eval --features cli
      - run: swink-eval run --set evals/pr.json --out result.json --reporter md > report.md
      - run: swink-eval gate result.json --gate-config evals/gate.yaml
      - uses: marocchino/sticky-pull-request-comment@v2
        with:
          path: report.md
```

---

## Feature cheat-sheet

| Goal | Crate | Feature |
|---|---|---|
| LLM judges at all | `swink-agent-eval` | `judge-core` |
| Quality rubrics | `swink-agent-eval` | `evaluator-quality` |
| Safety rubrics | `swink-agent-eval` | `evaluator-safety` |
| RAG rubrics + embedding-similarity | `swink-agent-eval` | `evaluator-rag` |
| Multi-agent / trajectory | `swink-agent-eval` | `evaluator-agent` |
| Structured output (JSON match + schema) | `swink-agent-eval` | `evaluator-structured` |
| Exact-match / Levenshtein | `swink-agent-eval` | `evaluator-simple` |
| Code compile + clippy | `swink-agent-eval` | `evaluator-code` |
| Sandboxed execution (Unix only) | `swink-agent-eval` | `evaluator-sandbox` |
| Image-safety multimodal | `swink-agent-eval` | `multimodal` |
| Multi-turn simulation | `swink-agent-eval` | `simulation` |
| Experiment auto-generation | `swink-agent-eval` | `generation` |
| Trace ingestion (always-available in-mem provider) | `swink-agent-eval` | `trace-ingest` |
| OTLP-HTTP trace ingestion | `swink-agent-eval` | `trace-ingest`, `trace-otlp` |
| Langfuse / OpenSearch / CloudWatch | `swink-agent-eval` | `trace-langfuse` / `trace-opensearch` / `trace-cloudwatch` |
| Emit OTel spans for eval runs | `swink-agent-eval` | `telemetry` |
| Self-contained HTML report | `swink-agent-eval` | `html-report` |
| Push to LangSmith | `swink-agent-eval` | `langsmith` |
| `swink-eval` CLI binary | `swink-agent-eval` | `cli` |
| Anthropic / OpenAI / … judge | `swink-agent-eval-judges` | `anthropic` / `openai` / ... |
| All-judges, for docs | `swink-agent-eval-judges` | `all-judges` |
| All-evaluators, for docs | `swink-agent-eval` | `all-evaluators` |
| Canary live-provider tests | `swink-agent-eval-judges` (dev) | `live-judges` |
