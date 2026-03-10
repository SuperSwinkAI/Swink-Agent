//! Top-level application state and event loop.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::ui;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Agent state as visible to the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
    Error,
    Aborted,
}

/// Top-level application state.
pub struct App {
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Current agent status.
    pub status: AgentStatus,
    /// Input editor content.
    pub input: String,
    /// Conversation messages for display.
    pub messages: Vec<DisplayMessage>,
    /// Current scroll offset in the conversation view.
    pub scroll_offset: u16,
    /// Model identifier string.
    pub model_name: String,
    /// Token usage counters.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Running cost.
    pub total_cost: f64,
}

/// A message formatted for display.
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: u64,
}

/// Message role for display styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
    Error,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            status: AgentStatus::Idle,
            input: String::new(),
            messages: Vec::new(),
            scroll_offset: 0,
            model_name: "not connected".into(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
        }
    }

    /// Main event loop.
    pub async fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> AppResult<()> {
        loop {
            terminal.draw(|frame| ui::render(frame, self))?;

            if self.should_quit {
                break;
            }

            // Poll for terminal events with a 50ms tick rate.
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key);
                }
            }
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key: event::KeyEvent) {
        match (key.modifiers, key.code) {
            // Quit: Ctrl+Q
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.should_quit = true;
            }
            // Quit: Ctrl+C when idle
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.status == AgentStatus::Idle {
                    self.should_quit = true;
                }
                // TODO: abort agent when running
            }
            // Submit: Enter
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if !self.input.is_empty() {
                    let content = std::mem::take(&mut self.input);
                    self.messages.push(DisplayMessage {
                        role: MessageRole::User,
                        content,
                        timestamp: 0,
                    });
                    // TODO: send to agent
                }
            }
            // Typing
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                self.input.push(c);
            }
            // Backspace
            (_, KeyCode::Backspace) => {
                self.input.pop();
            }
            _ => {}
        }
    }

    /// Tick handler for animations (spinners, elapsed time).
    pub fn tick(&mut self) {
        // Placeholder for animation updates.
    }
}
