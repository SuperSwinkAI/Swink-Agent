# Implementation Plan: Policy Recipes Crate

**Branch**: `032-policy-recipes-crate` | **Date**: 2026-03-24 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/032-policy-recipes-crate/spec.md`

## Summary

New workspace crate `swink-agent-policies` providing four application-level policies built entirely against `swink-agent`'s public API: `PromptInjectionGuard` (PreTurn + PostTurn), `PiiRedactor` (PostTurn), `ContentFilter` (PostTurn), and `AuditLogger` (PostTurn). Each policy is feature-gated independently. The crate doubles as a reference example for building custom policies — no internal imports, no `pub(crate)` access.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `swink-agent` (core types — policy traits, message types, verdict enums), `regex` (pattern matching for injection/PII/content), `chrono` (timestamps for audit records), `serde`/`serde_json` (audit record serialization), `tracing` (error logging in audit sink)
**Storage**: Local filesystem via JSONL (AuditLogger's `JsonlAuditSink` only)
**Testing**: `cargo test --workspace` — unit tests in each module, integration test file in `policies/tests/`
**Target Platform**: Cross-platform library (any target supporting swink-agent)
**Project Type**: Library crate (`swink-agent-policies`)
**Performance Goals**: Regex evaluation per policy < 1ms for typical message lengths. Zero overhead when a policy feature is disabled.
**Constraints**: `#[forbid(unsafe_code)]`; depends only on swink-agent public re-exports; no async in policy evaluate paths (sync trait)
**Scale/Scope**: New crate — 4 modules (one per policy), ~200-400 lines each. Plus `lib.rs` re-exports and `AuditSink` trait.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | New workspace crate `swink-agent-policies`. Self-contained, independently compilable, independently testable. Depends only on `swink-agent` public API. |
| II. Test-Driven Development | PASS | Unit tests per policy module. Integration tests verifying composition. TDD: tests written before implementation. |
| III. Efficiency & Performance | PASS | Regex patterns compiled once at construction. `new_messages` slice is zero-copy. Feature gates eliminate dead code. Sync evaluation — no async overhead. |
| IV. Leverage the Ecosystem | PASS | Uses `regex` crate (standard, high-quality). Uses `chrono` for timestamps. No hand-rolled regex engine or date handling. |
| V. Provider Agnosticism | PASS | Policies are provider-agnostic — they see `PolicyContext`, `TurnPolicyContext`, and message types, not provider-specific data. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Regex compilation errors at construction time (not evaluate). Audit sink errors logged, never panicked. |

**Crate count violation**: Constitution says 7 members, adding an 8th.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 8th workspace crate | Application-level policies with regex/chrono deps don't belong in core (which must stay provider/storage-free) or adapters (which are provider-specific). A separate crate keeps core lean, provides feature gating, and serves as an external-consumer reference example. | Putting policies in `src/policies/` in core would add regex/chrono to core's dependency tree and blur the line between structural policies (LoopDetection, Sandbox) and application-level recipes. |

## Project Structure

### Documentation (this feature)

```text
specs/032-policy-recipes-crate/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output (minimal — no unknowns)
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Phase 1 output
└── checklists/
    └── requirements.md  # Spec quality checklist
```

### Source Code (repository root)

```text
policies/                            # NEW crate: swink-agent-policies
├── Cargo.toml                       # workspace member, feature gates
├── src/
│   ├── lib.rs                       # re-exports all enabled policies + AuditSink trait
│   ├── prompt_guard.rs              # PromptInjectionGuard (PreTurnPolicy + PostTurnPolicy)
│   ├── pii_redactor.rs              # PiiRedactor (PostTurnPolicy)
│   ├── content_filter.rs            # ContentFilter (PostTurnPolicy)
│   └── audit_logger.rs              # AuditLogger (PostTurnPolicy) + AuditSink trait + JsonlAuditSink
└── tests/
    └── composition.rs               # Integration tests: policies composed in an agent config
```

**Structure Decision**: New `policies/` directory at workspace root, following the pattern of `adapters/`, `memory/`, `eval/`, `tui/`. Each policy is one file — they are self-contained ~200-400 line modules. `lib.rs` re-exports everything behind feature gates.

## Notes

- **Regex compilation**: All regex patterns (injection, PII, content filter) are compiled once in the constructor via `RegexSet` or individual `Regex` objects. `evaluate()` calls only run matches — no compilation on the hot path.
- **PromptInjectionGuard dual-trait**: Single struct implements both `PreTurnPolicy` and `PostTurnPolicy`. In PreTurn, scans `ctx.new_messages` for user messages containing injection patterns. In PostTurn, scans `turn.tool_results` content for indirect injection. Same regex set used in both paths.
- **PiiRedactor Inject construction**: Returns `PolicyVerdict::Inject(vec![agent_message])` where the message is an `AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage { ... }))` with redacted text in a `ContentBlock::Text`. The original assistant message's metadata (provider, model_id, usage, cost) is preserved in the replacement.
- **ContentFilter pattern categories**: Each pattern has an optional `category: Option<String>`. The filter builder accepts `with_enabled_categories(impl IntoIterator<Item = impl Into<String>>)`. At evaluate time, only patterns whose category is in the enabled set (or has no category) are checked.
- **AuditSink trait**: Defined as `pub trait AuditSink: Send + Sync { fn write(&self, record: &AuditRecord); }`. Synchronous to match the sync policy trait. `JsonlAuditSink` uses `std::fs::OpenOptions::append` for atomic-ish line writes. Errors logged via `tracing::warn!`, never propagated.
- **Feature gates**: `prompt-guard` enables `prompt_guard` module + `regex` dep. `pii` enables `pii_redactor` module + `regex` dep. `content-filter` enables `content_filter` module + `regex` dep. `audit` enables `audit_logger` module + `chrono`/`serde_json` deps. `all` (default) enables everything. `regex` is a shared optional dep activated by any of the first three features.
- **ContentBlock::extract_text()**: Public helper on `ContentBlock` that concatenates all `Text` block content. Used by PiiRedactor and ContentFilter to extract scannable text from `AssistantMessage.content` and by PromptInjectionGuard to extract text from `UserMessage.content` and `ToolResultMessage.content`.
