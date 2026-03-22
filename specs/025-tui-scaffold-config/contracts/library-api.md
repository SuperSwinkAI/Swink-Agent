# Contract: swink-agent-tui Library API

The TUI crate exposes a library layer (`lib.rs`) for embedding the TUI in custom binaries. This contract defines the public API surface.

## Terminal Lifecycle

```rust
/// Initialize terminal: raw mode + alternate screen + mouse capture.
/// Returns a ratatui Terminal ready for rendering.
pub fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>>;

/// Restore terminal: disable raw mode + leave alternate screen + disable mouse capture.
/// Safe to call multiple times (idempotent).
pub fn restore_terminal() -> io::Result<()>;
```

**Guarantees**:
- `setup_terminal()` leaves the terminal in a state where ratatui can render.
- `restore_terminal()` fully undoes `setup_terminal()` — the user's shell is usable after.
- Calling `restore_terminal()` without prior `setup_terminal()` is safe (may produce benign errors).

## System Prompt Resolution

```rust
/// Resolve system prompt from multiple sources.
/// Priority: explicit > LLM_SYSTEM_PROMPT env var > config.system_prompt > default.
pub fn resolve_system_prompt(explicit: Option<String>, config: &TuiConfig) -> String;
```

**Guarantees**:
- Always returns a non-panicking `String`.
- Default value: `"You are a helpful assistant."`.

## Approval Callback

```rust
/// Type alias for the approval channel sender.
pub type ApprovalSender = mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>;

/// Build the standard TUI approval callback.
/// Wraps approval requests through the TUI's channel for interactive prompting.
pub fn tui_approval_callback(approval_tx: &ApprovalSender) -> ApprovalCallbackFn;
```

**Guarantees**:
- If the channel closes, auto-approves (fail-open for graceful shutdown).
- Thread-safe (`Send + Sync`).

## Launch Convenience

```rust
/// High-level: create App, wire Agent, run event loop.
/// Approval callback wired automatically — do NOT set with_approve_tool on options.
pub async fn launch(
    config: TuiConfig,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    options: AgentOptions,
) -> Result<(), Box<dyn Error>>;
```

**Guarantees**:
- Returns `Ok(())` on clean quit (Ctrl+Q).
- Returns `Err` on unrecoverable I/O or agent errors.
- Does NOT call `setup_terminal()` or `restore_terminal()` — caller manages lifecycle.

## Configuration

```rust
/// TUI configuration loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TuiConfig { /* fields documented in data-model.md */ }

impl TuiConfig {
    /// Load from default config path. Falls back to defaults.
    pub fn load() -> Self;

    /// Parse from a TOML string. Falls back to defaults on error.
    pub fn from_toml(toml_str: &str) -> Self;
}

impl Default for TuiConfig { /* sensible defaults for all fields */ }
```

## Re-exports

```rust
pub use app::App;
pub use config::TuiConfig;
pub mod credentials;  // #[doc(hidden)] — internal but accessible
pub mod wizard;       // #[doc(hidden)] — internal but accessible
pub mod error;        // TuiError enum
```
