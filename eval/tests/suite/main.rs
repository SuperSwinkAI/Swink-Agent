//! Consolidated integration-test suite: one binary instead of one per file.
//! Each former `tests/<name>.rs` is a module below; its former top-level
//! `#![cfg(...)]` gate is the `#[cfg(...)]` attribute on its `mod` line.

#[allow(dead_code)]
mod common;

mod aggregator;
mod attachment_test;
mod audit;
mod budget;
mod cache_test;
#[cfg(all(feature = "cli", feature = "yaml"))]
mod cli_test;
#[cfg(feature = "judge-core")]
mod dispatch_judge_test;
mod efficiency;
mod environment_state;
mod eval_case_test;
mod evaluator;
#[cfg(all(feature = "judge-core", feature = "evaluator-agent"))]
mod evaluators_agent_test;
#[cfg(feature = "evaluator-code")]
mod evaluators_code_test;
#[cfg(feature = "multimodal")]
mod evaluators_multimodal_test;
#[cfg(all(feature = "judge-core", feature = "evaluator-quality"))]
mod evaluators_quality_test;
#[cfg(all(feature = "judge-core", feature = "evaluator-rag"))]
mod evaluators_rag_test;
#[cfg(all(feature = "judge-core", feature = "evaluator-safety"))]
mod evaluators_safety_test;
#[cfg(all(target_family = "unix", feature = "evaluator-sandbox"))]
mod evaluators_sandbox_test;
#[cfg(feature = "evaluator-simple")]
mod evaluators_simple_test;
#[cfg(feature = "evaluator-structured")]
mod evaluators_structured_test;
mod gate;
#[cfg(feature = "generation")]
mod generation_test;
mod judge_fixtures_test;
mod judge_registry_test;
mod match_;
mod no_live_llm_test;
#[cfg(feature = "judge-core")]
mod prompt_template_test;
mod registry_panic_isolation;
mod reporter_console_test;
#[cfg(feature = "html-report")]
mod reporter_html_test;
mod reporter_json_test;
#[cfg(feature = "langsmith")]
mod reporter_langsmith_test;
mod reporter_markdown_test;
mod response;
mod runner;
mod runner_cancel_test;
mod runner_initial_session_test;
mod runner_num_runs_test;
mod runner_parallelism_test;
#[cfg(all(feature = "judge-core", feature = "evaluator-quality"))]
mod sc_properties_test;
mod score;
#[cfg(all(feature = "judge-core", feature = "evaluator-quality"))]
mod score_clamp_test;
mod semantic_tool_parameter;
mod semantic_tool_selection;
#[cfg(feature = "simulation")]
mod simulation_state_test;
#[cfg(feature = "simulation")]
mod simulation_test;
mod store;
#[cfg(feature = "telemetry")]
mod telemetry_test;
#[cfg(feature = "trace-ingest")]
mod trace_ingest_test;
#[cfg(all(feature = "trace-ingest", feature = "trace-langfuse"))]
mod trace_langfuse_test;
#[cfg(all(feature = "trace-ingest", feature = "trace-otlp"))]
mod trace_otlp_test;
#[cfg(feature = "training-export")]
mod training_export;
mod trajectory;
mod unsafe_policy_test;
#[cfg(feature = "all-evaluators")]
mod us1_end_to_end_test;
mod us2_end_to_end_test;
#[cfg(all(
    feature = "judge-core",
    feature = "evaluator-quality",
    feature = "evaluator-safety",
    feature = "evaluator-rag",
    feature = "evaluator-agent",
    feature = "evaluator-code",
    feature = "multimodal"
))]
mod us3_custom_prompt_test;
#[cfg(all(feature = "simulation", feature = "evaluator-quality"))]
mod us4_end_to_end_test;
#[cfg(feature = "generation")]
mod us5_end_to_end_test;
#[cfg(all(feature = "trace-ingest", feature = "evaluator-simple"))]
mod us6_end_to_end_test;
#[cfg(feature = "telemetry")]
mod us7_end_to_end_test;
#[cfg(feature = "html-report")]
mod us8_end_to_end_test;
#[cfg(all(feature = "cli", feature = "yaml"))]
mod us9_end_to_end_test;
#[cfg(feature = "yaml")]
mod yaml;
