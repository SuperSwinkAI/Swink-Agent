# Public API Contract: Local LLM Crate

**Branch**: `022-local-llm-crate` | **Date**: 2026-03-20

## Crate: `swink-agent-local-llm`

All public types are re-exported from `lib.rs`. Consumers use `use swink_agent_local_llm::*`.

---

### `LocalModel`

```rust
pub struct LocalModel { /* state, config, runner, progress_callback */ }

impl LocalModel {
    pub fn new(config: ModelConfig) -> Self;
    pub fn from_preset(preset: ModelPreset) -> Self;
    pub fn with_progress(self, callback: ProgressCallbackFn) -> Result<Self, LocalModelError>;
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError>;
}
```

**Invariants**:
- `new` creates a model in `Unloaded` state; no I/O occurs.
- `from_preset` is equivalent to `new(preset.config())`.
- `with_progress` must be called before `ensure_ready()`; returns `Err` if called after.
- `ensure_ready()` is idempotent: no-op if already `Ready`, re-attempts if `Failed`.
- Downloads model weights on first call if not cached in `~/.cache/huggingface/hub/`.
- Transitions: `Unloaded` → `Downloading` → `Loading` → `Ready` (or `Failed`).
- Integrity verification is handled by `hf-hub` (ETag/SHA). No separate checksum step.

---

### `ModelConfig`

```rust
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub repo_id: String,
    pub filename: String,
    pub context_length: usize,
    pub chat_template: Option<String>,
}
```

**Invariants**:
- `context_length` defaults to 8192 for inference models.
- `LOCAL_CONTEXT_LENGTH` env var overrides `context_length` at runtime.

---

### `ModelState`

```rust
#[derive(Debug, Clone)]
pub enum ModelState {
    Unloaded,
    Downloading,
    Loading,
    Ready,
    Failed(String),
}
```

---

### `LocalStreamFn`

```rust
pub struct LocalStreamFn { /* model, convert */ }

impl LocalStreamFn {
    pub fn new(model: Arc<LocalModel>) -> Self;
}

impl StreamFn for LocalStreamFn { /* ... */ }
```

**Invariants**:
- Implements the standard `StreamFn` trait — interchangeable with cloud provider stream functions.
- Calls `ensure_ready()` on first invocation if the model is not yet loaded.
- Converts `LlmMessage` list to local format before inference.
- Wraps inference output into `AssistantMessageEvent` stream: `Start` → `ContentBlockStart` → `ContentBlockDelta` (per token) → `ContentBlockEnd` → `Done`.
- Parses `<think>` tags into `ThinkingStart`/`ThinkingDelta`/`ThinkingEnd` events via string matching.
- Input exceeding context window is silently truncated, keeping the most recent messages.
- Cost is always zero (`Usage { input_tokens: 0, output_tokens: 0, cost: 0.0 }`).

---

### `EmbeddingModel`

```rust
pub struct EmbeddingModel { /* state, config, runner, progress_callback */ }

impl EmbeddingModel {
    pub fn new(config: ModelConfig) -> Self;
    pub fn from_preset(preset: ModelPreset) -> Self;
    pub fn with_progress(self, callback: ProgressCallbackFn) -> Result<Self, LocalModelError>;
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError>;
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, LocalModelError>;
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, LocalModelError>;
}
```

**Invariants**:
- Same lifecycle behavior as `LocalModel` (`Unloaded` → `Ready`).
- `embed` returns a fixed-dimensional vector for the input text.
- `embed` returns `Err(LocalModelError::Embedding(...))` if the input exceeds the model's maximum input length.
- `embed` on empty input returns a valid zero-like vector (not an error).
- `embed_batch` applies the same per-text validation; fails on first invalid input.

---

### `ModelPreset`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPreset {
    SmolLM3_3B,
    EmbeddingGemma300M,
}

impl ModelPreset {
    pub fn config(&self) -> ModelConfig;
    pub fn all() -> &'static [ModelPreset];
}
```

**Invariants**:
- `SmolLM3_3B` configures SmolLM3-3B with Q4_K_M quantization, 8192 context window.
- `EmbeddingGemma300M` configures EmbeddingGemma-300M for text vectorization.
- `all()` returns a static slice of all available presets.
- Unknown preset names produce a clear error listing available presets (enforced at the `from_preset` level via the enum).

---

### `ProgressCallbackFn`

```rust
pub type ProgressCallbackFn = Arc<dyn Fn(ProgressEvent) + Send + Sync>;
```

---

### `ProgressEvent`

```rust
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    DownloadProgress { bytes_downloaded: u64, total_bytes: Option<u64> },
    DownloadComplete,
    LoadingProgress { message: String },
    LoadingComplete,
}
```

**Invariants**:
- `DownloadProgress` fires at least every 1% of completion (SC-002).
- `total_bytes` is `None` when the server does not report `Content-Length`.
- `DownloadComplete` fires exactly once after a successful download.
- `LoadingComplete` fires exactly once after the model is loaded into memory.

---

### `LocalModelError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum LocalModelError {
    #[error("download failed: {0}")]
    Download(String),

    #[error("loading failed: {0}")]
    Loading(String),

    #[error("inference failed: {0}")]
    Inference(String),

    #[error("embedding failed: {0}")]
    Embedding(String),

    #[error("model not ready — call ensure_ready() first")]
    NotReady,
}
```

---

### Message Conversion (internal, behavior is part of the contract)

```rust
// Not public, but behavior is documented:
// - LlmMessage::System → system prompt in local format
// - LlmMessage::User → user message with local tokens
// - LlmMessage::Assistant → assistant message with local tokens
// - LlmMessage::ToolCall → tool invocation in local format
// - LlmMessage::ToolResult → tool output in local format
// - LlmMessage::Custom → filtered out (not sent to local model)
```

---

## Error Handling Summary

| Scenario | Error Variant | Description |
|----------|--------------|-------------|
| Network failure during download | `Download` | Wraps network or I/O error from `hf-hub`. |
| Disk full during download | `Download` | Propagates OS I/O error. |
| Corrupted/incomplete GGUF file | `Loading` | GGUF parse failure from mistral.rs. |
| Out of memory during load | `Loading` | OOM error from mistral.rs. |
| Inference on unloaded model | `NotReady` | Must call `ensure_ready()` first. |
| Input exceeds embedding max length | `Embedding` | Explicit error with length details. |
| No network and no cached weights | `Download` | Clear error indicating download is required. |
| Runtime inference failure | `Inference` | Wraps mistral.rs runtime error. |
