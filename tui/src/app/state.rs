//! Shared state types for the TUI app.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use ratatui::layout::Rect;
use tokio::sync::{mpsc, oneshot};

use swink_agent::{
    Agent, AgentEvent, AgentTool, AssistantMessage, ContentBlock, DisplayRole, StopReason,
    ToolApproval, ToolApprovalRequest,
};

use crate::config::TuiConfig;
use crate::session::JsonlSessionStore;
use crate::ui::conversation::ConversationView;
use crate::ui::help_panel::HelpPanel;
use crate::ui::input::InputEditor;
use crate::ui::tool_panel::ToolPanel;

/// Agent state as visible to the TUI.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    /// Normal execution mode — all tools available.
    Execute,
    /// Plan mode — read-only tools only, agent produces plans.
    Plan,
}

/// A follow-up prompt asking whether to always approve a tool for this session.
#[derive(Debug)]
pub struct TrustFollowUp {
    /// Name of the tool to potentially trust.
    pub tool_name: String,
    /// When the follow-up expires (auto-dismiss).
    pub expires_at: Instant,
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

/// Top-level application state.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Current agent status.
    pub status: AgentStatus,
    /// Multi-line input editor.
    pub input: InputEditor,
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
    /// Model identifier string.
    pub model_name: String,
    /// Token usage counters.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Running cost.
    pub total_cost: f64,
    /// Session start time for elapsed display.
    pub session_start: Instant,
    /// Dirty flag — only redraw when true.
    pub dirty: bool,
    /// Blink state for streaming cursor (toggled on tick).
    pub blink_on: bool,
    /// Tick counter for blink timing.
    pub(crate) tick_count: u64,
    /// Agent instance (if connected).
    pub(crate) agent: Option<Agent>,
    /// Sender for agent events.
    pub(crate) agent_tx: mpsc::Sender<AgentEvent>,
    /// Receiver for agent events.
    pub(crate) agent_rx: mpsc::Receiver<AgentEvent>,
    /// Configuration.
    pub config: TuiConfig,
    /// Retry attempt counter for error display.
    pub retry_attempt: Option<u32>,
    /// Session manager for persistence.
    pub(crate) session_store: Option<JsonlSessionStore>,
    /// Current session ID.
    pub(crate) session_id: String,
    /// Current persisted session metadata. `None` until the first successful save
    /// or until a session is loaded. Owns `created_at` and the optimistic-concurrency
    /// `sequence` counter so subsequent saves don't race against the store.
    pub(crate) session_meta: Option<crate::session::SessionMeta>,
    /// Receiver for tool approval requests from the agent callback.
    pub(crate) approval_rx: mpsc::Receiver<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Sender for tool approval requests (cloned into the approval callback).
    pub(crate) approval_tx: mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Currently pending approval request and its response channel.
    pub(crate) pending_approval: Option<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Estimated context window token budget.
    pub context_budget: u64,
    /// Estimated tokens currently used in context.
    pub context_tokens_used: u64,
    /// Index of the currently selected tool result block (for collapse toggling).
    pub selected_tool_block: Option<usize>,
    /// Flag set when external editor should be opened (processed by event loop).
    pub open_editor_requested: bool,
    /// Set of tool names trusted for the current session (auto-approved in Smart mode).
    pub session_trusted_tools: HashSet<String>,
    /// Active trust follow-up prompt (shown after approving a tool in Smart mode).
    pub trust_follow_up: Option<TrustFollowUp>,
    /// Current operating mode.
    pub operating_mode: OperatingMode,
    /// Whether a plan approval prompt is pending.
    pub pending_plan_approval: bool,
    /// First message index belonging to the active plan-mode session.
    pub(crate) plan_session_start: Option<usize>,
    /// Available models for F4 cycling.
    pub(crate) available_models: Vec<swink_agent::ModelSpec>,
    /// Current index into `available_models`.
    pub(crate) model_index: usize,
    /// Model selected via F4 but not yet applied (applied on next send).
    pub(crate) pending_model: Option<swink_agent::ModelSpec>,
    /// Saved full tool set for restoring on plan→execute transition.
    pub(crate) saved_tools: Option<Vec<Arc<dyn AgentTool>>>,
    /// Original system prompt (before plan mode addendum).
    pub(crate) saved_system_prompt: Option<String>,
    /// Conversation viewport area from the most recent render pass.
    pub(crate) conversation_area: Rect,
    /// Visible line height inside the conversation viewport.
    pub(crate) conversation_visible_height: usize,
    /// Messages steered into the agent while it was already running.
    /// Held here until `AgentEnd`, then promoted into `messages`.
    pub(crate) pending_steered: Vec<String>,
    /// Ticks remaining for the fade-out animation after steered messages are
    /// consumed. Zero means the overlay is not visible.
    pub(crate) steered_fade_ticks: u8,
    /// Active click-drag selection in the conversation viewport.
    pub(crate) selection: Option<Selection>,
}
