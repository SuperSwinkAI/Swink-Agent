# Data Model: TUI: Scaffold, Event Loop & Config

## Entities

### TuiConfig

TOML-backed user configuration. Loaded from `dirs::config_dir()/swink-agent/tui.toml`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `show_thinking` | `bool` | `true` | Display LLM thinking/reasoning content |
| `auto_scroll` | `bool` | `true` | Auto-scroll conversation on new messages |
| `tick_rate_ms` | `u64` | `33` | Event loop tick interval in milliseconds (30 FPS) |
| `default_model` | `String` | `"not connected"` | Default model identifier |
| `theme` | `String` | `"default"` | Theme preset name |
| `system_prompt` | `Option<String>` | `None` | System prompt override (below env var priority) |
| `editor_command` | `Option<String>` | `None` | External editor override (over `$EDITOR`/`$VISUAL`) |
| `color_mode` | `String` | `"custom"` | Color mode: `custom`, `mono-white`, `mono-black` |

**Behavior**: `#[serde(default)]` — missing fields use defaults. Unknown keys silently ignored. Invalid TOML falls back to full defaults.

**Validation**: None at parse time — invalid `color_mode` strings fall back to `Custom`. Invalid `tick_rate_ms` of 0 uses serde default.

---

### ColorMode

Process-global display mode stored in `AtomicU8`.

| Variant | Value | Behavior |
|---------|-------|----------|
| `Custom` | `0` | Original theme colors (distinct per role) |
| `MonoWhite` | `1` | All colors resolve to `Color::White` |
| `MonoBlack` | `2` | All colors resolve to `Color::Black` |

**Transitions**: F3 key cycles `Custom → MonoWhite → MonoBlack → Custom`. Config `color_mode` field sets initial mode on startup.

---

### Focus

Tracks which UI component currently has keyboard focus.

| Variant | Description |
|---------|-------------|
| `Input` | Text input editor (default) |
| `Conversation` | Scrollable conversation history |

**Transitions**: Tab cycles forward (`Input → Conversation → Input`). Shift+Tab cycles backward. Typing any character while in Conversation focus returns to Input.

**Extensibility**: Features 026–029 may add variants (ToolPanel, etc.) — the cycle order expands accordingly.

---

### AgentStatus

Current state of the agent runtime.

| Variant | Description |
|---------|-------------|
| `Idle` | No agent operation in progress |
| `Running` | Agent is processing (streaming response or executing tools) |
| `Error` | Last operation failed |
| `Aborted` | User cancelled via Ctrl+C or Esc |

---

### OperatingMode

High-level operating mode of the TUI.

| Variant | Description |
|---------|-------------|
| `Execute` | Normal mode — agent executes tools freely |
| `Plan` | Plan mode — agent proposes actions, user approves |

---

### DisplayMessage

A single message rendered in the conversation view.

| Field | Type | Description |
|-------|------|-------------|
| `role` | `MessageRole` | Who sent the message |
| `content` | `String` | Message text content |
| `thinking` | `Option<String>` | LLM reasoning/thinking content (dimmed) |
| `is_streaming` | `bool` | Whether content is still being received |
| `collapsed` | `bool` | Whether tool result is collapsed |
| `summary` | `String` | Collapsed summary text |
| `user_expanded` | `bool` | Whether user manually expanded |
| `expanded_at` | `Option<Instant>` | When auto-expanded (for auto-collapse timer) |
| `plan_mode` | `bool` | Whether message was generated in plan mode |
| `diff_data` | `Option<DiffData>` | Parsed diff content for inline rendering |

---

### MessageRole

| Variant | Display Color | Description |
|---------|---------------|-------------|
| `User` | Green | User-submitted messages |
| `Assistant` | Cyan | LLM responses |
| `ToolResult` | Yellow | Tool execution results |
| `Error` | Red | Error messages |
| `System` | Magenta | System/informational messages |

---

### ProviderInfo

Static provider configuration for credential resolution.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `&'static str` | Human-readable provider name |
| `key_name` | `&'static str` | Keychain entry identifier |
| `env_var` | `&'static str` | Environment variable name for API key |
| `description` | `&'static str` | Short provider description |
| `requires_key` | `bool` | Whether the provider needs an API key |

**Instances**: Ollama (no key), OpenAI, Anthropic, Custom Proxy, Local (no key).

---

### App

Top-level application state. Not serialized — lives only in memory during a TUI session.

| Field | Type | Description |
|-------|------|-------------|
| `config` | `TuiConfig` | Loaded configuration |
| `focus` | `Focus` | Currently focused component |
| `status` | `AgentStatus` | Agent runtime state |
| `operating_mode` | `OperatingMode` | Execute or Plan |
| `messages` | `Vec<DisplayMessage>` | Conversation history |
| `should_quit` | `bool` | Exit flag |
| `dirty` | `bool` | Redraw flag |
| `agent` | `Option<Agent>` | The wired agent instance |
| `agent_tx` / `agent_rx` | `mpsc::channel` | Agent event channel |
| `approval_tx` / `approval_rx` | `mpsc::channel` | Approval request channel |
| `pending_approval` | `Option<(Request, Responder)>` | Currently pending approval |
| `session_trusted_tools` | `HashSet<String>` | Tools approved for this session |

---

### TuiError

Error type for the TUI crate.

| Variant | Source | Description |
|---------|--------|-------------|
| `Io` | `std::io::Error` | Terminal I/O errors |
| `Agent` | `String` | Agent runtime errors |
| `Other` | `Box<dyn Error>` | Catch-all |

## Relationships

```
App
├── has TuiConfig
├── has Focus
├── has AgentStatus
├── has OperatingMode
├── has Vec<DisplayMessage>
│   └── each has MessageRole
├── has Agent (optional)
└── uses ProviderInfo[] for credential resolution

TuiConfig
└── references ColorMode (via color_mode string field)

ColorMode
└── global AtomicU8 (independent of App lifecycle)
```

## State Transitions

### App Lifecycle
```
main() → TTY check → setup_terminal() → wizard? → App::new() → app.run() → restore_terminal()
                                                                    ↓
                                                          tokio::select! loop
                                                          ├── terminal event → handle
                                                          ├── agent event → handle
                                                          ├── approval → handle
                                                          └── tick → periodic update
```

### Agent Status
```
Idle → (user sends message) → Running → (stream completes) → Idle
                                      → (error) → Error → (user sends) → Running
                                      → (Ctrl+C / Esc) → Aborted → (user sends) → Running
```
