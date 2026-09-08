//! Tests for `yaml`.
#![cfg(test)]

use std::io::Write;

use tempfile::NamedTempFile;

use super::*;

#[test]
fn yaml_loader_rejects_duplicate_environment_state_names() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(
            f,
            "id: set\nname: Set\ncases:\n  - id: c1\n    name: C1\n    system_prompt: \"\"\n    user_messages: [\"hi\"]\n    expected_environment_state:\n      - name: alpha\n        state: {{a: 1}}\n      - name: alpha\n        state: {{a: 2}}\n"
        )
        .unwrap();

    let err = load_eval_set_yaml(f.path()).expect_err("duplicate should be rejected");
    match err {
        EvalError::InvalidCase { reason } => {
            assert!(reason.contains("alpha"), "reason: {reason}");
            assert!(reason.contains("c1"), "reason: {reason}");
        }
        other => panic!("expected InvalidCase, got {other:?}"),
    }
}

#[test]
fn yaml_loader_accepts_unique_environment_state_names() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(
            f,
            "id: set\nname: Set\ncases:\n  - id: c1\n    name: C1\n    system_prompt: \"\"\n    user_messages: [\"hi\"]\n    expected_environment_state:\n      - name: alpha\n        state: {{a: 1}}\n      - name: beta\n        state: {{a: 2}}\n"
        )
        .unwrap();

    let set = load_eval_set_yaml(f.path()).expect("unique names should load");
    assert_eq!(set.cases.len(), 1);
    assert_eq!(
        set.cases[0]
            .expected_environment_state
            .as_ref()
            .unwrap()
            .len(),
        2
    );
}
