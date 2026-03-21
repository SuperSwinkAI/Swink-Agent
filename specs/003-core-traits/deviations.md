# 003 — Core Traits: Deviation Log

This document records all known differences between the spec contracts
(`contracts/public-api.md`) and the shipped implementation. Every entry was
reviewed and accepted as an intentional design choice.

---

## Deviation Table

| ID | Location | Spec Says | Implementation Has | Rationale | Status |
|----|----------|-----------|-------------------|-----------|--------|
| D-001 | `AgentToolResult.details` | `Option<Value>` | `Value` (`Value::Null` serves as `None`) | Avoids double-wrapping (`Some(Value::Null)` vs `Value::Null`); simpler downstream matching. | Accepted — intentional design choice |
| D-002 | `AssistantMessageEvent` field names | `index`, `text`, `thinking`, `json_fragment` | `content_index`, `delta` (uniform across variants) | Uniform field names simplify accumulation logic; one accessor pattern instead of per-variant names. | Accepted — intentional design choice |
| D-003 | `StreamOptions.max_tokens` | `Option<u32>` | `Option<u64>` | u64 accommodates larger context windows (>4 B tokens). | Accepted — intentional design choice |
| D-004 | `StreamOptions` extra field | Field not present | `api_key: Option<String>` | Enables per-request key rotation without rebuilding the stream function. | Accepted — intentional design choice |
| D-005 | `RetryStrategy.should_retry` attempt type | `attempt: usize` | `attempt: u32` | u32 is sufficient (max ~4 B retries) and matches `DefaultRetryStrategy` internal fields. | Accepted — intentional design choice |
| D-006 | `RetryStrategy` extra method | Method not present | `as_any(&self) -> &dyn Any` | Enables downcasting for serialization and checkpoint persistence. | Accepted — intentional design choice |
| D-007 | `AssistantMessageEvent::Start` | `Start { provider: String, model: String }` | Unit variant `Start` (no fields) | Provider and model are passed to `accumulate_message()` instead; avoids duplicating provider info in every event stream. | Accepted — intentional design choice |
| D-008 | `StreamFn` method name | `call()` | `stream()` | `stream` is more descriptive of the method's purpose (it returns a stream, it does not "call" anything). | Accepted — intentional design choice |
| D-009 | `StreamFn` return type | `async fn call() -> Pin<Box<dyn Stream>>` | `fn stream() -> Pin<Box<dyn Stream>>` (not async) | No async needed — the method just constructs and returns the stream; the stream itself is lazy. | Accepted — intentional design choice |
| D-010 | `accumulate_message` signature | Takes `impl Stream<Item = AssistantMessageEvent>` | Takes `Vec<AssistantMessageEvent>` + `provider: &str` + `model_id: &str` | Vec-based input is simpler for the accumulator's purposes; provider/model moved here from the `Start` event (see D-007). | Accepted — intentional design choice |
| D-011 | `AgentTool.parameters_schema` return type | `serde_json::Value` (owned) | `&Value` (borrowed reference) | Avoids cloning the schema on every call; schemas are static per tool instance. | Accepted — intentional design choice |
| D-012 | `AgentTool.execute` update_callback | `Option<Box<dyn Fn(String) + Send>>` | `Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>` | `AgentToolResult` is more structured than a plain `String` (carries `is_error`, `details`); `+Sync` enables concurrent access from spawned tasks. | Accepted — intentional design choice |
| D-013 | `AssistantMessageEvent::Error` fields | `error_message: String` only | `error_message: String` + `stop_reason: Option<String>` + `usage: Option<Usage>` + `error_kind: Option<StreamErrorKind>` | Richer error metadata enables retry classification and usage tracking even on failure. | Accepted — intentional design choice |
| D-014 | `AssistantMessageDelta` variant names | `TextDelta`, `ThinkingDelta`, `ToolCallDelta` with fields `index`/`text`/`thinking`/`json_fragment` | `Text`, `Thinking`, `ToolCall` with fields `content_index`/`delta` | Consistent with `AssistantMessageEvent` naming (D-002); shorter names since the enum itself signals "delta". | Accepted — intentional design choice |
| D-015 | `StreamOptions.transport` type name | `Transport` | `StreamTransport` | More specific name avoids ambiguity with unrelated transport concepts elsewhere in the codebase. | Accepted — intentional design choice |
| D-016 | `AssistantMessageEvent` attributes | No `#[non_exhaustive]` | `#[non_exhaustive]` on the enum | Forward compatibility — allows adding new event variants in minor releases without breaking downstream matches. | Accepted — intentional design choice |
| D-017 | Implementation extras not in spec | Not specified | `StreamErrorKind` enum | Classifies stream errors (rate-limit, network, auth, etc.) for retry logic (see D-013). | Accepted — intentional design choice |
| D-018 | Implementation extras not in spec | Not specified | `ToolMetadata` struct | Attaches display hints and categorization to tools without polluting the `AgentTool` trait. | Accepted — intentional design choice |
| D-019 | Implementation extras not in spec | Not specified | `ToolApproval` system (`ToolApproval` enum, `selective_approve`) | Enables human-in-the-loop gating of dangerous tool calls; orthogonal to `ToolValidator`. | Accepted — intentional design choice |
| D-020 | Implementation extras not in spec | Not specified | `redact_sensitive_values` helper | Sanitizes tool arguments before logging; security utility beyond spec scope. | Accepted — intentional design choice |
| D-021 | Implementation extras not in spec | Not specified | `validate_schema` helper | Validates a `serde_json::Value` against a JSON Schema; shared by tool validation and tests. | Accepted — intentional design choice |
| D-022 | Implementation extras not in spec | Not specified | `unknown_tool_result` / `validation_error_result` constructors | Convenience constructors for common error-path `AgentToolResult` values; reduce boilerplate in the loop. | Accepted — intentional design choice |

---

## Notes

- All deviations were identified by diffing `contracts/public-api.md` against
  the implemented types in `src/`. None are accidental omissions.
- Deviations D-001 through D-016 are modifications to spec-defined contracts.
- Deviations D-017 through D-022 are additive extensions that do not conflict
  with the spec; they provide functionality the spec did not cover.
- No spec-required item was removed or left unimplemented.
