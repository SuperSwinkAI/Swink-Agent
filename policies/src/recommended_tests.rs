//! Tests for `recommended`.
#![cfg(test)]

use std::sync::Arc;

use swink_agent::StreamFn;
use swink_agent::testing::{MockStreamFn, default_model};

use super::*;

fn bare_options() -> AgentOptions {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![]));
    AgentOptions::new_simple("test", default_model(), stream_fn)
}

#[test]
fn defaults_are_sensible() {
    let preset = RecommendedPolicies::builder();
    assert!((preset.max_cost - RecommendedPolicies::DEFAULT_MAX_COST).abs() < f64::EPSILON);
    assert_eq!(preset.max_turns, RecommendedPolicies::DEFAULT_MAX_TURNS);
    assert_eq!(preset.sandbox_root, PathBuf::from("."));
    assert_eq!(preset.denied_tools, vec!["bash".to_string()]);
    assert!(preset.max_input_tokens.is_none());
    assert!(preset.max_output_tokens.is_none());
}

#[test]
fn builder_overrides_work() {
    let preset = RecommendedPolicies::builder()
        .with_max_cost(2.5)
        .with_max_input_tokens(100_000)
        .with_max_output_tokens(50_000)
        .with_max_turns(7)
        .with_sandbox_root("/srv/workspace")
        .with_deny_tools(["bash", "write_file"]);
    assert!((preset.max_cost - 2.5).abs() < f64::EPSILON);
    assert_eq!(preset.max_input_tokens, Some(100_000));
    assert_eq!(preset.max_output_tokens, Some(50_000));
    assert_eq!(preset.max_turns, 7);
    assert_eq!(preset.sandbox_root, PathBuf::from("/srv/workspace"));
    assert_eq!(
        preset.denied_tools,
        vec!["bash".to_string(), "write_file".to_string()]
    );
}

#[test]
fn apply_wires_all_four_policies() {
    let options = RecommendedPolicies::builder().apply(bare_options());

    let pre_turn_names: Vec<&str> = options.pre_turn_policies.iter().map(|p| p.name()).collect();
    assert_eq!(pre_turn_names, vec![BUDGET_NAME, MAX_TURNS_NAME]);

    let pre_dispatch_names: Vec<&str> = options
        .pre_dispatch_policies
        .iter()
        .map(|p| p.name())
        .collect();
    assert_eq!(pre_dispatch_names, vec![SANDBOX_NAME, DENY_LIST_NAME]);

    assert!(options.post_turn_policies.is_empty());
    assert!(options.post_loop_policies.is_empty());
}

#[test]
fn apply_appends_without_removing_existing_policies() {
    let options = bare_options().with_pre_turn_policy(MaxTurnsPolicy::new(3));
    let options = RecommendedPolicies::builder().apply(options);
    assert_eq!(options.pre_turn_policies.len(), 3);
    assert_eq!(options.pre_turn_policies[0].name(), MAX_TURNS_NAME);
}

#[test]
fn library_default_remains_anything_goes() {
    let options = bare_options();
    assert!(options.pre_turn_policies.is_empty());
    assert!(options.pre_dispatch_policies.is_empty());
    assert!(options.post_turn_policies.is_empty());
    assert!(options.post_loop_policies.is_empty());
}

#[test]
fn contract_passes_on_preset_wiring() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let options = RecommendedPolicies::builder()
        .with_sandbox_root(tempdir.path())
        .apply(bare_options());
    assert!(verify_production_guardrails(&options, "bash").is_ok());
}

#[test]
fn contract_reports_all_missing_policies() {
    let violations = verify_production_guardrails(&bare_options(), "bash").expect_err("must fail");
    assert_eq!(violations.len(), 4);
    assert!(violations.iter().any(|v| v.contains(BUDGET_NAME)));
    assert!(violations.iter().any(|v| v.contains(MAX_TURNS_NAME)));
    assert!(violations.iter().any(|v| v.contains(SANDBOX_NAME)));
    assert!(violations.iter().any(|v| v.contains(DENY_LIST_NAME)));
}

#[test]
fn contract_rejects_budget_without_limits() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let trivial = bare_options()
        .with_pre_turn_policy(BudgetPolicy::new())
        .with_pre_turn_policy(MaxTurnsPolicy::new(10))
        .with_pre_dispatch_policy(SandboxPolicy::new(tempdir.path()))
        .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash"]));
    let violations = verify_production_guardrails(&trivial, "bash").expect_err("must fail");
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains(BUDGET_NAME));
    assert!(violations[0].contains("no effective limit"));
}

#[test]
fn contract_rejects_deny_list_missing_expected_tool() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let options = RecommendedPolicies::builder()
        .with_sandbox_root(tempdir.path())
        .with_deny_tools(["write_file"])
        .apply(bare_options());
    let violations = verify_production_guardrails(&options, "bash").expect_err("must fail");
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("does not deny tool 'bash'"));
}

#[test]
#[should_panic(expected = "production guardrail contract violated")]
fn assert_helper_panics_on_violation() {
    assert_production_guardrails(&bare_options(), "bash");
}
