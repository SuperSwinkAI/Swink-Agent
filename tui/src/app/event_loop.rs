//! Event loop and input handling.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};

use swink_agent::{ApprovalMode, SessionState, ToolApproval};

use super::state::{ControlFollowUp, PathCompletion, Selection, SkillCompletion, TrustFollowUp};
use crate::commands::{self, ApprovalModeArg, ClipboardContent, CommandResult};
use crate::theme;
use crate::transport::{ControlRequest, ControlResponse, TransportError};
use crate::ui;

use super::render_helpers::extract_code_blocks;
use super::state::{AgentStatus, App, DisplayMessage, Focus, MessageRole};
use super::{AppResult, MOUSE_SCROLL_STEP};

impl App {
    /// Main async event loop using `tokio::select!`.
    pub async fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> AppResult<()> {
        let tick_rate = std::time::Duration::from_millis(self.config.tick_rate_ms);
        let mut tick_interval = tokio::time::interval(tick_rate);
        let mut event_stream = crossterm::event::EventStream::new();

        if self.view.messages.is_empty() {
            self.push_system_message("Press F1 for help.".to_string());
        }

        loop {
            if self.view.dirty {
                terminal.draw(|frame| ui::render(frame, self))?;
                self.view.dirty = false;
            }

            if self.should_quit {
                break;
            }

            // Forward any control requests, then any input, queued for a
            // host-installed transport. Both are no-ops on the default
            // in-process path, which drives the agent directly. Controls
            // flush first so e.g. a pending model change lands before the
            // prompt it should apply to.
            self.flush_controls().await;
            self.flush_outbound().await;
            self.flush_compact().await;

            tokio::select! {
                maybe_event = event_stream.next() => {
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_terminal_event(&event);
                    }
                }
                Some(event) = self.agent_io.transport.recv() => {
                    self.handle_agent_event(event);
                    // Drain any additional pending agent events before the next
                    // draw so rapid token bursts are batched into a single frame.
                    while let Some(event) = self.agent_io.transport.try_recv() {
                        self.handle_agent_event(event);
                    }
                }
                Some((request, responder)) = self.agent_io.approval_rx.recv() => {
                    self.handle_approval_request(request, responder);
                }
                _ = tick_interval.tick() => {
                    self.tick();
                }
            }

            if self.editor.open_editor_requested {
                self.editor.open_editor_requested = false;
                let editor = crate::editor::resolve_editor(self.config.editor_command.as_deref());

                let _ = crate::restore_terminal();
                let result = crate::editor::open_editor(&editor);

                let _ = crossterm::terminal::enable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::EnterAlternateScreen,
                    crossterm::event::EnableMouseCapture
                );
                terminal.clear()?;
                self.view.dirty = true;
                event_stream = crossterm::event::EventStream::new();

                match result {
                    Ok(Some(content)) => {
                        self.submit_user_text(content);
                    }
                    Ok(None) => {
                        self.push_system_message(
                            "Editor closed with empty content — cancelled.".to_string(),
                        );
                    }
                    Err(error) => {
                        self.push_system_message(format!("Editor error: {error}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Flush user input queued by `send_to_agent` to the installed transport.
    ///
    /// Only ever has work when a host routed agent I/O through
    /// [`App::with_transport`](App::with_transport); the in-process path
    /// starts turns on the [`Agent`](swink_agent::Agent) directly and never
    /// queues. A failed send surfaces like an in-process start failure: an
    /// error message in the conversation and [`AgentStatus::Error`].
    pub(super) async fn flush_outbound(&mut self) {
        if self.agent_io.outbound.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.agent_io.outbound);
        for input in queued {
            let result = self.agent_io.transport.send(input).await;
            if let Err(error) = result {
                self.agent_io.status = AgentStatus::Error;
                self.view.messages.push(DisplayMessage::new(
                    MessageRole::Error,
                    format!("Failed to send to agent: {error}"),
                ));
                self.view.dirty = true;
            }
        }
    }

    /// Flush control requests queued by sync handlers to the installed
    /// transport, applying each response as it arrives.
    ///
    /// Only ever has work when a host routed agent I/O through
    /// [`App::with_transport`](App::with_transport). Runs before
    /// [`flush_outbound`](App::flush_outbound) on the event loop's flush
    /// pass so control effects (abort, model change, ...) land ahead of any
    /// queued prompt. An [`Unsupported`](TransportError::Unsupported) reply
    /// surfaces as a status message — except for auto-save, which keeps its
    /// old silent skip — and any other error surfaces verbatim.
    pub(super) async fn flush_controls(&mut self) {
        if self.agent_io.pending_controls.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.agent_io.pending_controls);
        for control in queued {
            let result = self.agent_io.transport.control(control.request).await;
            self.apply_control_outcome(control.label, control.follow_up, result);
        }
    }

    /// Run a queued `/compact` against the in-process agent.
    ///
    /// Only ever has work on the in-process path — external transports route
    /// `/compact` through [`ControlRequest::Compact`] instead. Runs on the
    /// event loop's flush pass because
    /// [`compact_context`](swink_agent::Agent::compact_context) is async
    /// while command dispatch is sync.
    pub(super) async fn flush_compact(&mut self) {
        if !self.agent_io.pending_compact {
            return;
        }
        self.agent_io.pending_compact = false;
        let Some(agent) = &mut self.agent_io.agent else {
            return;
        };
        let feedback = match agent.compact_context().await {
            Ok(report) => Self::describe_compaction(report.as_ref()),
            Err(error) => format!("Compaction failed: {error}"),
        };
        self.push_system_message(feedback);
        self.view.dirty = true;
    }

    /// Human-readable summary of a manual compaction outcome.
    fn describe_compaction(report: Option<&swink_agent::CompactionReport>) -> String {
        report.map_or_else(
            || "Nothing to compact — history is already under budget.".to_string(),
            |report| {
                format!(
                    "Compacted context: dropped {} message(s), ~{} → ~{} tokens.",
                    report.dropped_count, report.tokens_before, report.tokens_after
                )
            },
        )
    }

    /// Apply one control response (or error) per its queued follow-up.
    fn apply_control_outcome(
        &mut self,
        label: &'static str,
        follow_up: ControlFollowUp,
        result: Result<ControlResponse, TransportError>,
    ) {
        match result {
            Err(TransportError::Unsupported) => {
                // Auto-save over a turn-I/O-only transport keeps the old
                // behavior: skip silently rather than nag on every turn end.
                if !matches!(follow_up, ControlFollowUp::SaveSnapshot { announce: false }) {
                    self.push_system_message(format!("{label}: not supported by remote backend"));
                }
            }
            Err(error) => {
                self.push_system_message(format!("{label} failed: {error}"));
            }
            Ok(response) => self.apply_control_response(label, follow_up, response),
        }
        self.view.dirty = true;
    }

    /// Apply a successful control response per its queued follow-up.
    fn apply_control_response(
        &mut self,
        label: &'static str,
        follow_up: ControlFollowUp,
        response: ControlResponse,
    ) {
        match (follow_up, response) {
            (ControlFollowUp::Discard, _) => {}
            (
                ControlFollowUp::PopulateModelsAndCycle,
                ControlResponse::Models { available, current },
            ) => {
                self.mode.model_index = available
                    .iter()
                    .position(|model| model.model_id == current.model_id)
                    .unwrap_or(0);
                self.mode.available_models = available;
                self.mode.model_name.clone_from(&current.model_id);
                // Perform the cycle the F4 press asked for, now that the
                // list is known; the chosen model is applied via `SetModel`
                // on the next send, as usual. Guarded so a backend that
                // reports no models cannot re-trigger the `ListModels` fetch.
                if !self.mode.available_models.is_empty() {
                    self.cycle_model();
                }
            }
            (ControlFollowUp::ShowApprovalMode, ControlResponse::ApprovalMode(mode)) => {
                let report = self.approval_mode_report(mode);
                self.push_system_message(report);
            }
            (ControlFollowUp::RenderCompaction, ControlResponse::Compacted { report }) => {
                let feedback = Self::describe_compaction(report.as_ref());
                self.push_system_message(feedback);
            }
            (
                ControlFollowUp::SaveSnapshot { announce },
                ControlResponse::SessionSnapshot { messages, state },
            ) => {
                let state = state.unwrap_or_else(|| SessionState::new().snapshot());
                match self.persist_remote_snapshot(&messages, &state) {
                    Ok(()) => {
                        if announce {
                            self.push_system_message(format!(
                                "Session saved: {}",
                                self.session.session_id
                            ));
                        }
                    }
                    Err(error) => {
                        if announce {
                            self.push_system_message(format!("Failed to save session: {error}"));
                        }
                    }
                }
            }
            // A backend answering with the wrong response shape is a bug on
            // its side; surface it rather than guessing — except for silent
            // auto-save, which must never nag on turn end (same policy as
            // `Unsupported` above).
            (follow_up, _) => {
                if matches!(follow_up, ControlFollowUp::SaveSnapshot { announce: false }) {
                    tracing::warn!("{label}: unexpected response from remote backend");
                } else {
                    self.push_system_message(format!(
                        "{label}: unexpected response from remote backend"
                    ));
                }
            }
        }
    }

    /// Apply agent events from the installed transport until its stream is
    /// exhausted ([`TuiTransport::recv`](crate::transport::TuiTransport::recv)
    /// returns `None`).
    ///
    /// [`App::run`] does this continuously inside its terminal event loop;
    /// this method is the terminal-free equivalent, letting a host or test
    /// drive an `App` through a
    /// [`TuiTransport`](crate::transport::TuiTransport) — e.g. a mock
    /// yielding scripted events — and assert on the resulting state. Note the
    /// default in-process transport's stream only ends when the app is torn
    /// down, so this is primarily useful with a transport installed via
    /// [`App::with_transport`](App::with_transport). Control requests queued
    /// by the handled events (e.g. auto-save's session snapshot at
    /// `AgentEnd`) are flushed between events, mirroring [`App::run`]'s
    /// flush pass.
    pub async fn pump_transport_events(&mut self) {
        loop {
            self.flush_controls().await;
            let Some(event) = self.agent_io.transport.recv().await else {
                break;
            };
            self.handle_agent_event(event);
        }
    }

    pub(super) fn handle_terminal_event(&mut self, event: &Event) {
        match event {
            Event::Key(key) => self.handle_key_event(*key),
            Event::Mouse(mouse) => self.handle_mouse_event(*mouse),
            Event::Resize(_, _) => {
                self.view.dirty = true;
            }
            _ => {}
        }
    }

    pub(super) fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if !self.mouse_in_conversation(mouse.column, mouse.row) {
                    return;
                }
                self.view.selection = None;
                self.view.conversation.scroll_up(MOUSE_SCROLL_STEP);
                self.view.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                if !self.mouse_in_conversation(mouse.column, mouse.row) {
                    return;
                }
                self.view.selection = None;
                self.view
                    .conversation
                    .scroll_down(MOUSE_SCROLL_STEP, self.conversation_page_height());
                self.view.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if !self.mouse_in_conversation(mouse.column, mouse.row) {
                    self.view.selection = None;
                    self.view.dirty = true;
                    return;
                }
                if let Some((row, col)) = self.inner_conversation_coords(mouse.column, mouse.row) {
                    self.view.selection = Some(Selection::new(row, col));
                    self.view.dirty = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.view.selection.is_none() {
                    return;
                }
                let (row, col) = self.clamped_conversation_coords(mouse.column, mouse.row);
                if let Some(sel) = self.view.selection.as_mut() {
                    sel.cursor = (row, col);
                    sel.dragging = true;
                }
                self.view.dirty = true;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(sel) = self.view.selection.as_mut() {
                    sel.dragging = false;
                }
                self.copy_selection_to_clipboard();
            }
            _ => {}
        }
    }

    const fn mouse_in_conversation(&self, column: u16, row: u16) -> bool {
        let area = self.view.conversation_area;
        let within_x = column >= area.x && column < area.x.saturating_add(area.width);
        let within_y = row >= area.y && row < area.y.saturating_add(area.height);
        within_x && within_y
    }

    /// Translate absolute terminal coordinates into `(row, col)` inside the
    /// conversation's inner area (i.e. excluding the border). Returns `None`
    /// if the position falls outside the inner area.
    fn inner_conversation_coords(&self, column: u16, row: u16) -> Option<(u16, u16)> {
        let area = self.view.conversation_area;
        let inner_x = area.x.checked_add(1)?;
        let inner_y = area.y.checked_add(1)?;
        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        if column < inner_x || row < inner_y {
            return None;
        }
        let col = column - inner_x;
        let r = row - inner_y;
        if col >= inner_w || r >= inner_h {
            return None;
        }
        Some((r, col))
    }

    /// Like [`Self::inner_conversation_coords`] but clamps to the viewport
    /// edges instead of returning `None` — used during drag so the selection
    /// still extends when the cursor leaves the conversation area.
    fn clamped_conversation_coords(&self, column: u16, row: u16) -> (u16, u16) {
        let area = self.view.conversation_area;
        let inner_x = area.x.saturating_add(1);
        let inner_y = area.y.saturating_add(1);
        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        let max_col = inner_w.saturating_sub(1);
        let max_row = inner_h.saturating_sub(1);
        let col = column.saturating_sub(inner_x).min(max_col);
        let r = row.saturating_sub(inner_y).min(max_row);
        (r, col)
    }

    /// Copy the current selection (if any) to the system clipboard.
    pub(super) fn copy_selection_to_clipboard(&mut self) {
        let Some(sel) = self.view.selection else {
            return;
        };
        let Some(text) = self.view.conversation.selection_text(&sel) else {
            return;
        };
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
            Ok(()) => {}
            Err(error) => {
                self.push_system_message(format!("Clipboard error: {error}"));
            }
        }
        self.view.dirty = true;
    }

    /// Feed one key event through the same path [`App::run`] uses.
    ///
    /// Public so a host can drive the input flow from outside the crate —
    /// including asserting on `@path` completion in its own tests — without
    /// forking. Mirrors
    /// [`handle_agent_event`](App::handle_agent_event) on the agent side.
    ///
    /// # Example
    /// ```rust
    /// use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use swink_agent_tui::{App, TuiConfig};
    ///
    /// let mut app = App::new(TuiConfig::default());
    /// app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    /// assert_eq!(app.editor.input.lines(), ["h"]);
    /// ```
    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.should_quit = true;
                self.view.dirty = true;
                return;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.view.selection.is_some() {
                    self.copy_selection_to_clipboard();
                    self.view.selection = None;
                } else if self.agent_io.status == AgentStatus::Running {
                    self.abort_agent();
                } else {
                    self.should_quit = true;
                }
                self.view.dirty = true;
                return;
            }
            (_, KeyCode::Esc) if self.view.selection.is_some() => {
                self.view.selection = None;
                self.view.dirty = true;
                return;
            }
            _ => {}
        }

        // Handle modal prompts (trust follow-up, plan approval, tool approval).
        // Returns true if the key was consumed.
        if self.handle_modal_key(key) {
            return;
        }

        if self.view.focus == Focus::Conversation {
            let page = self.conversation_page_height();
            match key.code {
                KeyCode::Up => self.view.conversation.scroll_up(1),
                KeyCode::Down => self.view.conversation.scroll_down(1, page),
                KeyCode::PageUp => self.view.conversation.scroll_up(page),
                KeyCode::PageDown => self.view.conversation.scroll_down(page, page),
                KeyCode::F(1) => self.view.help_panel.toggle(),
                _ => self.view.focus = Focus::Input,
            }
            self.view.dirty = true;
            if self.view.focus == Focus::Conversation {
                return;
            }
        }

        self.handle_input_key(key);
        self.view.dirty = true;
    }

    /// Handle modal prompts: trust follow-up, plan approval, tool approval.
    /// Returns `true` if the key was consumed by a modal.
    fn handle_modal_key(&mut self, key: KeyEvent) -> bool {
        // Priority 1: Trust follow-up prompt
        if self.agent_io.trust_follow_up.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    if let Some(follow_up) = self.agent_io.trust_follow_up.take() {
                        self.agent_io
                            .session_trusted_tools
                            .insert(follow_up.tool_name);
                    }
                    self.view.dirty = true;
                    return true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.agent_io.trust_follow_up = None;
                    self.view.dirty = true;
                    return true;
                }
                _ => {
                    // Clear follow-up on any other key, then re-process
                    self.agent_io.trust_follow_up = None;
                    self.view.dirty = true;
                    // Fall through to process the key normally
                }
            }
        }

        // Priority 2: Plan approval prompt
        if self.mode.pending_plan_approval {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    self.approve_plan();
                    self.view.dirty = true;
                    return true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.reject_plan();
                    self.view.dirty = true;
                    return true;
                }
                _ => {
                    return true;
                }
            }
        }

        // Priority 3: Per-hunk diff review (implies a pending approval)
        if self.agent_io.hunk_review.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') => self.decide_current_hunk(true),
                KeyCode::Char('n' | 'N') => self.decide_current_hunk(false),
                KeyCode::Char('a' | 'A') => self.approve_remaining_hunks(),
                KeyCode::Esc => self.cancel_hunk_review(),
                _ => {}
            }
            self.view.dirty = true;
            return true;
        }

        // Priority 4: Tool approval prompt
        if self.agent_io.pending_approval.is_some() {
            match key.code {
                KeyCode::Char('h' | 'H') => {
                    // Falls through to the plain prompt when there is no
                    // reviewable diff on this request.
                    self.start_hunk_review();
                    self.view.dirty = true;
                    return true;
                }
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    if let Some((req, responder)) = self.agent_io.pending_approval.take() {
                        let _ = responder.send(ToolApproval::Approved);
                        // In Smart mode, offer trust follow-up
                        if self.approval_mode() == ApprovalMode::Smart {
                            self.agent_io.trust_follow_up = Some(TrustFollowUp::new(
                                req.tool_name,
                                Instant::now() + Duration::from_secs(3),
                            ));
                        }
                    }
                    self.view.dirty = true;
                    return true;
                }
                KeyCode::Char('a' | 'A') => {
                    if let Some((req, responder)) = self.agent_io.pending_approval.take() {
                        self.agent_io.session_trusted_tools.insert(req.tool_name);
                        let _ = responder.send(ToolApproval::Approved);
                    }
                    self.view.dirty = true;
                    return true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    if let Some((_req, responder)) = self.agent_io.pending_approval.take() {
                        let _ = responder.send(ToolApproval::Rejected);
                    }
                    self.view.dirty = true;
                    return true;
                }
                _ => {
                    return true;
                }
            }
        }

        false
    }

    /// Recompute the `@path` completion popup against the cursor.
    ///
    /// Called after anything that moves the cursor or changes the text. Returns
    /// immediately when no host provider is registered, so hosts that never
    /// opted into `@path` completion pay nothing per keystroke. Note this only
    /// ever asks the host for *candidates* — file content is resolved at submit
    /// time (see [`App::send_to_agent`]), never here.
    pub(super) fn refresh_path_completion(&mut self) {
        if !self.extensions.has_path_completions() {
            return;
        }

        let previous = self.editor.path_completion.take();
        let Some(query) = self.editor.input.mention_query() else {
            self.view.dirty |= previous.is_some();
            return;
        };

        let candidates = self.extensions.complete_path(&query.query);
        if candidates.is_empty() {
            self.view.dirty |= previous.is_some();
            return;
        }

        // Keep the highlight where it was if the same candidate is still on
        // offer, so refining a query does not silently re-point the selection.
        let selected = previous
            .as_ref()
            .and_then(PathCompletion::selected_candidate)
            .and_then(|prior| candidates.iter().position(|next| next == prior))
            .unwrap_or(0);

        self.editor.path_completion = Some(PathCompletion {
            candidates,
            selected,
            start: query.start,
        });
        self.view.dirty = true;
    }

    /// Recompute whichever completion popup the cursor position calls for.
    ///
    /// The two trigger queries are mutually exclusive at the cursor — a
    /// leading `/name` is never an `@` mention and vice versa — so at most one
    /// popup is open after a refresh; each refresh clears its own popup when
    /// its query is absent.
    pub(super) fn refresh_completions(&mut self) {
        self.refresh_path_completion();
        self.refresh_skill_completion();
    }

    /// Recompute the `/skill` completion popup against the cursor.
    ///
    /// Mirrors [`App::refresh_path_completion`], with one addition: tier-2
    /// details are fetched for the highlighted candidate, through a per-name
    /// cache carried across refreshes so a keystroke or highlight move never
    /// re-invokes the host callback for a name it already answered.
    pub(super) fn refresh_skill_completion(&mut self) {
        if !self.extensions.has_skill_completions() {
            return;
        }

        let previous = self.editor.skill_completion.take();
        let Some(query) = self.editor.input.slash_query() else {
            self.view.dirty |= previous.is_some();
            return;
        };

        let candidates = self.extensions.complete_skills(&query.query);
        if candidates.is_empty() {
            self.view.dirty |= previous.is_some();
            return;
        }

        // Keep the highlight where it was if the same candidate is still on
        // offer, and keep the details cache — both survive query refinement.
        let selected = previous
            .as_ref()
            .and_then(SkillCompletion::selected_candidate)
            .and_then(|prior| candidates.iter().position(|next| next == prior))
            .unwrap_or(0);
        let details = previous.map(|prior| prior.details).unwrap_or_default();

        self.editor.skill_completion = Some(SkillCompletion {
            candidates,
            selected,
            start: query.start,
            details,
        });
        self.cache_selected_skill_details();
        self.view.dirty = true;
    }

    /// Fetch tier-2 details for the highlighted skill, unless already cached.
    ///
    /// This is the *only* place the details callback runs, so "once per
    /// highlighted name per popup" holds by construction.
    fn cache_selected_skill_details(&mut self) {
        let Some(name) = self
            .editor
            .skill_completion
            .as_ref()
            .and_then(SkillCompletion::selected_candidate)
            .map(|candidate| candidate.name.clone())
        else {
            return;
        };
        if self
            .editor
            .skill_completion
            .as_ref()
            .is_some_and(|completion| completion.details.contains_key(&name))
        {
            return;
        }

        let details = self.extensions.skill_details(&name);
        if let Some(completion) = self.editor.skill_completion.as_mut() {
            completion.details.insert(name, details);
        }
    }

    /// Accept the highlighted skill into the input, closing the popup.
    pub(super) fn accept_skill_completion(&mut self) {
        let Some(completion) = self.editor.skill_completion.take() else {
            return;
        };
        if let Some(candidate) = completion.selected_candidate() {
            // The trailing space terminates the token, which closes the popup
            // on the next refresh without a special case.
            self.editor
                .input
                .replace_mention_query(completion.start, &format!("/{} ", candidate.name));
        }
        self.view.dirty = true;
    }

    /// Keys the `/skill` popup consumes while open. Returns whether it took
    /// the key. Deliberately a near-duplicate of
    /// [`App::handle_path_completion_key`] — genericizing would drag a public
    /// struct into a shared abstraction for ~30 lines of code.
    fn handle_skill_completion_key(&mut self, key: KeyEvent) -> bool {
        let Some(completion) = &mut self.editor.skill_completion else {
            return false;
        };

        match (key.modifiers, key.code) {
            (_, KeyCode::Up) => {
                completion.select_prev();
                self.cache_selected_skill_details();
            }
            (_, KeyCode::Down) => {
                completion.select_next();
                self.cache_selected_skill_details();
            }
            (_, KeyCode::Tab | KeyCode::Enter) => {
                self.accept_skill_completion();
                return true;
            }
            (_, KeyCode::Esc) => self.editor.skill_completion = None,
            _ => return false,
        }

        self.view.dirty = true;
        true
    }

    /// Accept the highlighted candidate into the input, closing the popup.
    pub(super) fn accept_path_completion(&mut self) {
        let Some(completion) = self.editor.path_completion.take() else {
            return;
        };
        if let Some(candidate) = completion.selected_candidate() {
            // The trailing space terminates the mention, which closes the popup
            // on the next refresh without a special case.
            self.editor
                .input
                .replace_mention_query(completion.start, &format!("@{} ", candidate.path));
        }
        self.view.dirty = true;
    }

    /// Keys the `@path` popup consumes while open. Returns whether it took the key.
    ///
    /// Up/Down navigate rather than recalling history, Tab/Enter accept rather
    /// than switching focus or submitting, and Esc dismisses rather than
    /// aborting the agent — each only while the popup is actually open.
    fn handle_path_completion_key(&mut self, key: KeyEvent) -> bool {
        let Some(completion) = &mut self.editor.path_completion else {
            return false;
        };

        match (key.modifiers, key.code) {
            (_, KeyCode::Up) => completion.select_prev(),
            (_, KeyCode::Down) => completion.select_next(),
            (_, KeyCode::Tab | KeyCode::Enter) => {
                self.accept_path_completion();
                return true;
            }
            (_, KeyCode::Esc) => self.editor.path_completion = None,
            _ => return false,
        }

        self.view.dirty = true;
        true
    }

    /// Keys that edit the text or move the cursor. Returns whether the key was one.
    ///
    /// Grouped together because they share an epilogue: every one of them can
    /// move the cursor into or out of an `@path` mention or a leading `/skill`
    /// token, so each is followed by a single completion refresh.
    fn handle_editing_key(&mut self, key: KeyEvent) -> bool {
        match (key.modifiers, key.code) {
            (KeyModifiers::SHIFT, KeyCode::Enter) => self.editor.input.insert_newline(),
            (_, KeyCode::Home) | (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.editor.input.move_home();
            }
            (_, KeyCode::End) | (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.editor.input.move_end();
            }
            (KeyModifiers::NONE, KeyCode::Up) => {
                if self.editor.input.cursor_row() == 0 {
                    self.editor.input.history_prev();
                } else {
                    self.editor.input.move_up();
                }
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if self.editor.input.cursor_row() + 1 >= self.editor.input.line_count() {
                    self.editor.input.history_next();
                } else {
                    self.editor.input.move_down();
                }
            }
            (KeyModifiers::NONE, KeyCode::Left) => self.editor.input.move_left(),
            (KeyModifiers::NONE, KeyCode::Right) => self.editor.input.move_right(),
            (_, KeyCode::Backspace) => self.editor.input.backspace(),
            (_, KeyCode::Delete) => self.editor.input.delete(),
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(character)) => {
                self.editor.input.insert_char(character);
            }
            _ => return false,
        }

        self.refresh_completions();
        true
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        // Precedence: an open completion popup claims its keys first (at most
        // one is ever open), then the editing keys, then everything else.
        if self.handle_path_completion_key(key) {
            return;
        }
        if self.handle_skill_completion_key(key) {
            return;
        }
        if matches!(key.code, KeyCode::Esc) && self.agent_io.status == AgentStatus::Running {
            self.abort_agent();
            return;
        }
        if self.handle_editing_key(key) {
            return;
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                self.toggle_operating_mode();
            }
            (_, KeyCode::Tab) => {
                self.view.focus = match self.view.focus {
                    Focus::Input => Focus::Conversation,
                    Focus::Conversation => Focus::Input,
                };
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_input();
            }
            (_, KeyCode::PageUp) => {
                let page = self.conversation_page_height();
                self.view.conversation.scroll_up(page);
            }
            (_, KeyCode::PageDown) => {
                let page = self.conversation_page_height();
                self.view.conversation.scroll_down(page, page);
            }
            (_, KeyCode::F(1)) => {
                self.view.help_panel.toggle();
            }
            (_, KeyCode::F(2)) => {
                let target = self.view.selected_tool_block.or_else(|| {
                    self.view
                        .messages
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, message)| message.role == MessageRole::ToolResult)
                        .map(|(index, _)| index)
                });
                if let Some(index) = target {
                    self.toggle_collapse(index);
                    self.view.selected_tool_block = Some(index);
                }
            }
            (_, KeyCode::F(3)) => {
                theme::cycle_color_mode();
            }
            (_, KeyCode::F(4)) => {
                self.cycle_model();
            }
            (_, KeyCode::F(5)) => {
                self.view.conversation.show_hidden_channels =
                    !self.view.conversation.show_hidden_channels;
            }
            (KeyModifiers::SHIFT, KeyCode::Left) => {
                self.select_prev_tool_block();
            }
            (KeyModifiers::SHIFT, KeyCode::Right) => {
                self.select_next_tool_block();
            }
            _ => {}
        }
    }

    pub(super) fn submit_user_text(&mut self, text: String) {
        if self.agent_io.status != AgentStatus::Running {
            self.view
                .messages
                .push(DisplayMessage::new(MessageRole::User, text.clone()));
            self.trim_messages_to_recent_turns();
        }
        self.view.conversation.auto_scroll = true;
        self.send_to_agent(text);
    }

    fn command_mutates_session_during_stream(command: &CommandResult) -> bool {
        matches!(
            command,
            CommandResult::Clear
                | CommandResult::SetSystemPrompt(_)
                | CommandResult::Reset
                | CommandResult::Compact
                | CommandResult::SaveSession
                | CommandResult::LoadSession(_)
        )
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn submit_input(&mut self) {
        // Whatever is in the editor is what gets submitted; an open popup is
        // not an implicit accept.
        self.editor.path_completion = None;
        self.editor.skill_completion = None;

        // Classify the pending input BEFORE draining the editor so that
        // secret-bearing commands (e.g. `#key <provider> <api-key>`) can be
        // submitted without entering the input history. See issue #614.
        let pending = self.editor.input.lines().join("\n");
        let sensitive = commands::is_sensitive_input(&pending);
        let submit_result = if sensitive {
            self.editor.input.submit_without_history()
        } else {
            self.editor.input.submit()
        };
        let Some(text) = submit_result else {
            return;
        };

        let command = commands::execute_command(&text);

        if sensitive {
            match command {
                CommandResult::StoreKey { provider, key } => {
                    self.store_key(&provider, &key);
                }
                _ => {
                    self.push_system_message(
                        "Blocked secret-bearing input that did not parse as `#key <provider> <api-key>`."
                            .to_string(),
                    );
                }
            }
            return;
        }

        // Host-registered commands are matched before the built-in table so an
        // embedding binary can add commands — or shadow built-ins — without
        // forking the crate (issue #1084 §2). This runs after the secret
        // classification above so `#key` input never reaches a host handler.
        if let Some((name, args)) = commands::split_command(&text)
            && let Some(feedback) = self.extensions.dispatch(self, name, args)
        {
            self.push_system_message(feedback);
            return;
        }

        // Known skills are submitted as prompts rather than commands, so
        // `/deploy` never hits the Unknown-command fallback. The raw text is
        // what gets displayed; `send_to_agent` expands the invocation. Match
        // precedence is secrets → host commands → skills → built-ins
        // (first-match-wins), so a host `with_command` shadows a same-named
        // skill. Parsed with `parse_skill_invocation` (not `split_command`) so
        // only the leading-`/` form routes here — `#name` stays a command.
        if let Some(invocation) = crate::skills::parse_skill_invocation(&text)
            && self.extensions.is_known_skill(&invocation.name)
        {
            self.submit_user_text(text);
            return;
        }

        if self.agent_io.status == AgentStatus::Running
            && Self::command_mutates_session_during_stream(&command)
        {
            self.push_system_message(
                "Command blocked while the agent is running. Stop the active stream and try again."
                    .to_string(),
            );
            return;
        }

        match command {
            CommandResult::NotACommand => {}
            CommandResult::Quit => {
                self.should_quit = true;
                return;
            }
            CommandResult::Clear => {
                self.view.messages.clear();
                self.view.conversation = crate::ui::conversation::ConversationView::new();
                return;
            }
            CommandResult::ToggleHelp => {
                self.view.help_panel.toggle();
                self.view.dirty = true;
                return;
            }
            CommandResult::Feedback(msg) => {
                let feedback = if msg.is_empty() {
                    self.session_info()
                } else {
                    msg
                };
                self.push_system_message(feedback);
                return;
            }
            CommandResult::SetThinking(level) => {
                self.set_thinking_level(level);
                self.push_system_message(format!("Thinking level set to: {level:?}"));
                return;
            }
            CommandResult::SetSystemPrompt(prompt) => {
                if let Some(agent) = &mut self.agent_io.agent {
                    agent.set_system_prompt(prompt);
                } else if self.agent_io.external_transport {
                    self.queue_control(
                        "set system prompt",
                        ControlRequest::SetSystemPrompt(prompt),
                        ControlFollowUp::Discard,
                    );
                }
                self.push_system_message("System prompt updated.".to_string());
                self.trim_messages_to_recent_turns();
                return;
            }
            CommandResult::Reset => {
                self.reset_session_state();
                return;
            }
            CommandResult::Compact => {
                if self.agent_io.agent.is_some() {
                    // compact_context is async and command dispatch is sync:
                    // flag it for the event loop's next async pass.
                    self.agent_io.pending_compact = true;
                } else if self.agent_io.external_transport {
                    self.queue_control(
                        "compact",
                        ControlRequest::Compact,
                        ControlFollowUp::RenderCompaction,
                    );
                } else {
                    self.push_system_message("No agent connected.".to_string());
                }
                return;
            }
            CommandResult::CopyToClipboard(content) => {
                self.copy_to_clipboard(content);
                return;
            }
            CommandResult::SaveSession => {
                self.save_session();
                return;
            }
            CommandResult::LoadSession(id) => {
                let _ = self.load_session(&id);
                return;
            }
            CommandResult::ListSessions => {
                self.list_sessions();
                return;
            }
            CommandResult::StoreKey { provider, key } => {
                self.store_key(&provider, &key);
                return;
            }
            CommandResult::ListKeys => {
                self.list_keys();
                return;
            }
            CommandResult::SetApprovalMode(mode) => {
                let harness_mode = match mode {
                    ApprovalModeArg::On => ApprovalMode::Enabled,
                    ApprovalModeArg::Off => ApprovalMode::Bypassed,
                    ApprovalModeArg::Smart => ApprovalMode::Smart,
                };
                if let Some(agent) = &mut self.agent_io.agent {
                    agent.set_approval_mode(harness_mode);
                } else if self.agent_io.external_transport {
                    self.queue_control(
                        "set approval mode",
                        ControlRequest::SetApprovalMode(harness_mode),
                        ControlFollowUp::Discard,
                    );
                }
                let label = match mode {
                    ApprovalModeArg::On => "enabled",
                    ApprovalModeArg::Off => "disabled (auto-approve)",
                    ApprovalModeArg::Smart => {
                        "smart (auto-approve read-only and trusted tools, prompt for writes)"
                    }
                };
                self.push_system_message(format!("Tool approval: {label}"));
                return;
            }
            CommandResult::OpenEditor => {
                self.editor.open_editor_requested = true;
                return;
            }
            CommandResult::TogglePlanMode => {
                self.toggle_operating_mode();
                return;
            }
            CommandResult::ShowUsage => {
                let report = self.usage_report();
                self.push_system_message(report);
                return;
            }
            CommandResult::UntrustTool(name) => {
                self.agent_io.session_trusted_tools.remove(&name);
                self.push_system_message(format!("Untrusted tool: {name}"));
                return;
            }
            CommandResult::UntrustAll => {
                self.agent_io.session_trusted_tools.clear();
                self.push_system_message("Cleared all trusted tools".to_string());
                return;
            }
            CommandResult::QueryApprovalMode => {
                if self.agent_io.agent.is_none() && self.agent_io.external_transport {
                    // `approval_mode()` would misreport the remote's mode as
                    // the local default; ask the backend and display whatever
                    // it answers on the flush pass instead.
                    self.queue_control(
                        "query approval mode",
                        ControlRequest::QueryApprovalMode,
                        ControlFollowUp::ShowApprovalMode,
                    );
                    return;
                }
                let msg = self.approval_mode_report(self.approval_mode());
                self.push_system_message(msg);
                return;
            }
        }

        self.submit_user_text(text);
    }

    fn copy_to_clipboard(&mut self, content: ClipboardContent) {
        let text = match content {
            ClipboardContent::Last => self
                .view
                .messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .map(|message| message.content.clone()),
            ClipboardContent::All => {
                let all: String = self
                    .view
                    .messages
                    .iter()
                    .map(|message| {
                        let role = match message.role {
                            MessageRole::User => "You",
                            MessageRole::Assistant => "Assistant",
                            MessageRole::ToolResult => "Tool",
                            MessageRole::Error => "Error",
                            // Covers MessageRole::System and, since
                            // DisplayRole is #[non_exhaustive], any unknown
                            // future role.
                            _ => "System",
                        };
                        format!("{role}: {}", message.content)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Some(all)
            }
            ClipboardContent::Code => self
                .view
                .messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .and_then(|message| extract_code_blocks(&message.content)),
        };

        let feedback = text.map_or_else(
            || "Nothing to copy.".to_string(),
            |text| match arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text))
            {
                Ok(()) => "Copied to clipboard.".to_string(),
                Err(error) => format!("Clipboard error: {error}"),
            },
        );

        self.push_system_message(feedback);
    }
}
