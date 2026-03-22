# Research: TUI: Scaffold, Event Loop & Config

## R1: Terminal UI Framework Selection

**Decision**: ratatui 0.30 + crossterm 0.29
**Rationale**: ratatui is the maintained successor to tui-rs with active development, strong community, and comprehensive widget library. crossterm is the default backend — cross-platform (macOS/Linux/Windows), supports alternate screen, raw mode, mouse capture, and async event streaming via the `event-stream` feature. The combination is the de facto standard for Rust TUI applications.
**Alternatives considered**:
- termion: Linux/macOS only, no Windows support. Ruled out.
- cursive: Higher-level abstraction with its own widget system. Too opinionated — harder to integrate with a custom event loop.
- Raw crossterm without ratatui: Possible but requires manual layout, widget rendering, and styling. Significant boilerplate.

## R2: Async Event Loop Design

**Decision**: `tokio::select!` multiplexing four channels (terminal events, agent events, approval requests, tick interval)
**Rationale**: The TUI must remain responsive while the agent streams tokens. `tokio::select!` allows non-blocking dispatch of whichever event arrives first. crossterm's `EventStream` (via `event-stream` feature) integrates directly with tokio's async model. A tick interval provides a heartbeat for periodic UI updates (cursor blink, auto-collapse timers).
**Alternatives considered**:
- Separate thread for terminal input: Adds complexity, requires `Arc<Mutex<>>` for shared state. `EventStream` avoids this.
- `poll()`-based loop (crossterm `poll` + `read`): Synchronous, would block on terminal input during agent streaming. Ruled out.
- `async-std` runtime: Project uses tokio exclusively. No reason to introduce a second runtime.

## R3: Configuration Format

**Decision**: TOML via `toml` 0.8 crate, loaded from `dirs::config_dir()/swink-agent/tui.toml`
**Rationale**: TOML is human-readable, widely used in Rust ecosystem (`Cargo.toml`), and supported by the `toml` crate already in workspace deps. `dirs` provides platform-native config directories (`~/.config/` on Linux, `~/Library/Application Support/` on macOS, `%APPDATA%` on Windows). The `swink-agent/` subdirectory namespaces the config for the project; `tui.toml` distinguishes from potential future config files (e.g., `core.toml`).
**Alternatives considered**:
- YAML: More complex syntax, heavier parser. No advantage for simple key-value config.
- JSON: No comments, harder to hand-edit. Poor ergonomics for user-facing config.
- `$HOME/.swinkrc`: Non-standard, doesn't follow platform conventions.
- `dirs::config_dir()/swink/config.toml`: Simpler path but `swink` is ambiguous — `swink-agent` is the crate name and avoids collision with other hypothetical `swink-*` tools.

## R4: Credential Storage

**Decision**: `keyring` 3 crate for OS keychain, environment variable override
**Rationale**: `keyring` provides a unified API across macOS Keychain Services, Windows Credential Manager, and Linux secret-service (D-Bus). Environment variables take precedence — this allows CI/CD and container deployments where keychains are unavailable. When the keychain is unavailable at runtime, the system degrades gracefully to env-var-only mode with no local file fallback (per spec: no plaintext credential files).
**Alternatives considered**:
- Plaintext file in config dir: Security risk, explicitly ruled out by spec.
- `keyutils` (Linux-only): Not cross-platform.
- Encrypted file with user-provided passphrase: Complex UX, no advantage over OS keychain.

## R5: Color Theme Architecture

**Decision**: Three-mode system (Custom, MonoWhite, MonoBlack) with `resolve()` indirection
**Rationale**: All color access routes through `resolve(color)` which applies the active `ColorMode`. This means adding new UI components never requires color-mode awareness — they just use semantic color functions (`user_color()`, `assistant_color()`, etc.) and the mode is applied transparently. Monochrome modes are essential for accessibility (high-contrast terminals, screen readers, e-ink displays). Using `AtomicU8` for the global mode avoids passing mode through every render call while remaining thread-safe.
**Alternatives considered**:
- Per-widget theme parameter: Verbose, error-prone (easy to forget). Global resolve is simpler.
- Full theme struct (configurable RGB values): Over-engineered for current needs. The three-mode system covers the practical use cases. Can be extended later.

## R6: Focus Management

**Decision**: `Focus` enum with Tab/Shift+Tab cycling, visual border distinction
**Rationale**: A simple enum is sufficient for the initial component set (Input, Conversation). Tab cycling is the universal keyboard convention. Focused components get a bright border (`Color::White`), unfocused get dim (`Color::DarkGray`). Component-specific shortcuts are gated on focus state, preventing accidental activation. The enum is extensible — later features (026, 027) add more focus targets by extending the enum and the cycle order.
**Alternatives considered**:
- Tree-based focus (nested focus groups): Over-engineered for a flat component layout.
- Mouse-click focus: Added as a complement but not primary — the spec emphasizes keyboard-driven navigation.

## R7: Panic Safety

**Decision**: Custom panic hook that calls `restore_terminal()` before the original hook
**Rationale**: If the TUI panics, the terminal is left in raw mode with the alternate screen active — the user's shell becomes unusable. By intercepting the panic hook, we restore the terminal before printing the panic message. `std::panic::take_hook()` preserves the original hook (which prints the backtrace), and we chain our restoration before it. This pattern is standard in ratatui applications.
**Alternatives considered**:
- `Drop` impl on `App`: Not reliable — `Drop` is not guaranteed to run on panic. The panic hook is the only reliable mechanism.
- `catch_unwind` wrapper: Works but doesn't catch all panics (e.g., double panic). The hook approach is more comprehensive.

## R8: Non-Interactive Terminal Detection

**Decision**: Check `std::io::stdout().is_terminal()` at program start
**Rationale**: `IsTerminal` is stable since Rust 1.70 (well within MSRV 1.88). If stdout is not a terminal (piped, redirected, `script` command), the TUI cannot function — attempting to enter alternate screen or enable raw mode would produce garbage output or errors. Detecting early and printing a clear error to stderr provides a good user experience.
**Alternatives considered**:
- Let crossterm fail: Produces cryptic errors. Poor UX.
- `atty` crate: External dependency for something now in std. Unnecessary.

## R9: Minimum Terminal Size

**Decision**: Render a centered warning overlay when terminal dimensions are below 120×30
**Rationale**: Below 120×30, the layout cannot fit all components legibly. Rather than crashing or producing garbled output, show a clear message with current and required dimensions. The check runs in the render path so it responds immediately to resize events. Normal rendering resumes as soon as the terminal is large enough — no restart needed.
**Alternatives considered**:
- Refuse to launch: Too aggressive — the user may resize after launch.
- Degrade layout gracefully: Possible for moderate sizes but at 80×24 (common default) the UI would be unusable. A clear warning is more honest.

## R10: Provider Fallback Order

**Decision**: Proxy → OpenAI → Anthropic → Local LLM → Ollama
**Rationale**: Custom proxy is highest priority because it indicates intentional routing (enterprise, development). Commercial APIs (OpenAI, Anthropic) next by popularity. Local LLM (feature-gated) for offline use. Ollama as the zero-config fallback — runs locally with no API key. This order can be overridden by setting the relevant environment variables.
**Alternatives considered**:
- User-configurable order in config file: Future enhancement. Current fixed order covers all use cases since the user controls which credentials are available.
- Single-provider mode (no fallback): Too rigid — users may have multiple providers configured for different use cases.
