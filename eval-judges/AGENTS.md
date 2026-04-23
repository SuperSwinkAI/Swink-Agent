# AGENTS.md — swink-agent-eval-judges

## Lessons Learned

- `XaiJudgeClient` uses the same judge-call wire format as `OpenAiJudgeClient`: bearer auth plus `POST /v1/chat/completions`, so xAI judge tests can reuse the OpenAI-style canned response body and retry semantics.
