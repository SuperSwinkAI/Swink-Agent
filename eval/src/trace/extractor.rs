//! `EvaluationLevel` and the `TraceExtractor` trait (spec 043 FR-033).
//!
//! Evaluators work at one of three granularities:
//!
//! * [`EvaluationLevel::Tool`] — per tool-call judgement (one input per
//!   recorded tool invocation).
//! * [`EvaluationLevel::Trace`] — per trajectory (a single input covering
//!   the full recorded run).
//! * [`EvaluationLevel::Session`] — per aggregated session, potentially
//!   spanning multiple invocations (multi-agent swarms, graph runs).
//!
//! A [`TraceExtractor`] yields a vector of [`ExtractedInput`]s shaped for
//! the consuming evaluator family. Concrete extractors
//! (`SwarmExtractor`, `GraphExtractor`, tool-level extractors) follow in
//! tasks T132–T134; this module only defines the trait and the minimal
//! input payload so downstream code has a stable compile target.

use std::sync::Arc;

use crate::types::{Invocation, RecordedToolCall, TurnRecord};

/// Granularity at which an evaluator operates (FR-033).
///
/// Serde-ready so evaluation configs can name a level in YAML.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationLevel {
    /// One unit of input per individual tool call.
    Tool,
    /// One unit of input covering the full trajectory of a single
    /// invocation.
    Trace,
    /// One unit of input covering an entire multi-invocation session
    /// (e.g. swarm / graph runs).
    Session,
}

/// Unit of input produced by a [`TraceExtractor`] for an evaluator.
///
/// The variant selected MUST match the requested `EvaluationLevel`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ExtractedInput {
    /// A single tool call belonging to `turn_index` within the source
    /// invocation.
    Tool {
        /// Zero-based index of the turn the tool call was executed in.
        turn_index: usize,
        /// The recorded tool call payload.
        call: RecordedToolCall,
    },
    /// The entire invocation, passed through as-is.
    Trace(Box<Invocation>),
    /// A session-level payload carrying every turn of an invocation.
    Session {
        /// All turns that make up the session, in chronological order.
        turns: Vec<TurnRecord>,
    },
}

impl ExtractedInput {
    /// Level of granularity this input represents.
    #[must_use]
    pub fn level(&self) -> EvaluationLevel {
        match self {
            Self::Tool { .. } => EvaluationLevel::Tool,
            Self::Trace(_) => EvaluationLevel::Trace,
            Self::Session { .. } => EvaluationLevel::Session,
        }
    }
}

/// Extract one or more `ExtractedInput`s from an `Invocation` at a given
/// `EvaluationLevel` (FR-033).
///
/// Implementations MUST be deterministic: equal `Invocation` + `level`
/// inputs MUST produce equal output vectors (required for cache-key stability
/// and SC-008 replay-equality).
pub trait TraceExtractor: Send + Sync {
    /// Produce inputs of the requested granularity from `inv`.
    fn extract(&self, inv: &Invocation, level: EvaluationLevel) -> Vec<ExtractedInput>;
}

// ─── Tool-level helper ──────────────────────────────────────────────────────

/// Tool-level [`TraceExtractor`] (FR-033 default).
///
/// Emits one `ExtractedInput::Tool` per recorded tool call in an
/// invocation. At the `Trace` and `Session` levels it falls through to
/// the invocation-wide payload so evaluators that want every
/// granularity wired up still receive a non-empty vector.
#[derive(Debug, Default, Clone, Copy)]
pub struct ToolLevelExtractor;

impl TraceExtractor for ToolLevelExtractor {
    fn extract(&self, inv: &Invocation, level: EvaluationLevel) -> Vec<ExtractedInput> {
        match level {
            EvaluationLevel::Tool => inv
                .turns
                .iter()
                .flat_map(|turn| {
                    turn.tool_calls
                        .iter()
                        .cloned()
                        .map(move |call| ExtractedInput::Tool {
                            turn_index: turn.turn_index,
                            call,
                        })
                })
                .collect(),
            EvaluationLevel::Trace => vec![ExtractedInput::Trace(Box::new(inv.clone()))],
            EvaluationLevel::Session => vec![ExtractedInput::Session {
                turns: inv.turns.clone(),
            }],
        }
    }
}

// ─── Swarm-aware extractor (T132) ──────────────────────────────────────────

const TRANSFER_TO_AGENT_TOOL: &str = "transfer_to_agent";

/// Swarm-aware [`TraceExtractor`] (T132).
///
/// Consumes spec-040 swarm / handoff result shapes by detecting
/// `transfer_to_agent` boundaries inside an [`Invocation`] and emitting
/// one session-level input per cohort of turns.
///
/// * `EvaluationLevel::Session` — groups turns into cohorts split at
///   every turn that invoked the `transfer_to_agent` tool. The cohort is
///   inclusive (the transferring turn is attached to the caller's group)
///   so evaluators can inspect the handoff reason before it fires.
/// * `EvaluationLevel::Trace` — falls through to the canonical invocation.
/// * `EvaluationLevel::Tool` — emits one input per tool call, matching
///   [`ToolLevelExtractor`] (so swarm-aware evaluators can still run
///   tool-level rubrics).
///
/// The tool-name predicate is configurable via [`Self::with_handoff_tool`]
/// in case a deployment renames the transfer tool.
#[derive(Debug, Clone)]
pub struct SwarmExtractor {
    handoff_tool: String,
}

impl Default for SwarmExtractor {
    fn default() -> Self {
        Self {
            handoff_tool: TRANSFER_TO_AGENT_TOOL.to_string(),
        }
    }
}

impl SwarmExtractor {
    /// Build with the default `transfer_to_agent` handoff name.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the tool name that signals a handoff boundary.
    #[must_use]
    pub fn with_handoff_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.handoff_tool = tool_name.into();
        self
    }

    /// Borrow the handoff-tool name currently configured.
    #[must_use]
    pub fn handoff_tool(&self) -> &str {
        &self.handoff_tool
    }
}

impl TraceExtractor for SwarmExtractor {
    fn extract(&self, inv: &Invocation, level: EvaluationLevel) -> Vec<ExtractedInput> {
        match level {
            EvaluationLevel::Tool => ToolLevelExtractor.extract(inv, level),
            EvaluationLevel::Trace => vec![ExtractedInput::Trace(Box::new(inv.clone()))],
            EvaluationLevel::Session => {
                let mut out: Vec<ExtractedInput> = Vec::new();
                let mut cohort: Vec<crate::types::TurnRecord> = Vec::new();
                for turn in &inv.turns {
                    cohort.push(turn.clone());
                    let fires_handoff = turn
                        .tool_calls
                        .iter()
                        .any(|call| call.name == self.handoff_tool);
                    if fires_handoff && !cohort.is_empty() {
                        out.push(ExtractedInput::Session {
                            turns: std::mem::take(&mut cohort),
                        });
                    }
                }
                if !cohort.is_empty() {
                    out.push(ExtractedInput::Session { turns: cohort });
                }
                // Even an invocation without a handoff produces one session
                // input so consumers always see a non-empty vector.
                if out.is_empty() && !inv.turns.is_empty() {
                    out.push(ExtractedInput::Session {
                        turns: inv.turns.clone(),
                    });
                }
                out
            }
        }
    }
}

// ─── Graph extractor (T133) ────────────────────────────────────────────────

/// [`TraceExtractor`] that consumes spec-039 graph / pipeline result shapes.
///
/// Spec 039 `PipelineOutput` lives in the `patterns` crate and isn't a
/// direct dependency here (SC-009 keeps the eval default dep graph tight),
/// so this extractor reads graph topology from the fields already present
/// on [`Invocation`]: each [`crate::types::TurnRecord`] is attributed to
/// the assistant that produced it, and distinct `assistant_message.model_id`
/// values map one-to-one with graph nodes in practice.
///
/// * `EvaluationLevel::Session` — groups consecutive turns whose
///   `model_id` matches; changing `model_id` starts a new node-level
///   session input.
/// * `EvaluationLevel::Trace` — falls through to the canonical invocation.
/// * `EvaluationLevel::Tool` — one input per tool call, like
///   [`ToolLevelExtractor`].
///
/// Tests can override the grouping predicate via
/// [`Self::with_node_key`].
#[derive(Clone)]
pub struct GraphExtractor {
    node_key: Arc<dyn Fn(&crate::types::TurnRecord) -> String + Send + Sync>,
}

impl std::fmt::Debug for GraphExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphExtractor").finish_non_exhaustive()
    }
}

impl GraphExtractor {
    /// Build an extractor that partitions turns by
    /// `assistant_message.model_id` (the default graph-node key).
    #[must_use]
    pub fn new() -> Self {
        Self {
            node_key: Arc::new(|turn| turn.assistant_message.model_id.clone()),
        }
    }

    /// Override the function that maps a turn to a graph-node key.
    ///
    /// Consumers with custom instrumentation can split by agent name,
    /// a metadata attribute, or any other deterministic discriminator.
    #[must_use]
    pub fn with_node_key<F>(key: F) -> Self
    where
        F: Fn(&crate::types::TurnRecord) -> String + Send + Sync + 'static,
    {
        Self {
            node_key: Arc::new(key),
        }
    }
}

impl Default for GraphExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceExtractor for GraphExtractor {
    fn extract(&self, inv: &Invocation, level: EvaluationLevel) -> Vec<ExtractedInput> {
        use crate::types::TurnRecord;

        match level {
            EvaluationLevel::Tool => ToolLevelExtractor.extract(inv, level),
            EvaluationLevel::Trace => vec![ExtractedInput::Trace(Box::new(inv.clone()))],
            EvaluationLevel::Session => {
                let mut out: Vec<ExtractedInput> = Vec::new();
                let mut cohort: Vec<TurnRecord> = Vec::new();
                let mut current_key: Option<String> = None;
                for turn in &inv.turns {
                    let key = (self.node_key)(turn);
                    match &current_key {
                        Some(k) if k == &key => {
                            cohort.push(turn.clone());
                        }
                        _ => {
                            if !cohort.is_empty() {
                                out.push(ExtractedInput::Session {
                                    turns: std::mem::take(&mut cohort),
                                });
                            }
                            current_key = Some(key);
                            cohort.push(turn.clone());
                        }
                    }
                }
                if !cohort.is_empty() {
                    out.push(ExtractedInput::Session { turns: cohort });
                }
                out
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_level_serde_round_trip() {
        let yaml_like = serde_json::to_string(&EvaluationLevel::Trace).unwrap();
        assert_eq!(yaml_like, "\"trace\"");
        let back: EvaluationLevel = serde_json::from_str(&yaml_like).unwrap();
        assert_eq!(back, EvaluationLevel::Trace);
    }

    #[test]
    fn extracted_input_level_matches_variant() {
        let call = RecordedToolCall {
            id: "id".into(),
            name: "n".into(),
            arguments: serde_json::Value::Null,
        };
        assert_eq!(
            ExtractedInput::Tool {
                turn_index: 0,
                call
            }
            .level(),
            EvaluationLevel::Tool
        );
        assert_eq!(
            ExtractedInput::Session { turns: vec![] }.level(),
            EvaluationLevel::Session
        );
    }
}
