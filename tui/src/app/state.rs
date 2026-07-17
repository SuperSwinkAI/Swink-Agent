//! Shared state types for the TUI app.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use ratatui::layout::Rect;
use tokio::sync::{mpsc, oneshot};

use swink_agent::{
    Agent, AgentEvent, AgentTool, AssistantMessage, ContentBlock, DisplayRole, StopReason,
    ToolApproval, ToolApprovalRequest,
};

use crate::config::TuiConfig;
use crate::extensions::{PathCandidate, SkillCandidate};
use crate::session::JsonlSessionStore;
use crate::transport::{InProcessTransport, TuiTransport, UserInput};
use crate::ui::conversation::ConversationView;
use crate::ui::help_panel::HelpPanel;
use crate::ui::input::InputEditor;
use crate::ui::tool_panel::ToolPanel;

/// Agent state as visible to the TUI.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
    Error,
    Aborted,
}

/// Which UI component has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Conversation,
}

/// Operating mode for the TUI.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    /// Normal execution mode — all tools available.
    Execute,
    /// Plan mode — read-only tools only, agent produces plans.
    Plan,
}

/// A follow-up prompt asking whether to always approve a tool for this session.
#[non_exhaustive]
#[derive(Debug)]
pub struct TrustFollowUp {
    /// Name of the tool to potentially trust.
    pub tool_name: String,
    /// When the follow-up expires (auto-dismiss).
    pub expires_at: Instant,
}

impl TrustFollowUp {
    /// Create a follow-up prompt for `tool_name`, dismissing itself at `expires_at`.
    #[must_use]
    pub fn new(tool_name: impl Into<String>, expires_at: Instant) -> Self {
        Self {
            tool_name: tool_name.into(),
            expires_at,
        }
    }
}

/// An in-progress per-hunk review of a pending `write_file` approval.
///
/// Entered with `h` from the tool approval prompt. The user walks the hunks
/// one at a time; once every hunk has a decision the review finalizes and
/// answers the still-pending approval request.
#[non_exhaustive]
#[derive(Debug)]
pub struct HunkReview {
    /// Before/after content for the pending write.
    pub diff: crate::ui::diff::DiffData,
    /// Hunks derived from `diff`, in file order.
    pub hunks: Vec<crate::ui::diff::Hunk>,
    /// Per-hunk decision; `None` means not yet reviewed.
    pub decisions: Vec<Option<bool>>,
    /// Index of the hunk currently under review.
    pub cursor: usize,
}

impl HunkReview {
    /// Start a review of `hunks` derived from `diff`: every hunk starts
    /// undecided, cursor on the first one.
    #[must_use]
    pub fn new(diff: crate::ui::diff::DiffData, hunks: Vec<crate::ui::diff::Hunk>) -> Self {
        let decisions = vec![None; hunks.len()];
        Self {
            diff,
            hunks,
            decisions,
            cursor: 0,
        }
    }

    /// Indices (1-based, for display) of hunks the user rejected.
    pub fn rejected_hunks(&self) -> Vec<usize> {
        self.decisions
            .iter()
            .enumerate()
            .filter(|(_, decision)| !decision.unwrap_or(false))
            .map(|(index, _)| index + 1)
            .collect()
    }

    /// Decisions as a plain bool slice; undecided counts as rejected.
    pub fn approvals(&self) -> Vec<bool> {
        self.decisions
            .iter()
            .map(|decision| decision.unwrap_or(false))
            .collect()
    }
}

/// Click-drag text selection within the conversation viewport.
///
/// Coordinates are cell offsets inside the conversation's inner area (after
/// the border): `(row, col)` where `(0, 0)` is the top-left visible cell.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub cursor: (u16, u16),
    pub dragging: bool,
}

impl Selection {
    pub const fn new(row: u16, col: u16) -> Self {
        Self {
            anchor: (row, col),
            cursor: (row, col),
            dragging: true,
        }
    }

    /// Return `(start, end)` with `start <= end` in row-major order.
    pub fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        if (self.anchor.0, self.anchor.1) <= (self.cursor.0, self.cursor.1) {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// True when the selection covers at least one cell.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }
}

/// Message role for display styling.
///
/// Type alias for [`DisplayRole`] from the core crate. Kept as an alias
/// to avoid a mass-rename across the TUI codebase.
pub type MessageRole = DisplayRole;

/// A message formatted for display.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
    pub thinking: Option<String>,
    pub is_streaming: bool,
    /// Whether this tool result block is collapsed.
    pub collapsed: bool,
    /// One-line summary for collapsed display.
    pub summary: String,
    /// Whether the user manually expanded this block (prevents auto-collapse).
    pub user_expanded: bool,
    /// When the tool result was expanded (for auto-collapse timing).
    pub expanded_at: Option<Instant>,
    /// Whether this message was produced in plan mode.
    pub plan_mode: bool,
    /// Diff data for file modification tool results.
    pub diff_data: Option<crate::ui::diff::DiffData>,
}
impl DisplayMessage {
    /// Create a simple display message with default field values.
    pub const fn new(role: MessageRole, content: String) -> Self {
        Self {
            role,
            content,
            thinking: None,
            is_streaming: false,
            collapsed: false,
            summary: String::new(),
            user_expanded: false,
            expanded_at: None,
            plan_mode: false,
            diff_data: None,
        }
    }

    /// Attach thinking content shown dimmed above the message.
    #[must_use]
    pub fn with_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.thinking = Some(thinking.into());
        self
    }

    /// Mark the message as still streaming in.
    #[must_use]
    pub fn with_is_streaming(mut self, is_streaming: bool) -> Self {
        self.is_streaming = is_streaming;
        self
    }

    /// Collapse the message, showing `summary` in its place until expanded.
    #[must_use]
    pub fn with_collapsed(mut self, summary: impl Into<String>) -> Self {
        self.collapsed = true;
        self.summary = summary.into();
        self
    }

    /// Mark the message as manually expanded by the user (prevents auto-collapse).
    #[must_use]
    pub fn with_user_expanded(mut self, user_expanded: bool) -> Self {
        self.user_expanded = user_expanded;
        self
    }

    /// Record when the message was expanded, for auto-collapse timing.
    #[must_use]
    pub fn with_expanded_at(mut self, expanded_at: Instant) -> Self {
        self.expanded_at = Some(expanded_at);
        self
    }

    /// Mark the message as produced in plan mode.
    #[must_use]
    pub fn with_plan_mode(mut self, plan_mode: bool) -> Self {
        self.plan_mode = plan_mode;
        self
    }

    /// Attach diff data for a file modification tool result.
    #[must_use]
    pub fn with_diff_data(mut self, diff_data: crate::ui::diff::DiffData) -> Self {
        self.diff_data = Some(diff_data);
        self
    }

    /// Extract the assistant text/thinking payload shown by the TUI.
    pub fn assistant_content(message: &AssistantMessage) -> (String, Option<String>) {
        let mut text_parts = Vec::new();
        let mut thinking_parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => text_parts.push(text.as_str()),
                ContentBlock::Thinking { thinking, .. } => {
                    thinking_parts.push(thinking.as_str());
                }
                _ => {}
            }
        }

        let content = if !text_parts.is_empty() {
            text_parts.join("")
        } else if message.stop_reason == StopReason::Error {
            message.error_message.clone().unwrap_or_default()
        } else {
            String::new()
        };

        let thinking = (!thinking_parts.is_empty()).then(|| thinking_parts.join(""));
        (content, thinking)
    }
}

/// Token usage and cost recorded for a single assistant response.
///
/// Costs are whatever the agent loop reported: it prices each assistant message
/// from operator-declared rates or the compiled model catalog before the TUI
/// sees it (see [`swink_agent::price_assistant_message_with`]). The TUI never
/// prices anything itself, so `cost` is `0.0` for a model with neither.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsage {
    /// Model that produced the response.
    pub model_id: String,
    /// Prompt tokens billed for this response.
    pub input_tokens: u64,
    /// Completion tokens billed for this response.
    pub output_tokens: u64,
    /// Tokens read from a provider-side prompt cache.
    pub cache_read_tokens: u64,
    /// Tokens written to a provider-side prompt cache.
    pub cache_write_tokens: u64,
    /// Total cost of this response in USD, as priced by the agent loop.
    pub cost: f64,
}

impl TurnUsage {
    /// Record one turn's token usage and (already-priced) cost.
    #[must_use]
    pub fn new(
        model_id: impl Into<String>,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        cost: f64,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost,
        }
    }
}

/// Open `@path` completion popup.
///
/// Present only while a host-supplied provider has returned candidates for the
/// mention under the cursor; `None` whenever the popup is closed.
#[derive(Debug, Clone)]
pub struct PathCompletion {
    /// Candidates as the host returned them, in the host's order.
    pub candidates: Vec<PathCandidate>,
    /// Index of the highlighted candidate. Always in range.
    pub selected: usize,
    /// Byte offset of the `@` in the cursor's line, for splicing on accept.
    pub(crate) start: usize,
}

impl PathCompletion {
    /// The highlighted candidate.
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&PathCandidate> {
        self.candidates.get(self.selected)
    }

    /// Highlight the next candidate, wrapping to the first.
    pub fn select_next(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1) % self.candidates.len();
        }
    }

    /// Highlight the previous candidate, wrapping to the last.
    pub fn select_prev(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.candidates.len() - 1);
        }
    }
}

/// Open `/skill` completion popup.
///
/// Present only while a host-supplied provider has returned candidates for the
/// leading `/name` under the cursor; `None` whenever the popup is closed.
///
/// Tier-2 documentation (the SKILL.md body) is fetched lazily for the
/// highlighted candidate and cached here per name, so moving the highlight
/// back and forth never re-invokes the host callback.
#[derive(Debug, Clone)]
pub struct SkillCompletion {
    /// Candidates as the host returned them, in the host's order.
    pub candidates: Vec<SkillCandidate>,
    /// Index of the highlighted candidate. Always in range.
    pub selected: usize,
    /// Byte offset of the `/` in the cursor's line, for splicing on accept.
    pub(crate) start: usize,
    /// Cached tier-2 details per skill name. `Some(None)` records "fetched,
    /// nothing to show" so absent details are not re-fetched either. Carried
    /// across refreshes while the popup stays open.
    pub(crate) details: HashMap<String, Option<String>>,
}

impl SkillCompletion {
    /// The highlighted candidate.
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&SkillCandidate> {
        self.candidates.get(self.selected)
    }

    /// Cached tier-2 documentation for the highlighted candidate, if the host
    /// supplied any.
    #[must_use]
    pub fn selected_details(&self) -> Option<&str> {
        let candidate = self.selected_candidate()?;
        self.details.get(&candidate.name)?.as_deref()
    }

    /// Highlight the next candidate, wrapping to the first.
    pub fn select_next(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1) % self.candidates.len();
        }
    }

    /// Highlight the previous candidate, wrapping to the last.
    pub fn select_prev(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.candidates.len() - 1);
        }
    }
}

/// View and render state: conversation display, widgets, layout, and redraw bookkeeping.
#[non_exhaustive]
pub struct ViewState {
    /// Conversation messages for display.
    pub messages: Vec<DisplayMessage>,
    /// Conversation scroll state.
    pub conversation: ConversationView,
    /// Tool execution panel.
    pub tool_panel: ToolPanel,
    /// Help side panel (F1).
    pub help_panel: HelpPanel,
    /// Which component has focus.
    pub focus: Focus,
    /// Dirty flag — only redraw when true.
    pub dirty: bool,
    /// Blink state for streaming cursor (toggled on tick).
    pub blink_on: bool,
    /// Tick counter for blink timing.
    pub(crate) tick_count: u64,
    /// Index of the currently selected tool result block (for collapse toggling).
    pub selected_tool_block: Option<usize>,
    /// Conversation viewport area from the most recent render pass.
    pub(crate) conversation_area: Rect,
    /// Visible line height inside the conversation viewport.
    pub(crate) conversation_visible_height: usize,
    /// Ticks remaining for the fade-out animation after steered messages are
    /// consumed. Zero means the overlay is not visible.
    pub(crate) steered_fade_ticks: u8,
    /// Active click-drag selection in the conversation viewport.
    pub(crate) selection: Option<Selection>,
}

impl ViewState {
    /// Fresh view state: empty conversation, input focused, first draw pending.
    pub(crate) fn new() -> Self {
        Self {
            messages: Vec::new(),
            conversation: ConversationView::new(),
            tool_panel: ToolPanel::new(),
            help_panel: HelpPanel::new(),
            focus: Focus::Input,
            dirty: true,
            blink_on: true,
            tick_count: 0,
            selected_tool_block: None,
            conversation_area: Rect::new(0, 0, 0, 0),
            conversation_visible_height: 0,
            steered_fade_ticks: 0,
            selection: None,
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self::new()
    }
}

/// Input and editor state: the multi-line editor plus its completion popups.
#[non_exhaustive]
pub struct EditorState {
    /// Multi-line input editor.
    pub input: InputEditor,
    /// Open `@path` completion popup, or `None` when closed.
    ///
    /// Only ever populated when a host registered a completion provider via
    /// [`TuiExtensions::with_path_completions`](crate::TuiExtensions::with_path_completions).
    pub path_completion: Option<PathCompletion>,
    /// Open `/skill` completion popup, or `None` when closed.
    ///
    /// Only ever populated when a host registered a completion provider via
    /// [`TuiExtensions::with_skill_completions`](crate::TuiExtensions::with_skill_completions).
    /// At most one of `path_completion` and `skill_completion` is open at a
    /// time — their trigger queries are mutually exclusive at the cursor.
    pub skill_completion: Option<SkillCompletion>,
    /// Flag set when external editor should be opened (processed by event loop).
    pub open_editor_requested: bool,
}

impl EditorState {
    /// Fresh editor state: empty input, no popups open.
    pub(crate) fn new() -> Self {
        Self {
            input: InputEditor::new(),
            path_completion: None,
            skill_completion: None,
            open_editor_requested: false,
        }
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent I/O state: the agent handle, in-flight turn status, event channels,
/// pending tool approvals, and mid-turn steering.
#[non_exhaustive]
pub struct AgentIo {
    /// Current agent status.
    pub status: AgentStatus,
    /// Retry attempt counter for error display.
    pub retry_attempt: Option<u32>,
    /// Agent instance (if connected).
    pub(crate) agent: Option<Agent>,
    /// Sender for agent events. The in-process bridge (`agent_bridge.rs`)
    /// forwards the agent's stream into this channel; `transport` owns the
    /// receive side.
    pub(crate) agent_tx: mpsc::Sender<AgentEvent>,
    /// Transport the event loop consumes [`AgentEvent`]s from and — when a
    /// host installed one via [`App::with_transport`](App::with_transport) —
    /// sends user input through.
    ///
    /// Defaults to an [`InProcessTransport`] wrapping the receive side of
    /// `agent_tx`, so the in-process bridge keeps working unchanged.
    pub(crate) transport: Box<dyn TuiTransport>,
    /// True once a host replaced the default in-process wiring via
    /// [`App::with_transport`](App::with_transport). Routes `send_to_agent`
    /// through `transport` instead of driving the in-process [`Agent`].
    pub(crate) external_transport: bool,
    /// User input queued for [`TuiTransport::send`]; flushed by the event
    /// loop. Only ever populated when `external_transport` is set.
    pub(crate) outbound: Vec<UserInput>,
    /// Receiver for tool approval requests from the agent callback.
    pub(crate) approval_rx: mpsc::Receiver<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Sender for tool approval requests (cloned into the approval callback).
    pub(crate) approval_tx: mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Currently pending approval request and its response channel.
    pub(crate) pending_approval: Option<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Active per-hunk review for `pending_approval`, when the user opened one.
    pub hunk_review: Option<HunkReview>,
    /// Set of tool names trusted for the current session (auto-approved in Smart mode).
    pub session_trusted_tools: HashSet<String>,
    /// Active trust follow-up prompt (shown after approving a tool in Smart mode).
    pub trust_follow_up: Option<TrustFollowUp>,
    /// Messages steered into the agent while it was already running.
    /// Held here until `AgentEnd`, then promoted into `messages`.
    pub(crate) pending_steered: Vec<String>,
}

impl AgentIo {
    /// Fresh agent I/O state: no agent connected, new approval channel, and an
    /// [`InProcessTransport`] wired to a new event channel.
    pub(crate) fn new() -> Self {
        let (agent_tx, event_rx) = mpsc::channel(256);
        let (approval_tx, approval_rx) = mpsc::channel(16);
        // The in-process drive path never calls `TuiTransport::send` — the
        // bridge in `agent_bridge.rs` starts turns on the `Agent` directly —
        // so the input side is dropped here: a stray `send` fails fast with
        // `ChannelClosed` instead of queueing input nothing will ever read.
        let (input_tx, _unused_input_rx) = mpsc::channel(1);
        let transport: Box<dyn TuiTransport> =
            Box::new(InProcessTransport::from_channels(input_tx, event_rx));
        Self {
            status: AgentStatus::Idle,
            retry_attempt: None,
            agent: None,
            agent_tx,
            transport,
            external_transport: false,
            outbound: Vec::new(),
            approval_rx,
            approval_tx,
            pending_approval: None,
            hunk_review: None,
            session_trusted_tools: HashSet::new(),
            trust_follow_up: None,
            pending_steered: Vec::new(),
        }
    }
}

impl Default for AgentIo {
    fn default() -> Self {
        Self::new()
    }
}

/// Operating-mode and model-selection state applied to upcoming turns.
#[non_exhaustive]
pub struct ModeState {
    /// Current operating mode.
    pub operating_mode: OperatingMode,
    /// Whether a plan approval prompt is pending.
    pub pending_plan_approval: bool,
    /// First message index belonging to the active plan-mode session.
    pub(crate) plan_session_start: Option<usize>,
    /// Saved full tool set for restoring on plan→execute transition.
    pub(crate) saved_tools: Option<Vec<Arc<dyn AgentTool>>>,
    /// Original system prompt (before plan mode addendum).
    pub(crate) saved_system_prompt: Option<String>,
    /// Model identifier string.
    pub model_name: String,
    /// Available models for F4 cycling.
    pub(crate) available_models: Vec<swink_agent::ModelSpec>,
    /// Current index into `available_models`.
    pub(crate) model_index: usize,
    /// Model selected via F4 but not yet applied (applied on next send).
    pub(crate) pending_model: Option<swink_agent::ModelSpec>,
}

impl ModeState {
    /// Fresh mode state: Execute mode, showing `model_name` until the agent
    /// reports the real one.
    pub(crate) fn new(model_name: String) -> Self {
        Self {
            operating_mode: OperatingMode::Execute,
            pending_plan_approval: false,
            plan_session_start: None,
            saved_tools: None,
            saved_system_prompt: None,
            model_name,
            available_models: Vec::new(),
            model_index: 0,
            pending_model: None,
        }
    }
}

/// Session identity and persistence state.
#[non_exhaustive]
pub struct SessionPersistence {
    /// Session manager for persistence.
    pub(crate) session_store: Option<JsonlSessionStore>,
    /// Current session ID.
    pub(crate) session_id: String,
    /// Current persisted session metadata. `None` until the first successful save
    /// or until a session is loaded. Owns `created_at` and the optimistic-concurrency
    /// `sequence` counter so subsequent saves don't race against the store.
    pub(crate) session_meta: Option<crate::session::SessionMeta>,
    /// Session start time for elapsed display.
    pub session_start: Instant,
}

impl SessionPersistence {
    /// Fresh session state for `session_id`, starting the elapsed clock now.
    pub(crate) fn new(session_store: Option<JsonlSessionStore>, session_id: String) -> Self {
        Self {
            session_store,
            session_id,
            session_meta: None,
            session_start: Instant::now(),
        }
    }
}

/// Token, cost, and context-window accounting for the session.
#[derive(Default)]
#[non_exhaustive]
pub struct UsageTotals {
    /// Total prompt tokens billed across the session.
    pub total_input_tokens: u64,
    /// Total completion tokens billed across the session.
    pub total_output_tokens: u64,
    /// Running cost.
    pub total_cost: f64,
    /// Per-turn usage records, oldest first — the backing data for `/usage`.
    ///
    /// One entry is appended per assistant response, so this grows with the
    /// session and is cleared by `/reset`. The status bar shows the totals;
    /// this is the breakdown behind them.
    pub turn_usage: Vec<TurnUsage>,
    /// Estimated context window token budget.
    pub context_budget: u64,
    /// Estimated tokens currently used in context.
    pub context_tokens_used: u64,
}

/// Top-level application state.
///
/// State is grouped into cohesive sub-structs — view/render ([`ViewState`]),
/// input/editor ([`EditorState`]), agent I/O ([`AgentIo`]), operating mode and
/// model selection ([`ModeState`]), session persistence
/// ([`SessionPersistence`]), and usage accounting ([`UsageTotals`]) — with
/// cross-cutting flags and configuration kept directly on `App`.
pub struct App {
    /// Whether the application should exit.
    pub should_quit: bool,
    /// View and render state: conversation display, widgets, layout.
    pub view: ViewState,
    /// Input editor and completion popups.
    pub editor: EditorState,
    /// Agent I/O: agent handle, event channels, approvals, steering.
    pub agent_io: AgentIo,
    /// Operating mode and model selection for upcoming turns.
    pub mode: ModeState,
    /// Session identity and persistence.
    pub session: SessionPersistence,
    /// Token and cost accounting.
    pub usage: UsageTotals,
    /// Configuration.
    pub config: TuiConfig,
    /// Host-supplied extension points (custom commands, `@path` mention seams).
    pub(crate) extensions: crate::extensions::TuiExtensions,
}
