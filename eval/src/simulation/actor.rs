//! Simulated-user driver (US4, FR-024).
//!
//! [`ActorSimulator`] produces user turns backed by a [`JudgeClient`],
//! rotating through a greeting pool and optionally emitting a goal-completion
//! signal via the judge verdict's `label` field.

#![forbid(unsafe_code)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::judge::{JudgeClient, JudgeError, JudgeVerdict};

/// Descriptive profile of the simulated user (data-model.md §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorProfile {
    pub name: String,
    pub traits: Vec<String>,
    pub context: String,
    pub goal: String,
}

impl ActorProfile {
    #[must_use]
    pub fn new(name: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            traits: Vec::new(),
            context: String::new(),
            goal: goal.into(),
        }
    }

    /// Render the profile into a system-prompt fragment.
    #[must_use]
    pub fn as_system_prompt(&self) -> String {
        let mut prompt = format!("You are {}.\nGoal: {}\n", self.name, self.goal);
        if !self.context.is_empty() {
            prompt.push_str("Context: ");
            prompt.push_str(&self.context);
            prompt.push('\n');
        }
        if !self.traits.is_empty() {
            prompt.push_str("Traits: ");
            prompt.push_str(&self.traits.join(", "));
            prompt.push('\n');
        }
        prompt
    }
}

/// One turn produced by the actor simulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorTurn {
    pub message: String,
    /// When present, the actor asserted goal completion via this signal label.
    pub goal_completed: Option<String>,
}

/// Drives a scripted user persona over multiple dialogue turns.
pub struct ActorSimulator {
    profile: ActorProfile,
    judge: Arc<dyn JudgeClient>,
    model_id: String,
    greeting_pool: Vec<String>,
    max_turns: u32,
    goal_completion_signal: Option<String>,
    greeting_cursor: AtomicUsize,
}

impl ActorSimulator {
    /// Default turn cap if callers do not override it.
    pub const DEFAULT_MAX_TURNS: u32 = 10;

    #[must_use]
    pub fn new(
        profile: ActorProfile,
        judge: Arc<dyn JudgeClient>,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            profile,
            judge,
            model_id: model_id.into(),
            greeting_pool: vec!["Hello.".to_string()],
            max_turns: Self::DEFAULT_MAX_TURNS,
            goal_completion_signal: None,
            greeting_cursor: AtomicUsize::new(0),
        }
    }

    /// Override the greeting pool. Empty input is coerced to a single default.
    #[must_use]
    pub fn with_greeting_pool(mut self, pool: Vec<String>) -> Self {
        self.greeting_pool = if pool.is_empty() {
            vec!["Hello.".to_string()]
        } else {
            pool
        };
        self
    }

    #[must_use]
    pub const fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }

    #[must_use]
    pub fn with_goal_completion_signal(mut self, signal: impl Into<String>) -> Self {
        self.goal_completion_signal = Some(signal.into());
        self
    }

    #[must_use]
    pub const fn profile(&self) -> &ActorProfile {
        &self.profile
    }

    #[must_use]
    pub const fn max_turns(&self) -> u32 {
        self.max_turns
    }

    #[must_use]
    pub fn goal_completion_signal(&self) -> Option<&str> {
        self.goal_completion_signal.as_deref()
    }

    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Produce the opening user turn, rotating through `greeting_pool`.
    pub fn greeting(&self) -> ActorTurn {
        let idx = self.greeting_cursor.fetch_add(1, Ordering::Relaxed);
        let message = self.greeting_pool[idx % self.greeting_pool.len()].clone();
        ActorTurn {
            message,
            goal_completed: None,
        }
    }

    /// Produce the next user turn in response to an assistant message.
    pub async fn next_turn(&self, assistant_message: &str) -> Result<ActorTurn, JudgeError> {
        let prompt = self.render_prompt(assistant_message);
        let verdict = self.judge.judge(&prompt).await?;
        Ok(self.turn_from_verdict(verdict))
    }

    fn render_prompt(&self, assistant_message: &str) -> String {
        let mut prompt = self.profile.as_system_prompt();
        prompt.push_str("Assistant said: ");
        prompt.push_str(assistant_message);
        prompt.push('\n');
        prompt.push_str("Reply with your next message. ");
        if let Some(signal) = &self.goal_completion_signal {
            prompt.push_str(&format!(
                "If the goal is complete, reply with label `{signal}`."
            ));
        }
        prompt
    }

    fn turn_from_verdict(&self, verdict: JudgeVerdict) -> ActorTurn {
        let goal_completed = match (&verdict.label, &self.goal_completion_signal) {
            (Some(label), Some(signal)) if label == signal => Some(signal.clone()),
            _ => None,
        };
        ActorTurn {
            message: verdict.reason.unwrap_or_else(|| "…".to_string()),
            goal_completed,
        }
    }
}

impl std::fmt::Debug for ActorSimulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorSimulator")
            .field("profile", &self.profile)
            .field("max_turns", &self.max_turns)
            .field("goal_completion_signal", &self.goal_completion_signal)
            .finish_non_exhaustive()
    }
}
