# Testing Setup

Step-by-step guide to get the project running against live LLM APIs.

## 1. Install Rust

Requires **Rust 1.88+** (edition 2024).

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
rustc --version   # should print 1.88.0 or later
```

## 2. Build the Workspace

```bash
cargo build --workspace
```

This compiles all three crates: core library, adapters, and TUI. First build pulls dependencies and takes a few minutes.

## 3. Run Unit Tests

Verify everything compiles and passes before connecting to live APIs:

```bash
cargo test --workspace
```

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
cargo run -p agent-harness-tui
```

The TUI auto-loads `.env` via dotenvy — no need to source it manually. If no API keys are found (env or keychain), the first-run wizard prompts for provider selection and key entry.

## 7. Verify Each Provider

Once the TUI is running, test each configured provider:

**Check current provider:**
```
#info
```

**Switch model within the current provider:**
```
/model claude-sonnet-4-20250514
```
Note: `/model` changes the model ID on the active provider. To test a different provider, update `.env` and restart the TUI.

**Test basic conversation:**
```
What is 2 + 2?
```

**Test tool execution (built-in bash tool):**
```
List the files in the current directory
```

**Test thinking mode (Anthropic only):**
```
/thinking medium
Explain why the sky is blue
```

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
| Model switching | `/model <id>` changes model within current provider, `#info` confirms |

## 9. Logs

If something goes wrong, check the log file:

```bash
cat ~/.config/agent-harness/logs/agent-harness.log
```

Logs are daily rolling files with tracing output. Look for `ERROR` or `WARN` entries.

## 10. Cleanup

API keys stored in the OS keychain persist across sessions. To remove them:

```bash
# macOS — delete from Keychain Access app, search "agent-harness"
# Or from inside the TUI:
#keys            # see what's stored
```

To remove local config and sessions:

```bash
rm -rf ~/.config/agent-harness/
```
