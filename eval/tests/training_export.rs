//! Integration tests for training-format trace export (spec 023, feature: training-export).

#![cfg(feature = "training-export")]

mod common;

use swink_agent_eval::{
    ChatMlExporter, DpoExporter, EvalCaseResult, EvalMetricResult, ExportOptions, Score,
    ScoredTrace, ShareGptExporter, TrainingExporter, TrainingFormat, Verdict,
    training::ExportError,
};

use common::mock_invocation;

// ─── Helpers ────────────────────────────────────────────────────────────────

fn make_trace(case_id: &str, score: f64, final_response: Option<&str>) -> ScoredTrace {
    ScoredTrace {
        invocation: mock_invocation(&["read_file"], final_response, 0.01, 100),
        score,
        case_id: case_id.to_string(),
    }
}

fn make_trace_no_tools(case_id: &str, score: f64, final_response: &str) -> ScoredTrace {
    ScoredTrace {
        invocation: mock_invocation(&[], Some(final_response), 0.01, 100),
        score,
        case_id: case_id.to_string(),
    }
}

fn make_case_result(case_id: &str, score_val: f64) -> EvalCaseResult {
    let inv = mock_invocation(&["read_file"], Some("done"), 0.01, 100);
    EvalCaseResult {
        case_id: case_id.to_string(),
        invocation: inv,
        metric_results: vec![EvalMetricResult {
            evaluator_name: "trajectory".to_string(),
            score: Score::new(score_val, 0.5),
            details: None,
        }],
        verdict: if score_val >= 0.5 {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
    }
}

// ─── ChatML / SFT ───────────────────────────────────────────────────────────

/// ChatML export produces valid JSONL — one object per line, correct turn structure.
#[test]
fn chatml_export_produces_valid_jsonl_with_correct_turn_structure() {
    let traces = vec![make_trace("case-1", 1.0, Some("Hello world"))];
    let opts = ExportOptions::chatml_sft(0.0);
    let bytes = ChatMlExporter
        .export(&traces, &opts)
        .expect("export should succeed");

    let output = String::from_utf8(bytes).expect("output should be utf-8");
    // Should be non-empty JSONL (at least one line)
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 1, "one trace → one JSONL line");

    let record: serde_json::Value =
        serde_json::from_str(lines[0]).expect("line should be valid JSON");

    // Must have "messages" key
    let messages = record.get("messages").expect("record must have 'messages'");
    let messages = messages.as_array().expect("messages must be an array");
    assert!(!messages.is_empty(), "messages array must not be empty");

    // First message must be system
    assert_eq!(messages[0]["role"], "system");

    // Must contain at least one user and one assistant turn
    let has_user = messages.iter().any(|m| m["role"] == "user");
    let has_assistant = messages.iter().any(|m| m["role"] == "assistant");
    assert!(has_user, "must have a user turn");
    assert!(has_assistant, "must have an assistant turn");
}

/// ChatML export includes tool_calls array on assistant turns that used tools.
#[test]
fn chatml_export_includes_tool_calls_on_assistant_turns() {
    let traces = vec![make_trace("case-2", 1.0, Some("done"))];
    let opts = ExportOptions::chatml_sft(0.0);
    let bytes = ChatMlExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let line = output.lines().next().unwrap();
    let record: serde_json::Value = serde_json::from_str(line).unwrap();

    let messages = record["messages"].as_array().unwrap();
    // Find the assistant message that had tool calls
    let assistant_with_tools = messages
        .iter()
        .find(|m| m["role"] == "assistant" && m.get("tool_calls").is_some());
    assert!(
        assistant_with_tools.is_some(),
        "assistant turn should have tool_calls when tools were called"
    );

    let tool_calls = assistant_with_tools.unwrap()["tool_calls"]
        .as_array()
        .unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["function"]["name"], "read_file");
    assert_eq!(tool_calls[0]["type"], "function");
}

/// ChatML export includes metadata when include_metadata = true.
#[test]
fn chatml_export_includes_metadata_when_requested() {
    let traces = vec![make_trace("my-case", 0.9, Some("result"))];
    let opts = ExportOptions {
        format: TrainingFormat::ChatMlSft,
        quality_threshold: 0.0,
        include_metadata: true,
    };
    let bytes = ChatMlExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let record: serde_json::Value = serde_json::from_str(output.lines().next().unwrap()).unwrap();

    let metadata = record.get("metadata").expect("metadata should be present");
    assert_eq!(metadata["case_id"], "my-case");
    assert!((metadata["score"].as_f64().unwrap() - 0.9).abs() < 1e-9);
}

/// ChatML export omits metadata when include_metadata = false.
#[test]
fn chatml_export_omits_metadata_when_not_requested() {
    let traces = vec![make_trace("c", 1.0, Some("ok"))];
    let opts = ExportOptions {
        format: TrainingFormat::ChatMlSft,
        quality_threshold: 0.0,
        include_metadata: false,
    };
    let bytes = ChatMlExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let record: serde_json::Value = serde_json::from_str(output.lines().next().unwrap()).unwrap();
    assert!(
        record.get("metadata").is_none(),
        "metadata should be absent"
    );
}

// ─── Quality Threshold ───────────────────────────────────────────────────────

/// Quality threshold filters out traces below the threshold.
#[test]
fn quality_threshold_filters_low_score_traces() {
    let traces = vec![
        make_trace("case-1", 0.9, Some("good")),
        make_trace("case-2", 0.3, Some("bad")),
    ];
    let opts = ExportOptions::chatml_sft(0.5);
    let bytes = ChatMlExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let lines: Vec<&str> = output.lines().collect();

    // Only the high-score trace should pass
    assert_eq!(lines.len(), 1, "only one trace should pass threshold 0.5");
    let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(record["metadata"]["case_id"], "case-1");
}

/// Export is empty (error) when all traces are below the threshold.
#[test]
fn export_empty_when_all_traces_below_threshold() {
    let traces = vec![
        make_trace("case-1", 0.2, Some("low")),
        make_trace("case-2", 0.1, Some("lower")),
    ];
    let opts = ExportOptions::chatml_sft(0.5);
    let err = ChatMlExporter
        .export(&traces, &opts)
        .expect_err("should fail when all traces are below threshold");
    assert!(
        matches!(err, ExportError::NothingToExport { threshold } if (threshold - 0.5).abs() < 1e-6),
        "unexpected error variant: {err:?}"
    );
}

/// Empty trace slice returns NothingToExport.
#[test]
fn export_empty_slice_returns_nothing_to_export() {
    let traces: Vec<ScoredTrace> = vec![];
    let opts = ExportOptions::chatml_sft(0.0);
    let err = ChatMlExporter
        .export(&traces, &opts)
        .expect_err("empty slice should fail");
    assert!(matches!(err, ExportError::NothingToExport { .. }));
}

// ─── DPO Pairs ───────────────────────────────────────────────────────────────

/// DPO pairs are created from high/low score traces on the same case.
#[test]
fn dpo_pairs_created_from_high_and_low_score_on_same_case() {
    let traces = vec![
        make_trace("case-A", 0.9, Some("great answer")),
        make_trace("case-A", 0.2, Some("poor answer")),
    ];
    let opts = ExportOptions::dpo_pairs(0.0);
    let bytes = DpoExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 1, "one case → one DPO pair");

    let pair: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(pair["case_id"], "case-A");
    assert!(pair.get("chosen").is_some(), "pair must have 'chosen'");
    assert!(pair.get("rejected").is_some(), "pair must have 'rejected'");
}

/// DPO export emits nothing (error) for cases with fewer than 2 traces.
#[test]
fn dpo_pairs_emits_nothing_for_single_trace_case() {
    let traces = vec![make_trace("solo-case", 0.8, Some("only one"))];
    let opts = ExportOptions::dpo_pairs(0.0);
    let err = DpoExporter
        .export(&traces, &opts)
        .expect_err("single-trace case should produce NothingToExport");
    assert!(matches!(err, ExportError::NothingToExport { .. }));
}

/// DPO pairs correctly pair highest vs lowest when 3+ traces for same case.
#[test]
fn dpo_pairs_uses_highest_and_lowest_score_traces() {
    let traces = vec![
        make_trace("case-X", 0.5, Some("medium")),
        make_trace("case-X", 0.95, Some("best")),
        make_trace("case-X", 0.1, Some("worst")),
    ];
    let opts = ExportOptions::dpo_pairs(0.0);
    let bytes = DpoExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let line = output.lines().next().unwrap();
    let pair: serde_json::Value = serde_json::from_str(line).unwrap();

    // chosen metadata score should be the highest
    let chosen_score = pair["chosen"]["metadata"]["score"].as_f64().unwrap();
    let rejected_score = pair["rejected"]["metadata"]["score"].as_f64().unwrap();
    assert!(
        chosen_score > rejected_score,
        "chosen score ({chosen_score}) should be higher than rejected score ({rejected_score})"
    );
}

// ─── ShareGPT ───────────────────────────────────────────────────────────────

/// ShareGPT exporter produces valid JSONL with human/gpt turn structure.
#[test]
fn sharegpt_export_produces_valid_jsonl_with_human_gpt_turns() {
    let traces = vec![make_trace_no_tools("case-sg", 1.0, "Hi there!")];
    let opts = ExportOptions::sharegpt();
    let bytes = ShareGptExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 1);

    let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let conversations = record["conversations"]
        .as_array()
        .expect("should have conversations");
    assert!(!conversations.is_empty());

    // Should have a system turn
    let has_system = conversations.iter().any(|c| c["from"] == "system");
    let has_gpt = conversations.iter().any(|c| c["from"] == "gpt");
    assert!(has_system, "should have system turn");
    assert!(has_gpt, "should have gpt turn");
}

// ─── ScoredTrace from EvalCaseResult ────────────────────────────────────────

/// ScoredTrace::from_case_result correctly averages metric scores.
#[test]
fn scored_trace_from_case_result_averages_metric_scores() {
    let result = make_case_result("test-case", 0.8);
    let trace = ScoredTrace::from_case_result(&result);
    assert_eq!(trace.case_id, "test-case");
    assert!(
        (trace.score - 0.8).abs() < 1e-9,
        "score should be 0.8, got {}",
        trace.score
    );
}

/// ScoredTrace::from_case_result handles zero metric results gracefully.
#[test]
fn scored_trace_from_case_result_handles_no_metrics() {
    let inv = mock_invocation(&[], Some("done"), 0.0, 0);
    let result = EvalCaseResult {
        case_id: "empty".to_string(),
        invocation: inv,
        metric_results: vec![],
        verdict: Verdict::Fail,
    };
    let trace = ScoredTrace::from_case_result(&result);
    assert!((trace.score - 0.0).abs() < 1e-9);
}

// ─── Multiple traces, multiple cases ────────────────────────────────────────

/// Multiple traces for different cases produce the correct number of JSONL lines.
#[test]
fn chatml_multiple_traces_produce_one_line_each() {
    let traces = vec![
        make_trace("a", 1.0, Some("resp-a")),
        make_trace("b", 1.0, Some("resp-b")),
        make_trace("c", 1.0, Some("resp-c")),
    ];
    let opts = ExportOptions::chatml_sft(0.0);
    let bytes = ChatMlExporter.export(&traces, &opts).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3);

    // All lines must be valid JSON
    for line in &lines {
        serde_json::from_str::<serde_json::Value>(line).expect("each line must be valid JSON");
    }
}
