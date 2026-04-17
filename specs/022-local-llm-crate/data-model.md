# Data Model: Local LLM Crate

**Branch**: `022-local-llm-crate` | **Date**: 2026-03-20

## Entities

### LocalModel

On-device language model capable of text generation from quantized GGUF weights. Manages the model lifecycle from download through inference.

| Field | Type | Description |
|-------|------|-------------|
| `state` | `Arc<Mutex<ModelState>>` | Current lifecycle state: `Unloaded`, `Downloading`, `Loading`, `Ready`, or `Failed`. |
| `config` | `ModelConfig` | Configuration for this model (source, context length, quantization). |
| `runner` | `Option<Arc<LlamaModel>>` | The loaded llama.cpp inference model. `None` until `Ready`. |
| `progress_callback` | `Option<ProgressCallbackFn>` | Optional callback for download/load progress reporting. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(config: ModelConfig) -> Self` | Create a model in `Unloaded` state with the given configuration. |
| `from_preset` | `(preset: ModelPreset) -> Self` | Create a model from a named preset. |
| `with_progress` | `(self, callback: ProgressCallbackFn) -> Result<Self>` | Attach a progress callback. Must be called before `ensure_ready()`. |
| `ensure_ready` | `async fn(&self) -> Result<(), LocalModelError>` | Download (if needed) and load the model. Transitions through `Downloading` → `Loading` → `Ready`. No-op if already `Ready`. |
| `send_chat_request` | `async fn(&self, messages: Vec<LocalMessage>) -> Result<Response>` | Run inference on the loaded model. Errors if not `Ready`. |

### ModelConfig

Configuration parameters for a local model.

| Field | Type | Description |
|-------|------|-------------|
| `repo_id` | `String` | HuggingFace repository ID (e.g., `"HuggingFaceTB/SmolLM3-3B-GGUF"`). |
| `filename` | `String` | Model filename within the repository (e.g., `"smollm3-3b-q4_k_m.gguf"`). |
| `context_length` | `usize` | Maximum context window in tokens. Default: 8192. Overridable via `LOCAL_CONTEXT_LENGTH` env var. |
| `chat_template` | `Option<String>` | Optional chat template override. If `None`, uses the model's built-in template. |

### ModelState (enum)

Lifecycle state of a local model.

| Variant | Description |
|---------|-------------|
| `Unloaded` | Initial state. No model files downloaded or loaded. |
| `Downloading` | Model weights are being downloaded from HuggingFace. |
| `Loading` | Model weights are being loaded into memory from cache. |
| `Ready` | Model is loaded and ready for inference. |
| `Failed(String)` | Model failed to download or load. Contains error description. |

### LocalStreamFn

Streaming function adapter that bridges `LocalModel` to the standard `StreamFn` interface. Wraps local inference results into the `AssistantMessageEvent` protocol.

| Field | Type | Description |
|-------|------|-------------|
| `model` | `Arc<LocalModel>` | The local model instance to use for inference. |
| `convert` | `ConvertToLocalFn` | Message conversion function from `LlmMessage` to local format. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(model: Arc<LocalModel>) -> Self` | Create a stream function wrapping the given model. |

**StreamFn implementation**: Converts incoming `LlmMessage` list to local format, calls `send_chat_request`, wraps the response into `AssistantMessageEvent` stream (Start → ContentBlockStart → ContentBlockDelta → ContentBlockEnd → Done). Parses `<think>` tags into ThinkingStart/Delta/End events. Cost is always zero.

### EmbeddingModel

On-device embedding model that converts text passages into fixed-dimensional vectors.

| Field | Type | Description |
|-------|------|-------------|
| `state` | `Arc<Mutex<ModelState>>` | Current lifecycle state (same as `LocalModel`). |
| `config` | `ModelConfig` | Configuration for the embedding model. |
| `runner` | `Option<Arc<LlamaModel>>` | The loaded llama.cpp embedding model. `None` until `Ready`. |
| `progress_callback` | `Option<ProgressCallbackFn>` | Optional callback for download/load progress reporting. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(config: ModelConfig) -> Self` | Create an embedding model in `Unloaded` state. |
| `from_preset` | `(preset: ModelPreset) -> Self` | Create from a named preset (default: `EmbeddingGemma300M`). |
| `with_progress` | `(self, callback: ProgressCallbackFn) -> Result<Self>` | Attach a progress callback. |
| `ensure_ready` | `async fn(&self) -> Result<(), LocalModelError>` | Download and load the embedding model. |
| `embed` | `async fn(&self, text: &str) -> Result<Vec<f32>, LocalModelError>` | Compute an embedding vector. Returns error if text exceeds max input length. |
| `embed_batch` | `async fn(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, LocalModelError>` | Batch embedding for multiple texts. |

### ModelPreset (enum)

Named configuration bundles for supported local models.

| Variant | Model | Quantization | Context | Description |
|---------|-------|-------------|---------|-------------|
| `SmolLM3_3B` | SmolLM3-3B | Q4_K_M | 8192 | Default text generation and tool-calling model. ~2.1 GB. |
| `EmbeddingGemma300M` | EmbeddingGemma-300M | — | — | Default embedding model for text vectorization. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `config` | `(&self) -> ModelConfig` | Convert the preset to a `ModelConfig`. |
| `all` | `() -> &'static [ModelPreset]` | List all available presets. |

### ProgressCallbackFn

Type alias for the progress reporting callback.

```rust
pub type ProgressCallbackFn = Arc<dyn Fn(ProgressEvent) + Send + Sync>;
```

### ProgressEvent (enum)

Progress events emitted during download and loading.

| Variant | Fields | Description |
|---------|--------|-------------|
| `DownloadProgress` | `bytes_downloaded: u64, total_bytes: Option<u64>` | Download progress. `total_bytes` may be `None` if server does not report content length. |
| `DownloadComplete` | — | Download finished successfully. |
| `LoadingProgress` | `message: String` | Loading status message from llama.cpp. |
| `LoadingComplete` | — | Model loaded and ready. |

### LocalModelError (enum, thiserror)

Error type for local model operations.

| Variant | Description |
|---------|-------------|
| `Download(String)` | Model download failed. Wraps network or I/O errors. |
| `Loading(String)` | Model loading failed. Covers GGUF parse errors, OOM, and corrupted files. |
| `Inference(String)` | Inference failed. Covers runtime model errors. |
| `Embedding(String)` | Embedding failed. Covers input-too-long and model errors. |
| `NotReady` | Operation attempted before `ensure_ready()` completed. |

## Relationships

```
ModelPreset ----config()----> ModelConfig

LocalModel ----uses----> ModelConfig
    |                     ModelState
    |                     ProgressCallbackFn
    v
LocalStreamFn ----wraps----> LocalModel
    |                         |
    v  implements              v  uses
  StreamFn                  convert.rs (LlmMessage → local format)

EmbeddingModel ----uses----> ModelConfig
                              ModelState
                              ProgressCallbackFn

LocalModelError <----returned by---- LocalModel, LocalStreamFn, EmbeddingModel
```

## Model Lifecycle

```
Unloaded ──ensure_ready()──► Downloading ──► Loading ──► Ready
    │                            │              │
    │                            ▼              ▼
    └────────────────────────► Failed ◄─────────┘
```

- `ensure_ready()` is idempotent: no-op if already `Ready`.
- `Failed` state includes a description of the failure for diagnostics.
- Download is skipped if model files are already cached in `~/.cache/huggingface/hub/`.
