# Feature Specification: Agent Struct & Public API

**Feature Branch**: `005-agent-struct`
**Created**: 2026-03-20
**Status**: Draft
**Input**: The stateful public API wrapper over the agent loop. Owns conversation history, manages steering/follow-up queues, enforces single-invocation concurrency, provides three invocation modes (streaming, async, sync), implements structured output, fans events to subscribers, supports dynamic model swapping with StreamFn resolution, and provides idle-waiting for non-blocking integration patterns. References: PRD §13 (Agent Struct), HLD API Layer.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Send a Prompt and Get a Response (Priority: P1)

A developer creates an agent, configures it with a system prompt and model, and sends a text prompt. The agent runs the loop, manages conversation history internally, and returns a result containing the assistant's response, stop reason, and token usage. The developer can choose between streaming (event-by-event), async (awaits completion), or blocking sync invocation.

**Why this priority**: This is the primary API — the way every application interacts with the agent. All three invocation modes must work.

**Independent Test**: Can be tested with a mock provider, sending a prompt via each invocation mode and verifying the result contains the expected response.

**Acceptance Scenarios**:

1. **Given** a configured agent, **When** the developer sends a prompt via the async API, **Then** the agent returns a result with messages, stop reason, and usage.
2. **Given** a configured agent, **When** the developer sends a prompt via the streaming API, **Then** they receive an async stream of lifecycle events.
3. **Given** a configured agent, **When** the developer sends a prompt via the sync API, **Then** the call blocks until completion and returns the same result as async.
4. **Given** a prompt sent as plain text, **When** the agent processes it, **Then** the text is wrapped as a user message and added to conversation history.
5. **Given** a prompt sent with images, **When** the agent processes it, **Then** both text and image content blocks are included.

---

### User Story 2 - Observe Agent Events (Priority: P1)

A developer subscribes to the agent's event stream to build custom UI or logging. They register a callback that receives lifecycle events. If a subscriber panics, the agent continues running — the panicking subscriber is automatically removed without affecting other subscribers.

**Why this priority**: Event observation is essential for any UI (TUI, web, etc.) and for logging/monitoring. Panic isolation prevents one bad subscriber from crashing the system.

**Independent Test**: Can be tested by subscribing multiple callbacks, triggering a prompt, and verifying all callbacks receive events. A deliberately panicking subscriber should be auto-removed.

**Acceptance Scenarios**:

1. **Given** a subscriber callback, **When** the agent runs, **Then** the callback receives all lifecycle events.
2. **Given** a subscription, **When** unsubscribe is called, **Then** no further events are delivered to that callback.
3. **Given** a subscriber that panics, **When** it panics during event dispatch, **Then** the agent continues running and the panicking subscriber is automatically removed.
4. **Given** multiple subscribers, **When** one panics, **Then** the remaining subscribers continue to receive events normally.

---

### User Story 3 - Steer the Agent Mid-Run (Priority: P1)

While the agent is running (executing tools), the developer enqueues a steering message to redirect it. The agent processes the steering message after the current tool batch, interrupting remaining tools. If the agent is idle, the steering message is queued for the next run.

**Why this priority**: Steering is the primary mechanism for interactive control — users need to redirect agents without waiting for completion.

**Independent Test**: Can be tested by starting a long-running tool execution, enqueuing a steering message, and verifying the agent redirects.

**Acceptance Scenarios**:

1. **Given** an agent executing tools, **When** a steering message is enqueued, **Then** remaining tools are cancelled and the message is processed on the next turn.
2. **Given** an idle agent, **When** a steering message is enqueued, **Then** it is held in the queue until the next prompt or continue call.
3. **Given** steering delivery mode set to "all at once," **When** multiple steering messages are queued, **Then** all are delivered together on the next turn.
4. **Given** steering delivery mode set to "one at a time," **When** multiple steering messages are queued, **Then** only one is delivered per turn.

---

### User Story 4 - Structured Output with Schema Validation (Priority: P2)

A developer requests structured output by providing a prompt and a schema describing the expected response shape. The agent injects a synthetic tool, invokes the LLM, validates the response against the schema, and returns a typed result. If the response is invalid, the agent retries up to a configurable maximum.

**Why this priority**: Structured output is important for applications that need parsed, validated responses, but it builds on the basic prompt/response flow.

**Independent Test**: Can be tested with a mock provider that returns a valid structured response on the first call (or invalid then valid), verifying schema validation and retry behavior.

**Acceptance Scenarios**:

1. **Given** a prompt and schema, **When** the provider returns a valid response, **Then** the agent returns a validated result matching the schema.
2. **Given** a prompt and schema, **When** the provider returns an invalid response, **Then** the agent retries via continue.
3. **Given** retries exhausted, **When** the last response is still invalid, **Then** the agent returns a structured output failure error with the attempt count.
4. **Given** a schema, **When** a typed result is requested, **Then** the response is deserialized into the requested type.

---

### User Story 5 - Manage Agent State (Priority: P2)

A developer modifies the agent's state between runs: changing the system prompt, switching models, updating tools, or clearing conversation history. They can also abort a running agent, wait for it to become idle, or reset all state to initial values.

**Why this priority**: State management is essential for long-lived agents that adapt over time, but secondary to basic prompt/response.

**Independent Test**: Can be tested by modifying state between prompts and verifying the next prompt uses the updated state.

**Acceptance Scenarios**:

1. **Given** an idle agent, **When** the system prompt is changed, **Then** the next prompt uses the new system prompt.
2. **Given** an idle agent, **When** the model is changed, **Then** the next prompt targets the new model.
3. **Given** an idle agent, **When** tools are updated, **Then** the next prompt uses the new tool set.
4. **Given** a running agent, **When** abort is called, **Then** the current run exits with an aborted stop reason.
5. **Given** a running agent, **When** wait-for-idle is called, **Then** it resolves when the current run finishes.
6. **Given** an agent with history, **When** reset is called, **Then** all state returns to initial values.

---

### User Story 6 - Dynamic Model Swapping at Runtime (Priority: P2) — I20

A developer switches the agent's model mid-session — for example, starting with a fast model for triage, then switching to a powerful model for complex reasoning. `set_model()` looks up the model in the registered `available_models` list and swaps both the `ModelSpec` and the `StreamFn`. If the model is not in `available_models`, `set_model_with_stream()` accepts an explicit `StreamFn`. The change takes effect on the next turn. A `ModelCycled` event is emitted when the model changes programmatically.

**Why this priority**: Dynamic model swapping enables cost optimization and adaptive complexity — agents can use cheap models for simple tasks and expensive models for hard ones, all within a single session.

**Independent Test**: Can be tested by configuring an agent with two available models, calling `set_model()` to switch, prompting, and verifying the new model is used.

**Acceptance Scenarios**:

1. **Given** an agent with available_models containing Model A and Model B, **When** `set_model(Model B)` is called, **Then** the agent's `stream_fn` is swapped to Model B's StreamFn and the next prompt uses Model B.
2. **Given** `set_model()` called with a model in `available_models`, **When** the switch completes, **Then** a `ModelCycled` event is emitted with the old and new model specs.
3. **Given** `set_model()` called with a model NOT in `available_models`, **When** the switch is attempted, **Then** only the `ModelSpec` is updated (no StreamFn swap) — the existing StreamFn continues to be used.
4. **Given** `set_model_with_stream(model, stream_fn)` called, **When** the switch completes, **Then** both the ModelSpec and StreamFn are updated, regardless of `available_models`.
5. **Given** a running agent, **When** `set_model()` is called, **Then** the change takes effect on the NEXT turn, not mid-turn.

---

### User Story 7 - Wait for Agent Idle (Priority: P3) — N15

A developer awaits the agent becoming idle using `wait_for_idle()`. This is useful for fire-and-forget patterns where callers start a prompt in a background task and need to know when it finishes. The method returns immediately if the agent is not running, and awaits a notification otherwise. It is safe to call from multiple tasks concurrently.

**Why this priority**: Nice-to-have for non-blocking integration patterns. Most callers use `prompt_async()` which already awaits completion.

**Independent Test**: Can be tested by starting a prompt in a background task, calling `wait_for_idle()` from the main task, and verifying it resolves when the prompt completes.

**Acceptance Scenarios**:

1. **Given** an idle agent (`is_running == false`), **When** `wait_for_idle()` is called, **Then** it returns immediately.
2. **Given** a running agent, **When** `wait_for_idle()` is called, **Then** it resolves when the agent finishes its current run.
3. **Given** multiple tasks calling `wait_for_idle()` concurrently, **When** the agent finishes, **Then** all waiting tasks are notified.
4. **Given** a running agent that is aborted, **When** `wait_for_idle()` is awaited, **Then** it resolves after the abort completes.

---

### Edge Cases

- What happens when prompt is called while the agent is already running — `check_not_running()` returns `Err(AgentError::AlreadyRunning)`.
- What happens when continue is called with empty conversation history — `validate_continue()` returns `Err(AgentError::NoMessages)`.
- What happens when continue is called and the last message is an assistant message — `validate_continue()` returns `Err(AgentError::InvalidContinue)` when the last message is an assistant message AND there are no pending messages in the steering/follow-up queues. If there are pending messages, continue is allowed because the queued messages will be injected.
- How does the agent handle a subscriber that is registered and unregistered during a run? — `subscribe` returns a `SubscriptionId`; calling `unsubscribe` with that ID removes the callback immediately and no further events are delivered to it. This is safe to call at any time because the `ListenerRegistry` manages dispatch with panic isolation.
- What happens when the steering queue is cleared while the agent is running? — `clear_steering()` uses `Arc<Mutex<>>` with poison recovery, so it is safe to call concurrently. Clearing removes any undelivered messages from the shared queue; the `QueueMessageProvider` will see an empty queue on its next `poll_steering` call.
- What happens when `set_model()` is called during a run? — The model change updates internal state but takes effect on the NEXT turn. The current turn continues with the previous model and StreamFn. This is safe because `set_model()` takes `&mut self` which prevents concurrent invocation with the running loop (Rust borrow checker enforces this).
- What happens when `wait_for_idle()` is called from an event subscriber callback? — The subscriber holds a reference that prevents calling `&mut self` methods, but `wait_for_idle()` takes `&self`, so it can be called. However, awaiting inside a synchronous callback would require a runtime — this is a caller responsibility. In practice, subscribers should not block.
- What happens when `set_model_with_stream()` is called with the same model that is already active? — The swap still occurs (both ModelSpec and StreamFn are replaced). No deduplication check — the caller knows what they're doing.

## Clarifications

### Session 2026-03-20

- Q: What happens when prompt is called while the agent is already running? → A: `check_not_running()` (line 956) checks `self.state.is_running` and returns `Err(AgentError::AlreadyRunning)` immediately.
- Q: What happens when continue is called with empty conversation history? → A: `validate_continue()` (line 964) returns `Err(AgentError::NoMessages)` when `self.state.messages.is_empty()`.
- Q: What happens when continue is called and the last message is an assistant message? → A: `validate_continue()` (line 967) returns `Err(AgentError::InvalidContinue)` when the last message is `LlmMessage::Assistant` AND there are no pending steering/follow-up messages. If pending messages exist, continue is permitted because queued messages will be injected into the next turn.
- Q: How does the agent handle a subscriber that is registered and unregistered during a run? → A: `unsubscribe(id)` removes the callback from the `ListenerRegistry` immediately. No further events are delivered to it. This is safe to call at any time; the registry handles concurrent dispatch with panic isolation (via `catch_unwind`).
- Q: What happens when the steering queue is cleared while the agent is running? → A: `clear_steering()` acquires the `Arc<Mutex<>>` lock (with poison recovery) and clears the Vec. The `QueueMessageProvider` shares the same Arc, so its next `poll_steering` call will see an empty queue and return no messages.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a stateful agent struct that owns conversation history, model configuration, tools, and queues.
- **FR-002**: Agent MUST be configurable via an options object at construction time (system prompt, model, tools, streaming function, hooks, retry strategy, delivery modes).
- **FR-003**: Agent MUST support three invocation modes: streaming (returns event stream), async (awaits result), and sync (blocks until complete).
- **FR-004**: Agent MUST accept prompts as plain text, text with images, or a list of pre-constructed messages.
- **FR-005**: Agent MUST support continue operations (streaming, async, sync) that resume from existing context without adding new messages.
- **FR-006**: Only one active invocation MUST be permitted at a time. A second call while running MUST return an error.
- **FR-007**: Agent MUST provide steering and follow-up queues with configurable delivery modes (all at once or one at a time).
- **FR-008**: Steering and follow-up MUST be safe to call at any time, including while the agent is running.
- **FR-009**: Agent MUST provide structured output that injects a synthetic tool, validates the response against a schema, retries on invalid responses, and returns a typed result.
- **FR-010**: Agent MUST provide a subscriber registry where callbacks receive lifecycle events.
- **FR-011**: Event dispatch MUST be panic-isolated — a panicking subscriber MUST be automatically removed without affecting the agent or other subscribers.
- **FR-012**: Agent MUST provide abort (signals cancellation), wait-for-idle (resolves when done), and reset (clears all state) control operations.
- **FR-013**: Agent MUST provide state mutation methods: set system prompt, set model, set thinking level, set tools, set/append/clear messages.
- **FR-014**: The public API module MUST re-export all public types so consumers never reach into submodules.
- **FR-015**: `set_model()` MUST look up the model in `available_models` (registered at construction via `with_available_models()`) and swap both the `ModelSpec` and `StreamFn` when found. If the model is not in `available_models`, only the `ModelSpec` is updated.
- **FR-016**: The system MUST provide `set_model_with_stream(model, stream_fn)` for swapping to models not registered in `available_models`.
- **FR-017**: Model swaps MUST emit a `ModelCycled` event with old and new model specs.
- **FR-018**: Model swaps MUST take effect on the next turn, not mid-turn.
- **FR-019**: `wait_for_idle()` MUST return immediately if the agent is not running, and MUST resolve when the current run finishes.
- **FR-020**: `wait_for_idle()` MUST be safe to call from multiple tasks concurrently — all waiters are notified when the agent becomes idle.

### Key Entities

- **Agent**: The stateful public API wrapper — owns history, queues, subscribers, and configuration.
- **AgentOptions**: Construction-time configuration — initial state, hooks, delivery modes, streaming function, retry strategy.
- **AgentState**: Internal mutable state — system prompt, model, tools, messages, running flag, stream message, pending tool calls, error.
- **SubscriptionId**: Opaque handle returned by subscribe, used to unsubscribe.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All three invocation modes (streaming, async, sync) return correct results for a basic prompt/response cycle.
- **SC-002**: Calling prompt while running returns a concurrency error 100% of the time.
- **SC-003**: Subscriber callbacks receive all lifecycle events in the correct order.
- **SC-004**: A panicking subscriber is automatically removed without affecting agent operation or other subscribers.
- **SC-005**: Structured output validates against the schema and retries on invalid responses up to the configured maximum.
- **SC-006**: Steering messages enqueued during a run are processed after the current tool batch completes.
- **SC-007**: Abort causes the running agent to exit with an aborted stop reason.
- **SC-008**: Reset clears all state (messages, queues, error) to initial values.
- **SC-009**: The sync invocation mode blocks without requiring the caller to manage an async runtime.
- **SC-010**: `set_model()` correctly swaps the StreamFn when the model is found in `available_models`, and the next prompt uses the new model.
- **SC-011**: `set_model_with_stream()` swaps both ModelSpec and StreamFn regardless of `available_models`.
- **SC-012**: `wait_for_idle()` returns immediately for an idle agent and resolves when a running agent finishes.

## Assumptions

- The Agent struct is the primary public API surface — most applications interact with it rather than the loop directly.
- The sync API internally manages a runtime for blocking — callers do not need to provide one.
- Delivery mode defaults are "one at a time" for both steering and follow-up.
- The structured output retry mechanism uses continue (not a fresh prompt) to give the LLM its previous invalid attempt as context.
- Queues use thread-safe interior mutability with poisoned-lock recovery.
- `set_model()` and `set_model_with_stream()` both take `&mut self` — the Rust borrow checker prevents calling them while the agent is running (which holds a mutable borrow). No runtime guard needed beyond the existing `&mut self` requirement.
- `wait_for_idle()` uses `Arc<Notify>` internally — the Notify is signaled when `is_running` transitions to `false`.
- Both features (dynamic model swap, wait_for_idle) are already implemented in `src/agent.rs`. These user stories formalize the existing behavior with acceptance scenarios and test coverage requirements.
