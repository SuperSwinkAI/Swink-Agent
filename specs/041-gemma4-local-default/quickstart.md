# Quickstart: Gemma 4 Local Default (Direct Inference)

**Feature**: 041-gemma4-local-default  
**Date**: 2026-04-04

## Prerequisites

- Rust 1.88+ with edition 2024
- `mistralrs` 0.8+ (blocked until NaN logits bug #2051 is fixed)
- ~4 GB free disk space (for E2B Q4_K_M download + cache)
- ~6 GB RAM recommended for E2B inference

## Quick Start

### Using Gemma 4 E2B as default (after feature is stable)

```rust
// In Cargo.toml:
// swink-agent-local-llm = { version = "0.6", features = ["gemma4"] }

use swink_agent_local_llm::default_local_connection;

// Automatically uses Gemma 4 E2B (128K context)
// Downloads ~3.5 GB on first use
let connection = default_local_connection()?;
```

### Overriding the default model

```bash
# Use SmolLM3-3B instead of Gemma 4 E2B
export LOCAL_MODEL_REPO="bartowski/SmolLM3-3B-GGUF"
export LOCAL_MODEL_FILE="SmolLM3-3B-Q4_K_M.gguf"
export LOCAL_CONTEXT_LENGTH=8192
```

### Using a specific Gemma 4 variant

```rust
use swink_agent_local_llm::{ModelPreset, LocalModel, ModelConfig};

// Use the larger 26B variant (requires ~20 GB RAM)
let config = ModelPreset::Gemma4_26B.config();
let model = LocalModel::new(config);
```

### Using without the `gemma4` feature

```rust
// Without the feature, SmolLM3-3B remains the default
// swink-agent-local-llm = "0.6"  (no gemma4 feature)

use swink_agent_local_llm::default_local_connection;
let connection = default_local_connection()?; // SmolLM3-3B
```

## Build & Test

```bash
# Build with Gemma 4 support
cargo build -p swink-agent-local-llm --features gemma4

# Run unit tests (no model download needed)
cargo test -p swink-agent-local-llm --features gemma4

# Run live tests (downloads ~3.5 GB on first run)
cargo test -p swink-agent-local-llm --features gemma4 --test local_live -- --ignored

# Build with Metal acceleration (macOS)
cargo build -p swink-agent-local-llm --features "gemma4,metal"
```

## Alternative Backends (no code changes)

If you prefer not to use the local-llm crate, Gemma 4 E2B works with the existing OpenAI-compatible adapter via external servers. All backends below expose an OpenAI-compatible `/v1/chat/completions` endpoint, so no adapter code changes are needed -- just point `OpenAiStreamFn` at the server URL.

### Ollama (recommended)

The simplest option. Ollama handles model download, quantization, and chat template management automatically.

```bash
# Install Ollama (macOS)
brew install ollama

# Pull and run Gemma 4 E2B (downloads ~3.5 GB on first use)
ollama run gemma4:e2b
```

Then use the Ollama adapter as normal -- no `OpenAiStreamFn` configuration needed.

---

### llama.cpp server

Use llama.cpp's built-in HTTP server for a lightweight, single-binary inference backend.

#### Step 1: Download the GGUF model

Option A -- using `huggingface-cli`:

```bash
pip install huggingface-hub
huggingface-cli download bartowski/google_gemma-4-E2B-it-GGUF \
    gemma-4-E2B-it-Q4_K_M.gguf \
    --local-dir ./models
```

Option B -- direct URL download:

```bash
curl -L -o ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    "https://huggingface.co/bartowski/google_gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_K_M.gguf"
```

#### Step 2: Start the server

```bash
# Basic text + thinking mode
llama-server \
    -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' \
    --port 8080

# With GPU offloading (Metal on macOS, CUDA on Linux)
llama-server \
    -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' \
    --port 8080 \
    -ngl 99
```

The `--chat-template-kwargs '{"enable_thinking":true}'` flag enables Gemma 4's built-in thinking mode. The server applies the model's Jinja chat template and injects the `<|think|>` control token automatically.

#### Step 3: Configure swink-agent

```rust
use swink_agent_adapters::OpenAiStreamFn;
use swink_agent::types::ModelSpec;

let stream_fn = OpenAiStreamFn::new(
    "http://localhost:8080",  // llama.cpp server base URL
    "no-key-needed",          // llama.cpp does not require auth
);

let model_spec = ModelSpec::new("gemma-4-E2B-it")
    .with_context_window(131_072);
```

Or via environment variables:

```bash
export OPENAI_BASE_URL="http://localhost:8080"
export OPENAI_API_KEY="no-key-needed"
export OPENAI_MODEL="gemma-4-E2B-it"
```

#### Example interaction

```bash
# Verify the server is running
curl http://localhost:8080/v1/models

# Send a test completion
curl http://localhost:8080/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d '{
        "model": "gemma-4-E2B-it",
        "messages": [{"role": "user", "content": "What is 2+2?"}],
        "stream": true
    }'
```

---

### vLLM

vLLM provides high-throughput inference with PagedAttention. It serves the original (non-quantized) model weights from HuggingFace, requiring more VRAM than GGUF-based backends.

#### Step 1: Install vLLM

```bash
pip install vllm
```

Requires a CUDA-capable GPU with sufficient VRAM (~8 GB for E2B at float16).

#### Step 2: Start the server

```bash
vllm serve google/gemma-4-E2B-it \
    --enable-auto-tool-choice \
    --reasoning-parser gemma4 \
    --tool-call-parser gemma4 \
    --port 8000
```

Flag explanations:
- `--enable-auto-tool-choice`: Allows the model to decide when to use tools
- `--reasoning-parser gemma4`: Parses Gemma 4's `<|channel>thought\n...<channel|>` thinking output
- `--tool-call-parser gemma4`: Parses Gemma 4's `<|tool_call>call:{name}{args}<tool_call|>` format

The model downloads from HuggingFace on first launch and caches in `~/.cache/huggingface/hub/`.

#### Step 3: Configure swink-agent

```rust
use swink_agent_adapters::OpenAiStreamFn;
use swink_agent::types::ModelSpec;

let stream_fn = OpenAiStreamFn::new(
    "http://localhost:8000",  // vLLM server base URL
    "no-key-needed",          // local vLLM does not require auth
);

let model_spec = ModelSpec::new("google/gemma-4-E2B-it")
    .with_context_window(131_072);
```

Or via environment variables:

```bash
export OPENAI_BASE_URL="http://localhost:8000"
export OPENAI_API_KEY="no-key-needed"
export OPENAI_MODEL="google/gemma-4-E2B-it"
```

#### Note on `reasoning_content` field

vLLM may emit thinking content in a `reasoning_content` field within the streaming response delta, separate from the standard `content` field. The `openai_compat` adapter currently processes the `content` field for text output. If thinking content from vLLM does not appear in agent events, check whether the adapter needs to read `reasoning_content` from the delta in addition to `content`. This is a known area to verify when integrating vLLM with thinking-enabled models.

---

### LM Studio

LM Studio provides a GUI-based experience for downloading and serving local models. It exposes an OpenAI-compatible API on `localhost`.

#### Step 1: Download the model

1. Open LM Studio
2. Search for `gemma-4-E2B-it` in the model search bar
3. Select the `Q4_K_M` GGUF variant from bartowski (or your preferred quantization)
4. Click **Download** (~3.5 GB for Q4_K_M)

#### Step 2: Start the server

1. Go to the **Local Server** tab (or **Developer** tab in newer versions)
2. Load the downloaded Gemma 4 E2B model
3. Enable the local server (default port: 1234)
4. Optionally configure context length, GPU offloading, and other inference parameters in the server settings

#### Step 3: Configure swink-agent

```rust
use swink_agent_adapters::OpenAiStreamFn;
use swink_agent::types::ModelSpec;

let stream_fn = OpenAiStreamFn::new(
    "http://localhost:1234",  // LM Studio default server port
    "lm-studio",              // LM Studio accepts any non-empty key
);

let model_spec = ModelSpec::new("gemma-4-E2B-it")
    .with_context_window(131_072);
```

Or via environment variables:

```bash
export OPENAI_BASE_URL="http://localhost:1234"
export OPENAI_API_KEY="lm-studio"
export OPENAI_MODEL="gemma-4-E2B-it"
```

#### Known limitation: streaming + tool calling

LM Studio has a known bug where streaming responses combined with tool calling can produce malformed output or hang (lmstudio-bug-tracker#1066). If you experience issues with tool calls not completing or producing garbled JSON, disable streaming as a workaround:

```rust
// Workaround: disable streaming for tool-calling requests
let model_spec = ModelSpec::new("gemma-4-E2B-it")
    .with_context_window(131_072)
    .with_stream(false);  // disable streaming to avoid #1066
```

Text-only inference (without tools) works reliably with streaming enabled.

---

## Backend Comparison

| Feature | local-llm crate | Ollama | llama.cpp server | vLLM | LM Studio |
|---|---|---|---|---|---|
| Setup complexity | Cargo feature flag | `brew install` | Manual download + CLI | `pip install` + GPU | GUI download |
| Quantization | Q4_K_M GGUF | Automatic | Any GGUF | Float16 / AWQ | Any GGUF |
| VRAM (E2B) | ~4 GB | ~4 GB | ~4 GB | ~8 GB | ~4 GB |
| Thinking mode | Built-in | `think: true` | Chat template flag | `--reasoning-parser` | Prompt injection |
| Tool calling | Native parser | Automatic | Chat template | `--tool-call-parser` | Buggy with streaming |
| Adapter | N/A (in-process) | `OllamaStreamFn` | `OpenAiStreamFn` | `OpenAiStreamFn` | `OpenAiStreamFn` |
| GPU acceleration | Metal/CUDA flags | Automatic | `-ngl` flag | Automatic (CUDA) | GUI toggle |

---

## Known Limitations

### LM Studio: streaming + tool calling bug

LM Studio has a confirmed bug (lmstudio-bug-tracker#1066) where enabling both streaming and tool calling produces malformed tool call JSON or causes the response to hang mid-stream.

**Workaround**: Disable streaming for requests that include tool definitions. Text-only streaming (without tools registered) is unaffected. Configure `ModelSpec::with_stream(false)` when tools are in use with LM Studio.

**Status**: Open upstream. Monitor the LM Studio bug tracker for a fix.

### vLLM: `reasoning_content` field

When vLLM serves a model with reasoning/thinking enabled, it may place the thinking content in a `reasoning_content` field on the streaming delta object rather than in the standard `content` field. The `openai_compat` shared adapter infrastructure processes `content` for text deltas. If thinking content from vLLM is missing from agent events, the `openai_compat` module may need to be extended to read the `reasoning_content` field and emit `ThinkingDelta` events from it.

**Workaround**: Use the Ollama adapter or local-llm crate for reliable thinking event parsing. Both handle Gemma 4's thinking format natively.

### General: GGUF quantization quality varies by source

Not all GGUF quantizations are equal. The quality of a quantized model depends on the quantization method, calibration data, and source. The recommended source for Gemma 4 GGUF files is **bartowski** (`bartowski/google_gemma-4-E2B-it-GGUF`), which provides well-tested quantizations under Apache 2.0 license.

Symptoms of a poor quantization include:
- Garbled or repetitive output
- Degraded reasoning quality compared to the original model
- NaN or infinite values in logit computation (more likely with MoE variants)

When using GGUF files from other sources, verify output quality before deploying to production.

### mistral.rs: NaN logits with MoE variants

The upstream inference engine (mistral.rs) has a known NaN logits bug (#2051) affecting MoE architecture variants (E4B, 26B) with BF16 and UQFF quantization. The E2B variant (dense architecture) with Q4_K_M GGUF is likely unaffected, but should be validated via live testing. E4B and 26B presets in the local-llm crate remain behind the `gemma4` feature flag until the upstream fix ships in a tagged release.
