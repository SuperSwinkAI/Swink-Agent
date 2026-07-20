//! Consolidated integration-test suite: one binary instead of one per file.
//! Each former `tests/<name>.rs` is a module below; its former top-level
//! `#![cfg(...)]` gate is the `#[cfg(...)]` attribute on its `mod` line.

#[cfg(feature = "testkit")]
#[allow(dead_code)]
mod common;

#[cfg(feature = "testkit")]
mod abort_turn_end_reason;
#[cfg(feature = "testkit")]
mod ac_context;
#[cfg(feature = "testkit")]
mod ac_lifecycle;
#[cfg(feature = "testkit")]
mod ac_resilience;
#[cfg(feature = "testkit")]
mod ac_structured;
#[cfg(feature = "testkit")]
mod ac_tools;
#[cfg(feature = "testkit")]
mod agent;
#[cfg(feature = "testkit")]
mod agent_continuation;
#[cfg(feature = "testkit")]
mod agent_event_serde;
#[cfg(feature = "testkit")]
mod agent_loop;
#[cfg(feature = "testkit")]
mod agent_models;
#[cfg(feature = "testkit")]
mod agent_steering;
#[cfg(feature = "testkit")]
mod agent_structured;
#[cfg(feature = "testkit")]
mod approval;
#[cfg(feature = "testkit")]
mod cache_prefix_tracking;
#[cfg(feature = "testkit")]
mod composed;
#[cfg(feature = "testkit")]
mod context_compaction;
#[cfg(feature = "testkit")]
mod emit;
mod error;
#[cfg(feature = "testkit")]
mod event_forwarder;
#[cfg(feature = "testkit")]
mod fallback;
#[cfg(feature = "testkit")]
mod fn_tool;
#[cfg(feature = "testkit")]
mod handle;
#[cfg(feature = "testkit")]
mod incomplete_tool_call_sanitize;
#[cfg(feature = "testkit")]
mod integration;
#[cfg(feature = "testkit")]
mod loop_overflow;
#[cfg(feature = "testkit")]
mod manual_compaction;
#[cfg(feature = "testkit")]
mod message_end_pricing;
#[cfg(feature = "testkit")]
mod message_provider;
#[cfg(feature = "testkit")]
mod messaging;
mod model_connections;
#[cfg(feature = "testkit")]
mod otel_spans;
#[cfg(feature = "testkit")]
mod pause_resume;
#[cfg(all(feature = "plugins", feature = "testkit"))]
mod plugin_integration;
#[cfg(feature = "plugins")]
mod plugin_registry;
#[cfg(feature = "testkit")]
mod policy_slots;
mod pre_policy_panic_rollback;
mod property;
mod public_api;
#[cfg(feature = "testkit")]
mod reasoning_only;
#[cfg(feature = "testkit")]
mod registry;
mod retry;
mod schema;
mod stream;
mod stream_accumulate_errors;
#[cfg(feature = "testkit")]
mod stream_drop_history;
#[cfg(feature = "testkit")]
mod stream_middleware;
#[cfg(feature = "testkit")]
mod stream_resilience;
#[cfg(feature = "testkit")]
mod stress_concurrent_tool_calls;
#[cfg(feature = "testkit")]
mod stress_concurrent_tools;
#[cfg(feature = "testkit")]
mod stress_conversation;
#[cfg(feature = "testkit")]
mod stress_orchestrator;
#[cfg(feature = "testkit")]
mod sub_agent;
#[cfg(feature = "tiktoken")]
mod tiktoken_counter;
#[cfg(feature = "testkit")]
mod tool;
#[cfg(feature = "testkit")]
mod tool_execution_policy;
#[cfg(feature = "testkit")]
mod tool_middleware;
#[cfg(feature = "builtin-tools")]
mod tools;
#[cfg(feature = "testkit")]
mod transfer;
mod types;
mod workspace_tokio_manifest;
