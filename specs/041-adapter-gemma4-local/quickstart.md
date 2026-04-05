# Quickstart: Gemma 4 Local Default (Direct Inference)

**Feature**: 041-adapter-gemma4-local  
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

If you prefer not to use the local-llm crate, Gemma 4 E2B works with the existing OpenAI-compatible adapter via external servers. Each backend below exposes an OpenAI-compatible `/v1/chat/completions` endpoint that works with `OpenAiStreamFn` out of the box.

### Ollama (recommended)

Ollama is the simplest option -- single command, automatic model management, full thinking and tool calling support.

**Prerequisites:**
- [Ollama](https://ollama.ai) installed (macOS: `brew install ollama`, Linux: `curl -fsSL https://ollama.com/install.sh | sh`)

**1. Pull and run the model:**
```bash
# Downloads ~3.5 GB on first pull
ollama pull gemma4:e2b

# Start serving (runs on http://localhost:11434 by default)
ollama serve
```

**2. Configure the adapter:**
```rust
use swink_agent_adapters::OllamaStreamFn;

let stream_fn = OllamaStreamFn::new("gemma4:e2b");
// Thinking mode: set thinking_level to Low/Medium/High in ModelSpec capabilities
// Tool calling: works out of the box via Ollama's native tool support
```

**Thinking mode:** Ollama handles `<|think|>` injection automatically when thinking is enabled. No special flags needed.

**Tool calling:** Fully supported via Ollama's native tool call parsing.

---

### llama.cpp server

llama.cpp server provides high-performance GGUF inference with fine-grained control over quantization, context length, and GPU layers.

**Prerequisites:**
- [llama.cpp](https://github.com/ggml-org/llama.cpp) built from source or installed via package manager
  - macOS: `brew install llama.cpp`
  - From source: `git clone https://github.com/ggml-org/llama.cpp && cd llama.cpp && cmake -B build && cmake --build build --config Release`

**1. Download the model:**
```bash
# Option A: Use huggingface-cli
pip install huggingface-hub
huggingface-cli download bartowski/google_gemma-4-E2B-it-GGUF \
    gemma-4-E2B-it-Q4_K_M.gguf --local-dir ./models

# Option B: Direct download
curl -L -o ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    "https://huggingface.co/bartowski/google_gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_K_M.gguf"
```

**2. Start the server:**
```bash
# Basic (CPU only)
llama-server \
    -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --port 8080

# With thinking mode enabled
llama-server \
    -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' \
    --port 8080

# With GPU offloading (all layers to GPU)
llama-server \
    -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' \
    --n-gpu-layers 999 \
    --port 8080

# With custom context length (default is model's full 128K)
llama-server \
    -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' \
    --ctx-size 8192 \
    --port 8080
```

**3. Configure the adapter:**
```rust
use swink_agent_adapters::OpenAiStreamFn;

let stream_fn = OpenAiStreamFn::new(
    "http://localhost:8080/v1",  // llama.cpp server endpoint
    "not-needed",                // API key (llama.cpp ignores this)
    "gemma-4-E2B-it-Q4_K_M",    // model name (must match loaded model)
);
```

**Thinking mode:** Use `--chat-template-kwargs '{"enable_thinking":true}'` on the server command line. The server injects the thinking control token into the chat template automatically. Thinking output appears in the `reasoning_content` field of streamed chunks.

**Tool calling:** llama.cpp server supports tool calling via its built-in chat template handling. Pass tools in the standard OpenAI format in API requests.

---

### vLLM

vLLM provides high-throughput serving with PagedAttention, continuous batching, and native Gemma 4 support including reasoning and tool calling parsers.

**Prerequisites:**
- Python 3.9+
- NVIDIA GPU with CUDA support (vLLM does not support CPU-only or Metal)
- `pip install vllm` (installs PyTorch + CUDA dependencies automatically)

**1. Start the server (model downloads automatically):**
```bash
# Basic serving (downloads ~5 GB from HuggingFace on first run)
vllm serve google/gemma-4-E2B-it \
    --port 8000

# With tool calling and reasoning support
vllm serve google/gemma-4-E2B-it \
    --enable-auto-tool-choice \
    --reasoning-parser gemma4 \
    --tool-call-parser gemma4 \
    --port 8000

# With quantization for lower memory usage
vllm serve google/gemma-4-E2B-it \
    --enable-auto-tool-choice \
    --reasoning-parser gemma4 \
    --tool-call-parser gemma4 \
    --quantization awq \
    --port 8000

# With custom tensor parallelism (multi-GPU)
vllm serve google/gemma-4-E2B-it \
    --enable-auto-tool-choice \
    --reasoning-parser gemma4 \
    --tool-call-parser gemma4 \
    --tensor-parallel-size 2 \
    --port 8000
```

**2. Configure the adapter:**
```rust
use swink_agent_adapters::OpenAiStreamFn;

let stream_fn = OpenAiStreamFn::new(
    "http://localhost:8000/v1",  // vLLM server endpoint
    "not-needed",                // API key (vLLM ignores this by default)
    "google/gemma-4-E2B-it",    // must match the model name passed to vllm serve
);
```

**Thinking mode:** Use `--reasoning-parser gemma4` to enable reasoning extraction. vLLM parses `<|channel>thought\n...<channel|>` delimiters and exposes thinking content via the `reasoning_content` field in streamed response chunks.

**Tool calling:** Use `--enable-auto-tool-choice --tool-call-parser gemma4` to enable native Gemma 4 tool call parsing. Pass tools in the standard OpenAI format. vLLM parses the `<|tool_call>call:{name}{args}<tool_call|>` format automatically.

---

### LM Studio

LM Studio provides a desktop GUI for model management and an OpenAI-compatible local server. Suitable for developers who prefer a graphical interface.

**Prerequisites:**
- [LM Studio](https://lmstudio.ai) installed (macOS, Windows, or Linux)

**1. Download the model:**
- Open LM Studio
- Search for `gemma-4-E2B-it-GGUF` in the model search bar
- Select the `Q4_K_M` quantization variant (recommended, ~3.5 GB)
- Click Download and wait for completion

**2. Start the server:**
- In LM Studio, go to the "Local Server" tab (left sidebar)
- Load the downloaded Gemma 4 E2B model
- Click "Start Server" (default port: 1234)
- Alternatively, start from the command line:
  ```bash
  # If lms CLI is installed
  lms server start
  lms load gemma-4-E2B-it-Q4_K_M
  ```

**3. Configure the adapter:**
```rust
use swink_agent_adapters::OpenAiStreamFn;

let stream_fn = OpenAiStreamFn::new(
    "http://localhost:1234/v1",       // LM Studio server endpoint
    "lm-studio",                      // API key (LM Studio accepts any non-empty string)
    "gemma-4-E2B-it-Q4_K_M",         // model identifier as shown in LM Studio
);
```

**Thinking mode:** Configure in LM Studio's model settings UI. Enable "Reasoning" or set the chat template to include thinking tokens. Behavior depends on LM Studio version.

**Tool calling:** See Known Limitations below.

---

### Known Limitations

#### LM Studio: Streaming + Tool Calling Bug

LM Studio has a known bug where streaming responses combined with tool calling can produce malformed or incomplete tool call output ([LM Studio issue #1066](https://github.com/lmstudio-community/lmstudio-bugs/issues/1066)). Symptoms include:

- Tool call arguments arriving as empty or truncated JSON
- Streaming hanging after a tool call response
- Duplicate or missing tool call delimiters in the stream

**Workaround:** Disable streaming when using tool calling with LM Studio, or use LM Studio for text-only / thinking-only workloads and switch to Ollama or llama.cpp for tool-calling scenarios.

#### vLLM: `reasoning_content` Field Behavior

vLLM exposes Gemma 4 thinking output in a `reasoning_content` field within streamed chunks, separate from the main `content` field. This differs from Ollama (which uses a `think` field) and llama.cpp (which inlines thinking in the content with delimiters or uses `reasoning_content` depending on version).

Key behavior differences:
- The `reasoning_content` field may appear in the `delta` object of streamed chunks alongside (not instead of) regular `content`
- When `--reasoning-parser gemma4` is not set, thinking content appears inline in the `content` field with raw `<|channel>thought\n...<channel|>` delimiters
- The swink-agent OpenAI adapter handles the standard `content` field; if your vLLM version places thinking in `reasoning_content`, you may need to post-process or rely on the adapter's raw content parsing

**Recommendation:** Always pass `--reasoning-parser gemma4` to vLLM so it handles delimiter parsing server-side.

#### General: Context Length and Memory

All backends require sufficient RAM/VRAM for the configured context length:
- Q4_K_M at 8K context: ~6 GB RAM
- Q4_K_M at 32K context: ~10 GB RAM
- Q4_K_M at full 128K context: ~20+ GB RAM

If you encounter out-of-memory errors, reduce the context length via backend-specific flags (`--ctx-size` for llama.cpp, `--max-model-len` for vLLM, or model settings in LM Studio).

#### General: Model Naming Across Backends

Each backend uses a different model identifier in API requests. The model name in your adapter configuration must match what the backend expects:

| Backend | Model identifier |
|---------|-----------------|
| Ollama | `gemma4:e2b` |
| llama.cpp | Filename without extension (e.g., `gemma-4-E2B-it-Q4_K_M`) |
| vLLM | HuggingFace repo ID (e.g., `google/gemma-4-E2B-it`) |
| LM Studio | As shown in LM Studio UI (e.g., `gemma-4-E2B-it-Q4_K_M`) |
