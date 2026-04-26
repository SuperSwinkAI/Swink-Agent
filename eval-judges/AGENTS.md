# AGENTS.md — swink-agent-eval-judges

## Key Invariants

- `XaiJudgeClient` uses same wire format as `OpenAiJudgeClient` (bearer auth + `/v1/chat/completions`).
- `AzureJudgeClient` appends `/chat/completions?api-version=...` and uses `api-key` header.
- `eval-judges/src/lib.rs` must re-export canonical `OpenAI*` names alongside internal `OpenAi*` spellings.
- `JudgeClient` is object-safe with boxed-future return (no `async-trait` dep).
