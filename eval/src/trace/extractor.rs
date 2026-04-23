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
