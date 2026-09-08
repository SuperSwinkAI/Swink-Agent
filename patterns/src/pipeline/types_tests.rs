//! Tests for `types`.
#![cfg(test)]

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
