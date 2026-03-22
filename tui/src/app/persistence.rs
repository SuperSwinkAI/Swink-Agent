//! Session persistence and credential helpers.

use swink_agent::{AgentMessage, ContentBlock, LlmMessage};
use swink_agent_memory::now_utc;
use tracing::{info, warn};

use crate::credentials;
use crate::session::{SessionMeta, SessionStore};
use crate::ui::conversation::ConversationView;

use super::state::{App, DisplayMessage, MessageRole};

impl App {
    pub(super) fn auto_save_session(&self) {
        let Some(ref store) = self.session_store else {
            return;
        };
        let Some(ref agent) = self.agent else {
            return;
        };
        let state = agent.state();
        let now = now_utc();
        let meta = SessionMeta {
            id: self.session_id.clone(),
            title: self.model_name.clone(),
            created_at: now,
            updated_at: now,
        };
        // Filter AgentMessages to LlmMessages for storage.
        let llm_messages: Vec<LlmMessage> = state
            .messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();
        let _ = store.save(&self.session_id, &meta, &llm_messages);
    }

    pub(super) fn save_session(&mut self) {
        info!(session_id = %self.session_id, "saving session");
        self.auto_save_session();
        self.push_system_message(format!("Session saved: {}", self.session_id));
    }

    pub(super) fn load_session(&mut self, id: &str) {
        let Some(ref store) = self.session_store else {
            warn!("session persistence unavailable");
            self.push_system_message("Session persistence unavailable.".to_string());
            return;
        };
        info!(session_id = %id, "loading session");
        match store.load(id) {
            Ok((meta, messages)) => {
                self.messages.clear();
                for msg in &messages {
                    match msg {
                        LlmMessage::User(user) => {
                            self.messages.push(DisplayMessage {
                                role: MessageRole::User,
                                content: ContentBlock::extract_text(&user.content),
                                thinking: None,
                                is_streaming: false,
                                collapsed: false,
                                summary: String::new(),
                                user_expanded: false,
                                expanded_at: None,
                                plan_mode: false,
                                diff_data: None,
                            });
                        }
                        LlmMessage::Assistant(assistant) => {
                            self.messages.push(DisplayMessage {
                                role: MessageRole::Assistant,
                                content: ContentBlock::extract_text(&assistant.content),
                                thinking: None,
                                is_streaming: false,
                                collapsed: false,
                                summary: String::new(),
                                user_expanded: false,
                                expanded_at: None,
                                plan_mode: false,
                                diff_data: None,
                            });
                        }
                        LlmMessage::ToolResult(tool_result) => {
                            let content = ContentBlock::extract_text(&tool_result.content);
                            if !content.is_empty() {
                                let summary = content
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .chars()
                                    .take(60)
                                    .collect::<String>();
                                let diff_data = crate::ui::diff::DiffData::from_details(
                                    &tool_result.details,
                                );
                                self.messages.push(DisplayMessage {
                                    role: MessageRole::ToolResult,
                                    content,
                                    thinking: None,
                                    is_streaming: false,
                                    collapsed: true,
                                    summary,
                                    user_expanded: false,
                                    expanded_at: None,
                                    plan_mode: false,
                                    diff_data,
                                });
                            }
                        }
                    }
                }
                self.session_id = id.to_string();
                self.model_name.clone_from(&meta.title);
                self.conversation = ConversationView::new();
                self.trim_messages_to_recent_turns();
                if let Some(agent) = &mut self.agent {
                    // Convert LlmMessages back to AgentMessages for the agent.
                    let agent_messages: Vec<AgentMessage> = messages
                        .into_iter()
                        .map(AgentMessage::Llm)
                        .collect();
                    agent.set_messages(agent_messages);
                }
                self.push_system_message(format!(
                    "Loaded session: {} ({} messages)",
                    id,
                    self.messages.len()
                ));
            }
            Err(error) => {
                warn!(session_id = %id, error = %error, "failed to load session");
                self.push_system_message(format!("Failed to load session: {error}"));
            }
        }
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
                    let _ = writeln!(
                        text,
                        "  {} — {}{current}",
                        session.id, session.title
                    );
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
