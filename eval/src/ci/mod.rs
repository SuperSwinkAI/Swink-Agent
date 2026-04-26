//! Bundled CI templates for `swink-eval` consumers (spec 043 T156-T160).
//!
//! The four templates below are compile-time validated via
//! [`include_str!`] — deleting or mis-naming a template breaks
//! `cargo build`. Consumers either reference these constants directly
//! (for ergonomic doc / bootstrap tooling) or copy the `.yml` files out
//! of `eval/src/ci/templates/` into their repository.
//!
//! Template contract:
//! * `pr-eval.yml` — PR-trigger workflow: build → eval → gate → comment.
//! * `nightly-eval.yml` — cron workflow: full suite + HTML dashboard.
//! * `release-eval.yml` — tag-trigger workflow: blocking gate on release.
//! * `pre-commit-hook.yml` — local pre-commit smoke + gate hook.

/// GitHub Actions workflow: run the eval suite on every PR + comment on
/// the PR with the Markdown summary.
pub const PR_EVAL_TEMPLATE: &str = include_str!("templates/pr-eval.yml");

/// GitHub Actions workflow: nightly cron that exercises live-judge
/// providers + renders an HTML dashboard artifact.
pub const NIGHTLY_EVAL_TEMPLATE: &str = include_str!("templates/nightly-eval.yml");

/// GitHub Actions workflow: gated release acceptance suite triggered
/// on `vX.Y.Z` tags.
pub const RELEASE_EVAL_TEMPLATE: &str = include_str!("templates/release-eval.yml");

/// Pre-commit hook config: runs the local smoke eval set + gate.
pub const PRE_COMMIT_TEMPLATE: &str = include_str!("templates/pre-commit-hook.yml");

/// Manifest listing every bundled template as `(filename, body)`.
///
/// Consumers can iterate to write the whole bundle into a target
/// workspace — e.g. `swink-eval scaffold-ci` or similar tooling.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("pr-eval.yml", PR_EVAL_TEMPLATE),
    ("nightly-eval.yml", NIGHTLY_EVAL_TEMPLATE),
    ("release-eval.yml", RELEASE_EVAL_TEMPLATE),
    ("pre-commit-hook.yml", PRE_COMMIT_TEMPLATE),
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;

    #[test]
    fn every_template_is_non_empty() {
        for (name, body) in TEMPLATES {
            assert!(!body.is_empty(), "template {name} is empty");
        }
    }

    #[test]
    fn every_template_parses_as_yaml() {
        for (name, body) in TEMPLATES {
            let parsed: Value = serde_yaml::from_str(body)
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
}
