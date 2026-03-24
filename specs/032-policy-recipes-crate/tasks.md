# Tasks: Policy Recipes Crate

**Input**: Design documents from `/specs/032-policy-recipes-crate/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**TDD Note**: Per constitution principle II (Test-Driven Development), test tasks within each phase MUST be executed before their corresponding implementation tasks, regardless of task ID ordering. Write tests first, verify they fail, then implement.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Crate Scaffolding)

**Purpose**: Create the new workspace crate and configure feature gates

- [x] T001 Create `policies/Cargo.toml` with workspace membership, feature gates (`prompt-guard`, `pii`, `content-filter`, `audit`, `all` default), and dependencies (`swink-agent` path dep, optional `regex`, `chrono`, `serde`, `serde_json`, `tracing`)
- [x] T002 Add `"policies"` to workspace members list in root `Cargo.toml`
- [x] T003 Create `policies/src/lib.rs` with `#![forbid(unsafe_code)]`, feature-gated module declarations, and conditional re-exports
- [x] T004 [P] Create empty stub files: `policies/src/prompt_guard.rs`, `policies/src/pii_redactor.rs`, `policies/src/content_filter.rs`, `policies/src/audit_logger.rs`
- [x] T005 Create `policies/tests/composition.rs` integration test file
- [x] T006 Verify `cargo build -p swink-agent-policies` compiles with empty stubs and `cargo build -p swink-agent-policies --no-default-features` compiles with zero features

**Checkpoint**: Empty crate compiles with all feature gate combinations.

---

## Phase 2: Foundational (Shared Helpers)

**Purpose**: Any shared utilities used by multiple policies

- [x] T007 Verify `ContentBlock::extract_text()` is publicly available from `swink-agent` re-exports — this is the text extraction method all scanning policies will use. If not re-exported, document the alternative approach for extracting text from `Vec<ContentBlock>`.

**Checkpoint**: Text extraction path confirmed. All policies can proceed independently.

---

## Phase 3: User Story 1 - Prompt Injection Protection (Priority: P1) 🎯 MVP

**Goal**: PromptInjectionGuard blocks injection in user messages (PreTurn) and tool results (PostTurn)

**Independent Test**: Configure agent with guard in pre_turn, send injection phrases, verify Stop before LLM call

### Tests

- [x] T008 [P] [US1] Write unit tests in `policies/src/prompt_guard.rs` `#[cfg(test)]`: `default_patterns_block_ignore_instructions`, `default_patterns_block_role_reassignment`, `default_patterns_allow_benign_message`, `default_patterns_allow_partial_match`, `custom_pattern_blocks`, `empty_message_returns_continue`, `without_defaults_only_custom`
- [x] T009 [P] [US1] Write unit tests for PostTurn path in `policies/src/prompt_guard.rs` `#[cfg(test)]`: `post_turn_blocks_tool_result_injection`, `post_turn_allows_clean_tool_result`

### Implementation

- [x] T010 [US1] Define default injection patterns (~10 patterns) as a constant array in `policies/src/prompt_guard.rs`: "ignore all previous instructions", "disregard your system prompt", "you are now a", "forget your instructions", "override your programming", "new persona", "jailbreak", "pretend you are", "act as if you have no restrictions", "ignore the above"
- [x] T011 [US1] Implement `PromptInjectionGuard` struct in `policies/src/prompt_guard.rs`: `new()` (compiles default patterns), `with_pattern(name, regex) -> Result<Self, regex::Error>`, `without_defaults() -> Self`
- [x] T012 [US1] Implement `PreTurnPolicy` for `PromptInjectionGuard` in `policies/src/prompt_guard.rs`: iterate `ctx.new_messages`, extract user messages, run `extract_text()` on content, match against patterns, return Stop with pattern name on first match
- [x] T013 [US1] Implement `PostTurnPolicy` for `PromptInjectionGuard` in `policies/src/prompt_guard.rs`: iterate `turn.tool_results`, run `extract_text()` on content, match against patterns, return Stop with pattern name on first match
- [x] T014 [US1] Re-export `PromptInjectionGuard` from `policies/src/lib.rs` under `prompt-guard` feature gate

**Checkpoint**: PromptInjectionGuard works in both slots. Default patterns block >=10 injection phrases with zero false positives on benign messages.

---

## Phase 4: User Story 2 - PII Redaction (Priority: P1)

**Goal**: PiiRedactor detects and redacts PII from assistant responses

**Independent Test**: Configure agent with PiiRedactor in post_turn, trigger response with email/phone/SSN, verify redacted output

### Tests

- [x] T015 [P] [US2] Write unit tests in `policies/src/pii_redactor.rs` `#[cfg(test)]`: `redacts_email`, `redacts_phone`, `redacts_ssn`, `redacts_credit_card`, `redacts_ipv4`, `redacts_multiple_pii_types`, `overlapping_matches_resolved_left_to_right`, `no_pii_returns_continue`, `stop_mode_returns_stop`, `custom_placeholder_used`, `custom_pattern_works`

### Implementation

- [x] T016 [US2] Define default PII patterns in `policies/src/pii_redactor.rs`: email (RFC 5322 simplified), US phone (`(\+1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}`), SSN (`\d{3}-\d{2}-\d{4}`), credit card (`\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}`), IPv4 (`\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}`)
- [x] T017 [US2] Implement `PiiRedactor`, `PiiMode`, `PiiPattern` structs in `policies/src/pii_redactor.rs`: `new()` (default patterns, Redact mode), `with_mode(PiiMode)`, `with_placeholder(impl Into<String>)`, `with_pattern(name, regex) -> Result<Self, regex::Error>`
- [x] T018 [US2] Implement `PostTurnPolicy` for `PiiRedactor` in `policies/src/pii_redactor.rs`: extract text from `turn.assistant_message.content`, apply regex replacements in order, construct `AgentMessage::Llm(LlmMessage::Assistant(...))` with redacted text preserving original metadata (provider, model_id, usage, cost, stop_reason, timestamp), return Inject or Stop based on mode
- [x] T019 [US2] Re-export `PiiRedactor`, `PiiMode`, `PiiPattern` from `policies/src/lib.rs` under `pii` feature gate

**Checkpoint**: PiiRedactor correctly identifies and redacts all 5 PII categories. Inject mode replaces message, Stop mode blocks it.

---

## Phase 5: User Story 3 - Content Filtering (Priority: P2)

**Goal**: ContentFilter blocks responses containing prohibited terms

**Independent Test**: Configure agent with ContentFilter blocklist, trigger response with blocked term, verify Stop

### Tests

- [x] T020 [P] [US3] Write unit tests in `policies/src/content_filter.rs` `#[cfg(test)]`: `blocks_keyword`, `case_insensitive_match`, `whole_word_no_substring_match`, `regex_pattern_blocks`, `category_filtering_active`, `category_filtering_inactive_passes`, `empty_filter_allows_all`, `invalid_regex_returns_error`, `no_match_returns_continue`

### Implementation

- [x] T021 [US3] Implement `ContentFilter`, `FilterRule`, `ContentFilterError` in `policies/src/content_filter.rs`: `new()` (empty), `with_keyword(word)`, `with_regex(pattern) -> Result`, `with_category_keyword(cat, word)`, `with_category_regex(cat, pattern) -> Result`, `with_case_insensitive(bool)`, `with_whole_word(bool)`, `with_enabled_categories(iter)`. Keywords converted to regex at construction (with `\b` boundaries if whole-word, `(?i)` if case-insensitive).
- [x] T022 [US3] Implement `PostTurnPolicy` for `ContentFilter` in `policies/src/content_filter.rs`: extract text from `turn.assistant_message.content`, iterate active rules (filtered by enabled categories), return Stop with matched display_name on first match
- [x] T023 [US3] Re-export `ContentFilter`, `ContentFilterError`, `FilterRule` from `policies/src/lib.rs` under `content-filter` feature gate

**Checkpoint**: ContentFilter enforces case-insensitive, whole-word, and category filtering with zero false matches outside configured categories.

---

## Phase 6: User Story 4 - Audit Logging (Priority: P2)

**Goal**: AuditLogger records every turn to a pluggable sink

**Independent Test**: Configure agent with AuditLogger + JsonlAuditSink, run multi-turn conversation, verify JSONL output

### Tests

- [x] T024 [P] [US4] Write unit tests in `policies/src/audit_logger.rs` `#[cfg(test)]`: `always_returns_continue`, `sink_receives_record`, `record_has_all_fields`, `jsonl_sink_writes_valid_json` (using tempfile), `jsonl_sink_handles_write_error_gracefully`

### Implementation

- [x] T025 [US4] Define `AuditSink` trait, `AuditRecord`, `AuditUsage`, `AuditCost` in `policies/src/audit_logger.rs` with serde derives
- [x] T026 [US4] Implement `AuditLogger` struct in `policies/src/audit_logger.rs`: `new(impl AuditSink + 'static)` wrapping sink in `Arc`
- [x] T027 [US4] Implement `PostTurnPolicy` for `AuditLogger` in `policies/src/audit_logger.rs`: build `AuditRecord` from `PolicyContext` (turn_index, usage, cost) and `TurnPolicyContext` (extract text summary from assistant_message, collect tool call names from tool_results), call `sink.write(&record)`, always return Continue
- [x] T028 [US4] Implement `JsonlAuditSink` in `policies/src/audit_logger.rs`: `new(impl Into<PathBuf>)`, `AuditSink::write` uses `std::fs::OpenOptions::append` + `serde_json::to_string` + `writeln!`, errors logged via `tracing::warn!`
- [x] T029 [US4] Re-export `AuditLogger`, `AuditSink`, `AuditRecord`, `AuditUsage`, `AuditCost`, `JsonlAuditSink` from `policies/src/lib.rs` under `audit` feature gate

**Checkpoint**: AuditLogger produces valid JSONL with all expected fields. Always returns Continue. Write errors logged, never panicked.

---

## Phase 7: Composition & Integration

**Purpose**: Verify all policies compose together and feature gates work independently

### Tests

- [x] T030 Write integration test in `policies/tests/composition.rs`: `all_policies_compose` — PromptInjectionGuard in pre_turn + post_turn, PiiRedactor + ContentFilter + AuditLogger in post_turn. Verify they can all be instantiated and their `name()` methods return expected identifiers.
- [x] T031 [P] Write integration test in `policies/tests/composition.rs`: `feature_gates_independent` — verify each policy compiles independently by checking type availability under its feature gate

### Validation

- [x] T032 Run `cargo test -p swink-agent-policies` — all tests pass
- [x] T033 [P] Run `cargo test -p swink-agent-policies --no-default-features --features prompt-guard` — only prompt_guard compiles
- [x] T034 [P] Run `cargo test -p swink-agent-policies --no-default-features --features audit` — only audit compiles
- [x] T035 Run `cargo clippy -p swink-agent-policies -- -D warnings` — zero warnings

**Checkpoint**: All policies compose. Feature gates isolate correctly. Zero warnings.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, final validation

- [x] T036 [P] Add doc comments to all public types and trait methods in `policies/src/prompt_guard.rs` with usage examples
- [x] T037 [P] Add doc comments to all public types in `policies/src/pii_redactor.rs` with usage examples
- [x] T038 [P] Add doc comments to all public types in `policies/src/content_filter.rs` with usage examples
- [x] T039 [P] Add doc comments to all public types in `policies/src/audit_logger.rs` with usage examples
- [x] T040 Add crate-level doc comment in `policies/src/lib.rs` describing the crate's dual purpose (usable policies + reference examples)
- [x] T041 Validate quickstart.md examples match the implemented API (spot-check 3 examples)
- [x] T042 Update `CLAUDE.md` Lessons Learned section with policy recipes crate notes
- [x] T043 Run `cargo test --workspace` — verify no regressions across the entire workspace

**Checkpoint**: All documented. Zero warnings. All tests green. Feature complete.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — confirms text extraction path
- **US1 Prompt Guard (Phase 3)**: Depends on Phase 2
- **US2 PII Redaction (Phase 4)**: Depends on Phase 2 only
- **US3 Content Filter (Phase 5)**: Depends on Phase 2 only
- **US4 Audit Logger (Phase 6)**: Depends on Phase 2 only
- **Composition (Phase 7)**: Depends on Phases 3-6 (all policies)
- **Polish (Phase 8)**: Depends on Phase 7

### Parallel Opportunities

- **Phase 1**: T004 (stub files) can run in parallel
- **Phases 3-6**: US1, US2, US3, US4 can all proceed in parallel after Phase 2
- **Phase 7**: T033, T034 (feature gate tests) can run in parallel
- **Phase 8**: T036-T039 (doc comments) can run in parallel

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (crate scaffolding)
2. Complete Phase 2: Foundational (text extraction verification)
3. Complete Phase 3: User Story 1 (PromptInjectionGuard)
4. **STOP and VALIDATE**: Guard blocks injection in both PreTurn and PostTurn
5. This alone delivers the most impactful security policy

### Incremental Delivery

1. Setup + Foundational → Crate compiles
2. Add US1 (Prompt Guard) → MVP security
3. Add US2 (PII Redaction) → Privacy compliance
4. Add US3 (Content Filter) → Content compliance
5. Add US4 (Audit Logger) → Observability
6. Phase 7-8 (Composition + Polish) → Production ready

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires TDD: write tests first, verify they fail, then implement
- `ContentBlock::extract_text()` is the canonical text extraction helper — all scanning policies use it
- PiiRedactor's Inject verdict constructs `AgentMessage::Llm(LlmMessage::Assistant(...))` preserving original metadata
- Regex patterns are compiled once at construction — `evaluate()` only runs matches
- `JsonlAuditSink` uses `std::fs::OpenOptions::append` — no file locking, suitable for single-writer scenarios
