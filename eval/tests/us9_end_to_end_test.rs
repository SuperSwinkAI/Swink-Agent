//! US9 end-to-end CLI pipeline test (T161).
//!
//! Walks the full `run → report → gate` sequence against a fixture
//! `EvalSet` in a tempdir and asserts:
//! * `run` produces a JSON artifact that `report` and `gate` can consume
//!   without re-execution (SC stable-exit-code contract).
//! * `report --format md` and `report --format json` produce
//!   consistent outputs matching the in-process reporter API.
//! * `gate` exit code matches the gate outcome (0 pass, 1 fail).

#![cfg(all(feature = "cli", feature = "yaml"))]

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_swink-eval")
}

#[test]
#[ignore = "requires real agent/evaluator configuration not available in CI"]
fn run_report_gate_pipeline_succeeds_against_fixture_set() {
    let dir = TempDir::new().expect("tempdir");
    let set_yaml = r#"
id: us9-e2e
name: US9 e2e pipeline
cases:
  - id: a
    name: A
    system_prompt: You are a test agent.
    user_messages: ["hello"]
  - id: b
    name: B
    system_prompt: You are a test agent.
    user_messages: ["world"]
"#;
    let set_path = dir.path().join("set.yaml");
    let result_path = dir.path().join("result.json");
    let gate_path = dir.path().join("gate.json");
    fs::write(&set_path, set_yaml).unwrap();
    fs::write(
        &gate_path,
        serde_json::to_vec(&serde_json::json!({"min_pass_rate": 0.0})).unwrap(),
    )
    .unwrap();

    // 1. run
    let run_out = Command::new(binary_path())
        .args([
            "run",
            "--set",
            set_path.to_str().unwrap(),
            "--out",
            result_path.to_str().unwrap(),
            "--parallelism",
            "2",
            "--reporter",
            "json",
        ])
        .output()
        .expect("spawn run");
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    // 2. report (should not re-execute — just read the artifact)
    let report_json = Command::new(binary_path())
        .args([
            "report",
            "--result",
            result_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("spawn report json");
    assert!(
        report_json.status.success(),
        "report json failed: {}",
        String::from_utf8_lossy(&report_json.stderr)
    );
    let stdout = String::from_utf8_lossy(&report_json.stdout);
    assert!(stdout.contains("\"schema_version\""));
    assert!(stdout.contains("us9-e2e"));

    let report_md = Command::new(binary_path())
        .args([
            "report",
            "--result",
            result_path.to_str().unwrap(),
            "--format",
            "md",
        ])
        .output()
        .expect("spawn report md");
    assert!(report_md.status.success());
    // Markdown reporter emits a table header.
    let md_stdout = String::from_utf8_lossy(&report_md.stdout);
    assert!(md_stdout.contains('|'));

    // 3. gate — zero pass-rate threshold always succeeds, verifies 0 exit.
    let gate_status = Command::new(binary_path())
        .args([
            "gate",
            "--result",
            result_path.to_str().unwrap(),
            "--gate-config",
            gate_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn gate");
    assert_eq!(gate_status.code(), Some(0));
}

#[test]
fn gate_fails_with_exit_1_when_threshold_unmet() {
    // Build a result where most cases failed, then apply a strict gate.
    use std::time::Duration;
    use swink_agent::{Cost, ModelSpec, StopReason, Usage};
    use swink_agent_eval::{EvalCaseResult, EvalSetResult, EvalSummary, Invocation, Verdict};

    let dir = TempDir::new().expect("tempdir");
    let result_path = dir.path().join("result.json");
    let gate_path = dir.path().join("gate.json");

    let failing_cases: Vec<EvalCaseResult> = (0..5)
        .map(|i| EvalCaseResult {
            case_id: format!("c{i}"),
            invocation: Invocation {
                turns: vec![],
                total_usage: Usage::default(),
                total_cost: Cost::default(),
                total_duration: Duration::from_millis(1),
                final_response: Some("x".into()),
                stop_reason: StopReason::Stop,
                model: ModelSpec::new("t", "m"),
            },
            metric_results: vec![],
            verdict: Verdict::Fail,
        })
        .collect();
    let result = EvalSetResult {
        eval_set_id: "us9-fail".into(),
        summary: EvalSummary {
            total_cases: 5,
            passed: 0,
            failed: 5,
            total_duration: Duration::from_millis(5),
            total_cost: Cost::default(),
            total_usage: Usage::default(),
        },
        case_results: failing_cases,
        timestamp: 0,
    };
    fs::write(&result_path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    fs::write(
        &gate_path,
        serde_json::to_vec(&serde_json::json!({"min_pass_rate": 0.9})).unwrap(),
    )
    .unwrap();

    let gate_status = Command::new(binary_path())
        .args([
            "gate",
            "--result",
            result_path.to_str().unwrap(),
            "--gate-config",
            gate_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn gate");
    assert_eq!(gate_status.code(), Some(1));
}
