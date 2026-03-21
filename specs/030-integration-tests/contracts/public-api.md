# Contracts: Test Helper Public API

**Branch**: `030-integration-tests` | **Date**: 2026-03-20

## Overview

This document defines the API contracts for shared test helper types in `tests/common/mod.rs`. All integration test files depend on these contracts. Changes to these APIs require updating all consuming test files.

## MockStreamFn

### Contract

```rust
impl MockStreamFn {
    /// Create with a queue of scripted response sequences.
    /// Each call to `stream()` pops and returns the front sequence.
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self;
}

impl StreamFn for MockStreamFn {
    /// Returns the next scripted response sequence.
    /// If the queue is exhausted, returns a single Error event.
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}
```

### Guarantees

- Responses are consumed in FIFO order.
- Exhausted queue produces `AssistantMessageEvent::Error` with message `"no more scripted responses"`.
- Thread-safe: `Mutex` protects the response queue.
- Does not inspect `model`, `context`, `options`, or `cancellation_token`.

## MockTool

### Contract

```rust
impl MockTool {
    /// Create a mock tool with the given name. Default schema accepts any object.
    /// Default result is `AgentToolResult::text("ok")`.
    pub fn new(name: &str) -> Self;

    /// Set a custom JSON Schema for parameter validation.
    pub fn with_schema(self, schema: Value) -> Self;

    /// Set the result returned by `execute()`.
    pub fn with_result(self, result: AgentToolResult) -> Self;

    /// Add a simulated delay before returning the result.
    pub const fn with_delay(self, delay: Duration) -> Self;

    /// Mark this tool as requiring approval before execution.
    pub const fn with_requires_approval(self, required: bool) -> Self;

    /// Check whether `execute()` was called at least once.
    pub fn was_executed(&self) -> bool;

    /// Return the number of times `execute()` was called.
    pub fn execution_count(&self) -> u32;
}

impl AgentTool for MockTool {
    fn name(&self) -> &str;
    fn label(&self) -> &str;           // Returns tool_name
    fn description(&self) -> &'static str; // "A mock tool for testing"
    fn parameters_schema(&self) -> &Value;
    fn requires_approval(&self) -> bool;
    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>;
}
```

### Guarantees

- `execute()` respects `cancellation_token` during delay: returns `"cancelled"` if token fires before delay elapses.
- `executed` and `execute_count` are updated atomically before the async delay.
- Default schema: `{"type": "object", "properties": {}, "additionalProperties": true}`.

## EventCollector (new)

### Contract

```rust
impl EventCollector {
    /// Create an empty collector.
    pub fn new() -> Self;

    /// Return a closure suitable for `Agent::on_event()` that captures events
    /// into this collector's shared storage.
    pub fn subscriber(&self) -> impl Fn(AgentEvent) + Send + Sync + 'static;

    /// Return a snapshot of all collected events in order.
    pub fn events(&self) -> Vec<AgentEvent>;

    /// Return the number of collected events.
    pub fn count(&self) -> usize;
}
```

### Guarantees

- Events are stored in the order received.
- `subscriber()` can be called multiple times; all closures write to the same storage.
- Thread-safe: `Arc<Mutex<Vec<AgentEvent>>>` backing store.
- `events()` clones the current snapshot — caller gets a stable copy.

## Helper Functions

### Contract

```rust
/// Default model spec: provider "test", model "test-model".
pub fn default_model() -> ModelSpec;

/// Default message converter: extracts LlmMessage from AgentMessage::Llm,
/// returns None for AgentMessage::Custom.
pub fn default_convert(msg: &AgentMessage) -> Option<LlmMessage>;

/// Build a single user text message with timestamp 0.
pub fn user_msg(text: &str) -> AgentMessage;

/// Build a complete text-only response event sequence:
/// Start → TextStart(0) → TextDelta(0, text) → TextEnd(0) → Done(Stop).
pub fn text_only_events(text: &str) -> Vec<AssistantMessageEvent>;

/// Build a complete tool call response event sequence:
/// Start → ToolCallStart(0, id, name) → ToolCallDelta(0, args) → ToolCallEnd(0) → Done(ToolUse).
pub fn tool_call_events(id: &str, name: &str, args: &str) -> Vec<AssistantMessageEvent>;
```

### Guarantees

- All functions are deterministic and side-effect-free.
- Event sequences follow the strict ordering enforced by `accumulate_message`:
  one `Start`, indexed content blocks, one terminal (`Done` or `Error`).
- `Usage::default()` and `Cost::default()` are used for all generated events.
