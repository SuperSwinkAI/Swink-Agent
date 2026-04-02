# Quickstart: AWS Bedrock Adapter

**Feature**: 019-adapter-bedrock | **Date**: 2026-04-02

## Prerequisites

- Rust 1.88+ (edition 2024)
- AWS credentials with Bedrock access (set `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optionally `AWS_SESSION_TOKEN`)
- AWS region with Bedrock models enabled (set `AWS_REGION`)

## Add Dependency

```toml
[dependencies]
swink-agent-adapters = { path = "../adapters", features = ["bedrock"] }
```

## Basic Usage

```rust
use swink_agent_adapters::BedrockStreamFn;
use swink_agent::types::ModelSpec;

let stream_fn = BedrockStreamFn::new(
    std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into()),
    std::env::var("AWS_ACCESS_KEY_ID").unwrap(),
    std::env::var("AWS_SECRET_ACCESS_KEY").unwrap(),
    std::env::var("AWS_SESSION_TOKEN").ok(),
);

let agent = Agent::builder()
    .model(ModelSpec::new("us.anthropic.claude-sonnet-4-6-v1:0"))
    .stream_fn(stream_fn)
    .build();
```

## Using Model Catalog Presets

```rust
use swink_agent::model_catalog::ModelCatalog;
use swink_agent_adapters::build_remote_connection;

let catalog = ModelCatalog::load();
let preset = catalog.preset("anthropic_claude_sonnet_46").unwrap();
let (stream_fn, model_spec) = build_remote_connection(&preset, None)?;
```

## Run Tests

```bash
# Unit tests (no AWS credentials needed)
cargo test -p swink-agent-adapters --features bedrock

# Live tests (requires AWS credentials + Bedrock access)
cargo test -p swink-agent-adapters --test bedrock_live -- --ignored
```

## Available Models (selected)

| Preset ID | Provider | Model | Best For |
|-----------|----------|-------|----------|
| `anthropic_claude_opus_46` | Anthropic | Claude Opus 4.6 | Complex reasoning, 1M context |
| `anthropic_claude_sonnet_46` | Anthropic | Claude Sonnet 4.6 | Balanced quality/speed |
| `meta_llama_4_scout` | Meta | Llama 4 Scout 17B | Long context (3.5M), vision |
| `amazon_nova_2_lite` | Amazon | Nova 2 Lite | Fast, cheap reasoning |
| `deepseek_v3_2` | DeepSeek | V3.2 | Tool calling, cost-effective |
| `openai_gpt_oss_120b` | OpenAI | GPT-OSS 120B | Large open-weight model |
