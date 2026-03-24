# Research: Policy Recipes Crate

**Date**: 2026-03-24 | **Status**: Complete

## Public API Surface Verification

**Decision**: All four policies can be built using only `swink-agent`'s public re-exports.
**Rationale**: Verified that `lib.rs` re-exports: `PreTurnPolicy`, `PostTurnPolicy`, `PolicyContext`, `TurnPolicyContext`, `PolicyVerdict`, `AgentMessage`, `LlmMessage`, `UserMessage`, `AssistantMessage`, `ToolResultMessage`, `ContentBlock`, `Usage`, `Cost`, `StopReason`. No internal imports needed.
**Alternatives considered**: None — this was a prerequisite constraint, not a choice.

## PolicyContext.new_messages Availability

**Decision**: `PolicyContext.new_messages` field is implemented and available in core (031-policy-slots). PreTurn policies receive the pending message batch; PostTurn/PostLoop/PreDispatch receive empty slices.
**Rationale**: Verified in `src/policy.rs:73-81` and `src/loop_/turn.rs:52-60`. Zero-copy slice borrow.
**Alternatives considered**: Full `messages` history was the original approach — replaced with `new_messages` for efficiency.

## Regex Strategy

**Decision**: Use `regex::Regex` for individual patterns with `is_match()`. Compile all patterns at construction time, store as `Vec<Regex>`.
**Rationale**: `RegexSet` is faster for "does any pattern match?" but doesn't identify which pattern matched — needed for ContentFilter's Stop message. Individual `Regex` objects provide both match detection and match identification. Pattern count is small (10-30 patterns typical) so iteration overhead is negligible.
**Alternatives considered**: `regex::RegexSet` — faster but loses per-pattern identification. `aho-corasick` — overkill for the pattern counts involved.

## PII Redaction Approach

**Decision**: Use `regex::Regex::replace_all()` with pattern-specific replacements. Apply patterns in order: email → phone → SSN → credit card → IPv4. Each replacement yields `[REDACTED]` (or custom placeholder).
**Rationale**: Order matters for overlapping patterns. Applying more specific patterns first (email has `@`, phone has dashes/parens) reduces false positive overlap.
**Alternatives considered**: Single-pass approach with `RegexSet` — would require manual offset tracking for replacements, more complex with no real benefit at these pattern counts.

## Audit Record Serialization

**Decision**: `AuditRecord` derives `serde::Serialize`. `JsonlAuditSink` uses `serde_json::to_string()` + `writeln!()` for one-record-per-line JSONL.
**Rationale**: JSONL is the simplest structured log format — grep-friendly, append-only, no closing bracket needed. Matches `swink-agent-memory`'s JSONL session store pattern.
**Alternatives considered**: JSON array (requires rewriting the file on each append), CSV (loses nested structure), binary formats (not human-readable).

## AgentMessage Construction for Inject

**Decision**: PiiRedactor constructs `AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage { ... }))` with redacted text, preserving original metadata (provider, model_id, usage, cost, stop_reason, timestamp).
**Rationale**: The Inject verdict adds messages to the pending queue. An assistant message with redacted content replaces the original in the conversation flow. Preserving metadata ensures downstream consumers (metrics, checkpoints) see consistent data.
**Alternatives considered**: Injecting a `UserMessage` with redacted content — semantically wrong, the redaction is of the assistant's output. Injecting a `Custom` message — would not be rendered correctly by consumers expecting `LlmMessage`.
