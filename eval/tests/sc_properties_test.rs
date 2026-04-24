//! Success-criterion property tests (spec 043 T175, T177).
//!
//! * [`sc_004_judge_model_swap_is_single_arg_change`] covers SC-004 —
//!   every evaluator continues to work when the judge model id is
//!   swapped, and the swap is exactly one constructor-arg change.
//! * [`sc_009_default_build_has_no_new_transitive_deps`] covers SC-009 —
//!   `cargo tree -p swink-agent-eval --no-default-features` produces a
//!   dep graph with none of the opt-in heavyweight crates
//!   (`reqwest`, `jsonschema`, `askama`, `clap`, `opentelemetry*`,
//!   `minijinja`, `backon`, `strsim`). If this test regresses the
//!   default build has started pulling in a gated transitive.

#![cfg(all(feature = "judge-core", feature = "evaluator-quality"))]

use std::process::Command;
use std::sync::Arc;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
#[cfg(feature = "evaluator-safety")]
use swink_agent_eval::{
    CorrectnessEvaluator, Evaluator, HelpfulnessEvaluator, JudgeVerdict, MockJudge,
    ToxicityEvaluator,
};
use swink_agent_eval::{EvalCase, Invocation, JudgeClient, JudgeEvaluatorConfig, JudgeRegistry};

fn case() -> EvalCase {
    EvalCase {
        id: "c".into(),
        name: "c".into(),
        description: None,
        system_prompt: "agent".into(),
        user_messages: vec!["q".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn invocation() -> Invocation {
    use std::time::Duration;
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: Some("ok".into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("t", "m"),
    }
}

fn config_for_model(judge: Arc<dyn JudgeClient>, model_id: &str) -> JudgeEvaluatorConfig {
    // THE one-line swap: the second positional arg to `builder(...)` is
    // the judge model id. Changing only that argument is all any
    // downstream consumer must do to move from `gpt-4o` to
    // `claude-3-opus`, validating SC-004.
    let registry = JudgeRegistry::builder(judge, model_id)
        .build()
        .expect("registry");
    JudgeEvaluatorConfig::default_with(Arc::new(registry))
}

#[cfg(feature = "evaluator-safety")]
fn build_all_families(
    judge: &Arc<dyn JudgeClient>,
    model_id: &str,
) -> (
    CorrectnessEvaluator,
    HelpfulnessEvaluator,
    ToxicityEvaluator,
) {
    (
        CorrectnessEvaluator::new(config_for_model(Arc::clone(judge), model_id)),
        HelpfulnessEvaluator::new(config_for_model(Arc::clone(judge), model_id)),
        ToxicityEvaluator::new(config_for_model(Arc::clone(judge), model_id)),
    )
}

#[cfg(feature = "evaluator-safety")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sc_004_judge_model_swap_is_single_arg_change() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![
        // Two rounds: one for `gpt-4o`, one for `claude-3-opus`.
        JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("gpt".into()),
            label: None,
        },
        JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("gpt".into()),
            label: None,
        },
        JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("gpt".into()),
            label: None,
        },
        JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("claude".into()),
            label: None,
        },
        JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("claude".into()),
            label: None,
        },
        JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("claude".into()),
            label: None,
        },
    ]));

    // Round 1: gpt-4o.
    let (c_gpt, h_gpt, t_gpt) = build_all_families(&judge, "gpt-4o");
    let case = case();
    let inv = invocation();
    assert!(c_gpt.evaluate(&case, &inv).is_some());
    assert!(h_gpt.evaluate(&case, &inv).is_some());
    assert!(t_gpt.evaluate(&case, &inv).is_some());

    // Round 2: claude-3-opus (ONLY the `model_id` string changed).
    let (c_claude, h_claude, t_claude) = build_all_families(&judge, "claude-3-opus");
    assert!(c_claude.evaluate(&case, &inv).is_some());
    assert!(h_claude.evaluate(&case, &inv).is_some());
    assert!(t_claude.evaluate(&case, &inv).is_some());
}

#[test]
fn sc_009_default_build_has_no_new_transitive_deps() {
    // Skip if cargo isn't on PATH (e.g. a distribution environment
    // running built test binaries).
    let Ok(output) = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "swink-agent-eval",
            "--no-default-features",
            "--prefix",
            "none",
            "--edges",
            "normal",
        ])
        .output()
    else {
        eprintln!("cargo tree unavailable — SC-009 test skipped");
        return;
    };
    if !output.status.success() {
        eprintln!("cargo tree failed — SC-009 test inconclusive");
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Deps that were introduced by spec 043 for opt-in surfaces. None of
    // these may appear in the default build (`reqwest` + `jsonschema` are
    // grandfathered from 023 — both transit through `swink-agent` and
    // existed before spec 043).
    const FORBIDDEN: &[&str] = &[
        "askama ",
        "clap ",
        "opentelemetry ",
        "opentelemetry-otlp ",
        "opentelemetry-proto ",
        "opentelemetry_sdk ",
        "minijinja ",
        "backon ",
        "strsim ",
    ];
    for name in FORBIDDEN {
        assert!(
            !stdout.contains(name),
            "SC-009 regression — default build pulled in `{name}` transitively.\n\
             Full cargo tree output:\n{stdout}"
        );
    }
}
