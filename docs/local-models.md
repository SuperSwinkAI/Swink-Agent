# Running Local Models with Swink Agent

This guide covers every supported path for running a local LLM with Swink Agent. **Gemma 4 E2B** is used as the worked example throughout, but the same patterns apply to any compatible model.

## Table of Contents

- [Ollama (Primary Supported Path)](#ollama-primary-supported-path)
- [llama.cpp Server](#llamacpp-server)
- [vLLM](#vllm)
- [LM Studio](#lm-studio)
- [Custom Ollama Modelfile](#custom-ollama-modelfile)
- [Direct llama.cpp Path](#direct-llamacpp-path)

---

## Ollama (Primary Supported Path)

Ollama is the recommended way to run local models with Swink Agent. It handles model downloading, quantization selection, and GPU offloading automatically. The model catalog ships with pre-configured Ollama presets for Gemma 4 variants.

### Setup

1. Install Ollama: <https://ollama.com/download>

2. Pull the model:

   ```bash
   ollama pull gemma4:e2b
   ```

   Other available Gemma 4 tags: `gemma4:e4b`, `gemma4:26b`.

3. Verify the server is running (default: `http://localhost:11434`):

   ```bash
   ollama list
   ```

### Usage with Swink Agent

Use `OllamaStreamFn` from the adapters crate:

```rust
use swink_agent_adapters::OllamaStreamFn;

let stream_fn = OllamaStreamFn::new("http://localhost:11434");
```

The Ollama adapter uses NDJSON streaming (not SSE) and requires no authentication. Gemma 4 models registered in the model catalog use these preset IDs:

| Preset ID       | Ollama Tag    | Context Window | Notes          |
|-----------------|---------------|----------------|----------------|
| `gemma4_e2b`    | `gemma4:e2b`  | 128K tokens    | Default preset |
| `gemma4_e4b`    | `gemma4:e4b`  | 128K tokens    |                |
| `gemma4_26b`    | `gemma4:26b`  | 256K tokens    | Large; MoE     |

Ollama handles tool calling, thinking mode, and streaming natively for Gemma 4.

---

## llama.cpp Server

llama.cpp's built-in HTTP server exposes an OpenAI-compatible `/v1/chat/completions` endpoint, so it works with `OpenAiStreamFn`.

### Setup

1. Build llama.cpp with your preferred backend:

   ```bash
   git clone https://github.com/ggerganov/llama.cpp
   cd llama.cpp
   cmake -B build -DGGML_CUDA=ON    # or -DGGML_METAL=ON for Apple Silicon
   cmake --build build --config Release
   ```

2. Download a GGUF quantization of Gemma 4 E2B (e.g. from HuggingFace).

3. Start the server:

   ```bash
   ./build/bin/llama-server \
     -m /path/to/gemma-4-E2B-it-Q4_K_M.gguf \
     --port 8080 \
     -c 8192 \
     -ngl 99
   ```

   `-ngl 99` offloads all layers to GPU. Adjust `-c` for your available VRAM.

### Usage with Swink Agent

```rust
use swink_agent_adapters::OpenAiStreamFn;

// llama.cpp server requires no API key; pass an empty string
let stream_fn = OpenAiStreamFn::new("http://localhost:8080", "");
```

Point the `ModelSpec` at whatever model name llama.cpp reports (usually the filename stem). Tool calling support depends on the llama.cpp version and model; check llama.cpp docs for `--jinja` flag requirements.

---

## vLLM

vLLM provides an OpenAI-compatible API server with high-throughput batched inference. It works with `OpenAiStreamFn`.

### Setup

1. Install vLLM:

   ```bash
   pip install vllm
   ```

2. Start the server with Gemma 4 E2B:

   ```bash
   vllm serve google/gemma-4-E2B-it \
     --port 8000 \
     --max-model-len 8192 \
     --enable-auto-tool-choice \
     --tool-call-parser hermes
   ```

   `--enable-auto-tool-choice` and `--tool-call-parser` are required for tool calling support.

### Usage with Swink Agent

```rust
use swink_agent_adapters::OpenAiStreamFn;

// vLLM does not require authentication by default
let stream_fn = OpenAiStreamFn::new("http://localhost:8000", "");
```

Set the `model_id` in your `ModelSpec` to `"google/gemma-4-E2B-it"` (the HuggingFace repo ID you passed to `vllm serve`).

### Known Limitation: `reasoning_content` Not Parsed

vLLM surfaces model thinking output via an extended OpenAI field (`delta.reasoning_content` in streaming chunks). The `openai_compat` module does **not** currently parse this field, so thinking content from vLLM will be silently dropped rather than surfaced as `ThinkingStart`/`ThinkingDelta`/`ThinkingEnd` events. See [issue #445](https://github.com/SuperSwinkAI/Swink-Agent/issues/445) for tracking. Regular text and tool call output works normally.

---

## LM Studio

LM Studio provides a GUI for downloading and running models, with a built-in OpenAI-compatible server. It works with `OpenAiStreamFn`.

### Setup

1. Download LM Studio: <https://lmstudio.ai/>

2. In the LM Studio UI, search for and download a Gemma 4 E2B GGUF quantization.

3. Load the model and start the local server. By default it listens on `http://localhost:1234`.

4. Verify the server is running:

   ```bash
   curl http://localhost:1234/v1/models
   ```

### Usage with Swink Agent

```rust
use swink_agent_adapters::OpenAiStreamFn;

let stream_fn = OpenAiStreamFn::new("http://localhost:1234", "lm-studio");
```

LM Studio accepts any non-empty string as the API key. Set `model_id` in your `ModelSpec` to the model identifier shown in LM Studio's server panel (typically the filename or a short alias).

Tool calling support depends on LM Studio's version and the loaded model's chat template.

---

## Custom Ollama Modelfile

For fine-tuned weights, custom quantizations, or non-standard chat templates, you can create a custom Ollama Modelfile and register it as a local model.

### Create a Modelfile

```dockerfile
FROM /path/to/your-gemma4-e2b-finetune.gguf

TEMPLATE """{{ if .System }}<start_of_turn>system
{{ .System }}<end_of_turn>
{{ end }}{{ range .Messages }}{{ if eq .Role "user" }}<start_of_turn>user
{{ .Content }}<end_of_turn>
{{ else if eq .Role "assistant" }}<start_of_turn>model
{{ .Content }}<end_of_turn>
{{ end }}{{ end }}<start_of_turn>model
"""

PARAMETER stop "<end_of_turn>"
PARAMETER num_ctx 8192
PARAMETER temperature 0.7
```

### Register and run

```bash
ollama create my-gemma4-finetune -f Modelfile
ollama run my-gemma4-finetune "Hello"
```

### Usage with Swink Agent

Use `OllamaStreamFn` with the custom model name:

```rust
use swink_agent_adapters::OllamaStreamFn;

let stream_fn = OllamaStreamFn::new("http://localhost:11434");
// Set model_id to "my-gemma4-finetune" in your ModelSpec
```

This approach is useful when you need to:
- Run a fine-tuned checkpoint not available in the Ollama library
- Override the default chat template or system prompt
- Set specific parameter defaults (temperature, top_k, num_ctx)

---

## Direct llama.cpp Path

The `swink-agent-local-llm` crate provides direct in-process inference via llama.cpp (Rust bindings: `llama-cpp-2`). This eliminates the need for an external server process. All models use GGUF format.

### When to use this

- You want a single self-contained binary with no external server dependency
- You need programmatic control over model loading, progress callbacks, and state transitions
- You are building an embedded or offline application

### Available backends

Enable GPU acceleration via Cargo feature flags on `swink-agent-local-llm`:

| Feature      | Backend        | Platform          |
|--------------|----------------|-------------------|
| _(none)_     | CPU-only       | All               |
| `metal`      | Apple Metal    | macOS (Apple Silicon) |
| `cuda`       | NVIDIA CUDA    | Linux, Windows    |
| `vulkan`     | Vulkan         | Linux, Windows    |

Gemma 4 E2B works on CPU (unlike the previous mistralrs-based implementation), but GPU acceleration is strongly recommended for usable performance:

```toml
[dependencies]
swink-agent-local-llm = { path = "../local-llm", features = ["gemma4", "metal"] }
```

### Usage

```rust
use swink_agent_local_llm::{LocalModel, LocalStreamFn, ModelPreset};

// Create a model from a preset (downloads ~5 GB on first run)
let model = LocalModel::from_preset(ModelPreset::Gemma4E2B);

// Optional: track download and loading progress
let model = model.with_progress(|event| {
    println!("{event:?}");
})?;

// Ensure the model is downloaded and loaded
model.ensure_ready().await?;

// Create a StreamFn for use with the agent loop
let stream_fn = LocalStreamFn::new(model);
```

Models are cached in `~/.cache/huggingface/hub/` and reused across runs. The `ModelState` lifecycle is: `Unloaded -> Downloading -> Loading -> Ready` (or `Failed`).

### Available presets

| Preset             | Model              | Size    | Context | Feature Gate |
|--------------------|--------------------|---------|---------|-------------|
| `SmolLM3_3B`      | SmolLM3-3B (GGUF)  | ~1.9 GB | 8K      | _(none)_    |
| `Gemma4E2B`       | Gemma 4 E2B        | ~5 GB   | 128K    | `gemma4`    |
| `Gemma4E4B`       | Gemma 4 E4B        | ~5.5 GB | 128K    | `gemma4`    |
| `Gemma4_26B`      | Gemma 4 26B MoE    | ~16 GB  | 256K    | `gemma4`    |
| `Gemma4_31B`      | Gemma 4 31B Dense  | ~20 GB  | 256K    | `gemma4`    |

### Build notes

On macOS with Metal, you may need Apple's Metal Toolchain installed:

```bash
xcodebuild -downloadComponent MetalToolchain
```

On Windows with CUDA, you must have the MSVC C++ compiler (`cl.exe`) in `PATH`. Build from a Visual Studio 2022 Developer Command Prompt.
