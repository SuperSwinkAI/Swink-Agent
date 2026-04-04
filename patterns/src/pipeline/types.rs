//! Core pipeline types: PipelineId, Pipeline, MergeStrategy, ExitCondition.

use std::fmt;

use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── PipelineId ─────────────────────────────────────────────────────────────

/// Unique identifier for a pipeline definition.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipelineId(String);

impl PipelineId {
    /// Create a pipeline ID from a string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Generate a unique pipeline ID using UUID v4.
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl fmt::Display for PipelineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ─── MergeStrategy ──────────────────────────────────────────────────────────

/// Controls how parallel branch outputs are combined.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MergeStrategy {
    /// Join all outputs in declaration order with a separator.
    Concat { separator: String },
    /// Return the first branch to complete.
    First,
    /// Return the first N branches to complete.
    Fastest { n: usize },
    /// Pass all outputs to a named aggregator agent.
    Custom { aggregator: String },
}

// ─── ExitCondition ──────────────────────────────────────────────────────────

/// Controls when a loop pipeline terminates.
#[derive(Clone, Debug)]
pub enum ExitCondition {
    /// Exit when the body agent invokes the named tool.
    ToolCalled { tool_name: String },
    /// Exit when the output matches the regex pattern.
    OutputContains {
        pattern: String,
        #[allow(dead_code)]
        compiled: Regex,
    },
    /// Always run to the max_iterations cap.
    MaxIterations,
}

impl ExitCondition {
    /// Create an `OutputContains` condition, eagerly validating the regex.
    ///
    /// Returns `Err` if the pattern is not a valid regex.
    pub fn output_contains(pattern: impl Into<String>) -> Result<Self, String> {
        let pattern = pattern.into();
        let compiled =
            Regex::new(&pattern).map_err(|e| format!("invalid regex '{pattern}': {e}"))?;
        Ok(Self::OutputContains { pattern, compiled })
    }
}

impl Serialize for ExitCondition {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(tag = "type")]
        enum Helper<'a> {
            ToolCalled { tool_name: &'a str },
            OutputContains { pattern: &'a str },
            MaxIterations,
        }

        match self {
            Self::ToolCalled { tool_name } => {
                Helper::ToolCalled { tool_name }.serialize(serializer)
            }
            Self::OutputContains { pattern, .. } => {
                Helper::OutputContains { pattern }.serialize(serializer)
            }
            Self::MaxIterations => Helper::MaxIterations.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ExitCondition {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum Helper {
            ToolCalled { tool_name: String },
            OutputContains { pattern: String },
            MaxIterations,
        }

        let h = Helper::deserialize(deserializer)?;
        match h {
            Helper::ToolCalled { tool_name } => Ok(Self::ToolCalled { tool_name }),
            Helper::OutputContains { pattern } => {
                let compiled = Regex::new(&pattern).map_err(serde::de::Error::custom)?;
                Ok(Self::OutputContains { pattern, compiled })
            }
            Helper::MaxIterations => Ok(Self::MaxIterations),
        }
    }
}

// ─── Pipeline ───────────────────────────────────────────────────────────────

/// A pipeline definition describing how to compose multiple agents.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Pipeline {
    /// Execute agents in declared order, passing output forward.
    Sequential {
        id: PipelineId,
        name: String,
        steps: Vec<String>,
        pass_context: bool,
    },
    /// Execute agents concurrently and merge results.
    Parallel {
        id: PipelineId,
        name: String,
        branches: Vec<String>,
        merge_strategy: MergeStrategy,
    },
    /// Execute an agent repeatedly until an exit condition is met.
    Loop {
        id: PipelineId,
        name: String,
        body: String,
        exit_condition: ExitCondition,
        max_iterations: usize,
    },
}

impl Pipeline {
    /// Create a sequential pipeline without context passing.
    pub fn sequential(name: impl Into<String>, steps: Vec<String>) -> Self {
        Self::Sequential {
            id: PipelineId::generate(),
            name: name.into(),
            steps,
            pass_context: false,
        }
    }

    /// Create a sequential pipeline with context passing enabled.
    pub fn sequential_with_context(name: impl Into<String>, steps: Vec<String>) -> Self {
        Self::Sequential {
            id: PipelineId::generate(),
            name: name.into(),
            steps,
            pass_context: true,
        }
    }

    /// Create a parallel pipeline.
    pub fn parallel(
        name: impl Into<String>,
        branches: Vec<String>,
        merge_strategy: MergeStrategy,
    ) -> Self {
        Self::Parallel {
            id: PipelineId::generate(),
            name: name.into(),
            branches,
            merge_strategy,
        }
    }

    /// Create a loop pipeline.
    pub fn loop_(
        name: impl Into<String>,
        body: impl Into<String>,
        exit_condition: ExitCondition,
    ) -> Self {
        Self::Loop {
            id: PipelineId::generate(),
            name: name.into(),
            body: body.into(),
            exit_condition,
            max_iterations: 10,
        }
    }

    /// Create a loop pipeline with a custom max iterations cap.
    pub fn loop_with_max(
        name: impl Into<String>,
        body: impl Into<String>,
        exit_condition: ExitCondition,
        max_iterations: usize,
    ) -> Self {
        Self::Loop {
            id: PipelineId::generate(),
            name: name.into(),
            body: body.into(),
            exit_condition,
            max_iterations,
        }
    }

    /// Override the auto-generated ID.
    #[must_use]
    pub fn with_id(mut self, id: PipelineId) -> Self {
        match &mut self {
            Self::Sequential { id: i, .. }
            | Self::Parallel { id: i, .. }
            | Self::Loop { id: i, .. } => *i = id,
        }
        self
    }

    /// Returns the pipeline's unique identifier.
    pub fn id(&self) -> &PipelineId {
        match self {
            Self::Sequential { id, .. } | Self::Parallel { id, .. } | Self::Loop { id, .. } => id,
        }
    }

    /// Returns the pipeline's human-readable name.
    pub fn name(&self) -> &str {
        match self {
            Self::Sequential { name, .. }
            | Self::Parallel { name, .. }
            | Self::Loop { name, .. } => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // T014: PipelineId tests

    #[test]
    fn pipeline_id_new_and_display() {
        let id = PipelineId::new("test-pipeline");
        assert_eq!(id.to_string(), "test-pipeline");
    }

    #[test]
    fn pipeline_id_generate_is_unique() {
        let a = PipelineId::generate();
        let b = PipelineId::generate();
        assert_ne!(a, b);
    }

    #[test]
    fn pipeline_id_equality_and_hashing() {
        let a = PipelineId::new("same");
        let b = PipelineId::new("same");
        assert_eq!(a, b);

        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }

    #[test]
    fn pipeline_id_serde_roundtrip() {
        let id = PipelineId::new("round-trip");
        let json = serde_json::to_string(&id).unwrap();
        let parsed: PipelineId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    // T015: ExitCondition tests

    #[test]
    fn exit_condition_output_contains_valid_regex() {
        let cond = ExitCondition::output_contains(r"\bDONE\b").unwrap();
        match cond {
            ExitCondition::OutputContains { pattern, compiled } => {
                assert_eq!(pattern, r"\bDONE\b");
                assert!(compiled.is_match("task DONE here"));
            }
            _ => panic!("expected OutputContains"),
        }
    }

    #[test]
    fn exit_condition_output_contains_invalid_regex() {
        let result = ExitCondition::output_contains("[invalid");
        assert!(result.is_err());
    }

    #[test]
    fn exit_condition_serde_roundtrip_recompiles() {
        let cond = ExitCondition::output_contains(r"done|finished").unwrap();
        let json = serde_json::to_string(&cond).unwrap();
        let parsed: ExitCondition = serde_json::from_str(&json).unwrap();
        match parsed {
            ExitCondition::OutputContains { pattern, compiled } => {
                assert_eq!(pattern, "done|finished");
                assert!(compiled.is_match("all done"));
            }
            _ => panic!("expected OutputContains"),
        }
    }

    // T016: Pipeline constructor tests

    #[test]
    fn sequential_constructor() {
        let p = Pipeline::sequential("test", vec!["a".into(), "b".into()]);
        assert_eq!(p.name(), "test");
        match &p {
            Pipeline::Sequential {
                pass_context,
                steps,
                ..
            } => {
                assert!(!pass_context);
                assert_eq!(steps.len(), 2);
            }
            _ => panic!("expected Sequential"),
        }
    }

    #[test]
    fn parallel_constructor() {
        let p = Pipeline::parallel("par", vec!["x".into(), "y".into()], MergeStrategy::First);
        assert_eq!(p.name(), "par");
        assert!(matches!(p, Pipeline::Parallel { .. }));
    }

    #[test]
    fn loop_constructor() {
        let p = Pipeline::loop_("lp", "body-agent", ExitCondition::MaxIterations);
        assert_eq!(p.name(), "lp");
        match &p {
            Pipeline::Loop { max_iterations, .. } => assert_eq!(*max_iterations, 10),
            _ => panic!("expected Loop"),
        }
    }

    #[test]
    fn with_id_overrides_generated_id() {
        let custom = PipelineId::new("custom-id");
        let p = Pipeline::sequential("s", vec![]).with_id(custom.clone());
        assert_eq!(*p.id(), custom);
    }

    #[test]
    fn auto_generated_ids_are_unique() {
        let a = Pipeline::sequential("a", vec![]);
        let b = Pipeline::sequential("b", vec![]);
        assert_ne!(a.id(), b.id());
    }
}
