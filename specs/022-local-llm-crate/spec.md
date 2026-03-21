# Feature Specification: Local LLM Crate

**Feature Branch**: `022-local-llm-crate`
**Created**: 2026-03-20
**Status**: Draft
**Input**: On-device LLM inference. LocalModel for small language model inference with quantized weights, LocalStreamFn implementing the streaming function interface, EmbeddingModel for text vectorization, message conversion for local format, local model presets, lazy download and caching, download/load progress reporting. References: HLD Local LLM Layer, Design Decisions (local-llm is separate to isolate heavy native dependencies), HLD System Overview.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run Inference Locally Without Cloud Credentials (Priority: P1)

A developer wants to use the agent without any cloud API keys or network connectivity. They configure the agent to use a local model. On first use, the model weights are automatically downloaded and cached. The developer sends a conversation, and the local model generates a streaming response — text arrives incrementally, just like a cloud provider. The developer gets a working agent experience entirely on-device.

**Why this priority**: Local inference is the core reason this crate exists — without it, the crate provides no value.

**Independent Test**: Can be tested by configuring a local model, sending a prompt, and verifying that text tokens stream back incrementally without any network calls to a cloud provider.

**Acceptance Scenarios**:

1. **Given** a local model is configured, **When** a conversation is sent, **Then** the model generates a streaming response with incremental text tokens.
2. **Given** the model weights are not yet cached locally, **When** inference is first requested, **Then** the weights are automatically downloaded and cached before inference begins.
3. **Given** the model weights are already cached, **When** inference is requested, **Then** the model loads from cache without downloading.
4. **Given** no network connectivity and no cached weights, **When** inference is requested, **Then** a clear error indicates the model needs to be downloaded.

---

### User Story 2 - Track Model Download and Loading Progress (Priority: P1)

A developer initiates local model usage for the first time. The model weights are large and must be downloaded. The system reports download progress (bytes transferred, percentage complete) so the developer can monitor the process and estimate remaining time. Once downloaded, loading the model into memory also reports progress. The developer is never left wondering whether the system is working or stuck.

**Why this priority**: Without progress reporting, a multi-gigabyte download appears to hang — developers will kill the process thinking it is frozen.

**Independent Test**: Can be tested by triggering a model download and verifying that progress callbacks fire with increasing percentages until completion.

**Acceptance Scenarios**:

1. **Given** a model download is in progress, **When** data is transferred, **Then** progress events report bytes transferred and percentage complete.
2. **Given** a model is loading into memory, **When** loading progresses, **Then** progress events report loading status.
3. **Given** a download is interrupted, **When** retried, **Then** the download resumes from where it left off (if the source supports it) or restarts cleanly.

---

### User Story 3 - Embed Text for Similarity Comparisons (Priority: P2)

A developer wants to compute vector embeddings for text passages to enable similarity search, clustering, or retrieval-augmented generation. They use the embedding model to convert text into fixed-dimensional vectors. The embeddings are computed locally without cloud API calls. The developer can compare embeddings using standard distance metrics to find semantically similar content.

**Why this priority**: Text embedding enables downstream features like memory retrieval and semantic search, but the core agent experience works without it.

**Independent Test**: Can be tested by embedding two semantically similar texts and two dissimilar texts, then verifying that the similar pair has a higher cosine similarity score.

**Acceptance Scenarios**:

1. **Given** a text passage, **When** the embedding model processes it, **Then** a fixed-dimensional vector is returned.
2. **Given** two semantically similar passages, **When** their embeddings are compared, **Then** the similarity score is higher than for dissimilar passages.
3. **Given** the embedding model weights are not cached, **When** embedding is first requested, **Then** the weights are automatically downloaded and cached.
4. **Given** an empty text input, **When** embedding is requested, **Then** a valid vector is returned (not an error).

---

### User Story 4 - Use Local Model Presets (Priority: P2)

A developer wants to use a local model without manually specifying model paths, quantization settings, or context window sizes. The system provides presets for supported local models that bundle all necessary configuration. The developer selects a preset by name and the system handles the rest.

**Why this priority**: Presets reduce friction for getting started, but a developer who knows their model configuration can bypass them.

**Independent Test**: Can be tested by selecting a preset by name and verifying that the model loads with the correct configuration without any additional settings.

**Acceptance Scenarios**:

1. **Given** a known preset name, **When** selected, **Then** the model is configured with the correct parameters (context window, quantization level, download source).
2. **Given** an unknown preset name, **When** selected, **Then** a clear error lists available presets.

---

### User Story 5 - Convert Agent Messages to Local Model Format (Priority: P3)

A developer uses the agent loop with a local model. The agent loop produces messages in the standard agent format (system prompts, user messages, assistant messages, tool calls, tool results). The local model requires messages in a different format with specific tokens and structure. The conversion happens automatically — the developer does not need to know the local format.

**Why this priority**: Message conversion is an internal concern that is invisible to the developer, but necessary for correct inference results.

**Independent Test**: Can be tested by converting a representative set of agent messages (including tool calls) and verifying the output matches the expected local format.

**Acceptance Scenarios**:

1. **Given** standard agent messages, **When** converted for the local model, **Then** the output uses the local model's expected format and special tokens.
2. **Given** messages containing tool calls and tool results, **When** converted, **Then** tool interactions are represented correctly in the local format.
3. **Given** a system prompt, **When** converted, **Then** it appears in the correct position for the local model's expected format.

---

### Edge Cases

- What happens when the disk runs out of space during a model download — `Download` error variant propagates the OS I/O error.
- How does the system handle a corrupted or incomplete cached model file — `Loading` error variant covers GGUF parse failures. The model fails to load with a clear error; re-download resolves it.
- What happens when the model file's checksum does not match — integrity verification is delegated to the download library (HuggingFace ETag/SHA). No separate checksum step needed.
- How does the system behave on hardware without enough memory to load the model — `Loading` error variant covers OOM. Clear error message; model does not partially load.
- What happens when two processes attempt to download the same model simultaneously — last-writer-wins; no file locking. Single-process assumption documented.
- How does inference handle input that exceeds the local model's context window — input is silently truncated to fit, keeping the most recent messages.
- What happens when the embedding model receives extremely long text — returns an error indicating the input exceeds the model's maximum input length.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST support on-device text generation using quantized model weights.
- **FR-002**: The system MUST implement the standard streaming function interface so local models are interchangeable with cloud providers from the agent loop's perspective.
- **FR-003**: The system MUST lazily download model weights on first use and cache them locally for subsequent runs.
- **FR-004**: The system MUST report download progress (bytes transferred, percentage) via a callback or event mechanism.
- **FR-005**: The system MUST report model loading progress.
- **FR-006**: The system MUST provide an embedding model that converts text passages into fixed-dimensional vectors locally.
- **FR-007**: The system MUST convert standard agent messages to the local model's expected format automatically.
- **FR-008**: The system MUST provide presets for supported local models that bundle all necessary configuration.
- **FR-009**: The system MUST validate model file integrity after download (delegated to the download library's built-in verification).
- **FR-010**: The system MUST silently truncate input exceeding the local model's context window, keeping the most recent messages. The embedding model MUST return an error for inputs exceeding its maximum length.

### Key Entities

- **LocalModel**: The on-device language model capable of text generation from quantized weights. Configured via presets or manual parameters.
- **LocalStreamFn**: The streaming function adapter that bridges the local model to the standard agent streaming interface, emitting incremental tokens.
- **EmbeddingModel**: The on-device model that converts text passages into fixed-dimensional vector embeddings for similarity computation.
- **ModelPreset**: A named configuration bundle specifying model source, quantization level, context window size, and other parameters for a supported local model.
- **ProgressReporter**: The mechanism for communicating download and loading progress to the caller.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can run the agent with a local model and receive streaming responses without any cloud API keys configured.
- **SC-002**: Model download progress is reported at least every 1% of completion.
- **SC-003**: Cached models load without network access.
- **SC-004**: Text embeddings produce vectors where semantically similar inputs have measurably higher similarity scores than dissimilar inputs.
- **SC-005**: The local streaming function passes the same integration tests as cloud provider streaming functions (aside from output quality).

## Clarifications

### Session 2026-03-20

- Q: How should model file integrity be verified after download? → A: Rely on download library verification (HuggingFace ETag/SHA). No separate checksum step.
- Q: How should concurrent model downloads be handled? → A: Last-writer-wins; no locking. Single-process assumption.
- Q: Should input exceeding context window be truncated or error? → A: Silently truncate to fit, keeping most recent messages.
- Q: Should long embedding inputs be truncated or error? → A: Error — return error for inputs exceeding max length.
- Q: Disk full during download? → A: `Download` error propagates OS I/O error.
- Q: Corrupted cached model? → A: `Loading` error on GGUF parse failure; re-download resolves.
- Q: Not enough memory to load? → A: `Loading` error covers OOM; clear message.

## Assumptions

- The local-llm crate is a separate package to isolate heavy native dependencies (model inference libraries) from the core agent crate.
- The default text generation model targets a small, efficient architecture suitable for consumer hardware (8GB+ RAM).
- The default embedding model targets a compact architecture optimized for fast vectorization.
- Model weights are downloaded from a public source and cached in a platform-appropriate location (e.g., user cache directory).
- The local model's output quality is lower than large cloud models — this crate prioritizes offline availability and privacy over output quality.
- Quantized (4-bit) weights are used to balance quality and resource requirements.
