# Public API Contract: Agent Struct

**Feature**: 005-agent-struct | **Date**: 2026-03-20

## Constructors

```rust
// Full constructor with explicit message converter
Agent::new(options: AgentOptions) -> Agent

// AgentOptions constructors
AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm) -> AgentOptions
AgentOptions::new_simple(system_prompt, model, stream_fn) -> AgentOptions  // uses default_convert
AgentOptions::from_connections(system_prompt, connections) -> AgentOptions
```

## Identity & State

```rust
agent.id() -> AgentId
agent.state() -> &AgentState
```

## Invocation: Prompt

```rust
// Core invocations (accept Vec<AgentMessage>)
agent.prompt_stream(input) -> Result<Pin<Box<dyn Stream<Item = AgentEvent>>>, AgentError>
agent.prompt_async(input) -> Result<AgentResult, AgentError>   // async
agent.prompt_sync(input) -> Result<AgentResult, AgentError>    // blocking

// Convenience (accept text strings)
agent.prompt_text(text) -> Result<AgentResult, AgentError>                     // async
agent.prompt_text_with_images(text, images) -> Result<AgentResult, AgentError> // async
agent.prompt_text_sync(text) -> Result<AgentResult, AgentError>                // blocking
```

## Invocation: Continue

```rust
agent.continue_stream() -> Result<Pin<Box<dyn Stream<Item = AgentEvent>>>, AgentError>
agent.continue_async() -> Result<AgentResult, AgentError>   // async
agent.continue_sync() -> Result<AgentResult, AgentError>    // blocking
```

## Structured Output

```rust
agent.structured_output(prompt, schema) -> Result<Value, AgentError>          // async
agent.structured_output_sync(prompt, schema) -> Result<Value, AgentError>     // blocking
agent.structured_output_typed<T>(prompt, schema) -> Result<T, AgentError>     // async, deserializes
agent.structured_output_typed_sync<T>(prompt, schema) -> Result<T, AgentError> // blocking
```

## State Mutation

```rust
agent.set_system_prompt(prompt)
agent.set_model(model)
agent.set_thinking_level(level)
agent.set_tools(tools)
agent.add_tool(tool)
agent.remove_tool(name) -> bool
agent.set_approval_mode(mode)
agent.set_messages(messages)
agent.append_messages(messages)
agent.clear_messages()
```

## Tool Discovery

```rust
agent.find_tool(name) -> Option<&Arc<dyn AgentTool>>
agent.tools_matching(predicate) -> Vec<&Arc<dyn AgentTool>>
agent.tools_in_namespace(namespace) -> Vec<&Arc<dyn AgentTool>>
```

## Queue Management

```rust
agent.steer(message)
agent.follow_up(message)
agent.clear_steering()
agent.clear_follow_up()
agent.clear_queues()
agent.has_pending_messages() -> bool
```

## Control

```rust
agent.abort()
agent.pause() -> Option<LoopCheckpoint>
agent.resume(checkpoint) -> Result<AgentResult, AgentError>          // async
agent.resume_stream(checkpoint) -> Result<Stream, AgentError>
agent.reset()
agent.wait_for_idle() -> impl Future<Output = ()>
```

## Observation

```rust
agent.subscribe(callback) -> SubscriptionId
agent.unsubscribe(id) -> bool
agent.add_event_forwarder(f)
agent.forward_event(event)
agent.emit(name, payload)
agent.handle_stream_event(event)  // for manual stream processing
```

## Checkpointing

```rust
agent.save_checkpoint(id) -> Result<Checkpoint, io::Error>           // async
agent.restore_from_checkpoint(checkpoint)
agent.load_and_restore_checkpoint(id) -> Result<Option<Checkpoint>, io::Error>  // async
agent.checkpoint_store() -> Option<&dyn CheckpointStore>
```

## Plan Mode

```rust
agent.enter_plan_mode() -> (Vec<Arc<dyn AgentTool>>, String)  // returns saved state
agent.exit_plan_mode(saved_tools, saved_prompt)
```

## Error Variants

| Error | Trigger |
|-------|---------|
| `AgentError::AlreadyRunning` | Calling prompt/continue while agent is running |
| `AgentError::NoMessages` | Calling continue with empty history |
| `AgentError::InvalidContinue` | Last message is assistant with no pending queue messages |
| `AgentError::StructuredOutputFailed { attempts, last_error }` | Schema validation fails after all retries |

## AgentOptions Builder Methods

All return `Self` for chaining:

```rust
.with_tools(tools)
.with_retry_strategy(strategy)
.with_stream_options(options)
.with_transform_context(transformer)
.with_transform_context_fn(closure)
.with_get_api_key(resolver)
.with_steering_mode(mode)
.with_follow_up_mode(mode)
.with_structured_output_max_retries(n)
.with_approve_tool(callback)
.with_approval_mode(mode)
.with_tool_validator(validator)
.with_available_models(models)
.with_tool_call_transformer(transformer)
.with_loop_policy(policy)
.with_post_turn_hook(hook)
.with_event_forwarder(callback)
.with_async_transform_context(transformer)
.with_checkpoint_store(store)
.with_metrics_collector(collector)
.with_token_counter(counter)
.with_model_fallback(fallback)
.with_message_channel() -> MessageSender  // mutates in place, returns sender
.with_external_message_provider(provider)
.with_budget_guard(guard)
.with_cost_limit(max_cost)
.with_token_limit(max_tokens)
.with_tool_execution_policy(policy)
.with_plan_mode_addendum(addendum)
```
