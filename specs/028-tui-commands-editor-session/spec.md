# Feature Specification: TUI: Commands, Editor & Session

**Feature Branch**: `028-tui-commands-editor-session`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Dual command system (hash and slash commands), external editor integration, session persistence, clipboard integration. References: PRD §16.4, §16.8, HLD TUI, TUI_PHASES T4.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Execute Hash Commands for In-Session Actions (Priority: P1)

A developer types hash commands in the input editor to control the TUI session without leaving the conversation. Available hash commands: #help (list available commands), #clear (clear conversation display), #info (show session information), #copy (copy last assistant message to clipboard), #copy all (copy entire conversation), #copy code (copy only code blocks from the last response), #approve on/off/smart (change the approval mode). Commands are recognized when the input starts with `#`. Unrecognized commands produce a helpful error message listing valid commands.

**Why this priority**: Hash commands are the primary mechanism for controlling the TUI experience within a conversation. Without them, the developer cannot perform basic session management tasks.

**Independent Test**: Can be tested by typing #help and verifying a list of commands is displayed, then typing #clear and verifying the conversation display is cleared.

**Acceptance Scenarios**:

1. **Given** the input editor, **When** the developer types `#help` and submits, **Then** a list of available hash commands with descriptions is displayed.
2. **Given** a conversation with messages, **When** `#clear` is submitted, **Then** the conversation display is cleared.
3. **Given** a conversation, **When** `#info` is submitted, **Then** session information (model, provider, message count, token usage) is displayed.
4. **Given** an assistant response exists, **When** `#copy` is submitted, **Then** the last assistant message is copied to the system clipboard.
5. **Given** an assistant response with code blocks, **When** `#copy code` is submitted, **Then** only the code blocks are copied to the clipboard.
6. **Given** the input, **When** an unrecognized hash command is submitted, **Then** an error message lists the valid commands.

---

### User Story 2 - Execute Slash Commands for System Actions (Priority: P1)

A developer types slash commands to control agent behavior and application state. Available slash commands: /quit (exit the TUI), /thinking (toggle extended thinking display), /system (set the system prompt), /reset (reset the conversation), /plan (toggle plan mode), /editor (open external editor). Model switching is handled via F4 key cycling, not a slash command. Slash commands are recognized when the input starts with `/`. Unrecognized commands produce a helpful error message.

**Why this priority**: Slash commands control critical agent and application behavior. /quit is the primary exit mechanism and /model switching is essential for multi-model workflows.

**Independent Test**: Can be tested by typing /quit and verifying the TUI exits cleanly, or /model and verifying the model selection prompt appears.

**Acceptance Scenarios**:

1. **Given** a running TUI, **When** `/quit` is submitted, **Then** the TUI exits cleanly with terminal restoration.
2. **Given** a running TUI, **When** F4 is pressed, **Then** the model cycles to the next available model.
3. **Given** a conversation, **When** `/reset` is submitted, **Then** the conversation is cleared and the agent state is reset.
4. **Given** the input, **When** `/system new prompt here` is submitted, **Then** the system prompt is updated.
5. **Given** the input, **When** `/system` is submitted with no argument, **Then** a usage hint is displayed.
6. **Given** the input, **When** an unrecognized slash command is submitted, **Then** an error message lists valid commands.

---

### User Story 3 - Compose Messages in External Editor (Priority: P2)

A developer wants to compose a long or complex message using their preferred text editor rather than the TUI's built-in input. They invoke the external editor via the /editor command or a keyboard shortcut. The TUI suspends its display, the external editor opens with a temporary file, and the developer writes their message. When the editor closes, the TUI resumes and the file content is submitted as the message. If the developer saves an empty file, the action is cancelled. The editor is resolved from: a config file override, the EDITOR environment variable, the VISUAL environment variable, or a fallback to `vi`. Temporary files are cleaned up after use.

**Why this priority**: External editor support is important for complex prompts but the built-in editor handles most use cases.

**Independent Test**: Can be tested by setting EDITOR to a known editor, running /editor, writing content, closing the editor, and verifying the content is submitted.

**Acceptance Scenarios**:

1. **Given** the developer runs `/editor`, **When** the editor opens, **Then** the TUI suspends its display until the editor closes.
2. **Given** the editor is open, **When** the developer writes content and closes the editor, **Then** the content is submitted as a message.
3. **Given** the editor is open, **When** the developer saves an empty file and closes, **Then** the action is cancelled and no message is submitted.
4. **Given** no config override or EDITOR/VISUAL variables are set, **When** /editor is invoked, **Then** `vi` is used as the fallback editor.
5. **Given** the editor has closed, **When** the TUI resumes, **Then** the temporary file is deleted.
6. **Given** a config file specifies an editor override, **When** /editor is invoked, **Then** the config override takes precedence over environment variables.

---

### User Story 4 - Save and Restore Conversation Sessions (Priority: P2)

A developer wants to persist their conversation so they can resume it later. The TUI saves the conversation state (messages, metadata) through the session persistence system. When the developer restarts the TUI, they can load a previous session and continue the conversation where they left off. The session includes the full message history and relevant agent state.

**Why this priority**: Session persistence enables multi-session workflows but is not required for single-session use.

**Independent Test**: Can be tested by having a conversation, quitting the TUI, relaunching, loading the session, and verifying the conversation history is restored.

**Acceptance Scenarios**:

1. **Given** an active conversation, **When** the session is saved, **Then** the full message history and metadata are persisted.
2. **Given** a saved session exists, **When** the developer launches the TUI and loads it, **Then** the conversation history is restored.
3. **Given** a restored session, **When** the developer sends a new message, **Then** the agent continues the conversation with full history context.
4. **Given** no saved sessions exist, **When** the developer attempts to load, **Then** a message indicates no sessions are available.

---

### User Story 5 - Copy Conversation Content to Clipboard (Priority: P3)

A developer wants to share or save parts of the conversation by copying them to the system clipboard. The #copy command copies the last assistant message. The #copy all command copies the entire conversation. The #copy code command extracts only code blocks from the last assistant response. After copying, a brief confirmation is displayed. Clipboard operations work across supported platforms.

**Why this priority**: Clipboard integration is a convenience feature that improves workflow integration but is not essential for core functionality.

**Independent Test**: Can be tested by generating a response with code blocks, running #copy code, and pasting into another application to verify the code was copied.

**Acceptance Scenarios**:

1. **Given** an assistant response, **When** `#copy` is submitted, **Then** the last assistant message text is on the system clipboard and a confirmation is shown.
2. **Given** a conversation with multiple messages, **When** `#copy all` is submitted, **Then** the entire conversation text is on the clipboard.
3. **Given** a response with multiple code blocks, **When** `#copy code` is submitted, **Then** only the code block contents are on the clipboard, concatenated.
4. **Given** no assistant response exists, **When** `#copy` is submitted, **Then** an informative message indicates there is nothing to copy.

---

### Edge Cases

- What happens when the external editor crashes or is killed — TUI waits for process exit; temp file cleaned up on restart. No crash propagation.
- How does TUI handle corrupted/incompatible session file — JSONL error handling; corrupted files produce load error, TUI starts with empty history.
- What happens when clipboard unavailable — ClipboardBridge abstracts platform; unavailability shows informative error, no crash.
- How does command parser handle extra whitespace/mixed case — `trim()` and exact string matching; whitespace handled, case is exact.
- What happens when /model invoked during active response — model change is config update; applies on next turn, not mid-response.
- How does session save handle thousands of messages — streaming JSONL write line-by-line; no full buffering.
- What if editor binary doesn't exist — falls back: `$EDITOR` → `$VISUAL` → `vi`.
- How does #copy code behave with no code blocks — extracts zero blocks; shows feedback message.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The TUI MUST recognize hash commands (prefixed with `#`) and execute them: #help, #clear, #info, #copy, #copy all, #copy code, #approve on/off/smart.
- **FR-002**: The TUI MUST recognize slash commands (prefixed with `/`) and execute them: /quit, /thinking, /system, /reset, /plan, /editor. Model switching is via F4 key cycling.
- **FR-003**: Unrecognized hash or slash commands MUST produce an error message listing valid commands.
- **FR-004**: The external editor MUST be resolved in order: config override, EDITOR environment variable, VISUAL environment variable, `vi` fallback.
- **FR-005**: The TUI MUST suspend its display while the external editor is open and resume when the editor closes.
- **FR-006**: An empty file saved from the external editor MUST cancel the action (no message submitted).
- **FR-007**: Temporary files created for the external editor MUST be cleaned up after use.
- **FR-008**: The TUI MUST support saving conversation sessions (messages and metadata) to persistent storage via the session store.
- **FR-009**: The TUI MUST support loading previously saved sessions and restoring conversation history.
- **FR-010**: The #copy command MUST copy the last assistant message to the system clipboard.
- **FR-011**: The #copy all command MUST copy the entire conversation to the system clipboard.
- **FR-012**: The #copy code command MUST extract and copy only code blocks from the last assistant response.
- **FR-013**: Clipboard operations MUST display a brief confirmation message on success.
- **FR-014**: The /system command MUST accept an argument to set the system prompt. If no argument is given, it displays a usage hint.
- **FR-015**: The /reset command MUST clear the conversation and reset agent state.

### Key Entities

- **HashCommand**: A command prefixed with `#` that performs in-session actions (help, clear, info, copy, approve). Parsed from the input editor before being sent to the agent.
- **SlashCommand**: A command prefixed with `/` that controls agent behavior or application state (quit, model, thinking, system, reset, plan, editor). Parsed from the input editor before being sent to the agent.
- **CommandParser**: The component that detects, parses, and dispatches hash and slash commands from user input.
- **ExternalEditor**: The integration that suspends the TUI, launches the user's preferred editor with a temporary file, and captures the result on close.
- **SessionStore**: The persistence layer that saves and loads conversation state, using the memory crate's session storage capabilities.
- **ClipboardBridge**: The component that writes text to the system clipboard and reports success or failure.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All documented hash and slash commands execute their described action when invoked.
- **SC-002**: Unrecognized commands produce a helpful error within one render frame.
- **SC-003**: External editor round-trip (open, write, close, submit) works end-to-end without data loss.
- **SC-004**: A saved session can be loaded on a subsequent TUI launch with full conversation history intact.
- **SC-005**: Clipboard copy operations place the correct content on the system clipboard on all supported platforms.
- **SC-006**: The external editor fallback chain resolves to a working editor on a standard system.

## Clarifications

### Session 2026-03-20

- Q: Editor crash/kill behavior? → A: TUI waits for exit; temp file cleaned up on restart.
- Q: Corrupted session file? → A: JSONL error → starts with empty history.
- Q: Clipboard unavailable? → A: Shows informative error; no crash.
- Q: Extra whitespace/case in commands? → A: `trim()` + exact match.
- Q: /model during active response? → A: Config update; applies next turn.
- Q: Session save with thousands of messages? → A: Streaming JSONL, line-by-line.
- Q: Editor binary missing? → A: Fallback chain `$EDITOR` → `$VISUAL` → `vi`.
- Q: #copy code with no code blocks? → A: Shows "nothing to copy" feedback.

## Assumptions

- The TUI scaffold, event loop, conversation view, and input editor from specs 025-026 are in place.
- The memory crate's SessionStore (spec 021) provides the persistence API for session save/load.
- System clipboard access is available on the target platforms (macOS, Linux with X11/Wayland, Windows).
- The external editor is a blocking process — the TUI does not need to handle editor interaction, only launch and wait.
- Hash and slash commands are mutually exclusive with regular messages — a line starting with `#` or `/` is always interpreted as a command, never sent to the agent.
- The /plan command delegates to the plan mode system defined in a separate spec.
