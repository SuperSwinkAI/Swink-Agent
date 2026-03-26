# CLAUDE.md — swink-agent-policies

## Scope

`policies/` — Policy implementations for `swink-agent`. Separate crate to keep implementations optional and independently feature-gated. Depends only on `swink-agent` public API (no internal imports).

## Key Facts

- **10 policies total:**
  - Core (6): `BudgetPolicy`, `MaxTurnsPolicy`, `ToolDenyListPolicy`, `SandboxPolicy`, `LoopDetectionPolicy`, `CheckpointPolicy`
  - Application (4): `PromptInjectionGuard`, `PiiRedactor`, `ContentFilter`, `AuditLogger`
- Each feature-gated independently: `default = ["all"]`, individual flags (`budget`, `max-turns`, `deny-list`, `sandbox`, `loop-detection`, `checkpoint`, `prompt-guard`, `pii`, `content-filter`, `audit`)
- All implementations depend only on `swink-agent` public API — no internal imports
- Policy traits take `&self` — stateful policies use interior mutability (`Mutex`)

## Lessons Learned

- `CheckpointPolicy` bridges sync/async via `tokio::spawn` fire-and-forget. Captures `Handle::current()` at construction.
- `SandboxPolicy` checks configured field names (default: `["path", "file_path", "file"]`) — Skip with error, no silent rewriting.
- `PromptInjectionGuard` implements both `PreTurnPolicy` and `PostTurnPolicy` — single struct, dual trait.
- `PiiRedactor` Inject verdict constructs `AgentMessage::Llm(LlmMessage::Assistant(...))` preserving original metadata.
- `ContentFilter` converts keywords to regex at construction time (`\b` for whole-word, `(?i)` for case-insensitive).
- `AuditSink` trait is sync (`fn write(&self, record: &AuditRecord)`) — defined in this crate, not in core.
- All regex patterns compiled once at construction, `evaluate()` only runs matches.
- Slot runner uses `AssertUnwindSafe` + `catch_unwind` — policy traits only need `Send + Sync`, not `UnwindSafe`.

## Build & Test

```bash
cargo build -p swink-agent-policies
cargo test -p swink-agent-policies
cargo clippy -p swink-agent-policies -- -D warnings
```
