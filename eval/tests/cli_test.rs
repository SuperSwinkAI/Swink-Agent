//! `swink-eval` CLI tests (spec 043 T151).
//!
//! Covers the three subcommands + exit-code contract from
//! `contracts/public-api.md §Binary target`:
//! * `run --set <path>` produces output matching the in-process API.
//! * `report` re-renders a persisted result without re-executing.
//! * `gate` returns 0 on pass / 1 on fail / 2 on config error.
//!
//! The tests drive the `swink-eval` binary via the
//! `CARGO_BIN_EXE_swink-eval` environment variable cargo sets up for
//! test runs — no new dev-deps needed.

#![cfg(all(feature = "cli", feature = "yaml"))]

use std::fs;
use std::process::Command;
use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    EvalCaseResult, EvalSetResult, EvalSummary, Invocation, JsonReporter, Reporter, ReporterOutput,
    Verdict,
};
use tempfile::TempDir;

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_swink-eval")
}

/// Minimal `EvalSetResult` persisted to disk for report/gate tests.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn sample_result(pass_rate: f64) -> EvalSetResult {
    let passed = (pass_rate * 10.0).round() as usize;
    let total = 10;
    let case_results: Vec<EvalCaseResult> = (0..total)
        .map(|i| EvalCaseResult {
            case_id: format!("case-{i:02}"),
            invocation: Invocation {
                turns: vec![],
                total_usage: Usage::default(),
                total_cost: Cost::default(),
                total_duration: Duration::from_millis(1),
                final_response: Some("ok".into()),
                stop_reason: StopReason::Stop,
                model: ModelSpec::new("test", "test-model"),
            },
            metric_results: vec![],
            verdict: if i < passed {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
        })
        .collect();

    EvalSetResult {
        eval_set_id: "cli-test".into(),
        summary: EvalSummary {
            total_cases: total,
            passed,
            failed: total - passed,
            total_duration: Duration::from_millis(10),
            total_cost: Cost::default(),
            total_usage: Usage::default(),
        },
        case_results,
        timestamp: 0,
    }
}

fn write_json<T: serde::Serialize>(dir: &TempDir, name: &str, value: &T) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, serde_json::to_vec_pretty(value).expect("encode")).expect("write");
    path
}

fn write_reporter_json(dir: &TempDir, name: &str, result: &EvalSetResult) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let output = JsonReporter::new().render(result).expect("render json");
    let ReporterOutput::Artifact { bytes, .. } = output else {
        panic!("JsonReporter should emit an artifact");
    };
    fs::write(&path, bytes).expect("write");
    path
}

#[test]
fn report_re_renders_persisted_result_to_stdout() {
    let dir = TempDir::new().unwrap();
    let result_path = write_json(&dir, "result.json", &sample_result(1.0));

    let out = Command::new(binary_path())
        .args([
            "report",
            "--result",
            result_path.to_str().unwrap(),
            "--format",
            "console",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("cli-test") || stdout.contains("case-00"),
        "console output should surface case/set ids; got: {stdout}"
    );
}

#[test]
fn report_renders_json_format_matching_in_process_reporter() {
    let dir = TempDir::new().unwrap();
    let result = sample_result(0.8);
    let result_path = write_json(&dir, "result.json", &result);

    let out = Command::new(binary_path())
        .args([
            "report",
            "--result",
            result_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // JSON reporter emits a document carrying the schema version marker.
    assert!(
        stdout.contains("\"schema_version\"") && stdout.contains("043"),
        "JSON output should carry schema_version=043; got: {stdout}"
    );
}

#[test]
fn report_accepts_json_reporter_artifact() {
    let dir = TempDir::new().unwrap();
    let result = sample_result(1.0);
    let result_path = write_reporter_json(&dir, "reporter-result.json", &result);

    let out = Command::new(binary_path())
        .args([
            "report",
            "--result",
            result_path.to_str().unwrap(),
            "--format",
            "console",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("cli-test") && stdout.contains("case-00"),
        "console output should surface reporter artifact ids; got: {stdout}"
    );
}

#[test]
fn gate_returns_zero_when_thresholds_met() {
    let dir = TempDir::new().unwrap();
    let result_path = write_json(&dir, "result.json", &sample_result(1.0));
    let cfg_path = write_json(
        &dir,
        "gate.json",
        &serde_json::json!({ "min_pass_rate": 0.9 }),
    );

    let status = Command::new(binary_path())
        .args([
            "gate",
            "--result",
            result_path.to_str().unwrap(),
            "--gate-config",
            cfg_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert_eq!(status.code(), Some(0), "gate should pass at 100% pass rate");
}

#[test]
fn gate_accepts_json_reporter_artifact() {
    let dir = TempDir::new().unwrap();
    let result_path = write_reporter_json(&dir, "reporter-result.json", &sample_result(1.0));
    let cfg_path = write_json(
        &dir,
        "gate.json",
        &serde_json::json!({ "min_pass_rate": 0.9 }),
    );

    let status = Command::new(binary_path())
        .args([
            "gate",
            "--result",
            result_path.to_str().unwrap(),
            "--gate-config",
            cfg_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert_eq!(
        status.code(),
        Some(0),
        "gate should load reporter JSON artifacts"
    );
}

#[test]
fn gate_returns_one_when_thresholds_not_met() {
    let dir = TempDir::new().unwrap();
    let result_path = write_json(&dir, "result.json", &sample_result(0.5));
    let cfg_path = write_json(
        &dir,
        "gate.json",
        &serde_json::json!({ "min_pass_rate": 0.9 }),
    );

    let status = Command::new(binary_path())
        .args([
            "gate",
            "--result",
            result_path.to_str().unwrap(),
            "--gate-config",
            cfg_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert_eq!(
        status.code(),
        Some(1),
        "gate should fail at 50% pass rate with 0.9 threshold"
    );
}

#[test]
fn gate_returns_two_when_result_missing() {
    let dir = TempDir::new().unwrap();
    let cfg_path = write_json(
        &dir,
        "gate.json",
        &serde_json::json!({ "min_pass_rate": 0.9 }),
    );
    let missing = dir.path().join("does-not-exist.json");

    let status = Command::new(binary_path())
        .args([
            "gate",
            "--result",
            missing.to_str().unwrap(),
            "--gate-config",
            cfg_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert_eq!(
        status.code(),
        Some(2),
        "missing result file should map to config-error exit 2"
    );
}

#[test]
fn report_returns_two_when_result_missing() {
    let missing = "does-not-exist-at-all.json";
    let status = Command::new(binary_path())
        .args(["report", "--result", missing, "--format", "console"])
        .status()
        .expect("spawn");
    assert_eq!(
        status.code(),
        Some(2),
        "missing result file should map to config-error exit 2"
    );
}

#[test]
fn run_requires_real_execution_configuration() {
    let dir = TempDir::new().unwrap();
    let set_yaml = r#"
id: cli-run-needs-config
name: CLI run needs config
cases:
  - id: c1
    name: Case 1
    system_prompt: You are a test agent.
    user_messages: ["hi"]
"#;
    let set_path = dir.path().join("set.yaml");
    fs::write(&set_path, set_yaml).unwrap();
    let out_path = dir.path().join("result.json");

    let out = Command::new(binary_path())
        .args([
            "run",
            "--set",
            set_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
            "--parallelism",
            "1",
            "--reporter",
            "console",
        ])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        !out_path.exists(),
        "run should not write a false-green artifact when no real execution configuration exists"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("real agent and evaluator configuration is required"),
        "configuration error details should be emitted to stderr; got: {stderr}"
    );
}

#[test]
fn run_loads_and_validates_eval_set_before_configuration_check() {
    let dir = TempDir::new().unwrap();
    let set_path = dir.path().join("set.yaml");
    fs::write(&set_path, "not: [valid").unwrap();

    let out = Command::new(binary_path())
        .args([
            "run",
            "--set",
            set_path.to_str().unwrap(),
            "--parallelism",
            "1",
            "--reporter",
            "console",
        ])
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(2),
        "invalid eval set should map to config-error exit 2"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("swink-eval run: loading eval set"),
        "load error details should be emitted to stderr; got: {stderr}"
    );
}
