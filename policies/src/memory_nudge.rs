//! Memory nudge policy — heuristic detection of save-worthy content in agent turns.
//!
//! [`MemoryNudgePolicy`] implements [`PostTurnPolicy`] and scans the assistant message
//! text for four categories of save-worthy content:
//!
//! - **Correction** — user corrections ("no, actually…", "don't do X", "use Y instead")
//! - **ExplicitSave** — direct save requests ("remember this", "note that", "keep in mind")
//! - **Decision** — decision statements ("we decided to…", "the plan is…")
//! - **Preference** — configuration/preference declarations ("I prefer", "always use")
//!
//! When a match is found above the configured sensitivity threshold, the policy returns
//! [`PolicyVerdict::Inject`] carrying a single [`AgentMessage`] with a
//! `ContentBlock::Extension { type_name: "memory_nudge", data: <JSON MemoryNudge> }`.
//! The caller is responsible for consuming the extension block and persisting it.
//!
//! # Feature gate
//!
//! This module is only compiled when the `memory-nudge` feature is enabled.
//!
//! # Example
//!
//! ```rust,ignore
//! use swink_agent_policies::{MemoryNudgePolicy, NudgeSensitivity};
//! use std::sync::Arc;
//!
//! let policy = Arc::new(
//!     MemoryNudgePolicy::new().with_sensitivity(NudgeSensitivity::High),
//! );
//! // Add to agent's post_turn_policies slot:
//! // options.with_post_turn_policy(policy)
//! ```

#![forbid(unsafe_code)]
use swink_agent::{
    AgentMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict, PostTurnPolicy,
    TurnPolicyContext, UserMessage,
};

// ─── Category ───────────────────────────────────────────────────────────────

/// The category of save-worthy content detected by [`MemoryNudgePolicy`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryNudgeCategory {
    /// A user correction — the user (or assistant relaying a correction) clarifies
    /// that a previous assumption, approach, or fact was wrong.
    ///
    /// Example phrases: "no, actually", "don't do that", "use X instead".
    Correction,

    /// An explicit request to save information for future recall.
    ///
    /// Example phrases: "remember this", "note that", "keep in mind".
    ExplicitSave,

    /// A decision or plan statement about how to proceed.
    ///
    /// Example phrases: "we decided to", "the plan is", "going forward".
    Decision,

    /// A configuration or style preference the user has stated.
    ///
    /// Example phrases: "I prefer", "always use", "my preference is".
    Preference,
}

impl MemoryNudgeCategory {
    /// Return a stable lowercase string name for serialization.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Correction => "correction",
            Self::ExplicitSave => "explicit_save",
            Self::Decision => "decision",
            Self::Preference => "preference",
        }
    }
}

// ─── Nudge payload ──────────────────────────────────────────────────────────

/// A structured payload emitted when save-worthy content is detected in a turn.
///
/// Serialized as JSON inside a `ContentBlock::Extension { type_name: "memory_nudge" }`.
/// Callers should deserialize the `data` field to recover this struct, e.g. via
/// `serde_json::from_value::<MemoryNudge>(data)`.
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryNudge {
    /// Which category of save-worthy content was detected.
    pub category: MemoryNudgeCategory,
    /// A short summary of the detected content (≤ 200 characters).
    pub summary: String,
    /// Heuristic confidence score in [0.0, 1.0].
    pub confidence: f32,
    /// Zero-based index of the turn in which the nudge was detected.
    pub turn_number: usize,
}

impl MemoryNudge {
    /// Create a `MemoryNudge` from its parts.
    ///
    /// `summary` should be a short excerpt (≤ 200 characters) of the detected
    /// content and `confidence` a heuristic score in `[0.0, 1.0]`; neither is
    /// clamped here — [`MemoryNudgePolicy`] truncates and scores before
    /// constructing.
    #[must_use]
    pub fn new(
        category: MemoryNudgeCategory,
        summary: impl Into<String>,
        confidence: f32,
        turn_number: usize,
    ) -> Self {
        Self {
            category,
            summary: summary.into(),
            confidence,
            turn_number,
        }
    }

    /// Serialize `self` to a [`serde_json::Value`] for embedding in a content block.
    ///
    /// The output round-trips through [`serde_json::from_value`] back into a
    /// `MemoryNudge`.
    ///
    /// # Panics
    ///
    /// Panics if serialization fails, which cannot happen for this struct's
    /// field types.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("MemoryNudge serializes to JSON")
    }
}

// ─── Sensitivity ────────────────────────────────────────────────────────────

/// Controls how aggressively [`MemoryNudgePolicy`] flags borderline content.
///
/// Each level maps to a minimum confidence threshold: matches with a confidence
/// score strictly below the threshold are suppressed (no nudge emitted).
///
/// | Level  | Threshold | Behavior                                        |
/// |--------|-----------|-------------------------------------------------|
/// | Low    | 0.75      | Only high-confidence, unambiguous matches       |
/// | Medium | 0.55      | Balanced — default for most use cases           |
/// | High   | 0.35      | Catch borderline / partial matches too          |
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NudgeSensitivity {
    /// Only high-confidence matches (threshold 0.75).
    Low,
    /// Balanced detection — default (threshold 0.55).
    #[default]
    Medium,
    /// Aggressive detection — catches borderline patterns (threshold 0.35).
    High,
}

impl NudgeSensitivity {
    /// Return the minimum confidence score required to emit a nudge.
    pub const fn threshold(self) -> f32 {
        match self {
            Self::Low => 0.75,
            Self::Medium => 0.55,
            Self::High => 0.35,
        }
    }
}

// ─── Policy ─────────────────────────────────────────────────────────────────

/// A `PostTurnPolicy` that detects save-worthy content via heuristic pattern matching.
///
/// When a match is found above the configured [`NudgeSensitivity`] threshold,
/// the policy returns [`PolicyVerdict::Inject`] with an extension content block
/// (`type_name: "memory_nudge"`) containing a serialized [`MemoryNudge`].
///
/// The policy never returns `Stop` — it is purely additive and non-blocking.
/// Callers are responsible for consuming injected extension blocks.
///
/// # Example
///
/// ```rust,ignore
/// let policy = MemoryNudgePolicy::new().with_sensitivity(NudgeSensitivity::High);
/// ```
#[derive(Debug, Clone)]
pub struct MemoryNudgePolicy {
    sensitivity: NudgeSensitivity,
}

impl MemoryNudgePolicy {
    /// Create a `MemoryNudgePolicy` with [`NudgeSensitivity::Medium`] (default).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sensitivity: NudgeSensitivity::Medium,
        }
    }

    /// Set the sensitivity level.
    #[must_use]
    pub const fn with_sensitivity(mut self, sensitivity: NudgeSensitivity) -> Self {
        self.sensitivity = sensitivity;
        self
    }
}

impl Default for MemoryNudgePolicy {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Heuristic detectors ────────────────────────────────────────────────────

/// Phrase patterns for each category.
///
/// Each entry is a `(phrase, base_confidence)` pair. The policy does a
/// case-insensitive substring search; the first hit wins for that category.
const CORRECTION_PATTERNS: &[(&str, f32)] = &[
    ("no, actually", 0.90),
    ("don't do that", 0.85),
    ("don't do this", 0.85),
    ("use x instead", 0.80),
    ("use y instead", 0.80),
    ("instead of that", 0.75),
    ("that's wrong", 0.80),
    ("that is wrong", 0.80),
    ("incorrect, ", 0.78),
    ("not quite right", 0.75),
    ("actually, ", 0.60),
    ("rather than that", 0.65),
    ("instead, use", 0.70),
    ("you should use", 0.65),
    ("please use", 0.60),
];

const EXPLICIT_SAVE_PATTERNS: &[(&str, f32)] = &[
    ("remember this", 0.95),
    ("remember that", 0.90),
    ("note that", 0.85),
    ("keep in mind", 0.85),
    ("make a note", 0.90),
    ("save this", 0.90),
    ("don't forget", 0.80),
    ("important to remember", 0.85),
    ("worth noting", 0.80),
    ("for future reference", 0.85),
    ("bear in mind", 0.80),
    ("take note", 0.85),
];

const DECISION_PATTERNS: &[(&str, f32)] = &[
    ("we decided to", 0.90),
    ("we've decided to", 0.90),
    ("we have decided to", 0.90),
    ("the plan is", 0.85),
    ("the decision is", 0.88),
    ("going forward, ", 0.80),
    ("going forward we", 0.80),
    ("from now on", 0.80),
    ("we will use", 0.70),
    ("we are going to", 0.72),
    ("we agreed to", 0.85),
    ("we've agreed to", 0.85),
    ("the approach is", 0.75),
    ("our decision:", 0.88),
    ("decided on", 0.75),
];

const PREFERENCE_PATTERNS: &[(&str, f32)] = &[
    ("i prefer ", 0.90),
    ("my preference is", 0.92),
    ("always use", 0.80),
    ("i like to use", 0.82),
    ("i want to use", 0.78),
    ("please always", 0.75),
    ("i always use", 0.82),
    ("i'd like to use", 0.80),
    ("i would like to use", 0.80),
    ("my style is", 0.85),
    ("my convention is", 0.85),
    ("i use ", 0.55),
    ("our convention is", 0.82),
    ("our style is", 0.82),
    ("prefer to use", 0.80),
];

/// Detect a correction in `text`. Returns `Some(confidence)` on first match.
fn detect_correction(text: &str) -> Option<f32> {
    detect_any(text, CORRECTION_PATTERNS)
}

/// Detect an explicit save request in `text`. Returns `Some(confidence)` on first match.
fn detect_explicit_save(text: &str) -> Option<f32> {
    detect_any(text, EXPLICIT_SAVE_PATTERNS)
}

/// Detect a decision statement in `text`. Returns `Some(confidence)` on first match.
fn detect_decision(text: &str) -> Option<f32> {
    detect_any(text, DECISION_PATTERNS)
}

/// Detect a preference declaration in `text`. Returns `Some(confidence)` on first match.
fn detect_preference(text: &str) -> Option<f32> {
    detect_any(text, PREFERENCE_PATTERNS)
}

/// Case-insensitive substring search across a slice of `(phrase, confidence)` pairs.
fn detect_any(text: &str, patterns: &[(&str, f32)]) -> Option<f32> {
    let lower = text.to_lowercase();
    for (phrase, confidence) in patterns {
        if lower.contains(*phrase) {
            return Some(*confidence);
        }
    }
    None
}

/// Truncate `text` to at most `max_chars` Unicode scalar values.
fn truncate_summary(text: &str, max_chars: usize) -> String {
    let mut chars = text.char_indices();
    if let Some((idx, _)) = chars.nth(max_chars) {
        format!("{}…", &text[..idx])
    } else {
        text.to_string()
    }
}

/// Build a `MemoryNudge` and wrap it in an `AgentMessage` extension block.
fn nudge_message(
    category: MemoryNudgeCategory,
    summary: &str,
    confidence: f32,
    turn_number: usize,
) -> AgentMessage {
    let nudge = MemoryNudge::new(
        category,
        truncate_summary(summary, 200),
        confidence,
        turn_number,
    );
    let data = nudge.to_json();
    // Embed the nudge as an extension block inside a user message so it can
    // be stored in the message history without being forwarded to the LLM
    // (extension blocks are stripped during message conversion).
    AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Extension {
            type_name: "memory_nudge".to_string(),
            data,
        }])
        .with_timestamp(0),
    ))
}

// ─── PostTurnPolicy impl ─────────────────────────────────────────────────────

impl PostTurnPolicy for MemoryNudgePolicy {
    fn name(&self) -> &str {
        "memory-nudge"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let text = ContentBlock::extract_text(&turn.assistant_message.content);
        if text.is_empty() {
            return PolicyVerdict::Continue;
        }

        let threshold = self.sensitivity.threshold();
        let mut messages: Vec<AgentMessage> = Vec::new();

        // Check each category in priority order; emit one nudge per matching category.
        type Detector = fn(&str) -> Option<f32>;
        let detectors: &[(Detector, MemoryNudgeCategory)] = &[
            (detect_correction, MemoryNudgeCategory::Correction),
            (detect_explicit_save, MemoryNudgeCategory::ExplicitSave),
            (detect_decision, MemoryNudgeCategory::Decision),
            (detect_preference, MemoryNudgeCategory::Preference),
        ];

        for (detector, category) in detectors {
            if let Some(confidence) = detector(&text)
                && confidence >= threshold
            {
                messages.push(nudge_message(
                    category.clone(),
                    &text,
                    confidence,
                    ctx.turn_index,
                ));
            }
        }

        if messages.is_empty() {
            PolicyVerdict::Continue
        } else {
            PolicyVerdict::Inject(messages)
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "memory_nudge_tests.rs"]
mod tests;
