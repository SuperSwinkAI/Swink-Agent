//! First-run credential setup wizard.
//!
//! Renders a full-screen ratatui UI that guides the user through
//! configuring LLM provider credentials.

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

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

impl Default for SetupWizard {
    fn default() -> Self {
        Self::new()
    }
}

impl SetupWizard {
    pub fn new() -> Self {
        let providers = credentials::providers();
        let configured: Vec<bool> = providers
            .iter()
            .map(|p| {
                if p.requires_key {
                    credentials::credential(p).is_some()
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
            .title(" Swink Agent — Setup ")
            .border_style(Style::default().fg(Color::Cyan));

        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Welcome to Swink Agent!",
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
                Constraint::Min(0),    // remaining
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

            lines.push(Line::from(vec![status, Span::raw(provider.name)]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(
            "You can update credentials later with the #key command.",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Press Enter to continue to Swink Agent.",
            Style::default().add_modifier(Modifier::DIM),
        )));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
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
            (_, KeyCode::Up) if self.selected > 0 => {
                self.selected -= 1;
            }
            (_, KeyCode::Down) if self.selected < max_index => {
                self.selected += 1;
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
            (_, KeyCode::Backspace) if !input.is_empty() && *cursor > 0 => {
                input.remove(*cursor - 1);
                *cursor -= 1;
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

    /// Test-only constructor that bypasses keychain lookups.
    #[cfg(test)]
    fn new_for_test() -> Self {
        let providers = credentials::providers();
        let configured = vec![false; providers.len()];
        Self {
            step: WizardStep::Welcome,
            providers,
            configured,
            selected: 0,
            should_quit: false,
            should_continue: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn initial_state_is_welcome() {
        let wizard = SetupWizard::new_for_test();
        assert!(matches!(wizard.step, WizardStep::Welcome));
    }

    #[test]
    fn enter_on_welcome_goes_to_provider_list() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.handle_key(key(KeyCode::Enter));
        assert!(matches!(wizard.step, WizardStep::ProviderList));
    }

    #[test]
    fn enter_on_provider_list_goes_to_key_entry() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;

        // Find a provider that requires a key
        let key_provider_idx = wizard
            .providers
            .iter()
            .position(|p| p.requires_key)
            .expect("should have at least one provider requiring a key");
        wizard.selected = key_provider_idx;

        wizard.handle_key(key(KeyCode::Enter));

        match &wizard.step {
            WizardStep::KeyEntry {
                provider_index,
                input,
                cursor,
            } => {
                assert_eq!(*provider_index, key_provider_idx);
                assert!(input.is_empty());
                assert_eq!(*cursor, 0);
            }
            other => panic!(
                "expected KeyEntry step, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[test]
    fn esc_on_welcome_sets_quit() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.handle_key(key(KeyCode::Esc));
        assert!(wizard.should_quit);
    }

    #[test]
    fn esc_on_provider_list_sets_quit() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;
        wizard.handle_key(key(KeyCode::Esc));
        assert!(wizard.should_quit);
    }

    #[test]
    fn navigation_clamps_in_provider_list() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;
        let max_index = wizard.providers.len(); // includes "Continue" item

        // At top, pressing Up should stay at 0
        wizard.selected = 0;
        wizard.handle_key(key(KeyCode::Up));
        assert_eq!(wizard.selected, 0);

        // At bottom, pressing Down should stay at max
        wizard.selected = max_index;
        wizard.handle_key(key(KeyCode::Down));
        assert_eq!(wizard.selected, max_index);
    }

    #[test]
    fn navigation_moves_up_and_down() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;
        wizard.selected = 1;

        wizard.handle_key(key(KeyCode::Down));
        assert_eq!(wizard.selected, 2);

        wizard.handle_key(key(KeyCode::Up));
        assert_eq!(wizard.selected, 1);
    }

    #[test]
    fn key_entry_accepts_input() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::KeyEntry {
            provider_index: 1,
            input: String::new(),
            cursor: 0,
        };

        wizard.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        wizard.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        wizard.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        match &wizard.step {
            WizardStep::KeyEntry { input, cursor, .. } => {
                assert_eq!(input, "abc");
                assert_eq!(*cursor, 3);
            }
            _ => panic!("should still be in KeyEntry"),
        }
    }

    #[test]
    fn backspace_in_key_entry_removes_char() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::KeyEntry {
            provider_index: 1,
            input: "abc".to_string(),
            cursor: 3,
        };

        wizard.handle_key(key(KeyCode::Backspace));

        match &wizard.step {
            WizardStep::KeyEntry { input, cursor, .. } => {
                assert_eq!(input, "ab");
                assert_eq!(*cursor, 2);
            }
            _ => panic!("should still be in KeyEntry"),
        }
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::KeyEntry {
            provider_index: 1,
            input: "abc".to_string(),
            cursor: 0,
        };

        wizard.handle_key(key(KeyCode::Backspace));

        match &wizard.step {
            WizardStep::KeyEntry { input, cursor, .. } => {
                assert_eq!(input, "abc");
                assert_eq!(*cursor, 0);
            }
            _ => panic!("should still be in KeyEntry"),
        }
    }

    #[test]
    fn esc_in_key_entry_returns_to_provider_list() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::KeyEntry {
            provider_index: 1,
            input: "some-key".to_string(),
            cursor: 8,
        };

        wizard.handle_key(key(KeyCode::Esc));

        assert!(matches!(wizard.step, WizardStep::ProviderList));
    }

    #[test]
    fn continue_option_goes_to_done() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;
        wizard.selected = wizard.providers.len(); // "Continue" item

        wizard.handle_key(key(KeyCode::Enter));

        assert!(matches!(wizard.step, WizardStep::Done));
    }

    #[test]
    fn s_key_skips_to_done() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;

        wizard.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));

        assert!(matches!(wizard.step, WizardStep::Done));
    }

    #[test]
    fn enter_on_done_sets_continue() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::Done;

        wizard.handle_key(key(KeyCode::Enter));

        assert!(wizard.should_continue);
        assert!(!wizard.should_quit);
    }

    #[test]
    fn esc_on_done_sets_quit() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::Done;

        wizard.handle_key(key(KeyCode::Esc));

        assert!(wizard.should_quit);
        assert!(!wizard.should_continue);
    }

    #[test]
    fn shift_char_in_key_entry_inserts() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::KeyEntry {
            provider_index: 1,
            input: String::new(),
            cursor: 0,
        };

        wizard.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));

        match &wizard.step {
            WizardStep::KeyEntry { input, cursor, .. } => {
                assert_eq!(input, "A");
                assert_eq!(*cursor, 1);
            }
            _ => panic!("should still be in KeyEntry"),
        }
    }

    #[test]
    fn q_on_welcome_sets_quit() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(wizard.should_quit);
    }

    #[test]
    fn enter_on_no_key_provider_is_noop() {
        let mut wizard = SetupWizard::new_for_test();
        wizard.step = WizardStep::ProviderList;

        // Find Ollama (no key required)
        let ollama_idx = wizard
            .providers
            .iter()
            .position(|p| !p.requires_key)
            .expect("should have a no-key provider");
        wizard.selected = ollama_idx;

        wizard.handle_key(key(KeyCode::Enter));

        // Should remain on ProviderList since Ollama doesn't need a key
        assert!(matches!(wizard.step, WizardStep::ProviderList));
    }
}
