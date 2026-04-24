# AGENTS.md — swink-agent-eval-judges

## Lessons Learned

- `XaiJudgeClient` uses the same judge-call wire format as `OpenAiJudgeClient`: bearer auth plus `POST /v1/chat/completions`, so xAI judge tests can reuse the OpenAI-style canned response body and retry semantics.
- `AzureJudgeClient` expects a deployment-scoped base URL and appends `/chat/completions?api-version=...`; unlike the other OpenAI-compatible judge clients it authenticates with the `api-key` header, so Azure tests should match the query string and header explicitly.
