# Feature Specification: TUI: Scaffold, Event Loop & Config

**Feature Branch**: `025-tui-scaffold-config`
**Created**: 2026-03-20
**Status**: Draft
**Input**: TUI binary entry point, terminal setup/teardown with panic safety, async event loop multiplexing terminal input and agent events, focus management (Tab cycles components), terminal resize handling, config file for appearance and behavior settings, color theme, credential resolution (environment variables then system keychain), first-run setup wizard, provider selection (prioritized fallback order). References: PRD (TUI Architecture, Event Model), HLD TUI Architecture, TUI Phases T1+T4.

## Clarifications

### Session 2026-03-22

- Q: What format should the config file use? → A: TOML
- Q: What target frame rate should the TUI use? → A: 30 FPS (~33ms per frame)
- Q: What config directory convention should be used? → A: `dirs::config_dir()/swink/` (platform-native)
- Q: What should the default quit shortcut be? → A: `Ctrl+Q` (quit); `Ctrl+C` cancels running agent operation
- Q: What minimum terminal dimensions should trigger the size warning? → A: 120x30
- Q: What should happen when the system keychain is unavailable? → A: Require environment variables only (no local storage fallback)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Launch and Exit the TUI Cleanly (Priority: P1)

A developer launches the TUI from the command line. The terminal switches to an alternate screen, raw input mode is enabled, and the initial UI renders. The developer interacts with the application. When they quit (via `Ctrl+Q` or a quit command), the terminal is restored to its original state: alternate screen is exited, raw mode is disabled, and the cursor reappears. If the application panics, the terminal is still restored — the developer's shell is never left in a broken state.

**Why this priority**: If the TUI cannot start and stop cleanly, nothing else works. A panic that corrupts the terminal makes the entire tool unusable.

**Independent Test**: Can be tested by launching the TUI, verifying the alternate screen appears, quitting, and verifying the terminal is fully restored. Separately, trigger a panic and verify terminal restoration.

**Acceptance Scenarios**:

1. **Given** a terminal, **When** the TUI launches, **Then** the alternate screen is activated and the initial UI renders.
2. **Given** a running TUI, **When** the user quits, **Then** the terminal is restored to its pre-launch state.
3. **Given** a running TUI, **When** the application panics, **Then** the terminal is restored before the panic message is printed.
4. **Given** a terminal that does not support alternate screen, **When** the TUI launches, **Then** a clear error message is shown (not a crash).

---

### User Story 2 - Respond to Keyboard Input and Agent Events Simultaneously (Priority: P1)

A developer is using the TUI while the agent is generating a response. The event loop multiplexes two event sources: terminal input (keystrokes, mouse events) and agent events (streaming tokens, tool calls, completions). The developer can type, scroll, or switch focus while the agent response streams in. Neither event source blocks the other — the UI remains responsive at all times.

**Why this priority**: The event loop is the heart of the TUI. Without multiplexing, the UI freezes during agent responses or agent events are missed during user input.

**Independent Test**: Can be tested by sending keyboard events and agent events concurrently and verifying both are processed without dropped events or UI freezes.

**Acceptance Scenarios**:

1. **Given** the event loop is running, **When** a keystroke arrives, **Then** it is processed within one render frame.
2. **Given** the event loop is running, **When** an agent event arrives, **Then** it is processed and the UI updates within one render frame.
3. **Given** simultaneous keyboard and agent events, **When** both arrive at the same time, **Then** both are processed without either blocking the other.
4. **Given** a burst of rapid agent events, **When** they arrive faster than the render rate, **Then** events are batched and the UI remains responsive.

---

### User Story 3 - Navigate Between UI Components with Keyboard (Priority: P2)

A developer wants to move focus between different areas of the TUI (e.g., input field, conversation history, tool panel) using the keyboard. Pressing Tab cycles focus forward through the components; Shift+Tab cycles backward. The currently focused component is visually highlighted. Keyboard shortcuts that are specific to a component only activate when that component has focus.

**Why this priority**: Focus management enables keyboard-driven workflows, but a single-panel TUI could function without it.

**Independent Test**: Can be tested by pressing Tab repeatedly and verifying focus cycles through all components in order, with the active component visually indicated.

**Acceptance Scenarios**:

1. **Given** the TUI has multiple components, **When** Tab is pressed, **Then** focus moves to the next component in the cycle.
2. **Given** focus is on the last component, **When** Tab is pressed, **Then** focus wraps to the first component.
3. **Given** a focused component, **When** it has focus, **Then** it is visually distinguished from unfocused components.
4. **Given** a component-specific shortcut, **When** the component does not have focus, **Then** the shortcut is not activated.

---

### User Story 4 - Configure Appearance and Behavior via Config File (Priority: P2)

A developer wants to customize the TUI's appearance (color theme, layout preferences) and behavior (key bindings, default provider). They edit a TOML configuration file stored in the platform-native config directory (`dirs::config_dir()/swink/config.toml`). On next launch, the TUI applies the custom settings. If the config file does not exist, sensible defaults are used. If the config file contains errors, the TUI launches with defaults and warns about the invalid configuration.

**Why this priority**: Configuration is important for long-term usability and personal preference, but the TUI works with defaults out of the box.

**Independent Test**: Can be tested by writing a config file with a custom color theme, launching the TUI, and verifying the custom colors are applied.

**Acceptance Scenarios**:

1. **Given** no config file exists, **When** the TUI launches, **Then** default settings are applied.
2. **Given** a valid config file with a custom color theme, **When** the TUI launches, **Then** the custom theme is applied.
3. **Given** a config file with syntax errors, **When** the TUI launches, **Then** defaults are used and a warning identifies the config error.
4. **Given** a config file with unknown keys, **When** the TUI launches, **Then** unknown keys are ignored and valid keys are applied.

---

### User Story 5 - Set Up Provider Credentials on First Run (Priority: P1)

A developer launches the TUI for the first time without any provider credentials configured. The TUI detects that no credentials are available and presents a first-run setup wizard. The wizard guides the developer through selecting a provider and entering their API key. The credential is stored in the system keychain when available; when the keychain is unavailable, the wizard instructs the user to set the appropriate environment variable instead (no local file storage fallback). On subsequent launches, the saved credential is used automatically. The provider selection follows a prioritized fallback order when multiple credentials are available.

**Why this priority**: Without credentials, the TUI cannot connect to any provider — the first-run experience determines whether the developer continues using the tool.

**Independent Test**: Can be tested by launching with no credentials, completing the wizard, and verifying the next launch connects to the configured provider without re-prompting.

**Acceptance Scenarios**:

1. **Given** no provider credentials are configured, **When** the TUI launches, **Then** the first-run setup wizard is presented.
2. **Given** the setup wizard, **When** the developer selects a provider and enters an API key, **Then** the credential is saved securely.
3. **Given** saved credentials, **When** the TUI launches subsequently, **Then** the saved credential is used without prompting.
4. **Given** multiple provider credentials are available, **When** the TUI selects a provider, **Then** it uses the highest-priority available provider from the fallback order.
5. **Given** credentials exist in both environment variables and the system keychain, **When** resolving, **Then** environment variables take precedence.

---

### User Story 6 - Handle Terminal Resize (Priority: P3)

A developer resizes their terminal window while the TUI is running. The TUI detects the resize event, recalculates the layout for the new dimensions, and re-renders all components to fit. Content is reflowed appropriately — no truncation artifacts or rendering glitches. If the terminal is resized below a minimum usable size, a warning is displayed.

**Why this priority**: Resize handling is expected behavior but is secondary to core functionality — most developers set their terminal size before launching.

**Independent Test**: Can be tested by launching the TUI, resizing the terminal, and verifying the layout adapts correctly without rendering artifacts.

**Acceptance Scenarios**:

1. **Given** a running TUI, **When** the terminal is resized, **Then** the layout recalculates and re-renders within one frame.
2. **Given** a resize to a very small terminal, **When** below the minimum usable size, **Then** a warning is displayed indicating the minimum size.
3. **Given** a resize back to a larger size, **When** the terminal grows, **Then** the full layout is restored without artifacts.

---

### Edge Cases

- What happens when the TUI is launched in a non-interactive context (e.g., piped input, no TTY)?
- How does the event loop handle a flood of resize events (e.g., continuous window dragging)?
- What happens when the system keychain is unavailable or locked? → Fall back to environment variables only; no local file storage.
- How does the TUI handle a config file that is not readable (permissions issue)?
- What happens when the provider selected in the first-run wizard becomes unavailable on a subsequent launch?
- How does the TUI behave when the terminal does not support color?
- What happens when the user interrupts the first-run wizard (`Ctrl+C`) before completing it? → Cancels the running operation; `Ctrl+Q` to quit entirely.
- How does the TUI handle conflicting key bindings in the config file?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The TUI MUST set up the terminal (alternate screen, raw mode) on launch and restore it on exit, including after panics.
- **FR-002**: The TUI MUST run an async event loop that multiplexes terminal input events and agent events without either source blocking the other.
- **FR-003**: The TUI MUST support focus cycling through UI components via Tab (forward) and Shift+Tab (backward).
- **FR-004**: The TUI MUST visually indicate which component currently has focus.
- **FR-005**: The TUI MUST load configuration from a TOML file at `dirs::config_dir()/swink/config.toml`, falling back to defaults when the file is absent or invalid.
- **FR-006**: The TUI MUST support color theme customization via the config file.
- **FR-007**: The TUI MUST resolve provider credentials by checking environment variables first, then the system keychain. When the keychain is unavailable, only environment variables are used (no local file storage fallback).
- **FR-008**: The TUI MUST present a first-run setup wizard when no provider credentials are detected.
- **FR-009**: The TUI MUST select the provider using a prioritized fallback order when multiple credentials are available.
- **FR-010**: The TUI MUST detect and respond to terminal resize events, recalculating layout and re-rendering.
- **FR-011**: The TUI MUST display a warning when the terminal size is below 120 columns by 30 rows.
- **FR-012**: The TUI MUST detect non-interactive terminals and exit with a clear error message rather than crashing.

### Key Entities

- **EventLoop**: The central multiplexer that receives terminal input events and agent events, dispatching them to the appropriate handlers without blocking.
- **FocusManager**: Tracks which UI component currently has focus and cycles focus in response to Tab/Shift+Tab.
- **Config**: The user's appearance and behavior settings, loaded from a file in the standard config directory. Includes color theme, key bindings, and default provider.
- **ColorTheme**: A named set of colors applied to UI components (foreground, background, accent, borders, highlights).
- **CredentialResolver**: Resolves provider API keys by checking environment variables first, then the system keychain, following a prioritized provider fallback order.
- **SetupWizard**: The first-run experience that guides the user through provider selection and credential entry.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The terminal is restored to its original state after exit in 100% of cases, including panics.
- **SC-002**: The event loop processes keyboard input within one render frame (33ms at 30 FPS) even while agent events are streaming.
- **SC-003**: Focus cycling via Tab visits every registered component exactly once per full cycle.
- **SC-004**: A developer with no prior configuration can go from first launch to a working agent conversation via the setup wizard.
- **SC-005**: Custom color themes from the config file are visibly applied on launch.
- **SC-006**: Terminal resize results in a correctly laid-out UI within one render frame (33ms at 30 FPS).

## Assumptions

- The TUI runs in a terminal emulator that supports alternate screen and raw input mode (standard on modern systems).
- The system keychain is available on supported platforms (macOS Keychain, Windows Credential Manager, Linux Secret Service) but is not required — when unavailable, only environment variables are supported (no local file storage).
- The config file uses TOML format, loaded from `dirs::config_dir()/swink/config.toml`.
- The prioritized provider fallback order is a fixed default that can be overridden in the config file.
- The minimum usable terminal size is 120 columns by 30 rows.
- The TUI is a separate binary that depends on the core agent library — it is not embedded in the library.
