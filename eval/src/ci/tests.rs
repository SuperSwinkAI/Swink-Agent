//! Tests for `mod`.
#![cfg(test)]

use super::*;
use serde_yaml_ng::Value;

#[test]
fn every_template_is_non_empty() {
    for (name, body) in TEMPLATES {
        assert!(!body.is_empty(), "template {name} is empty");
    }
}

#[test]
fn every_template_parses_as_yaml() {
    for (name, body) in TEMPLATES {
        let parsed: Value = serde_yaml_ng::from_str(body)
            .unwrap_or_else(|err| panic!("{name} is invalid YAML: {err}"));
        assert!(
            parsed.is_mapping(),
            "template {name} should parse to a YAML mapping"
        );
    }
}

#[test]
fn pr_template_references_subcommands() {
    assert!(PR_EVAL_TEMPLATE.contains("swink-eval run"));
    assert!(PR_EVAL_TEMPLATE.contains("swink-eval gate"));
}
