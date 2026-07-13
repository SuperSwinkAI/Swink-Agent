# Provider Expansion Roadmap

**Related Documents:**
- [PRD](./PRD.md)
- [HLD](../architecture/HLD.md)
- [Streaming Architecture](../architecture/streaming/README.md)

**Status:** Live provider backlog — open items: Groq, Together, Fireworks, OpenRouter. Shipped providers are summarized in one line each under "Shipped Providers" below.

**Goal:** Expand provider coverage in a way that maximizes user value early, keeps the adapter surface coherent, and reuses the shared model catalog instead of proliferating one-off integrations.

---

## Planning Principle

Work is ordered by:

1. **Ease of implementation**
2. **Popularity / likely user demand**

That means we should land the easiest, highest-leverage provider additions first, especially when they can reuse the existing OpenAI-compatible adapter and only need preset packs plus provider-specific environment handling.

---

## Current Baseline

The repo already has:

- Native adapters (all shipped):
  - Anthropic
  - OpenAI-compatible
  - Google Gemini
  - Ollama
  - Proxy
  - Azure OpenAI (spec 016)
  - xAI / Grok (spec 017)
  - Mistral (spec 018)
  - AWS Bedrock (spec 019)
- Catalog-backed preset groups: Anthropic, OpenAI, Google, Azure, xAI, Mistral, Bedrock, Cohere (compatibility-API preset pack), Local SmolLM3-3B

The remaining wave of work is all **preset-first providers** — providers that
ride on the existing OpenAI-compatible stream adapter and only need preset
packs plus provider-specific environment handling.

---

## Shipped Providers

These were formerly Tier 2/3 tasks in this document; each is done and needs no further roadmap tracking:

- **Cohere** — shipped as a compatibility-API preset pack; catalog preset exists.
- **Azure OpenAI / AI Foundry** — shipped as a native adapter (spec 016, `specs/016-adapter-azure/`).
- **xAI / Grok** — shipped as a native adapter (spec 017, `specs/017-adapter-xai/`).
- **Mistral** — shipped as a native adapter (spec 018, `specs/018-adapter-mistral/`).
- **AWS Bedrock** — shipped as a native Converse/ConverseStream adapter with SigV4 signing (spec 019, `specs/019-adapter-bedrock/`).

---

## Priority Queue

## Tier 1 — Easiest, High-Demand Additions

These should be treated as the first ordered tasks because they have the best ease-to-value ratio.

### Task 1 — Groq preset pack

**Why first**
- Very low implementation cost if their OpenAI-compatible API path is sufficient
- Popular for speed-sensitive inference
- Good fit for F4 model cycling and benchmark comparisons

**Expected work**
- Add `groq` provider section to the shared TOML catalog
- Add grouped presets for common text/tool-capable models
- Add `GROQ_API_KEY` and `GROQ_BASE_URL` to `.env.example`
- Extend remote preset helpers to construct Groq connections via the OpenAI-compatible adapter
- Add focused preset-loading and wiremock request tests

**Suggested preset groups**
- `llama`
- `qwen`
- `deepseek`
- `kimi` if exposed through the platform

**Official references**
- [Groq OpenAI compatibility](https://console.groq.com/docs/openai)
- [Groq tool use](https://console.groq.com/docs/tool-use/overview)

### Task 2 — Together AI preset pack

**Why second**
- Broad model catalog
- High user familiarity in open-model workflows
- Also likely achievable without a new native adapter

**Expected work**
- Add `together` provider section to the shared TOML catalog
- Add common instruct/chat model presets
- Add `TOGETHER_API_KEY` and `TOGETHER_BASE_URL`
- Validate tool-calling compatibility against the current OpenAI-style path

**Suggested preset groups**
- `llama`
- `qwen`
- `deepseek`
- `mistral`

**Official references**
- [Together docs](https://docs.together.ai/)
- [Together recommended models](https://docs.together.ai/docs/recommended-models)

### Task 3 — Fireworks AI preset pack

**Why third**
- Similar ease profile to Groq and Together
- Useful for open-model access and enterprise-ish deployment flexibility

**Expected work**
- Add `fireworks` provider section to the shared TOML catalog
- Add common chat/tool-capable presets
- Add `FIREWORKS_API_KEY` and `FIREWORKS_BASE_URL`
- Confirm request compatibility with the current OpenAI adapter

**Suggested preset groups**
- `llama`
- `qwen`
- `deepseek`
- `mistral`

**Official references**
- [Fireworks OpenAI compatibility](https://docs.fireworks.ai/tools-sdks/openai-compatibility)
- [Fireworks recommended models](https://fireworks.ai/docs/guides/recommended-models)

### Task 4 — OpenRouter preset pack

**Why fourth**
- High leverage because it fronts many providers
- Useful for users who want one API key for lots of models
- Slightly lower priority than Groq/Together/Fireworks because it overlaps with direct-provider support

**Expected work**
- Add `openrouter` provider section to the shared TOML catalog
- Add a curated preset list instead of mirroring the entire catalog
- Add `OPENROUTER_API_KEY` and `OPENROUTER_BASE_URL`
- Decide whether to expose provider-specific routing metadata in display labels

**Suggested preset groups**
- `anthropic`
- `openai`
- `google`
- `open_models`

**Official references**
- [OpenRouter models overview](https://openrouter.ai/docs/docs/overview/models)

---

## Task Ordering Summary

Use this as the default execution order unless product priorities change:

1. Groq preset pack
2. Together preset pack
3. Fireworks preset pack
4. OpenRouter preset pack

(Former tasks 5–9 — Cohere, Azure, xAI, Mistral, Bedrock — have all shipped; see "Shipped Providers" above.)

---

## Shared Implementation Rules

Every provider addition should follow the same rollout checklist:

1. Add provider and presets to `src/model_catalog.toml`
2. Keep provider-specific construction in the owning crate, not the TUI
3. Add `.env.example` entries for required credentials and optional base URL overrides
4. Add preset-loading tests
5. Add adapter wiremock tests for:
   - auth
   - request path
   - text streaming
   - tool calling
   - error handling
6. Add ignored live tests where external credentials are required
7. Only add providers to default example/TUI model cycling after text/tool support is proven stable

---

## Catalog Shape Guidance

As the preset list grows, keep the shared catalog grouped by provider and family:

- provider
  - credential env var
  - base URL env var
  - default base URL
  - presets
- preset
  - `id`
  - `display_name`
  - `group`
  - `model_id`
  - `api_version` when relevant
  - `capabilities`
  - `status`
  - `context_window_tokens` when known

This keeps the catalog scalable as providers add more models and lets UIs filter by capability without hard-coding provider-specific lists.

---

## Recommendation

The best next concrete milestone is a **Preset Expansion Sprint**:

- Groq
- Together
- Fireworks
- OpenRouter

That sprint should produce the biggest user-visible increase in provider coverage for the least engineering cost. The heavier native adapters (Azure, xAI, Mistral, Bedrock) have already shipped, so this preset sprint is the entire remaining backlog.
