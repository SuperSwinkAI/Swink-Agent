//! Session persistence and credential helpers.

use std::io;

use swink_agent::{AgentMessage, ContentBlock, LlmMessage, StopReason};
use swink_agent_memory::now_utc;
use tracing::{info, warn};

use crate::credentials;
use crate::session::{SessionMeta, SessionStore};
use crate::ui::conversation::ConversationView;

use super::state::{App, DisplayMessage, MessageRole};

impl App {
    /// Persist the current session. Returns an error if the save failed so that
    /// callers can surface it to the user instead of silently dropping failures.
    pub(super) fn auto_save_session(&mut self) -> io::Result<()> {
        let Some(ref store) = self.session_store else {
            return Ok(());
        };
        let Some(ref agent) = self.agent else {
            return Ok(());
        };
        let state = agent.state();
        let now = now_utc();

        // Build the meta to send to the store. Preserve `created_at` and the
        // optimistic-concurrency `sequence` from the last successful save.
        let mut meta = match self.session_meta.clone() {
            Some(mut existing) => {
                existing.id.clone_from(&self.session_id);
                existing.title.clone_from(&self.model_name);
                existing.updated_at = now;
                existing
            }
            None => SessionMeta {
                id: self.session_id.clone(),
                title: self.model_name.clone(),
                created_at: now,
                updated_at: now,
                version: 1,
                sequence: 0,
            },
        };

        match store.save(&self.session_id, &meta, &state.messages) {
            Ok(()) => {
                // Store increments sequence by 1 on successful write — mirror that
                // locally so the next save doesn't race on a stale sequence.
                meta.sequence += 1;
                self.session_meta = Some(meta);
                Ok(())
            }
            Err(error) => {
                warn!(session_id = %self.session_id, error = %error, "failed to save session");
                Err(error)
            }
        }
    }

    pub(super) fn save_session(&mut self) {
        info!(session_id = %self.session_id, "saving session");
        match self.auto_save_session() {
            Ok(()) => {
                self.push_system_message(format!("Session saved: {}", self.session_id));
            }
            Err(error) => {
                self.push_system_message(format!("Failed to save session: {error}"));
            }
        }
    }

    pub(super) fn load_session(&mut self, id: &str) -> io::Result<()> {
        let Some(ref store) = self.session_store else {
            warn!("session persistence unavailable");
            self.push_system_message("Session persistence unavailable.".to_string());
            return Err(io::Error::other("session persistence unavailable"));
        };
        info!(session_id = %id, "loading session");
        match store.load(id, None) {
            Ok((meta, messages)) => {
                self.messages.clear();
                for msg in &messages {
                    match msg {
                        AgentMessage::Llm(LlmMessage::User(user)) => {
                            self.messages.push(DisplayMessage::new(
                                MessageRole::User,
                                ContentBlock::extract_text(&user.content),
                            ));
                        }
                        AgentMessage::Llm(LlmMessage::Assistant(assistant)) => {
                            let text = ContentBlock::extract_text(&assistant.content);
                            let (role, content) =
                                if assistant.stop_reason == StopReason::Error {
                                    let content = if text.is_empty() {
                                        assistant
                                            .error_message
                                            .clone()
                                            .unwrap_or_default()
                                    } else {
                                        text
                                    };
                                    (MessageRole::Error, content)
                                } else {
                                    (MessageRole::Assistant, text)
                                };
                            self.messages.push(DisplayMessage::new(role, content));
                        }
                        AgentMessage::Llm(LlmMessage::ToolResult(tool_result)) => {
                            let content = ContentBlock::extract_text(&tool_result.content);
                            if !content.is_empty() {
                                let summary = content
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .chars()
                                    .take(60)
                                    .collect::<String>();
                                let mut dm = DisplayMessage::new(MessageRole::ToolResult, content);
                                dm.collapsed = true;
                                dm.summary = summary;
                                dm.diff_data =
                                    crate::ui::diff::DiffData::from_details(&tool_result.details);
                                self.messages.push(dm);
                            }
                        }
                        AgentMessage::Custom(_) => {
                            // Custom messages are not displayed in the TUI
                        }
                    }
                }
                self.session_id = id.to_string();
                self.model_name.clone_from(&meta.title);
                self.session_meta = Some(meta.clone());
                self.conversation = ConversationView::new();
                self.trim_messages_to_recent_turns();
                if let Some(agent) = &mut self.agent {
                    agent.set_messages(messages);
                }
                self.push_system_message(format!(
                    "Loaded session: {} ({} messages)",
                    id,
                    self.messages.len()
                ));
                Ok(())
            }
            Err(error) => {
                warn!(session_id = %id, error = %error, "failed to load session");
                self.push_system_message(format!("Failed to load session: {error}"));
                Err(error)
            }
        }
    }

    /// Load a prior session into the app before the event loop starts.
    ///
    /// Returns [`io::Error`] with kind [`io::ErrorKind::NotFound`] when the session
    /// does not exist, allowing CLI callers to surface the correct exit code.
    pub fn resume_into(&mut self, id: &str) -> io::Result<()> {
        self.load_session(id)
    }

    pub(super) fn list_sessions(&mut self) {
        use std::fmt::Write;

        let Some(ref store) = self.session_store else {
            self.push_system_message("Session persistence unavailable.".to_string());
            return;
        };
        match store.list() {
            Ok(sessions) if sessions.is_empty() => {
                self.push_system_message("No saved sessions.".to_string());
            }
            Ok(sessions) => {
                let mut text = String::from("Saved sessions:\n");
                for session in &sessions {
                    let current = if session.id == self.session_id {
                        " (current)"
                    } else {
                        ""
                    };
                    let _ = writeln!(text, "  {} — {}{current}", session.id, session.title);
                }
                text.push_str("\nUse #load <id> to restore a session.");
                self.push_system_message(text);
            }
            Err(error) => {
                self.push_system_message(format!("Failed to list sessions: {error}"));
            }
        }
    }

    pub(super) fn store_key(&mut self, provider: &str, key: &str) {
        match credentials::store_credential(provider, key) {
            Ok(()) => {
                info!(provider = %provider, "API key stored");
                self.push_system_message(format!("API key stored for: {provider}"));
            }
            Err(error) => {
                warn!(provider = %provider, error = %error, "failed to store API key");
                self.push_system_message(format!("Failed to store key: {error}"));
            }
        }
    }

    pub(super) fn list_keys(&mut self) {
        use std::fmt::Write;

        let status = credentials::check_credentials();
        let providers = credentials::providers();
        let mut text = String::from("Provider credentials:\n");
        for provider in &providers {
            let configured = status.get(provider.key_name).copied().unwrap_or(false);
            let icon = if configured { "✓" } else { "✗" };
            let note = if provider.requires_key {
                ""
            } else {
                " (no key needed)"
            };
            let _ = writeln!(
                text,
                "  {icon} {} — {}{note}",
                provider.name, provider.description
            );
        }
        text.push_str("\nUse #key <provider> <api-key> to store a key.");
        self.push_system_message(text);
    }
}
