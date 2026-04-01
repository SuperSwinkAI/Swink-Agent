# Provider Expansion Roadmap

**Related Documents:**
- [PRD](./PRD.md)
- [HLD](../architecture/HLD.md)
- [Streaming Architecture](../architecture/streaming/README.md)

**Status:** Draft planning document

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

- Native adapters:
  - Anthropic
  - OpenAI-compatible
  - Google Gemini
  - Ollama
  - Proxy
- Catalog-backed preset groups:
  - Anthropic
  - OpenAI
  - Google
  - Local SmolLM3-3B

This makes the next wave of work naturally split into two tracks:

- **Track A: preset-first providers**
  Providers that can likely ride on the existing OpenAI-compatible stream adapter
- **Track B: native adapter providers**
  Providers with distinct protocol/auth/request semantics that deserve first-class adapters

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

## Tier 2 — Moderate Effort, Strong Demand

These are still attractive, but they need more provider-specific thought than simple preset packs.

### Task 5 — Cohere support

**Why here**
- Popular enough to matter
- Has a compatibility path, but may also merit native handling later for Cohere-specific behavior
- Good candidate for a two-step rollout

**Recommended approach**
- Phase 1: preset pack on compatibility API if tool streaming maps cleanly
- Phase 2: revisit whether a native Cohere adapter is worth it

**Expected work**
- Add `cohere` provider section and preset groups
- Add `COHERE_API_KEY` and `COHERE_BASE_URL`
- Add compatibility-path tests for streaming and tool use

**Suggested preset groups**
- `command`
- `reasoning`

**Official references**
- [Cohere compatibility API](https://docs.cohere.com/docs/compatibility-api)
- [Cohere Command A Reasoning](https://docs.cohere.com/docs/command-a-reasoning)

### Task 6 — Azure OpenAI / Azure AI Foundry adapter

**Why here**
- Very common in enterprise environments
- Higher implementation complexity because deployment naming and endpoint semantics differ from plain OpenAI
- Deserves a clearer native integration rather than being hidden behind generic base URLs

**Expected work**
- Add an `azure` provider section to the catalog
- Model presets should carry deployment-oriented metadata, not just raw model IDs
- Add a native adapter or a provider-specific wrapper around the OpenAI transport
- Add `.env.example` entries for endpoint, API key, deployment, and API version

**Suggested preset groups**
- `gpt`
- `o_series`
- `azure_hosted_open_models` if relevant later

**Official references**
- [Azure AI Foundry inference overview](https://learn.microsoft.com/en-us/azure/ai-foundry/model-inference/overview)
- [Azure AI Foundry inference how-to](https://learn.microsoft.com/en-us/azure/ai-foundry/model-inference/how-to/inference)

---

## Tier 3 — Native Adapter Investments

These are likely worth doing, but they should come after the easy wins above unless there is a specific product reason to prioritize them.

### Task 7 — xAI / Grok native adapter

**Why**
- Strong user interest
- Growing importance for coding/reasoning use cases
- Worth supporting natively if the OpenAI-compatible route is incomplete or lossy

**Expected work**
- Add `xai` provider section and Grok preset groups
- Implement native auth/request/stream handling if needed
- Add live tests for text and tool use

**Suggested preset groups**
- `grok_reasoning`
- `grok_fast`

**Official references**
- [xAI docs](https://docs.x.ai/docs)
- [xAI models](https://docs.x.ai/docs/models/)

### Task 8 — Mistral native adapter

**Why**
- Important independent provider
- Strong open-model and enterprise relevance
- Clean long-term addition to the adapter set

**Expected work**
- Add `mistral` provider section and grouped presets
- Implement native streaming and tool-calling support
- Add `.env.example` support and live ignored tests

**Suggested preset groups**
- `codestral`
- `magistral`
- `mistral_large`
- `mistral_small`

**Official references**
- [Mistral models](https://docs.mistral.ai/getting-started/models)

### Task 9 — AWS Bedrock native adapter

**Why**
- Very important for enterprise adoption
- Supports many underlying model families
- Higher complexity due to AWS auth/signing and Converse API semantics

**Expected work**
- Add `bedrock` provider section
- Decide whether presets should be grouped by Bedrock model family or upstream provider
- Implement native Converse / ConverseStream adapter
- Add environment and credential-chain documentation

**Suggested preset groups**
- `anthropic`
- `meta`
- `amazon`
- `mistral`
- `ai21`

**Official references**
- [Bedrock supported conversation models](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference-supported-models-features.html)
- [Bedrock APIs overview](https://docs.aws.amazon.com/bedrock/latest/userguide/apis.html)

---

## Task Ordering Summary

Use this as the default execution order unless product priorities change:

1. Groq preset pack
2. Together preset pack
3. Fireworks preset pack
4. OpenRouter preset pack
5. Cohere support
6. Azure OpenAI / Foundry adapter
7. xAI / Grok native adapter
8. Mistral native adapter
9. AWS Bedrock native adapter

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

That sprint should produce the biggest user-visible increase in provider coverage for the least engineering cost, while the heavier native adapters can be scheduled as follow-on milestones.
