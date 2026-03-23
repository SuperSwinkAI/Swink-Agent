# Research: Eval Trajectory & Matching

## Context

The `swink-agent-eval` crate already contains a complete implementation of all spec-023 components. This research documents a gap analysis between the spec (post-clarification) and the implementation, plus decisions on how to close each gap.

## Gap Analysis

### Gap 1: Custom matching function panic handling (FR-008)

**Spec says**: Custom matching function panics MUST be caught and treated as criterion failures with diagnostic context.

**Code does**: `ResponseMatcher::evaluate()` calls `f(actual)` directly without `catch_unwind`. A panicking custom function will propagate the panic to the caller.

- **Decision**: Wrap the `Custom` arm in `std::panic::catch_unwind` and return `Score::fail()` with the panic message.
- **Rationale**: Matches the spec requirement. The `catch_unwind` pattern is already used in `agent.rs` for event subscriber panics.
- **Alternatives considered**: Requiring custom functions to return `Result<Score, String>` — rejected because it changes the public API and the `Fn(&str) -> Score` signature is already shipped.

### Gap 2: `score_exact` empty golden path behavior

**Spec says**: Empty golden path → all actual steps are "unexpected"; match score is 0.0.

**Code does**: `score_exact` returns `(Score::new(0.0, 1.0), "expected 0 tool calls, got N")` when counts differ. When both are empty, it returns `0/0 = 0.0` due to `max(1)` denominator. Actually: empty expected + empty actual → `expected.len() == actual.len()` (both 0), matched = 0, total = max(0,1) = 1, value = 0/1 = 0.0. This is correct for the empty-empty case. For empty expected + non-empty actual, the length check catches it (0 != N) → returns 0.0 with message. Correct.

- **Decision**: No code change needed. Behavior matches spec.
- **Rationale**: The `score_exact` function already handles this case correctly.

### Gap 3: `score_in_order` empty golden path behavior

**Spec says**: Empty golden path → score is 0.0 (no expected steps to match).

**Code does**: Returns `Score::pass()` with "no expected tool calls" message when expected is empty.

- **Decision**: This is a **divergence**. The code treats "nothing expected" as a pass (vacuous truth), while the spec says 0.0. The code's behavior is arguably more correct for evaluation semantics — if you don't expect any tool calls and none happened, that's not a failure. If you don't expect any tool calls and some happened, InOrder still passes because all 0 expected calls were found.
- **Rationale**: Keep the code's behavior (vacuous truth = pass). Update the spec's edge case to note that empty golden path behavior differs by mode: Exact returns 0.0 (length mismatch if actual is non-empty), InOrder/AnyOrder return pass (vacuous truth). This is the standard behavior in evaluation frameworks.
- **Alternatives considered**: Changing InOrder/AnyOrder to return 0.0 for empty expected — rejected because it would make "no trajectory constraint" cases fail, which breaks `EvaluatorRegistry` semantics where evaluators return `None` for non-applicable cases.

### Gap 4: Test coverage for spec acceptance scenarios

**Spec defines**: 16 acceptance scenarios across 4 user stories + 8 edge cases.

**Code has**: Unit tests covering core algorithm correctness, but not explicitly named after spec acceptance scenarios.

- **Decision**: Add integration tests in `eval/tests/` that map 1:1 to spec acceptance scenarios, using descriptive names matching the spec (e.g., `us1_multi_tool_invocations_captured`).
- **Rationale**: Constitution requires test-driven development; explicit acceptance test mapping ensures spec compliance is verifiable.

### Gap 5: `ResponseCriteria::Custom` — no `Debug` for inner panic message

When `catch_unwind` catches a panic from a custom function, we need to extract a message. The panic payload is `Box<dyn Any + Send>`, which we can downcast to `&str` or `String`.

- **Decision**: Use the standard downcast pattern: try `&str` first, then `String`, else "unknown panic".
- **Rationale**: Same pattern used in `agent.rs` `dispatch_event`.

## Technology Decisions

| Component | Choice | Why |
|---|---|---|
| Panic catching | `std::panic::catch_unwind` | Already used in core crate; no new dependencies |
| Test structure | Integration tests mapping to spec scenarios | Constitution requires TDD; explicit mapping aids auditing |
| Empty golden path | Vacuous truth (pass) for InOrder/AnyOrder | Standard eval semantics; prevents false failures |

## Unresolved Items

None. All gaps have clear resolution paths.
