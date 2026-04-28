# AGENTS.md — swink-agent-policies

## Scope

`policies/` — Policy implementations. Separate crate, depends only on `swink-agent` public API.

## Key Facts

- **10 policies**: BudgetPolicy, MaxTurnsPolicy, ToolDenyListPolicy, SandboxPolicy, LoopDetectionPolicy, CheckpointPolicy, PromptInjectionGuard, PiiRedactor, ContentFilter, AuditLogger.
- Feature-gated individually; `full` enables all. Traits take `&self`; stateful policies use interior mutability.

## Key Invariants

- `CheckpointPolicy` bridges sync/async via `tokio::spawn` fire-and-forget (`Handle::current()` at construction).
- `SandboxPolicy` checks path fields (`path`, `file_path`, `file`) — Skip with error, no rewriting. Must resolve against canonical root + `execution_root`.
- `PromptInjectionGuard` implements both `PreTurnPolicy` and `PostTurnPolicy`.
- `ContentFilter` compiles keyword regexes at construction (`\b`, `(?i)`). All regex patterns compiled once.
- `AuditSink` trait is sync, defined in this crate (not core).
