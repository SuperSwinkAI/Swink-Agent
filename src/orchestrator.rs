//! Multi-agent orchestration with parent/child hierarchies and supervision.
//!
//! [`AgentOrchestrator`] manages a set of named agents, tracks parent/child
//! relationships, and applies a [`SupervisorPolicy`] when agents fail. Each
//! spawned agent is represented by an [`OrchestratedHandle`] that supports
//! request/response messaging, result retrieval, and cancellation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::agent::{Agent, AgentOptions};
use crate::error::AgentError;
use crate::handle::AgentStatus;
use crate::task_core::{TaskCore, resolve_status};
use crate::types::{AgentMessage, AgentResult, ContentBlock, LlmMessage, UserMessage};
use crate::util::now_timestamp;

// ─── Type aliases ───────────────────────────────────────────────────────────

type OptionsFactoryArc = Arc<dyn Fn() -> AgentOptions + Send + Sync>;

// ─── Request / Response channel ─────────────────────────────────────────────

/// A message sent to a running agent via its request channel.
#[non_exhaustive]
pub struct AgentRequest {
    /// The messages to inject into the agent.
    pub messages: Vec<AgentMessage>,
    /// A one-shot channel for the agent's response.
    pub reply: oneshot::Sender<Result<AgentResult, AgentError>>,
}

impl AgentRequest {
    /// Create a new agent request with the given messages and reply channel.
    #[must_use]
    pub fn new(
        messages: Vec<AgentMessage>,
        reply: oneshot::Sender<Result<AgentResult, AgentError>>,
    ) -> Self {
        Self { messages, reply }
    }
}

fn send_agent_reply<T>(
    agent_name: &str,
    outcome: &'static str,
    reply: oneshot::Sender<T>,
    value: T,
) -> bool {
    if reply.send(value).is_ok() {
        true
    } else {
        warn!(
            agent = %agent_name,
            outcome,
            "orchestrator reply receiver dropped"
        );
        false
    }
}

// ─── Supervisor ─────────────────────────────────────────────────────────────

/// What the supervisor decides after an agent error.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorAction {
    /// Restart the failed agent with the same options.
    Restart,
    /// Stop the agent permanently.
    Stop,
    /// Escalate the error to the caller but keep the agent alive.
    Escalate,
}

/// Policy that determines how to handle agent failures.
///
/// Implement this trait and pass it to
/// [`AgentOrchestrator::with_supervisor`] to customise recovery behaviour.
pub trait SupervisorPolicy: Send + Sync {
    /// Called when a spawned agent terminates with an error.
    fn on_agent_error(&self, name: &str, error: &AgentError) -> SupervisorAction;
}

/// A supervisor that restarts on retryable errors and stops otherwise.
#[derive(Debug, Clone)]
pub struct DefaultSupervisor {
    max_restarts: u32,
}

impl DefaultSupervisor {
    /// Create a supervisor that allows up to `max_restarts` consecutive restarts.
    #[must_use]
    pub const fn new(max_restarts: u32) -> Self {
        Self { max_restarts }
    }

    /// The maximum number of consecutive restarts allowed.
    #[must_use]
    pub const fn max_restarts(&self) -> u32 {
        self.max_restarts
    }
}

impl Default for DefaultSupervisor {
    fn default() -> Self {
        Self { max_restarts: 3 }
    }
}

impl SupervisorPolicy for DefaultSupervisor {
    fn on_agent_error(&self, _name: &str, error: &AgentError) -> SupervisorAction {
        if error.is_retryable() {
            SupervisorAction::Restart
        } else {
            SupervisorAction::Stop
        }
    }
}

// ─── Agent entry (internal bookkeeping) ─────────────────────────────────────

/// Registration info stored in the orchestrator for each agent.
struct AgentEntry {
    /// Factory that produces fresh `AgentOptions` for (re)spawning.
    options_factory: OptionsFactoryArc,
    /// Parent agent name, if this is a child.
    parent: Option<String>,
    /// Child agent names.
    children: Vec<String>,
    /// Max restarts allowed by the supervisor (per spawn cycle).
    max_restarts: u32,
}

// ─── OrchestratedHandle ─────────────────────────────────────────────────────

/// Handle to a spawned orchestrated agent.
///
/// Provides request/response messaging, status polling, and cancellation.
/// Lifecycle methods (status, cancel, `is_done`) are delegated to a shared task core.
pub struct OrchestratedHandle {
    name: String,
    request_tx: mpsc::Sender<AgentRequest>,
    core: TaskCore,
}

impl OrchestratedHandle {
    /// The name of the agent this handle refers to.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Send a text message to the running agent and await its response.
    pub async fn send_message(&self, text: impl Into<String>) -> Result<AgentResult, AgentError> {
        let msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: now_timestamp(),
            cache_hint: None,
        }));
        self.send_messages(vec![msg]).await
    }

    /// Send multiple messages to the agent and await its response.
    pub async fn send_messages(
        &self,
        messages: Vec<AgentMessage>,
    ) -> Result<AgentResult, AgentError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let request = AgentRequest::new(messages, reply_tx);
        self.request_tx.send(request).await.map_err(|_| {
            AgentError::plugin(
                "orchestrator",
                std::io::Error::other("agent channel closed"),
            )
        })?;

        reply_rx.await.map_err(|_| {
            AgentError::plugin("orchestrator", std::io::Error::other("agent reply dropped"))
        })?
    }

    /// Consume the handle and await the agent's final result.
    ///
    /// Drops the request channel so the agent shuts down after processing
    /// any remaining requests.
    pub async fn await_result(self) -> Result<AgentResult, AgentError> {
        drop(self.request_tx);
        self.core.result().await
    }

    /// Cancel the agent.
    pub fn cancel(&self) {
        self.core.cancel();
    }

    /// Current status of the agent.
    pub fn status(&self) -> AgentStatus {
        self.core.status()
    }

    /// Whether the agent has finished (completed, failed, or cancelled).
    pub fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

impl std::fmt::Debug for OrchestratedHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratedHandle")
            .field("name", &self.name)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

// ─── AgentOrchestrator ──────────────────────────────────────────────────────

/// Manages a set of named agents with parent/child hierarchies and supervision.
///
/// # Usage
///
/// ```ignore
/// let mut orchestrator = AgentOrchestrator::new();
/// orchestrator.add_agent("planner", || planner_options());
/// orchestrator.add_child("researcher", "planner", || researcher_options());
///
/// let handle = orchestrator.spawn("planner")?;
/// let result = handle.send_message("Plan a trip to Paris").await?;
/// ```
pub struct AgentOrchestrator {
    entries: HashMap<String, AgentEntry>,
    supervisor: Option<Arc<dyn SupervisorPolicy>>,
    /// Channel buffer size for request channels.
    channel_buffer: usize,
    /// Default max restarts for agents (used when supervisor is set).
    default_max_restarts: u32,
}

impl AgentOrchestrator {
    /// Create a new empty orchestrator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            supervisor: None,
            channel_buffer: 32,
            default_max_restarts: 3,
        }
    }

    /// Set a supervisor policy for error recovery.
    #[must_use]
    pub fn with_supervisor(mut self, policy: impl SupervisorPolicy + 'static) -> Self {
        self.supervisor = Some(Arc::new(policy));
        self
    }

    /// Set the request channel buffer size (default: 32).
    #[must_use]
    pub const fn with_channel_buffer(mut self, size: usize) -> Self {
        self.channel_buffer = size;
        self
    }

    /// Set the default max restarts for supervised agents (default: 3).
    #[must_use]
    pub const fn with_max_restarts(mut self, max: u32) -> Self {
        self.default_max_restarts = max;
        self
    }

    /// Register an agent by name with a factory that produces its options.
    ///
    /// The factory is called each time the agent is spawned or restarted.
    ///
    /// # Panics
    ///
    /// Panics if an agent with the same name has already been registered.
    pub fn add_agent(
        &mut self,
        name: impl Into<String>,
        options_factory: impl Fn() -> AgentOptions + Send + Sync + 'static,
    ) {
        let name = name.into();
        assert!(
            !self.entries.contains_key(&name),
            "agent '{name}' already registered"
        );
        self.entries.insert(
            name,
            AgentEntry {
                options_factory: Arc::new(options_factory),
                parent: None,
                children: Vec::new(),
                max_restarts: self.default_max_restarts,
            },
        );
    }

    /// Register a child agent under the given parent.
    ///
    /// # Panics
    ///
    /// Panics if the parent agent has not been registered or if the child name
    /// is already registered.
    pub fn add_child(
        &mut self,
        name: impl Into<String>,
        parent: impl Into<String>,
        options_factory: impl Fn() -> AgentOptions + Send + Sync + 'static,
    ) {
        let name = name.into();
        let parent = parent.into();
        assert!(
            self.entries.contains_key(&parent),
            "parent agent '{parent}' not registered"
        );
        assert!(
            !self.entries.contains_key(&name),
            "agent '{name}' already registered"
        );

        self.entries
            .get_mut(&parent)
            .expect("parent checked above")
            .children
            .push(name.clone());

        self.entries.insert(
            name,
            AgentEntry {
                options_factory: Arc::new(options_factory),
                parent: Some(parent),
                children: Vec::new(),
                max_restarts: self.default_max_restarts,
            },
        );
    }

    /// Get the parent name for a registered agent.
    #[must_use]
    pub fn parent_of(&self, name: &str) -> Option<&str> {
        self.entries.get(name).and_then(|e| e.parent.as_deref())
    }

    /// Get the child names for a registered agent.
    #[must_use]
    pub fn children_of(&self, name: &str) -> Option<&[String]> {
        self.entries.get(name).map(|e| e.children.as_slice())
    }

    /// List all registered agent names.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// Whether an agent with this name is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Spawn a registered agent, returning a handle for interaction.
    ///
    /// The agent runs in a background tokio task listening for requests. Each
    /// request triggers `prompt_async` and the result is sent via a one-shot
    /// reply channel.
    ///
    /// If a [`SupervisorPolicy`] is set, the agent is automatically restarted
    /// when the supervisor returns [`SupervisorAction::Restart`].
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::Plugin`] if the agent name is not registered.
    pub fn spawn(&self, name: &str) -> Result<OrchestratedHandle, AgentError> {
        let entry = self.entries.get(name).ok_or_else(|| {
            AgentError::plugin(
                "orchestrator",
                std::io::Error::other(format!("agent not registered: {name}")),
            )
        })?;

        let factory = Arc::clone(&entry.options_factory);
        let max_restarts = entry.max_restarts;
        let agent_name = name.to_owned();
        let supervisor = self.supervisor.clone();

        let (request_tx, request_rx) = mpsc::channel::<AgentRequest>(self.channel_buffer);
        let cancellation_token = CancellationToken::new();
        let status = Arc::new(Mutex::new(AgentStatus::Running));

        let status_clone = Arc::clone(&status);
        let token_clone = cancellation_token.clone();

        let join_handle = tokio::spawn(run_agent_loop(
            agent_name,
            factory,
            request_rx,
            token_clone,
            status_clone,
            supervisor,
            max_restarts,
        ));

        Ok(OrchestratedHandle {
            name: name.to_owned(),
            request_tx,
            core: TaskCore::new(join_handle, cancellation_token, status),
        })
    }
}

/// The core agent loop that runs inside a spawned tokio task.
///
/// Receives requests on the channel, processes them with the agent, and
/// optionally restarts the agent on failure per the supervisor policy.
async fn run_agent_loop(
    agent_name: String,
    factory: OptionsFactoryArc,
    mut request_rx: mpsc::Receiver<AgentRequest>,
    cancellation_token: CancellationToken,
    status: Arc<Mutex<AgentStatus>>,
    supervisor: Option<Arc<dyn SupervisorPolicy>>,
    max_restarts: u32,
) -> Result<AgentResult, AgentError> {
    let mut agent = Agent::new(factory());
    let mut restarts: u32 = 0;

    let final_result = loop {
        tokio::select! {
            biased;

            () = cancellation_token.cancelled() => {
                agent.abort();
                break Err(AgentError::Aborted);
            }

            maybe_req = request_rx.recv() => {
                if let Some(req) = maybe_req {
                    let result = tokio::select! {
                        biased;
                        () = cancellation_token.cancelled() => {
                            agent.abort();
                            send_agent_reply(
                                &agent_name,
                                "aborted",
                                req.reply,
                                Err(AgentError::Aborted),
                            );
                            break Err(AgentError::Aborted);
                        }
                        r = agent.prompt_async(req.messages) => r,
                    };

                    match result {
                        Ok(r) => {
                            send_agent_reply(&agent_name, "completed", req.reply, Ok(r));
                            // Reset restart counter on success.
                            restarts = 0;
                        }
                        Err(err) => {
                            let action = supervisor
                                .as_ref()
                                .map_or(SupervisorAction::Escalate, |s| {
                                    s.on_agent_error(&agent_name, &err)
                                });

                            match action {
                                SupervisorAction::Restart if restarts < max_restarts => {
                                    warn!(
                                        agent = %agent_name,
                                        restart = restarts + 1,
                                        max = max_restarts,
                                        "supervisor restarting agent"
                                    );
                                    restarts += 1;
                                    send_agent_reply(&agent_name, "restart", req.reply, Err(err));
                                    agent = Agent::new(factory());
                                }
                                SupervisorAction::Escalate => {
                                    send_agent_reply(&agent_name, "escalate", req.reply, Err(err));
                                    // Agent stays alive.
                                }
                                _ => {
                                    // Stop (or restart budget exhausted).
                                    send_agent_reply(&agent_name, "stop", req.reply, Err(err));
                                    break Err(AgentError::plugin(
                                        "orchestrator",
                                        std::io::Error::other(format!(
                                            "agent '{agent_name}' stopped by supervisor"
                                        )),
                                    ));
                                }
                            }
                        }
                    }
                } else {
                    // Channel closed — clean shutdown.
                    info!(agent = %agent_name, "request channel closed, shutting down");
                    break Ok(AgentResult {
                        messages: Vec::new(),
                        stop_reason: crate::types::StopReason::Stop,
                        usage: crate::types::Usage::default(),
                        cost: crate::types::Cost::default(),
                        error: None,
                        transfer_signal: None,
                    });
                }
            }
        }
    };

    *status.lock().unwrap_or_else(PoisonError::into_inner) = resolve_status(&final_result);
    final_result
}

impl Default for AgentOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AgentOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOrchestrator")
            .field("agents", &self.entries.keys().collect::<Vec<_>>())
            .field(
                "supervisor",
                &if self.supervisor.is_some() {
                    "Some"
                } else {
                    "None"
                },
            )
            .field("channel_buffer", &self.channel_buffer)
            .finish_non_exhaustive()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;
