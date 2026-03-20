# Research: Adapter: Ollama

**Feature**: 014-adapter-ollama | **Date**: 2026-03-20

## Decision 1: NDJSON Protocol (Not SSE)

**Question**: How should the adapter parse Ollama's streaming response format?

**Decision**: Implement a custom `ndjson_lines()` combinator that buffers incoming bytes and yields complete newline-delimited JSON lines. Each line is a self-contained JSON object (`OllamaChatChunk`). The `done: true` field in a chunk signals stream termination. The shared SSE parser (`sse_data_lines`) is not used.

**Rationale**: Ollama streams responses as NDJSON -- one complete JSON object per line, terminated by `\n`. This is fundamentally different from SSE, which uses `data:` prefixed lines, `event:` type labels, and `[DONE]` sentinels. The NDJSON format is simpler to parse: read a line, deserialize it as JSON, check the `done` flag. Reusing the SSE parser would require forcing NDJSON into SSE's event/data model, adding unnecessary complexity.

**Alternatives rejected**:
- *Shared SSE parser (`sse_data_lines`)*: SSE and NDJSON are different protocols. Adapting one to the other adds indirection without benefit.
- *`async-ndjson` crate*: Low download count, unmaintained. The parser is ~40 lines -- wrapping a crate adds a dependency for trivial functionality.

## Decision 2: Native Tool Calling Protocol

**Question**: How should the adapter handle Ollama's tool calling?

**Decision**: Tool definitions are passed in the request body as a `tools` array. Ollama returns tool calls in the `message.tool_calls` array of the response chunk. Unlike OpenAI's delta-based tool call streaming (where arguments arrive in fragments across multiple chunks), Ollama delivers complete tool calls in a single chunk. Each tool call is emitted as a `ToolCallStart` / `ToolCallDelta` / `ToolCallEnd` triplet with the full arguments in one delta.

**Rationale**: Ollama's tool calling protocol delivers the entire tool call (name + arguments) in one NDJSON line rather than streaming argument fragments. This simplifies the adapter -- no accumulation state or `HashMap` tracking is needed. A `HashSet` tracks which tool names have been seen to deduplicate in case the model re-sends a tool call across multiple chunks.

**Alternatives rejected**:
- *Delta accumulation (like OpenAI adapter)*: Unnecessary complexity since Ollama delivers complete tool calls. Would add a `HashMap`, accumulation logic, and multi-chunk finalization for a protocol that doesn't need it.

## Decision 3: `#[serde(default)]` for Lenient Parsing

**Question**: How should the adapter handle missing or unexpected fields in Ollama's JSON responses?

**Decision**: All optional and collection fields in response structs (`OllamaChatChunk`, `OllamaResponseMessage`, `OllamaResponseToolCall`) use `#[serde(default)]`. Missing fields default to `None`, empty string, or empty collection. Unknown fields are ignored by default (serde's behavior for structs without `#[serde(deny_unknown_fields)]`).

**Rationale**: Ollama's response format may vary across versions and models. Fields like `done_reason`, `prompt_eval_count`, `eval_count`, and `thinking` are not always present. Using `#[serde(default)]` ensures the adapter tolerates sparse responses without producing parse errors. This matches the spec's edge case requirement: "all optional fields use `#[serde(default)]`; missing fields default to None/empty. No crash."

**Alternatives rejected**:
- *Manual `serde_json::Value` parsing*: Loses type safety, more verbose, harder to maintain.
- *Strict deserialization with error handling*: Would produce false errors on valid but sparse responses from different Ollama versions.

## Decision 4: Error Classification

**Question**: How should the adapter classify errors for retry?

**Decision**: Connection failures (Ollama not running, network timeout) map to `error_network()` (retryable). Non-success HTTP status codes map to `error_network()` with the status code and response body in the message. NDJSON parse errors mid-stream map to `error()` (non-retryable). The adapter does not use the shared `classify_http_status` helper because Ollama has no authentication (no 401/403 cases) and no throttling (no 429 cases) -- all HTTP errors are connection-level issues.

**Rationale**: Ollama runs locally (or on a known remote host) without authentication or rate limiting. The most common error is "connection refused" (Ollama not running), which is retryable. Model-not-found is an HTTP error from Ollama itself, but since it requires user intervention (pull the model), it maps to `error_network` with a descriptive message rather than a special variant.

**Alternatives rejected**:
- *Shared `classify_http_status()`*: Designed for cloud providers with auth and throttling. Ollama has neither -- using it would misclassify errors or require Ollama-specific overrides.
- *Custom error enum*: Over-engineering for a local service with a small error surface.

## Decision 5: Localhost Default

**Question**: What should the default base URL be?

**Decision**: The adapter requires a `base_url` parameter in `new()`. The spec states "default to localhost" but this is a documentation/example concern, not a constructor default. Callers pass `http://localhost:11434` explicitly. This avoids hidden behavior and matches the pattern of other adapters that require explicit base URLs.

**Rationale**: Making the default explicit in the constructor signature keeps the API predictable. A `Default` impl or optional parameter would hide a network assumption. The quickstart and documentation make the default URL obvious to callers.

**Alternatives rejected**:
- *`Default` impl with hardcoded localhost*: Hides a network configuration assumption. Other adapters (`OpenAiStreamFn`, `AnthropicStreamFn`) require explicit URLs.
- *`Option<String>` with fallback*: Adds branching for marginal convenience.
