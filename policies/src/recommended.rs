//! Recommended production guardrails — a bundled preset of the four core
//! safety policies, plus a contract-test helper for embedders.
//!
//! `swink-agent` itself ships "anything-goes": no policy slots are populated
//! by default, and this module does not change that. It exists for embedders
//! that run agents autonomously in production and want a one-call way to wire
//! (and later verify) the recommended guardrail set:
//!
//! - [`BudgetPolicy`] (pre-turn) — cost/token ceiling
//! - [`MaxTurnsPolicy`] (pre-turn) — turn-count ceiling
//! - [`SandboxPolicy`] (pre-dispatch) — filesystem root restriction
//! - [`ToolDenyListPolicy`] (pre-dispatch) — blocked tool names
//!
//! # Example
//! ```rust,ignore
//! use swink_agent_policies::RecommendedPolicies;
//!
//! let options = RecommendedPolicies::builder()
//!     .with_max_cost(10.0)
//!     .with_max_turns(50)
//!     .with_sandbox_root("/srv/agent-workspace")
//!     .with_deny_tools(["bash"])
//!     .apply(options);
//! ```
#![forbid(unsafe_code)]

use std::path::PathBuf;

use swink_agent::{
    AgentOptions, Cost, PolicyContext, PolicyVerdict, PreDispatchVerdict, SessionState,
    ToolDispatchContext, Usage,
};

use crate::{BudgetPolicy, MaxTurnsPolicy, SandboxPolicy, ToolDenyListPolicy};

/// Canonical `name()` of [`BudgetPolicy`].
const BUDGET_NAME: &str = "budget";
/// Canonical `name()` of [`MaxTurnsPolicy`].
const MAX_TURNS_NAME: &str = "max_turns";
/// Canonical `name()` of [`SandboxPolicy`].
const SANDBOX_NAME: &str = "sandbox";
/// Canonical `name()` of [`ToolDenyListPolicy`].
const DENY_LIST_NAME: &str = "tool_deny_list";

/// A builder that bundles the four recommended production guardrails and
/// applies them to an [`AgentOptions`] in one call.
///
/// Every knob has a sensible default (see the associated constants), so the
/// minimal invocation is `RecommendedPolicies::builder().apply(options)`.
/// The library default remains anything-goes — nothing is wired unless you
/// call [`apply`](Self::apply).
#[derive(Debug, Clone)]
pub struct RecommendedPolicies {
    max_cost: f64,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    max_turns: usize,
    sandbox_root: PathBuf,
    denied_tools: Vec<String>,
}

impl RecommendedPolicies {
    /// Default cost ceiling in USD ([`BudgetPolicy::with_max_cost`]).
    pub const DEFAULT_MAX_COST: f64 = 10.0;
    /// Default turn ceiling ([`MaxTurnsPolicy`]).
    pub const DEFAULT_MAX_TURNS: usize = 50;
    /// Default denied tool names ([`ToolDenyListPolicy`]).
    pub const DEFAULT_DENIED_TOOLS: &'static [&'static str] = &["bash"];

    /// Start a builder with the recommended defaults.
    ///
    /// Defaults: max cost [`Self::DEFAULT_MAX_COST`] USD, no token limits,
    /// [`Self::DEFAULT_MAX_TURNS`] turns, sandbox rooted at the process
    /// working directory (`.`), and [`Self::DEFAULT_DENIED_TOOLS`] denied.
    #[must_use]
    pub fn builder() -> Self {
        Self {
            max_cost: Self::DEFAULT_MAX_COST,
            max_input_tokens: None,
            max_output_tokens: None,
            max_turns: Self::DEFAULT_MAX_TURNS,
            sandbox_root: PathBuf::from("."),
            denied_tools: Self::DEFAULT_DENIED_TOOLS
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }

    /// Set the maximum total cost in USD (default: [`Self::DEFAULT_MAX_COST`]).
    #[must_use]
    pub const fn with_max_cost(mut self, limit: f64) -> Self {
        self.max_cost = limit;
        self
    }

    /// Set a maximum accumulated input-token limit (default: none).
    #[must_use]
    pub const fn with_max_input_tokens(mut self, limit: u64) -> Self {
        self.max_input_tokens = Some(limit);
        self
    }

    /// Set a maximum accumulated output-token limit (default: none).
    #[must_use]
    pub const fn with_max_output_tokens(mut self, limit: u64) -> Self {
        self.max_output_tokens = Some(limit);
        self
    }

    /// Set the maximum number of turns (default: [`Self::DEFAULT_MAX_TURNS`]).
    #[must_use]
    pub const fn with_max_turns(mut self, limit: usize) -> Self {
        self.max_turns = limit;
        self
    }

    /// Set the sandbox root directory (default: `.`, the process working
    /// directory, resolved at tool-call time).
    #[must_use]
    pub fn with_sandbox_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.sandbox_root = root.into();
        self
    }

    /// Replace the denied tool list (default: [`Self::DEFAULT_DENIED_TOOLS`]).
    #[must_use]
    pub fn with_deny_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.denied_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    /// Wire all four guardrails into the given [`AgentOptions`].
    ///
    /// Appends to existing slots (never removes policies already present):
    /// [`BudgetPolicy`] and [`MaxTurnsPolicy`] as pre-turn policies,
    /// [`SandboxPolicy`] and [`ToolDenyListPolicy`] as pre-dispatch policies.
    #[must_use]
    pub fn apply(self, options: AgentOptions) -> AgentOptions {
        let mut budget = BudgetPolicy::new().with_max_cost(self.max_cost);
        if let Some(limit) = self.max_input_tokens {
            budget = budget.with_max_input(limit);
        }
        if let Some(limit) = self.max_output_tokens {
            budget = budget.with_max_output(limit);
        }

        options
            .with_pre_turn_policy(budget)
            .with_pre_turn_policy(MaxTurnsPolicy::new(self.max_turns))
            .with_pre_dispatch_policy(SandboxPolicy::new(self.sandbox_root))
            .with_pre_dispatch_policy(ToolDenyListPolicy::new(self.denied_tools))
    }
}

impl Default for RecommendedPolicies {
    fn default() -> Self {
        Self::builder()
    }
}

// ─── Integration-contract test helper ───────────────────────────────────────

/// Verify that the four recommended production guardrails are wired into an
/// [`AgentOptions`] with non-trivial limits.
///
/// This is an integration-contract helper for embedders: run it in a test
/// against your own `AgentOptions` construction to catch accidental guardrail
/// removal in downstream code. It checks — by canonical policy name and by
/// evaluating each policy against an extreme probe input — that:
///
/// 1. a pre-turn policy named `budget` exists and stops at extreme
///    accumulated cost/tokens (i.e. at least one limit is actually set),
/// 2. a pre-turn policy named `max_turns` exists and stops at an extreme
///    turn index,
/// 3. a pre-dispatch policy named `sandbox` exists and skips a tool call
///    whose `path` points outside any non-root sandbox (a sandbox rooted at
///    `/` is treated as trivial and fails this check),
/// 4. a pre-dispatch policy named `tool_deny_list` exists and skips
///    `expected_denied_tool` (pass a tool your deployment must never run,
///    e.g. `"bash"`).
///
/// Assumptions: policies are identified by the canonical names of the
/// `swink-agent-policies` implementations, `max_turns` is wired as a
/// pre-turn policy (as [`RecommendedPolicies::apply`] does), and the sandbox
/// inspects at least one of its default path fields
/// (`path`/`file_path`/`file`).
///
/// # Errors
///
/// Returns the list of human-readable contract violations, one per failed
/// check. Empty on success (`Ok(())`).
pub fn verify_production_guardrails(
    options: &AgentOptions,
    expected_denied_tool: &str,
) -> Result<(), Vec<String>> {
    let state = SessionState::new();
    let violations: Vec<String> = [
        check_budget(options, &state),
        check_max_turns(options, &state),
        check_sandbox(options, &state),
        check_deny_list(options, &state, expected_denied_tool),
    ]
    .into_iter()
    .flatten()
    .collect();

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Budget: present and stops under extreme accumulated cost/usage.
fn check_budget(options: &AgentOptions, state: &SessionState) -> Option<String> {
    let Some(policy) = options
        .pre_turn_policies
        .iter()
        .find(|p| p.name() == BUDGET_NAME)
    else {
        return Some(format!(
            "no pre-turn policy named '{BUDGET_NAME}' (BudgetPolicy) is wired"
        ));
    };
    let usage = Usage::default()
        .with_input(u64::MAX)
        .with_output(u64::MAX)
        .with_total(u64::MAX);
    let cost = Cost::default().with_total(f64::MAX);
    let ctx = probe_policy_context(0, &usage, &cost, state);
    if matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)) {
        None
    } else {
        Some(format!(
            "pre-turn policy '{BUDGET_NAME}' does not stop at extreme cost/token \
             usage — no effective limit is configured"
        ))
    }
}

/// Max turns: present and stops at an extreme turn index.
fn check_max_turns(options: &AgentOptions, state: &SessionState) -> Option<String> {
    let Some(policy) = options
        .pre_turn_policies
        .iter()
        .find(|p| p.name() == MAX_TURNS_NAME)
    else {
        return Some(format!(
            "no pre-turn policy named '{MAX_TURNS_NAME}' (MaxTurnsPolicy) is wired"
        ));
    };
    let usage = Usage::default();
    let cost = Cost::default();
    let ctx = probe_policy_context(usize::MAX, &usage, &cost, state);
    if matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)) {
        None
    } else {
        Some(format!(
            "pre-turn policy '{MAX_TURNS_NAME}' does not stop at an extreme turn \
             index — no effective turn limit is configured"
        ))
    }
}

/// Sandbox: present and rejects an escape probe path. The probe is an
/// absolute path directly under the filesystem root, so it resolves outside
/// every allowed root except `/` itself (which is trivial).
fn check_sandbox(options: &AgentOptions, state: &SessionState) -> Option<String> {
    let Some(policy) = options
        .pre_dispatch_policies
        .iter()
        .find(|p| p.name() == SANDBOX_NAME)
    else {
        return Some(format!(
            "no pre-dispatch policy named '{SANDBOX_NAME}' (SandboxPolicy) is wired"
        ));
    };
    let probe = "/swink-agent-guardrail-probe/escape.txt";
    let mut arguments = serde_json::json!({
        "path": probe,
        "file_path": probe,
        "file": probe,
    });
    let mut ctx = ToolDispatchContext::new(
        "guardrail_probe",
        "guardrail-probe",
        &mut arguments,
        None,
        state,
    );
    if matches!(policy.evaluate(&mut ctx), PreDispatchVerdict::Skip(_)) {
        None
    } else {
        Some(format!(
            "pre-dispatch policy '{SANDBOX_NAME}' does not reject a path outside \
             its allowed root — the sandbox is trivial or checks no default \
             path field"
        ))
    }
}

/// Deny list: present and denies the caller-designated tool.
fn check_deny_list(
    options: &AgentOptions,
    state: &SessionState,
    expected_denied_tool: &str,
) -> Option<String> {
    let Some(policy) = options
        .pre_dispatch_policies
        .iter()
        .find(|p| p.name() == DENY_LIST_NAME)
    else {
        return Some(format!(
            "no pre-dispatch policy named '{DENY_LIST_NAME}' (ToolDenyListPolicy) is wired"
        ));
    };
    let mut arguments = serde_json::json!({});
    let mut ctx = ToolDispatchContext::new(
        expected_denied_tool,
        "guardrail-probe",
        &mut arguments,
        None,
        state,
    );
    if matches!(policy.evaluate(&mut ctx), PreDispatchVerdict::Skip(_)) {
        None
    } else {
        Some(format!(
            "pre-dispatch policy '{DENY_LIST_NAME}' does not deny tool \
             '{expected_denied_tool}'"
        ))
    }
}

/// Build a pre-turn [`PolicyContext`] probe.
const fn probe_policy_context<'a>(
    turn_index: usize,
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(turn_index, usage, cost, 0, false, &[], state)
}

/// Panicking wrapper around [`verify_production_guardrails`] for use in
/// embedder test suites.
///
/// # Panics
///
/// Panics with the full list of contract violations if any guardrail is
/// missing or trivially configured.
pub fn assert_production_guardrails(options: &AgentOptions, expected_denied_tool: &str) {
    if let Err(violations) = verify_production_guardrails(options, expected_denied_tool) {
        panic!(
            "production guardrail contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }
}

#[cfg(test)]
mod tests {
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

        let pre_turn_names: Vec<&str> =
            options.pre_turn_policies.iter().map(|p| p.name()).collect();
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
        let violations =
            verify_production_guardrails(&bare_options(), "bash").expect_err("must fail");
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
}
