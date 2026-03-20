# Data Model: Agent Struct & Public API

**Feature**: 005-agent-struct | **Date**: 2026-03-20

## Entities

### Agent

The stateful public API wrapper. Owns all mutable state and provides invocation methods.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `AgentId` | Unique monotonic identifier (from atomic counter) |
| `state` | `AgentState` | Observable agent state (see below) |
| `steering_queue` | `Arc<Mutex<Vec<AgentMessage>>>` | Thread-safe steering message queue |
| `follow_up_queue` | `Arc<Mutex<Vec<AgentMessage>>>` | Thread-safe follow-up message queue |
| `listeners` | `ListenerRegistry` | Subscriber callbacks with panic isolation |
| `abort_controller` | `Option<CancellationToken>` | Active loop cancellation token |
| `steering_mode` | `SteeringMode` | Queue drain mode: `All` or `OneAtATime` |
| `follow_up_mode` | `FollowUpMode` | Queue drain mode: `All` or `OneAtATime` |
| `stream_fn` | `Arc<dyn StreamFn>` | LLM streaming function (provider-agnostic) |
| `convert_to_llm` | `ConvertToLlmFn` | Message conversion filter (drops CustomMessage) |
| `transform_context` | `Option<TransformContextArc>` | Context compaction (sliding window) |
| `retry_strategy` | `Arc<dyn RetryStrategy>` | Retry policy for transient errors |
| `stream_options` | `StreamOptions` | Per-call streaming options |
| `structured_output_max_retries` | `usize` | Max retries for structured output validation (default: 3) |
| `idle_notify` | `Arc<Notify>` | Notifier for `wait_for_idle()` |
| `in_flight_llm_messages` | `Option<Vec<AgentMessage>>` | Accumulated messages during a run |
| `approve_tool` | `Option<ApproveToolArc>` | Tool approval callback |
| `approval_mode` | `ApprovalMode` | Whether approval gate is active |
| `tool_validator` | `Option<Arc<dyn ToolValidator>>` | Custom validation hook |
| `loop_policy` | `Option<Arc<dyn LoopPolicy>>` | Loop continuation policy |
| `tool_call_transformer` | `Option<Arc<dyn ToolCallTransformer>>` | Pre-execution argument transformer |
| `post_turn_hook` | `Option<Arc<dyn PostTurnHook>>` | Post-turn lifecycle hook |
| `model_stream_fns` | `Vec<(ModelSpec, Arc<dyn StreamFn>)>` | Model/StreamFn pairs for cycling |
| `event_forwarders` | `Vec<EventForwarderFn>` | Event forwarder callbacks |
| `async_transform_context` | `Option<AsyncTransformContextArc>` | Async context transformer |
| `checkpoint_store` | `Option<CheckpointStoreArc>` | Checkpoint persistence |
| `metrics_collector` | `Option<Arc<dyn MetricsCollector>>` | Per-turn metrics |
| `fallback` | `Option<ModelFallback>` | Model fallback chain |
| `external_message_provider` | `Option<Arc<dyn MessageProvider>>` | External message source |
| `budget_guard` | `Option<BudgetGuard>` | Pre-call cost/token guard |
| `tool_execution_policy` | `ToolExecutionPolicy` | Concurrent vs sequential tool dispatch |
| `plan_mode_addendum` | `Option<String>` | Custom plan mode prompt addendum |

### AgentState

Observable state exposed via `agent.state()`.

| Field | Type | Description |
|-------|------|-------------|
| `system_prompt` | `String` | Current system prompt |
| `model` | `ModelSpec` | Current model specification |
| `tools` | `Vec<Arc<dyn AgentTool>>` | Currently available tools |
| `messages` | `Vec<AgentMessage>` | Full conversation history |
| `is_running` | `bool` | Whether the agent loop is executing |
| `stream_message` | `Option<AgentMessage>` | Message currently being streamed |
| `pending_tool_calls` | `HashSet<String>` | Tool call IDs currently executing |
| `error` | `Option<String>` | Last error from a run |
| `available_models` | `Vec<ModelSpec>` | Models available for cycling |

### AgentOptions

Construction-time configuration consumed by `Agent::new()`.

| Field | Type | Default |
|-------|------|---------|
| `system_prompt` | `String` | Required |
| `model` | `ModelSpec` | Required |
| `stream_fn` | `Arc<dyn StreamFn>` | Required |
| `convert_to_llm` | `ConvertToLlmFn` | Required (or `default_convert` via `new_simple`) |
| `tools` | `Vec<Arc<dyn AgentTool>>` | `Vec::new()` |
| `transform_context` | `Option<TransformContextArc>` | `SlidingWindowTransformer(100k, 50k, 2)` |
| `retry_strategy` | `Box<dyn RetryStrategy>` | `DefaultRetryStrategy::default()` |
| `stream_options` | `StreamOptions` | `StreamOptions::default()` |
| `steering_mode` | `SteeringMode` | `OneAtATime` |
| `follow_up_mode` | `FollowUpMode` | `OneAtATime` |
| `structured_output_max_retries` | `usize` | `3` |
| `approval_mode` | `ApprovalMode` | `ApprovalMode::default()` |
| All optional hooks/policies | `Option<...>` | `None` |

### SubscriptionId

Opaque handle returned by `Agent::subscribe()`, used to unsubscribe.

| Field | Type | Description |
|-------|------|-------------|
| (inner) | `u64` | Monotonic counter value |

### SteeringMode / FollowUpMode

Enums controlling queue drain behavior.

| Variant | Behavior |
|---------|----------|
| `All` | Drain all pending messages at once |
| `OneAtATime` (default) | Drain one message per poll |

## Relationships

```
AgentOptions --constructs--> Agent
Agent --owns--> AgentState
Agent --owns--> ListenerRegistry --manages--> SubscriptionId
Agent --shares--> Arc<Mutex<Vec<AgentMessage>>> (queues) --read-by--> QueueMessageProvider
Agent --delegates--> agent_loop() / agent_loop_continue()
AgentHandle --owns--> Agent (moved into spawned task)
```
