# Getting Started

## Prerequisites

- **Rust 1.88+** (edition 2024)
- At least one LLM provider:
  - [Ollama](https://ollama.ai) running locally (no key required)
  - An Anthropic API key
  - An OpenAI API key
  - Any OpenAI-compatible proxy endpoint

## Build

```bash
git clone <repo-url> && cd agent-harness
cargo build --workspace
```

## Configure a Provider

Copy the example environment file and fill in the keys for the providers you want to use:

```bash
cp .env.example .env
```

The TUI picks the first available provider in this order:

| Priority | Provider | Trigger |
|:---:|---|---|
| 1 | Custom SSE Proxy | `LLM_BASE_URL` is set |
| 2 | OpenAI | `OPENAI_API_KEY` is set |
| 3 | Anthropic | `ANTHROPIC_API_KEY` is set |
| 4 | Ollama | Always available (local) |

Keys can also be stored in the OS keychain instead of env vars — the first-run wizard will prompt you, or use `#key <provider> <value>` inside the TUI.

### Environment Variables

**Shared**

| Variable | Default | Description |
|---|---|---|
| `LLM_SYSTEM_PROMPT` | `You are a helpful assistant.` | System prompt for all providers |

**Anthropic**

| Variable | Default |
|---|---|
| `ANTHROPIC_API_KEY` | — |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-20250514` |

**OpenAI**

| Variable | Default |
|---|---|
| `OPENAI_API_KEY` | — |
| `OPENAI_BASE_URL` | `https://api.openai.com` |
| `OPENAI_MODEL` | `gpt-4o` |

**Custom SSE Proxy**

| Variable | Default |
|---|---|
| `LLM_BASE_URL` | — |
| `LLM_API_KEY` | — |
| `LLM_MODEL` | `claude-sonnet-4-20250514` |

**Ollama**

| Variable | Default |
|---|---|
| `OLLAMA_HOST` | `http://localhost:11434` |
| `OLLAMA_MODEL` | `llama3.2` |

## Run the TUI

```bash
cargo run -p agent-harness-tui
```

The TUI auto-loads `.env` via dotenvy — no need to source it manually.

For Ollama with no other config:

```bash
ollama serve &
ollama pull llama3.2
cargo run -p agent-harness-tui
```

On first launch with no keys configured, the setup wizard walks you through provider selection and key entry.

## TUI Commands

| Command | Action |
|---|---|
| `#help` | Show all commands and key bindings |
| `#clear` | Clear conversation |
| `#info` | Show session info (model, tokens, cost) |
| `#copy` | Copy last assistant message |
| `#copy code` | Copy last code block |
| `#save` / `#load <id>` | Session persistence |
| `#keys` | Show configured providers |
| `#key <provider> <key>` | Store an API key in the OS keychain |
| `/model <id>` | Change model |
| `/thinking <level>` | Set thinking depth (off/minimal/low/medium/high) |
| `/system <prompt>` | Set system prompt |
| `/reset` | Reset conversation |

**Key bindings:** `Enter` send, `Shift+Enter` newline, `Esc` abort, `Ctrl+Q` quit, `Tab` switch focus, `Up/Down` scroll or history.

## Use as a Library

```rust
use agent_harness::{Agent, AgentOptions, ModelSpec, AgentMessage, LlmMessage};
use std::sync::Arc;

let agent = Agent::new(AgentOptions::new(
    "You are a helpful assistant.".into(),
    ModelSpec::new("ollama", "llama3.2"),
    Arc::new(my_stream_fn),
    |msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    },
));

// Simple text prompt
let result = agent.prompt_text("Hello").await?;

// Streaming with event subscription
let id = agent.subscribe(|event| { /* handle AgentEvent */ });
let result = agent.prompt_async(messages).await?;
```

## Tests

```bash
cargo test --workspace          # all tests
cargo test -p agent-harness     # core library only
```

## Further Reading

- [Architecture (HLD)](architecture/HLD.md)
- [Product Requirements](planning/PRD.md)
- [TUI Architecture](architecture/tui/README.md)
- [Implementation Phases](planning/IMPLEMENTATION_PHASES.md)
