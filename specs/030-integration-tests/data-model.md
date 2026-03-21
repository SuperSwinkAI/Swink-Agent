# Data Model: Integration Tests

**Branch**: `030-integration-tests` | **Date**: 2026-03-20

## Entities

### MockStreamFn (existing)

A configurable mock replacing the real stream provider. Returns scripted event sequences in FIFO order.

| Field | Type | Purpose |
|-------|------|---------|
| `responses` | `Mutex<Vec<Vec<AssistantMessageEvent>>>` | Queue of scripted event sequences; each `stream()` call pops the front |

**Location**: `tests/common/mod.rs` (already implemented)

**Behavior**: When `responses` is exhausted, returns an error event. Thread-safe via `Mutex`.

### FlagStreamFn (existing)

A `StreamFn` variant that sets a boolean flag when called. Used to verify which stream function was invoked (e.g., in fallback scenarios).

| Field | Type | Purpose |
|-------|------|---------|
| `called` | `AtomicBool` | Set to `true` on first `stream()` call |
| `responses` | `Mutex<Vec<Vec<AssistantMessageEvent>>>` | Same FIFO queue as `MockStreamFn` |

**Location**: `tests/common/mod.rs` (already implemented)

### ContextCapturingStreamFn (existing)

A `StreamFn` that captures the message count from each context snapshot. Used to verify context compaction and sliding window behavior.

| Field | Type | Purpose |
|-------|------|---------|
| `responses` | `Mutex<Vec<Vec<AssistantMessageEvent>>>` | Scripted responses |
| `captured_message_counts` | `Mutex<Vec<usize>>` | Number of messages in context at each call |

**Location**: `tests/common/mod.rs` (already implemented)

### ApiKeyCapturingStreamFn (existing)

A `StreamFn` that captures resolved API keys. Used to verify API key resolution and rotation.

| Field | Type | Purpose |
|-------|------|---------|
| `responses` | `Mutex<Vec<Vec<AssistantMessageEvent>>>` | Scripted responses |
| `captured_api_keys` | `Mutex<Vec<Option<String>>>` | API keys seen at each call |

**Location**: `tests/common/mod.rs` (already implemented)

### MockTool (existing)

A configurable mock tool with controllable behavior.

| Field | Type | Purpose |
|-------|------|---------|
| `tool_name` | `String` | Tool identifier |
| `schema` | `Value` | JSON Schema for parameter validation |
| `result` | `Mutex<Option<AgentToolResult>>` | Configurable return value |
| `delay` | `Option<Duration>` | Simulated execution latency |
| `executed` | `AtomicBool` | Whether `execute()` was called |
| `execute_count` | `AtomicU32` | How many times `execute()` was called |
| `approval_required` | `bool` | Whether the tool requires approval |

**Location**: `tests/common/mod.rs` (already implemented)

**Builder methods**: `with_schema()`, `with_result()`, `with_delay()`, `with_requires_approval()`

### EventCollector (new)

A subscriber that captures all agent events for post-hoc assertion.

| Field | Type | Purpose |
|-------|------|---------|
| `events` | `Arc<Mutex<Vec<AgentEvent>>>` | Ordered collection of all received events |

**Location**: `tests/common/mod.rs` (to be added)

**Methods**:
- `new() -> Self` — Creates empty collector
- `subscriber() -> impl Fn(AgentEvent)` — Returns closure suitable for `Agent::on_event()`
- `events() -> Vec<AgentEvent>` — Snapshot of collected events
- `count() -> usize` — Number of events collected

### TestHelpers (existing + extensions)

Shared utility functions providing convenience constructors.

| Function | Signature | Purpose |
|----------|-----------|---------|
| `default_model()` | `() -> ModelSpec` | Test model spec ("test", "test-model") |
| `default_convert()` | `(&AgentMessage) -> Option<LlmMessage>` | Standard message converter |
| `user_msg()` | `(&str) -> AgentMessage` | Build a user message |
| `text_only_events()` | `(&str) -> Vec<AssistantMessageEvent>` | Text-only response events |
| `tool_call_events()` | `(&str, &str, &str) -> Vec<AssistantMessageEvent>` | Tool call response events |

**Location**: `tests/common/mod.rs` (already implemented)

### Acceptance Criterion Mapping

| AC | Description | Test File | Test Function(s) |
|----|-------------|-----------|-------------------|
| AC 1 | Agent creation with mock stream | `ac_lifecycle.rs` | `agent_creation_with_mock_stream` |
| AC 2 | Message processing | `ac_lifecycle.rs` | `message_processing_produces_response` |
| AC 3 | Lifecycle event emission | `ac_lifecycle.rs` | `lifecycle_events_emitted_in_order` |
| AC 4 | Streaming text delivery | `ac_lifecycle.rs` | `streaming_delivers_text_tokens` |
| AC 5 | Turn completion and history | `ac_lifecycle.rs` | `turn_completion_accumulates_history` |
| AC 6 | Tool registration and discovery | `ac_tools.rs` | `tool_registration_and_discovery` |
| AC 7 | Schema validation rejects invalid args | `ac_tools.rs` | `schema_validation_rejects_invalid_args` |
| AC 8 | Tool execution with valid args | `ac_tools.rs` | `tool_execution_with_valid_args` |
| AC 9 | Concurrent tool execution | `ac_tools.rs` | `concurrent_tool_execution` |
| AC 10 | Tool error handling | `ac_tools.rs` | `tool_error_handling` |
| AC 11 | Tool result in follow-up | `ac_tools.rs` | `tool_result_in_followup_message` |
| AC 12 | Tool call transformation | `ac_tools.rs` | `tool_call_transformation` |
| AC 13 | Context window tracking | `ac_context.rs` | `context_window_tracking` |
| AC 14 | Sliding window compaction | `ac_context.rs` | `sliding_window_preserves_anchor_and_tail` |
| AC 15 | Context overflow retry | `ac_context.rs` | `context_overflow_triggers_retry` |
| AC 16 | Tool-result pair preservation | `ac_context.rs` | `tool_result_pairs_kept_together` |
| AC 17 | Retry with backoff | `ac_resilience.rs` | `retry_with_backoff_on_throttle` |
| AC 18 | Steering callback | `ac_resilience.rs` | `steering_callback_modifies_messages` |
| AC 19 | Abort mechanism | `ac_resilience.rs` | `abort_stops_running_turn` |
| AC 20 | Synchronous API | `ac_resilience.rs` | `sync_api_blocks_until_complete` |
| AC 21 | Follow-up decision callback | `ac_resilience.rs` | `followup_decision_controls_continuation` |
| AC 22 | Custom messages survive compaction | `ac_resilience.rs` | `custom_messages_survive_compaction` |
| AC 23 | Structured output with schema | `ac_structured.rs` | `structured_output_with_schema` |
| AC 24 | Schema enforcement | `ac_structured.rs` | `schema_enforcement_rejects_invalid` |
| AC 25 | Proxy stream reconstruction | `ac_structured.rs` | `proxy_stream_reconstruction` |
| AC 26 | Role-based border colors | `ac_tui.rs` | `role_based_border_colors` |
| AC 27 | Inline diff rendering | `ac_tui.rs` | `inline_diff_color_coding` |
| AC 28 | Context gauge thresholds | `ac_tui.rs` | `context_gauge_color_thresholds` |
| AC 29 | Plan mode restricts write tools | `ac_tui.rs` | `plan_mode_restricts_write_tools` |
| AC 30 | Approval mode classification | `ac_tui.rs` | `approval_mode_classifies_tools` |
