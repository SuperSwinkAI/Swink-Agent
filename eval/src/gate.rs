//! Post-evaluation gating for CI/CD pipelines.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::EvalSetResult;

/// Configuration for CI/CD gate checks against evaluation results.
///
/// Serde-ready so consumers (notably the `swink-eval gate` subcommand)
/// can load thresholds from a YAML/JSON file without re-declaring the
/// shape.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GateConfig {
    /// Minimum fraction of cases that must pass (e.g. 0.95 for 95%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_pass_rate: Option<f64>,
    /// Maximum total cost in dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<f64>,
    /// Maximum total wall-clock duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<Duration>,
}

impl GateConfig {
    /// Create a new empty gate configuration (all checks disabled).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            min_pass_rate: None,
            max_cost: None,
            max_duration: None,
        }
    }

    /// Set the minimum pass rate threshold.
    #[must_use]
    pub fn with_min_pass_rate(mut self, rate: f64) -> Self {
        assert!(
            (0.0..=1.0).contains(&rate),
            "pass rate must be in [0.0, 1.0], got {rate}"
        );
        self.min_pass_rate = Some(rate);
        self
    }

    /// Set the maximum allowed cost in dollars.
    #[must_use]
    pub fn with_max_cost(mut self, cost: f64) -> Self {
        assert!(cost >= 0.0, "cost must be non-negative, got {cost}");
        self.max_cost = Some(cost);
        self
    }

    /// Set the maximum allowed wall-clock duration.
    #[must_use]
    pub const fn with_max_duration(mut self, duration: Duration) -> Self {
        self.max_duration = Some(duration);
        self
    }
}

/// Result of a CI/CD gate check.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GateResult {
    /// Whether the gate check passed.
    pub passed: bool,
    /// Process exit code: 0 for pass, 1 for fail.
    pub exit_code: i32,
    /// Human-readable summary of the gate result.
    pub summary: String,
}

impl GateResult {
    /// Create a gate result from its passed flag, exit code, and summary.
    #[must_use]
    pub fn new(passed: bool, exit_code: i32, summary: impl Into<String>) -> Self {
        Self {
            passed,
            exit_code,
            summary: summary.into(),
        }
    }

    /// Exit the process with this result's exit code.
    pub fn exit(&self) -> ! {
        std::process::exit(self.exit_code)
    }
}

/// Check evaluation results against gate configuration.
///
/// Returns a [`GateResult`] indicating whether all configured thresholds were met.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn check_gate(result: &EvalSetResult, config: &GateConfig) -> GateResult {
    let mut failures: Vec<String> = Vec::new();

    let total = result.summary.total_cases;
    let passed = result.summary.passed;

    if let Some(min_rate) = config.min_pass_rate {
        let rate = if total == 0 {
            1.0
        } else {
            passed as f64 / total as f64
        };
        if rate < min_rate {
            failures.push(format!(
                "pass rate {rate:.2} < minimum {min_rate:.2} ({passed}/{total})"
            ));
        }
    }

    if let Some(max_cost) = config.max_cost
        && result.summary.total_cost.total > max_cost
    {
        failures.push(format!(
            "cost ${:.4} > max ${max_cost:.4}",
            result.summary.total_cost.total
        ));
    }

    if let Some(max_dur) = config.max_duration
        && result.summary.total_duration > max_dur
    {
        failures.push(format!(
            "duration {:.1}s > max {:.1}s",
            result.summary.total_duration.as_secs_f64(),
            max_dur.as_secs_f64()
        ));
    }

    if failures.is_empty() {
        GateResult {
            passed: true,
            exit_code: 0,
            summary: format!("GATE PASSED: {passed}/{total} cases passed"),
        }
    } else {
        GateResult {
            passed: false,
            exit_code: 1,
            summary: format!("GATE FAILED: {}", failures.join("; ")),
        }
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
