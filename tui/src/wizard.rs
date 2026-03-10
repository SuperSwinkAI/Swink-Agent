//! First-run credential setup wizard.
//!
//! Renders a full-screen ratatui UI that guides the user through
//! configuring LLM provider credentials.

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;

use crate::credentials::{self, ProviderInfo};

/// Current step in the setup wizard.
pub enum WizardStep {
    Welcome,
    ProviderList,
    KeyEntry {
        provider_index: usize,
        input: String,
        cursor: usize,
    },
    Done,
}

/// Interactive setup wizard for first-run credential configuration.
pub struct SetupWizard {
    step: WizardStep,
    providers: Vec<ProviderInfo>,
    configured: Vec<bool>,
    selected: usize,
    should_quit: bool,
    should_continue: bool,
}

impl SetupWizard {
    pub fn new() -> Self {
        let providers = credentials::providers();
        let configured: Vec<bool> = providers
            .iter()
            .map(|p| {
                if p.requires_key {
                    credentials::get_credential(p).is_some()
                } else {
                    true
                }
            })
            .collect();

        Self {
            step: WizardStep::Welcome,
            providers,
            configured,
            selected: 0,
            should_quit: false,
            should_continue: false,
        }
    }

    /// Run the wizard. Returns `true` if the user wants to continue to the app,
    /// `false` if they chose to quit.
    pub fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> io::Result<bool> {
        loop {
            terminal.draw(|f| self.render(f))?;

            if self.should_continue {
                return Ok(true);
            }
            if self.should_quit {
                return Ok(false);
            }

            if let Event::Key(key) = event::read()? {
                self.handle_key(key);
            }
        }
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();

        // Clear the entire screen
        frame.render_widget(Clear, area);

        match &self.step {
            WizardStep::Welcome => self.render_welcome(frame, area),
            WizardStep::ProviderList => self.render_provider_list(frame, area),
            WizardStep::KeyEntry {
                provider_index,
                input,
                ..
            } => self.render_key_entry(frame, area, *provider_index, input),
            WizardStep::Done => self.render_done(frame, area),
        }
    }

    #[allow(clippy::unused_self)]
    fn render_welcome(&self, frame: &mut ratatui::Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Agent Harness — Setup ")
            .border_style(Style::default().fg(Color::Cyan));

        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Welcome to Agent Harness!",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("This wizard will help you configure API keys for your"),
            Line::from("LLM providers. Credentials are stored securely in your"),
            Line::from("operating system's native keychain:"),
            Line::from(""),
            Line::from(Span::styled(
                "  • macOS: Keychain Services",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  • Windows: Credential Manager",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  • Linux: secret-service (D-Bus)",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                "Press Enter to continue, or Esc to quit.",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];

        let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }

    fn render_provider_list(&self, frame: &mut ratatui::Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Select a Provider ")
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = vec![
            Line::from(""),
            Line::from("Configure API keys for the providers you want to use."),
            Line::from("Use Up/Down to navigate, Enter to configure, 's' to skip."),
            Line::from(""),
        ];

        for (i, provider) in self.providers.iter().enumerate() {
            let is_selected = i == self.selected;
            let check = if self.configured[i] {
                Span::styled("[✓] ", Style::default().fg(Color::Green))
            } else {
                Span::styled("[ ] ", Style::default().fg(Color::DarkGray))
            };

            let name_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let cursor = if is_selected { "▸ " } else { "  " };

            lines.push(Line::from(vec![
                Span::styled(
                    cursor,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                check,
                Span::styled(provider.name, name_style),
                Span::styled(
                    format!("  — {}", provider.description),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]));
        }

        // "Continue" option after all providers
        let continue_idx = self.providers.len();
        let is_continue_selected = self.selected == continue_idx;
        let continue_style = if is_continue_selected {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        let cursor = if is_continue_selected { "▸ " } else { "  " };

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                cursor,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Continue →", continue_style),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Esc to quit",
            Style::default().add_modifier(Modifier::DIM),
        )));

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }

    fn render_key_entry(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        provider_index: usize,
        input: &str,
    ) {
        let provider = &self.providers[provider_index];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Configure {} ", provider.name))
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let masked: String = "•".repeat(input.len());

        let chunks = Layout::default()
            .constraints([
                Constraint::Length(5), // instructions
                Constraint::Length(3), // input box
                Constraint::Min(0),   // remaining
            ])
            .split(inner);

        let instructions = vec![
            Line::from(""),
            Line::from(format!(
                "Enter the API key for {} (env: {})",
                provider.name, provider.env_var
            )),
            Line::from("The key will be stored in your OS keychain."),
            Line::from(""),
        ];
        let instructions_widget = Paragraph::new(instructions).wrap(Wrap { trim: false });
        frame.render_widget(instructions_widget, chunks[0]);

        let input_block = Block::default()
            .borders(Borders::ALL)
            .title(" API Key ")
            .border_style(Style::default().fg(Color::Yellow));

        let input_widget = Paragraph::new(masked).block(input_block);
        frame.render_widget(input_widget, chunks[1]);

        // Position cursor in the input field
        #[allow(clippy::cast_possible_truncation)]
        let cursor_x = chunks[1].x + 1 + input.len() as u16;
        let cursor_y = chunks[1].y + 1;
        frame.set_cursor_position((cursor_x.min(chunks[1].x + chunks[1].width - 2), cursor_y));

        let help = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Enter to save, Esc to go back",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        let help_widget = Paragraph::new(help);
        frame.render_widget(help_widget, chunks[2]);
    }

    fn render_done(&self, frame: &mut ratatui::Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Setup Complete ")
            .border_style(Style::default().fg(Color::Green));

        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Setup is complete!",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Configured providers:"),
            Line::from(""),
        ];

        for (i, provider) in self.providers.iter().enumerate() {
            let status = if self.configured[i] {
                Span::styled("  ✓ ", Style::default().fg(Color::Green))
            } else {
                Span::styled("  ✗ ", Style::default().fg(Color::DarkGray))
            };

            lines.push(Line::from(vec![
                status,
                Span::raw(provider.name),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from("You can update credentials later with the #key command."));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Press Enter to continue to Agent Harness.",
            Style::default().add_modifier(Modifier::DIM),
        )));

        let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match &mut self.step {
            WizardStep::Welcome => self.handle_welcome_key(key),
            WizardStep::ProviderList => self.handle_provider_list_key(key),
            WizardStep::KeyEntry { .. } => self.handle_key_entry_key(key),
            WizardStep::Done => self.handle_done_key(key),
        }
    }

    fn handle_welcome_key(&mut self, key: crossterm::event::KeyEvent) {
        match (key.modifiers, key.code) {
            (_, KeyCode::Enter) => self.step = WizardStep::ProviderList,
            (_, KeyCode::Esc) | (KeyModifiers::NONE, KeyCode::Char('q')) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    fn handle_provider_list_key(&mut self, key: crossterm::event::KeyEvent) {
        let max_index = self.providers.len(); // includes "Continue" item

        match (key.modifiers, key.code) {
            (_, KeyCode::Up) => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            (_, KeyCode::Down) => {
                if self.selected < max_index {
                    self.selected += 1;
                }
            }
            (_, KeyCode::Enter) => {
                if self.selected == max_index {
                    // "Continue" selected
                    self.step = WizardStep::Done;
                } else if self.providers[self.selected].requires_key {
                    self.step = WizardStep::KeyEntry {
                        provider_index: self.selected,
                        input: String::new(),
                        cursor: 0,
                    };
                }
                // For providers that don't require a key (Ollama), Enter does nothing
            }
            (KeyModifiers::NONE, KeyCode::Char('s')) => {
                self.step = WizardStep::Done;
            }
            (_, KeyCode::Esc) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    fn handle_key_entry_key(&mut self, key: crossterm::event::KeyEvent) {
        // Extract current key-entry state to work with
        let (provider_index, input, cursor) = match &mut self.step {
            WizardStep::KeyEntry {
                provider_index,
                input,
                cursor,
            } => (*provider_index, input, cursor),
            _ => return,
        };

        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.step = WizardStep::ProviderList;
            }
            (_, KeyCode::Enter) => {
                if !input.is_empty() {
                    let provider_key = self.providers[provider_index].key_name;
                    // Attempt to store the credential
                    match credentials::store_credential(provider_key, input) {
                        Ok(()) => {
                            self.configured[provider_index] = true;
                        }
                        Err(_e) => {
                            // On failure, still go back to the list; the checkbox
                            // won't be checked so the user can retry.
                        }
                    }
                }
                self.step = WizardStep::ProviderList;
            }
            (_, KeyCode::Backspace) => {
                if !input.is_empty() && *cursor > 0 {
                    input.remove(*cursor - 1);
                    *cursor -= 1;
                }
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                input.insert(*cursor, c);
                *cursor += 1;
            }
            _ => {}
        }
    }

    #[allow(clippy::missing_const_for_fn)]
    fn handle_done_key(&mut self, key: crossterm::event::KeyEvent) {
        match key.code {
            KeyCode::Enter => self.should_continue = true,
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }
}
