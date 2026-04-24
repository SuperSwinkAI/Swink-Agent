//! Regression tests for `LangSmithExporter` (T148).

#![cfg(feature = "langsmith")]

mod common;

use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, LangSmithExportError,
    LangSmithExporter, Reporter, ReporterOutput, Score, Verdict,
};
use url::Url;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::mock_invocation;

fn structured_details(prompt_version: &str, feedback_key: Option<&str>, note: &str) -> String {
    let mut lines = vec![format!(
        "{{\"kind\":\"prompt_version\",\"version\":\"{prompt_version}\"}}"
    )];
    if let Some(feedback_key) = feedback_key {
        lines.push(format!(
            "{{\"kind\":\"feedback_key\",\"key\":\"{feedback_key}\"}}"
        ));
    }
    lines.push(format!("{{\"kind\":\"note\",\"text\":\"{note}\"}}"));
    lines.join("\n")
}

fn sample_result() -> EvalSetResult {
    EvalSetResult {
        eval_set_id: "demo-set".into(),
        case_results: vec![
            EvalCaseResult {
                case_id: "case-alpha".into(),
                invocation: mock_invocation(&[], Some("alpha"), 0.01, 120),
                metric_results: vec![EvalMetricResult {
                    evaluator_name: "correctness".into(),
                    score: Score::new(0.91, 0.5),
                    details: Some(structured_details(
                        "correctness_v0",
                        Some("quality.correctness"),
                        "alpha looks correct",
                    )),
                }],
                verdict: Verdict::Pass,
            },
            EvalCaseResult {
                case_id: "case-beta".into(),
                invocation: mock_invocation(&[], Some("beta"), 0.01, 120),
                metric_results: vec![EvalMetricResult {
                    evaluator_name: "harmfulness".into(),
                    score: Score::new(0.2, 0.5),
                    details: Some(structured_details("harmfulness_v0", None, "beta is risky")),
                }],
                verdict: Verdict::Fail,
            },
        ],
        summary: EvalSummary {
            total_cases: 2,
            passed: 1,
            failed: 1,
            total_cost: Cost {
                input: 0.01,
                output: 0.01,
                total: 0.02,
                ..Default::default()
            },
            total_usage: Usage {
                input: 120,
                output: 120,
                total: 240,
                ..Default::default()
            },
            total_duration: Duration::from_millis(220),
        },
        timestamp: 1_700_000_000,
    }
}

#[test]
fn langsmith_export_posts_runs_and_feedback() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(MockServer::start());
    let endpoint = Url::parse(&server.uri()).unwrap();

    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/api/v1/runs"))
            .and(header("x-api-key", "test-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"id":"run-alpha"}"#, "application/json"),
            )
            .expect(2)
            .mount(&server)
            .await;
    });
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .and(header("x-api-key", "test-token"))
            .and(body_partial_json(serde_json::json!({
                "key": "quality.correctness",
                "comment": "alpha looks correct"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .and(header("x-api-key", "test-token"))
            .and(body_partial_json(serde_json::json!({
                "key": "harmfulness",
                "comment": "beta is risky"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    });

    let exporter = LangSmithExporter::new("test-token").with_endpoint(endpoint);
    let output = exporter.export(&sample_result()).expect("export succeeds");

    match output {
        ReporterOutput::Remote {
            backend,
            identifier,
        } => {
            assert_eq!(backend, "langsmith");
            assert_eq!(identifier, "demo-set");
        }
        other => panic!("expected remote output, got {other:?}"),
    }
}

#[test]
fn langsmith_export_surfaces_partial_feedback_failure() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(MockServer::start());
    let endpoint = Url::parse(&server.uri()).unwrap();

    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/api/v1/runs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(r#"{"id":"run-ok"}"#, "application/json"),
            )
            .expect(2)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .and(body_partial_json(serde_json::json!({
                "key": "quality.correctness"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .and(body_partial_json(serde_json::json!({
                "key": "harmfulness"
            })))
            .respond_with(ResponseTemplate::new(500).set_body_string("feedback write failed"))
            .expect(1)
            .mount(&server)
            .await;
    });

    let exporter = LangSmithExporter::new("test-token").with_endpoint(endpoint);
    let err = exporter
        .export(&sample_result())
        .expect_err("second case feedback should fail");

    match err {
        LangSmithExportError::Push {
            pushed,
            failed,
            first_error,
        } => {
            assert_eq!(pushed, 1);
            assert_eq!(failed, 1);
            assert!(first_error.contains("feedback write failed"));
        }
        other => panic!("expected Push error, got {other:?}"),
    }
}

#[test]
fn langsmith_reporter_maps_export_failures_to_reporter_error() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(MockServer::start());
    let endpoint = Url::parse(&server.uri()).unwrap();

    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/api/v1/runs"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
    });

    let exporter = LangSmithExporter::new("bad-token").with_endpoint(endpoint);
    let err = exporter
        .render(&sample_result())
        .expect_err("auth failure should map to reporter error");

    assert!(err.to_string().contains("authentication failed"));
}
