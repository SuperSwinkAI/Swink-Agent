# Swink Agent — Product Requirements Document

**Version:** 0.1
**Language:** Rust (stable toolchain)
**Goals:** Performance, simplicity, maintainability

---

## 1. Overview

A pure-Rust swink agent that provides the core scaffolding for running LLM-powered agentic loops. The harness manages message context, tool execution, streaming responses, and lifecycle events. It is provider-agnostic — LLM backends are swapped via a pluggable stream function interface.

The implementation leverages Rust's type system, ownership model, and async runtime (Tokio) for correctness and performance.

---

## 2. Non-Goals

- No built-in web UI or GUI interface
- No bundled LLM provider SDK dependencies — provider adapters use direct HTTP calls, not vendor SDKs
- No experimental memory features (RAG, tool-aware compaction, explicit memory tools) in this workspace — session persistence and memory management live in the `swink-agent-memory` crate

---

## 3. Core Data Model

### 3.1 Content Blocks

A content block is the atomic unit of message content. All message bodies are composed of one or more content blocks:

- **Text** — a plain text string
- **Thinking** — a reasoning/chain-of-thought string with an optional signature for provider verification
- **ToolCall** — a tool invocation with an ID, tool name, parsed arguments, and an optional partial JSON buffer used during streaming
- **Image** — image data from a supported source type

### 3.2 Message Types

There are three message roles: `user`, `assistant`, and `tool_result`. All three are variants of a single `LlmMessage` type.

- **UserMessage** — role, content (text and optional image blocks), timestamp
- **AssistantMessage** — role, content (one or more content blocks), provider, model identifier, usage stats, stop reason, optional error message, timestamp
- **ToolResultMessage** — role, tool call ID, content (text and optional image blocks), error flag, timestamp

**AgentMessage** is an open type that wraps either an `LlmMessage` or a custom application-defined message. Custom messages are defined via a trait, allowing downstream code to attach application-specific message types (e.g. notifications, artifacts) without modifying the harness.

### 3.3 Usage & Cost

Every assistant message carries token usage counters (input, output, cache read, cache write, total) and a cost breakdown (per-category and total) expressed as floating-point currency values.

### 3.4 Stop Reason

An assistant message always carries a stop reason indicating why generation ended: `stop` (natural end), `length` (token limit), `tool_use` (tool call requested), `aborted` (cancelled by caller), or `error`.

### 3.5 Model Specification

A `ModelSpec` identifies the target model for a request. It carries:

- **provider** — string identifier for the backend (e.g. `"anthropic"`, `"openai"`)
- **model_id** — string identifier for the specific model (e.g. `"claude-sonnet-4-6"`)
- **thinking_level** — reasoning depth: off, minimal, low, medium, high, or extra-high
- **thinking_budgets** — optional per-level token budget overrides for providers that support token-based reasoning control

### 3.6 AgentResult

`AgentResult` is the value returned by non-streaming invocations. It contains:

- **messages** — all new `AgentMessage` values produced during the run
- **stop_reason** — the final stop reason from the last assistant message
- **usage** — aggregated token usage and cost across all turns in the run
- **error** — optional error string if the run ended in an error state

---

## 4. Tool System

### 4.1 AgentTool

A tool is defined by a trait with the following contract:

- **name** — unique string identifier used in tool call routing
- **label** — human-readable display name for logging and UI
- **description** — natural-language description passed to the LLM
- **parameters_schema** — JSON Schema describing the tool's input shape
- **execute** — async function that receives the tool call ID, validated parameters, a `CancellationToken`, and an optional streaming update callback; returns an `AgentToolResult`

### 4.2 AgentToolResult

The result of a tool execution contains:

- **content** — one or more content blocks (text or image) returned to the LLM as the tool result
- **details** — structured JSON data intended for logging or display, not sent to the LLM

### 4.3 Tool Argument Validation

Before `execute` is called, the harness validates the provided arguments against the tool's JSON Schema. Invalid arguments produce an error result without invoking `execute`.

---

## 5. Agent Context

The agent context is the immutable snapshot passed into each loop turn. It contains the system prompt, the current message history as a list of `AgentMessage` values, and the list of available tools. Mutations during a turn produce new snapshots rather than modifying in place.

---

## 6. Agent State

The agent state is the mutable record owned by the `Agent` struct between turns. It holds:

- System prompt
- Active model specification
- Registered tools
- Full message history
- Running flag and the in-progress streaming message
- Set of tool call IDs currently executing
- Last error string, if any

---

## 7. Streaming Interface

### 7.1 StreamOptions

`StreamOptions` carries per-call configuration passed through to the provider:

- **temperature** — sampling temperature (optional)
- **max_tokens** — output token limit (optional)
- **session_id** — provider-side session identifier for caching (optional)
- **transport** — preferred transport protocol; SSE by default

### 7.2 StreamFn

The harness is provider-agnostic. Callers supply a `StreamFn` implementation that accepts a `ModelSpec`, an `AgentContext`, and `StreamOptions`, and returns an async stream of `AssistantMessageEvent` values. The harness consumes this stream to build up the assistant message incrementally.

`AssistantMessageEvent` values follow a start/delta/end protocol for each content type: text, thinking, and tool calls. A terminal `done` or `error` event closes the stream.

### 7.3 AssistantMessageDelta

`AssistantMessageDelta` describes a single incremental update during streaming. It is a typed enum:

- **TextDelta** — a content index and an appended string fragment
- **ThinkingDelta** — a content index and an appended reasoning fragment
- **ToolCallDelta** — a content index and an appended JSON argument fragment

### 7.4 Proxy StreamFn

A built-in proxy implementation forwards LLM calls to an HTTP proxy server over SSE. This covers environments where direct provider access is unavailable.

The proxy accepts a POST request carrying the model, context, and options, authenticates via a bearer token, and responds with an SSE stream of delta events. The proxy strips the partial message field from delta events to reduce bandwidth; the harness reconstructs the full `AssistantMessage` client-side from those deltas.

---

## 8. Event System

A fine-grained lifecycle event type drives all observability. Consumers subscribe to events; the harness never calls back into application logic for display concerns. All events are variants of the `AgentEvent` enum.

**Agent lifecycle**
- `AgentStart` — emitted once when the loop begins
- `AgentEnd` — emitted once when the loop exits, carrying all new messages produced

**Turn lifecycle** (one assistant response plus its tool calls and results)
- `TurnStart` — emitted at the beginning of each assistant turn
- `TurnEnd` — emitted at the end of each turn, carrying the assistant message and any tool result messages

**Message lifecycle**
- `MessageStart` — emitted when a message begins (user, assistant, or tool result)
- `MessageUpdate` — emitted for each incremental delta during assistant streaming; carries an `AssistantMessageDelta`
- `MessageEnd` — emitted when a message is complete

**Tool execution lifecycle**
- `ToolExecutionStart` — emitted when a tool call begins, with the call ID, tool name, and arguments
- `ToolExecutionUpdate` — emitted for intermediate partial results from a streaming tool
- `ToolExecutionEnd` — emitted when a tool call completes, with the final result and an error flag

---

## 9. Cancellation

All async operations accept a `CancellationToken` from `tokio-util`. There are no atomic abort flags or polling loops. Cancellation propagates cleanly through async boundaries and into `StreamFn` implementations. When a token is cancelled the active operation surfaces `StopReason::Aborted` and exits without panicking.

---

## 10. Error Handling

- LLM and tool errors are never panics. They produce an assistant message with `stop_reason: error` and an `error_message` field, appended to the message history like any other message.
- The `Agent` struct stores the most recent error string in state for inspection.
- Tool argument validation failures produce an error `AgentToolResult` without invoking `execute`.

### 10.1 Context Window Overflow

When the provider rejects a request because the input exceeds the model's context window, the harness surfaces a typed `ContextWindowOverflow` error rather than a generic LLM error. The message history is left intact and the harness signals the overflow condition to the `transform_context` hook. On retry via `continue_loop()`, the hook receives this signal and applies a more aggressive pruning strategy. All context management stays in `transform_context`.

### 10.2 Max Tokens Reached

When the model stops mid-response because it hit the output token limit (`stop_reason: length`) and there are incomplete tool calls in the response, the harness replaces each incomplete tool call with an informative error tool result before continuing the loop. This prevents the next model turn from receiving a broken tool call / tool result pair.

This is purely internal recovery — it is not surfaced as a `AgentError` to the caller. The harness repairs incomplete tool calls and continues the loop silently.

### 10.3 AgentError Variants

The `AgentError` enum defines all error conditions surfaced to the caller:

- **`ContextWindowOverflow { model: String }`** — provider rejects request because input exceeds context window
- **`ModelThrottled`** — rate limit / 429 from provider
- **`NetworkError`** — transient IO/connection failure
- **`StructuredOutputFailed { attempts: usize, last_error: String }`** — structured output max retries exceeded
- **`AlreadyRunning`** — `prompt()` called while a run is already active
- **`NoMessages`** — `continue_loop()` called with empty context
- **`InvalidContinue`** — `continue_loop()` called when last message is an assistant message
- **`StreamError { source: Box<dyn Error> }`** — non-retryable `StreamFn` failure
- **`Aborted`** — `CancellationToken` was cancelled

Note: `MaxTokensReached` is not included — that condition is handled internally (see §10.2).

---

## 11. Retry Strategy

Model calls can fail transiently due to rate limits, transient network errors, or provider-side throttling. The harness defines a `RetryStrategy` trait with the following contract:

- **should_retry** — given the error type and the attempt number, returns whether to retry
- **delay** — returns the duration to wait before the next attempt

A default implementation is provided with exponential back-off, jitter, a configurable maximum number of attempts, and a configurable maximum delay cap. Callers may supply their own implementation at `Agent` construction time. The strategy applies only to model calls, not to tool execution.

---

## 12. Agent Loop

### 12.1 Entry Points

There are two loop entry points:

- **agent_loop** — starts a new turn by prepending one or more prompt messages to the context, then runs the loop
- **agent_loop_continue** — resumes from existing context without adding new messages; used for retries and resumption after tool results

Both return an async stream of `AgentEvent` values.

### 12.2 Loop Config

The loop config carries:

- **model** — `ModelSpec` passed through to `StreamFn`
- **stream_options** — `StreamOptions` passed through to `StreamFn`
- **retry_strategy** — `RetryStrategy` implementation applied to model calls
- **convert_to_llm** — required function that converts a single `AgentMessage` to an optional `LlmMessage`. Signature: `Fn(&AgentMessage) → Option<LlmMessage>`. Returns `None` to filter out custom or UI-only messages that should not reach the provider. Context-level filtering (pruning, budget enforcement) is handled by `transform_context`
- **transform_context** — optional async hook called before `convert_to_llm`; used for context pruning, token budget enforcement, or external context injection
- **get_api_key** — optional async callback for dynamic API key resolution; supports short-lived tokens that may expire during long tool execution phases
- **get_steering_messages** — optional async callback polled after each tool execution; if messages are returned the remaining tools in that batch are skipped and the messages are injected before the next assistant turn
- **get_follow_up_messages** — optional async callback polled when the agent would otherwise stop; if messages are returned the loop continues with another turn

### 12.3 Loop Behaviour

The loop runs two nested phases:

**Inner loop** — processes tool calls and steering interrupts:
1. Emit `TurnStart`
2. Inject any pending steering or follow-up messages into context
3. Call `StreamFn`, emit `MessageStart` / `MessageUpdate` / `MessageEnd` as the response streams in; on a retryable failure apply the retry strategy before re-invoking `StreamFn`
4. If stop reason is `error` or `aborted`, emit `TurnEnd` and `AgentEnd` and exit immediately — no follow-up polling occurs
5. Extract tool calls from the completed assistant message; if any tool calls are incomplete due to `stop_reason: length`, apply max tokens recovery (section 10.2) before proceeding. If no tool calls are present, emit `TurnEnd` and exit the inner loop
6. Execute all tool calls concurrently; emit `ToolExecution*` events; after each completes, poll `get_steering_messages`. If steering messages arrive, cancel remaining in-flight tools via their `CancellationToken`, inject an error `ToolResultMessage` for each cancelled tool (content: "tool call cancelled: user requested steering interrupt"), and proceed to the next turn. After converting context via `transform_context` and `convert_to_llm`, resolve the API key via `get_api_key` before calling `StreamFn`
7. Emit `TurnEnd`
8. Poll `get_steering_messages`; if messages are returned, push to pending and continue the inner loop

**Outer loop** — handles follow-up after the agent would stop:
- When the inner loop exits due to error or abort, do NOT poll `get_follow_up_messages` — emit `AgentEnd` and exit immediately
- When the inner loop exits normally with no pending messages, poll `get_follow_up_messages`
- If follow-up messages are returned, push to pending and re-enter the inner loop
- If none, emit `AgentEnd` and exit

---

## 13. Agent Struct

The `Agent` struct is the high-level stateful wrapper over the loop, intended for direct use in applications.

### 13.1 Configuration

Options provided at construction:

- Initial state overrides (system prompt, model, tools, messages)
- Custom `convert_to_llm` and `transform_context` functions
- Steering mode: deliver all queued steering messages at once, or one per turn (default: one at a time)
- Follow-up mode: deliver all queued follow-up messages at once, or one per turn (default: one at a time)
- Custom `StreamFn` (defaults to a direct provider stream)
- Dynamic API key callback
- Retry strategy (default: exponential back-off with jitter)

### 13.2 Core API

**State mutation**
- Set system prompt, model, thinking level, and tools
- Replace, append, or clear the message history

**Prompt — streaming**
- Accept input as a plain string, a string with images, or a list of `AgentMessage` values
- Returns an async stream of `AgentEvent` values; returns an error if already running

**Prompt — non-streaming async**
- Same input variants as streaming; awaits completion and returns an `AgentResult`

**Prompt — non-streaming sync**
- Blocking variant of the above; drives the Tokio runtime internally; intended for simple scripts and tests

**Structured output**
- Accepts a prompt and a JSON Schema describing the required output shape
- Injects a synthetic tool that the model must invoke as its final action; validates the response against the schema
- Returns a validated `serde_json::Value` (or a typed result via a generic parameter bound to `DeserializeOwned`)
- Retries up to a configurable maximum if the model produces an invalid response
- Available in both async and sync variants

**Continue**
- Resume from existing context; used for retries or resuming after queued messages
- Available in streaming, async, and sync variants

**Steering and follow-up queues**
- Enqueue steering messages to interrupt the agent mid-run
- Enqueue follow-up messages to be processed after the agent finishes
- Clear individual queues or all queues at once
- Query whether any queued messages are pending

**Control**
- Abort the current run via `CancellationToken`
- Await idle (returns when the current run finishes)
- Reset all state to initial values

**Observation**
- Subscribe to events with a callback; returns a handle to unsubscribe

### 13.3 Concurrency Contract

- Only one active invocation is permitted at a time; a second call while running returns an error
- `steer` and `follow_up` are safe to call at any time; the running loop drains the queue after each tool execution
- `abort` signals the active `CancellationToken`; the loop surfaces `StopReason::Aborted` and exits cleanly

---

## 14. Dependencies

| Crate | Purpose |
|---|---|
| `tokio` (full) | Async runtime |
| `tokio-util` | `CancellationToken` |
| `futures` | Stream and future combinators |
| `serde` / `serde_json` | JSON serialization and tool argument handling |
| `jsonschema` | Tool parameter validation against JSON Schema |
| `reqwest` | HTTP client for proxy stream |
| `eventsource-stream` | SSE parsing for proxy stream |
| `thiserror` | Structured error types |
| `uuid` | Tool call ID generation |

No `unsafe` code. No global mutable state.

### 14.1 Adapters Crate Dependencies

| Crate | Purpose |
|---|---|
| `swink-agent` | Core types and `StreamFn` trait |
| `reqwest` | HTTP client for provider APIs |
| `bytes` | Byte buffer handling for NDJSON parsing |
| `serde` / `serde_json` | JSON serialization for provider payloads |
| `sha2` | Content hashing for caching |

### 14.2 TUI Crate Dependencies

| Crate | Purpose |
|---|---|
| `swink-agent` | Core types and agent API |
| `swink-agent-adapters` | Provider adapters (Ollama by default) |
| `ratatui` | Terminal UI framework |
| `crossterm` | Terminal backend |
| `syntect` | Syntax highlighting |
| `arboard` | Clipboard access |
| `toml` / `dirs` | Config file support |

---

## 15. Crate Structure

The project is a 6-crate Cargo workspace:

```
swink-agent/              Workspace root + core library
  Cargo.toml
  src/
    lib.rs          — public re-exports
    types.rs        — AgentMessage, AgentEvent, AgentResult, ContentBlock, Usage, ModelSpec, …
    tool.rs         — AgentTool trait, AgentToolResult, argument validation
    stream.rs       — StreamFn trait, StreamOptions, AssistantMessageEvent, AssistantMessageDelta
    error.rs        — AgentError, ContextWindowOverflow
    retry.rs        — RetryStrategy trait and default implementation
    loop_.rs        — agent_loop, agent_loop_continue, run_loop, AgentLoopConfig
    agent.rs        — Agent struct

adapters/                   LLM provider adapters
  Cargo.toml
  src/
    lib.rs          — public re-exports
    anthropic.rs    — AnthropicStreamFn for Anthropic Messages API (SSE)
    openai.rs       — OpenAiStreamFn for OpenAI-compatible /v1/chat/completions (SSE)
    ollama.rs       — OllamaStreamFn for Ollama /api/chat (NDJSON)
    google.rs       — GeminiStreamFn for Google Gemini API
    azure.rs        — AzureStreamFn for Azure OpenAI
    xai.rs          — XAiStreamFn for xAI (Grok)
    mistral.rs      — MistralStreamFn for Mistral API
    bedrock.rs      — BedrockStreamFn for AWS Bedrock
    proxy.rs        — ProxyStreamFn for HTTP proxy forwarding (SSE)
    convert.rs      — MessageConverter trait (shared across adapters)
    classify.rs     — model classification utilities
    sse.rs          — shared SSE parsing helpers
    remote_presets.rs — catalog presets for remote model connections

memory/                     Session persistence and memory management
  Cargo.toml
  src/
    lib.rs          — public re-exports
    store.rs        — synchronous session store
    store_async.rs  — async session store
    compaction.rs   — context compaction strategies
    jsonl.rs        — JSONL serialization for message logs
    meta.rs         — session metadata
    time.rs         — timestamp utilities

local-llm/                  On-device LLM inference
  Cargo.toml
  src/
    lib.rs          — public re-exports
    model.rs        — local model loading and management
    stream.rs       — StreamFn implementation for local models
    convert.rs      — message conversion for local inference
    embedding.rs    — embedding model support
    preset.rs       — local model presets
    progress.rs     — download/load progress reporting
    error.rs        — local-llm error types

eval/                       Evaluation and benchmarking
  Cargo.toml
  src/
    lib.rs          — public re-exports
    trajectory.rs   — TrajectoryCollector for capturing agent execution traces
    match_.rs       — golden path comparison
    efficiency.rs   — EfficiencyEvaluator (duplicate ratio, step ratio scoring)
    budget.rs       — BudgetGuard (real-time cost/token/turn monitoring) + BudgetEvaluator
    gate.rs         — CI/CD gating support
    audit.rs        — deterministic audit trail generation
    response.rs     — ResponseCriteria and response matching
    evaluator.rs    — Evaluator trait and EvaluatorRegistry
    runner.rs       — EvalRunner for executing evaluation cases
    score.rs        — Score types and aggregation
    store.rs        — evaluation result persistence
    types.rs        — shared eval types (EvalCase, etc.)
    yaml.rs         — YAML-based eval case definitions
    error.rs        — eval error types

tui/                        Terminal UI binary
  Cargo.toml
  src/
    main.rs         — entry point, agent setup from env vars
    app/            — top-level App state machine, event loop, lifecycle, agent bridge
    commands.rs     — slash-command and hash-command system
    config.rs       — TOML config file support
    credentials.rs  — credential resolution (env vars, keychain)
    editor.rs       — external editor integration
    format.rs       — token, elapsed, and context gauge formatting
    session.rs      — session persistence
    theme.rs        — color theme definitions
    wizard.rs       — first-run setup wizard
    ui/             — UI components (conversation, input, tool panel, status bar, diff, help, markdown, syntax)
```

### 15.1 Adapters Crate

The `swink-agent-adapters` crate provides concrete `StreamFn` implementations for specific LLM providers. Each adapter translates between the provider's native streaming protocol and the harness's `AssistantMessageEvent` stream.

Current adapters:

- **`AnthropicStreamFn`** — connects to Anthropic's `/v1/messages` endpoint via SSE. Supports thinking blocks with budget control
- **`OpenAiStreamFn`** — connects to any OpenAI-compatible `/v1/chat/completions` endpoint via SSE (also works with vLLM, LM Studio, Groq, Together, etc.)
- **`OllamaStreamFn`** — connects to Ollama's `/api/chat` endpoint, parses NDJSON streaming responses. Supports tool calls via Ollama's native tool-calling protocol
- **`GeminiStreamFn`** — connects to Google's Gemini API
- **`AzureStreamFn`** — connects to Azure OpenAI endpoints
- **`XAiStreamFn`** — connects to xAI (Grok) API
- **`MistralStreamFn`** — connects to Mistral API
- **`BedrockStreamFn`** — connects to AWS Bedrock
- **`ProxyStreamFn`** — forwards LLM calls to an HTTP proxy server over SSE

---

## 16. Terminal User Interface (TUI)

The TUI is a terminal-based interactive interface for the swink agent, provided as a separate binary crate within the workspace. It renders the agent conversation, tool execution, and streaming responses directly in the terminal.

### 16.1 Architecture

The TUI follows a component-based architecture using `ratatui` for rendering and `crossterm` for terminal input/output. Components are stateful widgets that render into terminal frames.

- **App** — top-level application state machine managing layout, focus, and event dispatch
- **Conversation View** — scrollable view displaying the message history with syntax-highlighted code blocks, markdown rendering, and thinking block display
- **Input Editor** — multi-line text editor for composing user messages with line wrapping and cursor management
- **Tool Panel** — displays active tool executions with progress indicators and results
- **Status Bar** — shows model info, token usage, cost, and agent state (idle/running/error)

### 16.2 Event Model

The TUI subscribes to `AgentEvent` values from the harness and maps them to UI updates:

- `AgentStart` / `AgentEnd` — toggle running state indicator
- `TurnStart` / `TurnEnd` — update turn counter
- `MessageStart` / `MessageUpdate` / `MessageEnd` — incrementally render assistant response
- `ToolExecutionStart` / `ToolExecutionEnd` — show tool activity in the tool panel

Terminal input events (keyboard, mouse, resize) are handled by `crossterm` and dispatched to the focused component.

### 16.3 Rendering

The TUI uses `ratatui`'s immediate-mode rendering: each frame, the entire UI is re-rendered from current state. `crossterm`'s alternate screen and raw mode provide a clean full-screen terminal experience.

Key rendering features:
- Word-wrapped markdown with ANSI color support
- Syntax highlighting for code blocks (via `syntect`)
- Streaming text display with cursor indicator during generation
- Scrollable conversation history with viewport tracking
- Responsive layout adapting to terminal dimensions

### 16.4 Interaction

- **Compose and send messages** — type in the input editor, press Enter to send
- **Scroll history** — navigate conversation with arrow keys, Page Up/Down, mouse wheel
- **Cancel generation** — Escape or Ctrl+C to abort the current agent run
- **Quit** — Ctrl+Q or `/quit` command to exit

### 16.5 Dependencies

| Crate | Purpose |
|---|---|
| `ratatui` | Terminal UI framework (immediate-mode rendering) |
| `crossterm` | Cross-platform terminal backend (input, raw mode, alternate screen) |
| `syntect` | Syntax highlighting for code blocks |
| `tokio` | Async runtime (shared with swink agent) |

### 16.6 Inline Diff View

When the agent modifies a file, the TUI displays the change as a syntax-highlighted inline diff rather than raw tool output. Users review modifications in context before they are finalized.

- Unified diff as the default view
- Syntax highlighting uses the same `syntect` pipeline as code blocks
- Diffs for new files show all lines as additions; diffs for deleted files show all lines as deletions

### 16.7 Context Window Progress Bar

The status bar includes a visual gauge showing the estimated context window fill percentage. This gives the user awareness of how much conversation history remains before context compaction or overflow occurs.

- Gauge color transitions from green (<60%) to yellow (60–85%) to red (>85%)
- Percentage is computed from the same `estimate_tokens` heuristic used by the sliding window compactor, relative to the model's context window size
- Updates after every turn (not per-delta, to avoid flicker)

### 16.8 External Editor Mode

Users can compose complex, multi-line prompts in their preferred external editor. The TUI opens the editor specified by `$EDITOR` (falling back to `$VISUAL`, then `vi`), waits for it to close, and submits the resulting text as the user prompt.

- Triggered via `/editor` command
- TUI suspends (leaves alternate screen, restores terminal mode) while the editor is open, then resumes
- Empty file on close is treated as cancellation — no message is sent
- Temporary file is created in the OS temp directory and deleted after submission

### 16.9 Plan Mode

Plan mode is a read-only operating mode where the agent analyzes the user's request and produces a structured plan but does not execute any write or destructive tools. The user reviews the proposed plan, then switches to execute mode to carry it out.

- Toggled via `Shift+Tab` keybinding or `/plan` slash command. A status bar indicator shows the current mode (Plan / Execute)
- In plan mode, the agent's tool set is restricted to read-only tools (e.g., `ReadFileTool`). Write tools (`WriteFileTool`, `BashTool`) are temporarily removed from the agent context
- Switching to execute mode re-registers all tools and optionally re-sends the plan as a follow-up message so the agent can act on it
- Plan mode output is styled distinctly (e.g., a different border color or a "PLAN" label) to avoid confusion with executed results

### 16.10 Collapsible Tool Result Blocks

Each tool invocation and its result are rendered as a discrete, collapsible block in the conversation view. This reduces visual clutter when the agent makes many tool calls in a single turn.

- Default state is collapsed — shows a one-line summary (tool name, status badge, truncated first line of output)
- Expand/collapse toggled with `F2` key; `Shift+←`/`Shift+→` cycles selection across tool blocks
- When the agent is streaming and tool results arrive, new blocks start expanded, then auto-collapse after 3 seconds (matching the existing tool panel fade behavior)
- Expanded view shows the full tool output with syntax highlighting where applicable

### 16.11 Tiered Approval Modes

Extends the existing binary approval system (`#approve on` / `#approve off`) with a third `Smart` mode. Smart mode auto-approves read-only tools and prompts only for tools that could modify state.

- Three modes: `Enabled` (prompt for all), `Smart` (auto-approve reads, prompt for writes/deletes/commands), `Bypassed` (auto-approve all)
- Classification uses the existing `requires_approval()` trait method — tools that return `false` are always auto-approved regardless of mode
- Per-tool session trust: after approving a specific tool once in Smart mode, the user can choose "always approve this tool for this session." Trusted tools are auto-approved for the remainder of the session
- Configurable via `#approve smart`, `#approve on`, `#approve off` commands. `Smart` is the new default

---

## 17. Acceptance Criteria

| # | Criterion |
|---|---|
| 1 | The agent loop emits all lifecycle events in the correct order for a single-turn, no-tool conversation |
| 2 | Tool arguments are validated against JSON Schema; invalid arguments produce error results without invoking execute |
| 3 | Tool calls within a single turn execute concurrently |
| 4 | Steering messages interrupt tool execution — remaining tools in the current batch are skipped |
| 5 | Follow-up messages cause the agent to continue after it would otherwise stop |
| 6 | Aborting via `CancellationToken` produces a clean shutdown with stop reason aborted |
| 7 | The proxy stream correctly reconstructs an assistant message from delta SSE events |
| 8 | Calling prompt while already running returns an error |
| 9 | transform_context is called before convert_to_llm on every turn |
| 10 | All public types are Send and Sync |
| 11 | Structured output retries up to the configured maximum when the model returns an invalid response |
| 12 | A provider rejection due to context window overflow surfaces as a typed ContextWindowOverflow error, not a generic error |
| 13 | Incomplete tool calls caused by max tokens are replaced with error tool results before the next turn |
| 14 | The default retry strategy applies exponential back-off with jitter and respects the maximum delay cap |
| 15 | Sync prompt blocks until completion without requiring the caller to manage a Tokio runtime |
| 16 | TUI renders streaming assistant responses incrementally as deltas arrive |
| 17 | TUI input editor accepts multi-line input and submits on Enter |
| 18 | Cancel via Escape/Ctrl+C aborts the running agent and shows aborted state |
| 19 | Tool execution panel shows active tools and their results |
| 20 | TUI adapts layout to terminal resize events |
| 21 | Inline diff view renders file modifications as syntax-highlighted unified diffs |
| ~~22~~ | ~~Inline diff view switches to side-by-side layout when terminal width exceeds the configured threshold~~ — planned, not yet implemented |
| 23 | Context window progress bar displays estimated fill percentage with green/yellow/red color transitions |
| 24 | External editor opens `$EDITOR`, suspends the TUI, and submits the file content on close |
| 25 | External editor treats an empty file on close as cancellation — no message is sent |
| 26 | Plan mode restricts the agent to read-only tools and labels output distinctly |
| 27 | Switching from plan mode to execute mode re-registers write tools and continues with the plan |
| 28 | Tool result blocks default to collapsed with a one-line summary and expand on user interaction |
| 29 | Smart approval mode auto-approves tools where `requires_approval()` returns false and prompts for all others |
| 30 | Per-tool session trust persists approved tool names for the session duration in Smart mode |
