# AGENTS.md — swink-agent-eval-judges

## Lessons Learned

- `XaiJudgeClient` uses the same judge-call wire format as `OpenAiJudgeClient`: bearer auth plus `POST /v1/chat/completions`, so xAI judge tests can reuse the OpenAI-style canned response body and retry semantics.
- `AzureJudgeClient` expects a deployment-scoped base URL and appends `/chat/completions?api-version=...`; unlike the other OpenAI-compatible judge clients it authenticates with the `api-key` header, so Azure tests should match the query string and header explicitly.
- Spec-facing type aliases are not enough inside provider modules alone; `eval-judges/src/lib.rs` must re-export the canonical `OpenAI*` names as well or downstream users only see the internal `OpenAi*` spellings.
- `swink-agent-eval` keeps `JudgeClient` object-safe with a boxed-future method so `swink-agent-eval --no-default-features` does not pull in `async-trait`. Provider clients in `eval-judges` must implement that boxed-future signature directly rather than reintroducing the proc-macro dependency.
