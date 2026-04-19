# Getting Started

## Prerequisites

- **Rust 1.88+** (edition 2024)
- **LLVM/libclang** available for workspace-wide `cargo build/test/clippy` commands because `swink-agent-local-llm` builds `llama-cpp-sys-2` via `bindgen`
- At least one LLM provider:
  - [Ollama](https://ollama.ai) running locally (no key required)
  - An Anthropic API key
  - An OpenAI API key
  - A Google Gemini API key
  - An Azure OpenAI endpoint and key
  - An xAI API key
  - A Mistral API key
  - AWS credentials for Bedrock
  - Any OpenAI-compatible proxy endpoint
  - Local on-device inference via `swink-agent-local-llm` (no key required)

## Build

```bash
git clone <repo-url> && cd swink-agent
cargo build --workspace
```

If `cargo build --workspace` fails with `Unable to find libclang` or asks for `LIBCLANG_PATH`, install LLVM and point `LIBCLANG_PATH` at the directory containing `libclang` (`bin` on Windows LLVM installs).

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
| 4 | Local (SmolLM3) | `local` feature enabled, no remote keys set |
| 5 | Ollama | Always available (fallback) |

> **Note:** The adapters crate supports additional providers (Google Gemini, Azure OpenAI, xAI, Mistral, Bedrock) for use as a library, but the TUI binary currently only supports the providers listed above.

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
| `ANTHROPIC_MODEL` | `claude-sonnet-4-6` |

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
| `LLM_MODEL` | `claude-sonnet-4-6` |

**Ollama**

| Variable | Default |
|---|---|
| `OLLAMA_HOST` | `http://localhost:11434` |
| `OLLAMA_MODEL` | `llama3.2` |

**Local build prerequisite**

| Variable | Default | Description |
|---|---|---|
| `LIBCLANG_PATH` | auto-discovered | Directory containing `libclang` / `clang.dll` when `llama-cpp-sys-2` bindgen cannot find LLVM automatically |

## Run the TUI

```bash
cargo run -p swink-agent-tui
```

The TUI auto-loads `.env` via dotenvy — no need to source it manually.

For Ollama with no other config:

```bash
ollama serve &
ollama pull llama3.2
cargo run -p swink-agent-tui
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
| `/thinking <level>` | Set thinking depth (off/minimal/low/medium/high) |
| `/system <prompt>` | Set system prompt |
| `/reset` | Reset conversation |

**Key bindings:** `Enter` send, `Shift+Enter` newline, `Esc` abort, `Ctrl+Q` quit, `Tab` switch focus, `Up/Down` scroll or history.

## Use as a Library

```rust
use swink_agent::{Agent, AgentOptions, ModelSpec, AgentMessage, LlmMessage};
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
cargo test -p swink-agent     # core library only
```

## Further Reading

- [Architecture (HLD)](architecture/HLD.md)
- [Product Requirements](planning/PRD.md)
- [TUI Architecture](architecture/tui/README.md)
- [Implementation Phases](planning/IMPLEMENTATION_PHASES.md)
