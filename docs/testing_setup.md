# Testing Setup

Step-by-step guide to get the project running against live LLM APIs.

## 1. Install Rust

Requires **Rust 1.95+** (edition 2024).

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
rustc --version   # should print 1.95.0 or later
```

## 2. Build the Workspace

```bash
cargo build --workspace
```

This compiles all workspace crates: core library, adapters, local-llm, memory, eval, and TUI. First build pulls dependencies and takes a few minutes.

`swink-agent-local-llm` currently builds `llama-cpp-sys-2`, which runs `bindgen` during compilation. Install LLVM/libclang before running workspace-wide commands; if the build reports `Unable to find libclang`, set `LIBCLANG_PATH` to the LLVM directory that contains the shared library (`bin` on Windows).

## 3. Run the Local Validation Gate

Verify the same local validation sequence required for PRs before connecting to live APIs:

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --workspace
cargo test --workspace --features testkit
cargo test -p swink-agent --no-default-features
cargo adapters-no-default-features
cargo local-llm-no-default-features
cargo workspace-no-default-features
cargo eval-no-default-features
cargo eval-advanced-no-default-features
cargo publish --workspace --dry-run --locked --allow-dirty
```

`just validate` and `just check` run this exact command set.

These validation commands intentionally do not load `.env`; they should not
inherit provider API keys or cloud credentials. Live-provider tests load
credentials from the environment only when you run those tests explicitly.

## 4. Get API Keys

You need at least one provider key. Set up whichever providers you want to test.

### Anthropic (Claude)

1. Go to https://console.anthropic.com/
2. Sign up or log in
3. Navigate to **API Keys** in the left sidebar
4. Click **Create Key**, give it a name, copy the key (`sk-ant-...`)
5. Anthropic offers a free trial tier; paid plans at https://console.anthropic.com/settings/plans

### OpenAI (GPT-4o)

1. Go to https://platform.openai.com/
2. Sign up or log in
3. Navigate to **API Keys** (https://platform.openai.com/api-keys)
4. Click **Create new secret key**, copy the key (`sk-...`)
5. Requires a payment method; see pricing at https://openai.com/api/pricing/

### Ollama (Local, Free)

No API key needed. Install and pull a model:

```bash
# macOS
brew install ollama

# Or download from https://ollama.ai/download

# Start the server
ollama serve

# Pull a model (default: llama3.2, ~2GB)
ollama pull llama3.2
```

Ollama runs entirely on your machine — no account, no API key, no cost.

### Local On-Device (swink-agent-local-llm)

No API key needed. Models are lazily downloaded from HuggingFace on first use and cached in `~/.cache/huggingface/hub/`.

Build prerequisite: local-LLM compilation currently requires LLVM/libclang because `llama-cpp-sys-2` uses `bindgen`. On Windows, the usual fix is installing LLVM and setting `LIBCLANG_PATH` to something like `C:\Program Files\LLVM\bin` before running `cargo build/test/clippy --workspace`.

- **SmolLM3-3B** (GGUF Q4_K_M, ~1.92 GB) — text generation, tool use, reasoning
- **EmbeddingGemma-300M** (<200 MB) — text vectorization/embeddings

Context is capped at 8192 tokens by default; override with the `LOCAL_CONTEXT_LENGTH` env var. First run downloads ~2.1 GB of model weights.

```rust
use swink_agent_local_llm::default_local_connection;

let local_connection = default_local_connection()?;
```

### Google Gemini

1. Go to https://aistudio.google.com/
2. Sign up or log in
3. Navigate to **API Keys**
4. Create a key, copy it

### Azure OpenAI

1. Go to https://portal.azure.com/
2. Create an Azure OpenAI resource
3. Deploy a model and note the endpoint URL
4. Copy the API key from the resource's **Keys and Endpoint** section

### xAI (Grok)

1. Go to https://console.x.ai/
2. Sign up or log in
3. Navigate to **API Keys**, create and copy a key

### Mistral

1. Go to https://console.mistral.ai/
2. Sign up or log in
3. Navigate to **API Keys**, create and copy a key

### AWS Bedrock

1. Configure AWS credentials (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optionally `AWS_SESSION_TOKEN`)
2. Set `AWS_REGION` to a region with Bedrock access (e.g. `us-east-1`)
3. Ensure your IAM role has `bedrock:InvokeModelWithResponseStream` permission

## 5. Configure Environment

```bash
cp .env.example .env
```

Edit `.env` and uncomment/fill in the keys for the providers you set up:

```bash
# For Anthropic testing:
ANTHROPIC_API_KEY=sk-ant-your-key-here

# For OpenAI testing:
OPENAI_API_KEY=sk-your-key-here

# For Ollama (usually no changes needed):
# OLLAMA_HOST=http://localhost:11434
# OLLAMA_MODEL=llama3.2
```

Leave variables commented out to skip that provider. The TUI auto-selects by priority: Proxy > OpenAI > Anthropic > Ollama.

## 6. Launch the TUI

```bash
cargo run -p swink-agent-tui
```

The TUI auto-loads `.env` via dotenvy — no need to source it manually. If no API keys are found (env or keychain), the first-run wizard prompts for provider selection and key entry.

The development `justfile` does not load `.env` globally. `just validate`,
`just check`, and `just package-preflight` therefore run without provider
secrets unless you exported them in your shell.

## 7. Verify Each Provider

Once the TUI is running, test each configured provider:

**Check current provider:**
```
#info
```

**Cycle available models in the TUI:**
```
Press F4
```
Note: `F4` cycles the models currently available to the TUI and applies the selected model on the next prompt. Use `#info` to confirm the active model. To test a different provider, update `.env` and restart the TUI.

**Test basic conversation:**
```
What is 2 + 2?
```

**Test tool execution (built-in bash tool):**
```
List the files in the current directory
```

**Test thinking mode (Anthropic, Google Gemini, Ollama with supported models):**
```
/thinking medium
Explain why the sky is blue
```
Thinking mode availability by provider:

| Provider | Thinking support |
|---|---|
| Anthropic | Full (`/thinking` levels: minimal, low, medium, high, extra-high) |
| Google Gemini | Supported on deep-think models |
| Ollama | Supported on models that emit `<think>` tags (e.g. DeepSeek) |
| Local (SmolLM3) | Emits `<think>` tags, parsed into thinking events |
| OpenAI, Azure, xAI, Mistral, Bedrock | Not supported |

## 8. Test Scenarios Checklist

| Scenario | What to verify |
|---|---|
| Basic chat | Assistant responds, tokens/cost update in status bar |
| Multi-turn | Context is preserved across messages |
| Tool use | Agent calls bash/read_file/write_file, tool panel shows spinner then result |
| Streaming | Text appears incrementally, not all at once |
| Thinking | Dimmed thinking section appears (Anthropic with `/thinking` enabled) |
| Abort | Press `Esc` during generation — stops cleanly |
| Long output | Conversation scrolls, manual scroll with arrow keys works |
| Session save/load | `#save` then `#load <id>` restores conversation |
| Error recovery | Send a prompt that exceeds context window — agent should recover |
| Model switching | Press `F4` to cycle available models; `#info` confirms the active model |

## 9. Logs

If something goes wrong, check the log file:

```bash
cat ~/.config/swink-agent/logs/swink-agent.log
```

Logs are daily rolling files with tracing output. Look for `ERROR` or `WARN` entries.

## 10. Cleanup

API keys stored in the OS keychain persist across sessions. To remove them:

```bash
# macOS — delete from Keychain Access app, search "swink-agent"
# Or from inside the TUI:
#keys            # see what's stored
```

To remove local config and sessions:

```bash
rm -rf ~/.config/swink-agent/
```
