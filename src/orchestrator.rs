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
pub struct AgentRequest {
    /// The messages to inject into the agent.
    pub messages: Vec<AgentMessage>,
    /// A one-shot channel for the agent's response.
    pub reply: oneshot::Sender<Result<AgentResult, AgentError>>,
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
        let request = AgentRequest {
            messages,
            reply: reply_tx,
        };
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
mod tests {
    use std::panic::AssertUnwindSafe;

    use super::*;

    #[test]
    fn add_agent_and_names() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("alpha", || panic!("not called"));
        orch.add_agent("beta", || panic!("not called"));

        let mut names = orch.names();
        names.sort_unstable();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn contains_registered() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("a", || panic!("not called"));
        assert!(orch.contains("a"));
        assert!(!orch.contains("b"));
    }

    #[test]
    fn parent_child_hierarchy() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("parent", || panic!("not called"));
        orch.add_child("child1", "parent", || panic!("not called"));
        orch.add_child("child2", "parent", || panic!("not called"));

        assert_eq!(orch.parent_of("child1"), Some("parent"));
        assert_eq!(orch.parent_of("child2"), Some("parent"));
        assert_eq!(orch.parent_of("parent"), None);

        let children = orch.children_of("parent").unwrap();
        assert_eq!(children, &["child1", "child2"]);
        assert!(orch.children_of("child1").unwrap().is_empty());
    }

    #[test]
    #[should_panic(expected = "parent agent 'missing' not registered")]
    fn add_child_missing_parent_panics() {
        let mut orch = AgentOrchestrator::new();
        orch.add_child("child", "missing", || panic!("not called"));
    }

    #[test]
    #[should_panic(expected = "agent 'alpha' already registered")]
    fn add_agent_duplicate_name_panics() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("alpha", || panic!("not called"));
        orch.add_agent("alpha", || panic!("not called"));
    }

    #[test]
    fn duplicate_child_registration_preserves_existing_hierarchy() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("parent1", || panic!("not called"));
        orch.add_agent("parent2", || panic!("not called"));
        orch.add_child("child", "parent1", || panic!("not called"));

        let duplicate = std::panic::catch_unwind(AssertUnwindSafe(|| {
            orch.add_child("child", "parent2", || panic!("not called"));
        }));

        assert!(duplicate.is_err());
        assert_eq!(orch.parent_of("child"), Some("parent1"));
        assert_eq!(orch.children_of("parent1").unwrap(), &["child"]);
        assert!(orch.children_of("parent2").unwrap().is_empty());
    }

    #[test]
    fn duplicate_top_level_registration_preserves_child_link() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("parent", || panic!("not called"));
        orch.add_child("child", "parent", || panic!("not called"));

        let duplicate = std::panic::catch_unwind(AssertUnwindSafe(|| {
            orch.add_agent("child", || panic!("not called"));
        }));

        assert!(duplicate.is_err());
        assert_eq!(orch.parent_of("child"), Some("parent"));
        assert_eq!(orch.children_of("parent").unwrap(), &["child"]);
    }

    #[test]
    fn spawn_unregistered_agent_errors() {
        let orch = AgentOrchestrator::new();
        let result = orch.spawn("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("orchestrator"));
    }

    #[test]
    fn send_agent_reply_reports_dropped_receiver() {
        let (reply_tx, reply_rx) = oneshot::channel();
        drop(reply_rx);

        assert!(!send_agent_reply("worker", "completed", reply_tx, "reply"));
    }

    #[test]
    fn default_supervisor_retryable_restarts() {
        let supervisor = DefaultSupervisor::default();
        assert_eq!(supervisor.max_restarts(), 3);

        let retryable = AgentError::ModelThrottled;
        assert_eq!(
            supervisor.on_agent_error("test", &retryable),
            SupervisorAction::Restart
        );

        let non_retryable = AgentError::Aborted;
        assert_eq!(
            supervisor.on_agent_error("test", &non_retryable),
            SupervisorAction::Stop
        );
    }

    #[test]
    fn supervisor_action_variants() {
        assert_eq!(format!("{:?}", SupervisorAction::Restart), "Restart");
        assert_eq!(format!("{:?}", SupervisorAction::Stop), "Stop");
        assert_eq!(format!("{:?}", SupervisorAction::Escalate), "Escalate");
    }

    #[test]
    fn orchestrator_debug_format() {
        let orch = AgentOrchestrator::new();
        let debug = format!("{orch:?}");
        assert!(debug.contains("AgentOrchestrator"));
        assert!(debug.contains("channel_buffer"));
    }

    #[test]
    fn with_supervisor_sets_policy() {
        let orch = AgentOrchestrator::new().with_supervisor(DefaultSupervisor::default());
        assert!(orch.supervisor.is_some());
    }

    #[test]
    fn with_channel_buffer_sets_size() {
        let orch = AgentOrchestrator::new().with_channel_buffer(64);
        assert_eq!(orch.channel_buffer, 64);
    }

    #[test]
    fn with_max_restarts_sets_default() {
        let mut orch = AgentOrchestrator::new().with_max_restarts(5);
        orch.add_agent("a", || panic!("not called"));
        assert_eq!(orch.entries["a"].max_restarts, 5);
    }

    #[test]
    fn default_impl() {
        let orch = AgentOrchestrator::default();
        assert!(orch.entries.is_empty());
        assert!(orch.supervisor.is_none());
    }

    #[test]
    fn custom_supervisor_policy() {
        struct AlwaysEscalate;
        impl SupervisorPolicy for AlwaysEscalate {
            fn on_agent_error(&self, _name: &str, _error: &AgentError) -> SupervisorAction {
                SupervisorAction::Escalate
            }
        }

        let supervisor = AlwaysEscalate;
        assert_eq!(
            supervisor.on_agent_error("x", &AgentError::ModelThrottled),
            SupervisorAction::Escalate
        );
    }

    #[test]
    fn grandchild_hierarchy() {
        let mut orch = AgentOrchestrator::new();
        orch.add_agent("root", || panic!("not called"));
        orch.add_child("mid", "root", || panic!("not called"));
        orch.add_child("leaf", "mid", || panic!("not called"));

        assert_eq!(orch.parent_of("leaf"), Some("mid"));
        assert_eq!(orch.parent_of("mid"), Some("root"));
        assert_eq!(orch.parent_of("root"), None);

        assert_eq!(orch.children_of("root").unwrap(), &["mid"]);
        assert_eq!(orch.children_of("mid").unwrap(), &["leaf"]);
        assert!(orch.children_of("leaf").unwrap().is_empty());
    }
}
