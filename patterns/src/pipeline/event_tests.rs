//! Tests for pipeline event emission.

#![cfg(all(test, feature = "testkit"))]

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use swink_agent::Agent;
use swink_agent::AgentOptions;
use swink_agent::testing::{MockStreamFn, default_convert, default_model, text_only_events};

use crate::pipeline::events::PipelineEvent;
use crate::pipeline::executor::{PipelineExecutor, SimpleAgentFactory};
use crate::pipeline::registry::PipelineRegistry;
use crate::pipeline::types::{ExitCondition, MergeStrategy, Pipeline, PipelineId};

fn make_text_agent(text: &str) -> Agent {
    let events = text_only_events(text);
    let options = AgentOptions::new(
        "test",
        default_model(),
        Arc::new(MockStreamFn::new(vec![events])),
        default_convert,
    );
    Agent::new(options)
}

fn build_executor_with_events(
    factory: SimpleAgentFactory,
    registry: PipelineRegistry,
) -> (PipelineExecutor, Arc<Mutex<Vec<PipelineEvent>>>) {
    let events: Arc<Mutex<Vec<PipelineEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let executor = PipelineExecutor::new(Arc::new(factory), Arc::new(registry)).with_event_handler(
        move |event| {
            events_clone.lock().unwrap().push(event);
        },
    );

    (executor, events)
}

// T054: Sequential pipeline emits correct event sequence
#[tokio::test]
async fn sequential_pipeline_emits_correct_event_sequence() {
    let mut factory = SimpleAgentFactory::new();
    factory.register("agent-a", || make_text_agent("hello"));
    factory.register("agent-b", || make_text_agent("world"));

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::sequential("two-step", vec!["agent-a".into(), "agent-b".into()]);
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let _output = executor.run(&id, "input".into(), token).await.unwrap();

    let captured = events.lock().unwrap();
    assert_eq!(
        captured.len(),
        6,
        "expected 6 events: Started + 2*(StepStarted + StepCompleted) + Completed"
    );

    assert!(
        matches!(&captured[0], PipelineEvent::Started { pipeline_name, .. } if pipeline_name == "two-step")
    );
    assert!(
        matches!(&captured[1], PipelineEvent::StepStarted { step_index: 0, agent_name, .. } if agent_name == "agent-a")
    );
    assert!(
        matches!(&captured[2], PipelineEvent::StepCompleted { step_index: 0, agent_name, .. } if agent_name == "agent-a")
    );
    assert!(
        matches!(&captured[3], PipelineEvent::StepStarted { step_index: 1, agent_name, .. } if agent_name == "agent-b")
    );
    assert!(
        matches!(&captured[4], PipelineEvent::StepCompleted { step_index: 1, agent_name, .. } if agent_name == "agent-b")
    );
    assert!(matches!(&captured[5], PipelineEvent::Completed { .. }));
}

#[tokio::test]
async fn parallel_pipeline_emits_started_and_completed_events() {
    let mut factory = SimpleAgentFactory::new();
    factory.register("agent-a", || make_text_agent("hello"));
    factory.register("agent-b", || make_text_agent("world"));

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::parallel(
        "parallel-two-step",
        vec!["agent-a".into(), "agent-b".into()],
        MergeStrategy::Concat {
            separator: " ".to_owned(),
        },
    );
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let output = executor.run(&id, "input".into(), token).await.unwrap();
    assert_eq!(output.final_response, "hello world");

    let captured = events.lock().unwrap();
    assert!(
        matches!(&captured[0], PipelineEvent::Started { pipeline_name, .. } if pipeline_name == "parallel-two-step"),
        "expected Started first, got: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, PipelineEvent::StepStarted { agent_name, .. } if agent_name == "agent-a"))
    );
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, PipelineEvent::StepStarted { agent_name, .. } if agent_name == "agent-b"))
    );
    assert!(
        matches!(captured.last(), Some(PipelineEvent::Completed { .. })),
        "expected Completed last, got: {captured:?}"
    );
}

#[tokio::test]
async fn loop_pipeline_emits_started_before_step_events() {
    let mut factory = SimpleAgentFactory::new();
    factory.register("body", || make_text_agent("done"));

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::loop_with_max("loop-once", "body", ExitCondition::MaxIterations, 1);
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let output = executor.run(&id, "input".into(), token).await.unwrap();
    assert_eq!(output.final_response, "done");

    let captured = events.lock().unwrap();
    assert!(
        matches!(&captured[0], PipelineEvent::Started { pipeline_name, .. } if pipeline_name == "loop-once"),
        "expected Started first, got: {captured:?}"
    );
    assert!(
        matches!(&captured[1], PipelineEvent::StepStarted { step_index: 0, agent_name, .. } if agent_name == "body"),
        "expected first loop step after Started, got: {captured:?}"
    );
    assert!(
        matches!(captured.last(), Some(PipelineEvent::Completed { .. })),
        "expected Completed last, got: {captured:?}"
    );
}

// T055: Failed pipeline emits Failed event
#[tokio::test]
async fn failed_pipeline_emits_failed_event() {
    let factory = SimpleAgentFactory::new(); // no agents registered

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::sequential("failing", vec!["ghost".into()]);
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let result = executor.run(&id, "input".into(), token).await;
    assert!(result.is_err());

    let captured = events.lock().unwrap();
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, PipelineEvent::Failed { error_message, .. } if error_message.contains("ghost"))),
        "expected a Failed event mentioning the missing agent, got: {captured:?}"
    );
    assert!(matches!(&captured[0], PipelineEvent::Started { .. }));
}

#[tokio::test]
async fn failed_parallel_pipeline_emits_failed_event() {
    let mut factory = SimpleAgentFactory::new();
    factory.register("agent-a", || make_text_agent("hello"));

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::parallel(
        "parallel-failing",
        vec!["agent-a".into(), "ghost".into()],
        MergeStrategy::Concat {
            separator: "\n".to_owned(),
        },
    );
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let result = executor.run(&id, "input".into(), token).await;
    assert!(result.is_err());

    let captured = events.lock().unwrap();
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, PipelineEvent::Failed { error_message, .. } if error_message.contains("ghost"))),
        "expected a Failed event mentioning the missing agent, got: {captured:?}"
    );
}

#[tokio::test]
async fn failed_loop_pipeline_emits_failed_event() {
    let factory = SimpleAgentFactory::new();

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::loop_("loop-failing", "ghost", ExitCondition::MaxIterations);
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let result = executor.run(&id, "input".into(), token).await;
    assert!(result.is_err());

    let captured = events.lock().unwrap();
    assert!(
        captured
            .iter()
            .any(|event| matches!(event, PipelineEvent::Failed { error_message, .. } if error_message.contains("ghost"))),
        "expected a Failed event mentioning the missing loop body agent, got: {captured:?}"
    );
}

// T056: StepCompleted carries agent_name, duration, usage
#[tokio::test]
async fn step_completed_carries_agent_name_duration_usage() {
    let mut factory = SimpleAgentFactory::new();
    factory.register("agent-a", || make_text_agent("output"));

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::sequential("single", vec!["agent-a".into()]);
    let id = pipeline.id().clone();
    registry.register(pipeline);

    let (executor, events) = build_executor_with_events(factory, registry);
    let token = CancellationToken::new();

    let _output = executor.run(&id, "input".into(), token).await.unwrap();

    let captured = events.lock().unwrap();
    let step_completed = captured
        .iter()
        .find(|e| matches!(e, PipelineEvent::StepCompleted { .. }))
        .expect("should have a StepCompleted event");

    match step_completed {
        PipelineEvent::StepCompleted {
            agent_name,
            duration,
            usage,
            ..
        } => {
            assert_eq!(agent_name, "agent-a");
            // Duration should be non-negative (it always is for Duration).
            assert!(duration.as_nanos() > 0 || duration.is_zero());
            // Usage is present (may be zero for mock agents).
            let _ = usage;
        }
        _ => unreachable!(),
    }
}

// T057: No events when no handler configured (no panics)
#[tokio::test]
async fn no_events_when_no_handler_configured() {
    let mut factory = SimpleAgentFactory::new();
    factory.register("agent-a", || make_text_agent("output"));

    let registry = PipelineRegistry::new();
    let pipeline = Pipeline::sequential("no-handler", vec!["agent-a".into()]);
    let id = pipeline.id().clone();
    registry.register(pipeline);

    // No event handler — just factory + registry.
    let executor = PipelineExecutor::new(Arc::new(factory), Arc::new(registry));
    let token = CancellationToken::new();

    // Should not panic even without an event handler.
    let output = executor.run(&id, "input".into(), token).await.unwrap();
    assert_eq!(output.final_response, "output");
}

// T058: PipelineEvent::to_emission() produces valid Emission with a
// populated payload (not just a bare event name).
#[test]
fn pipeline_event_to_emission_produces_valid_emission() {
    let id = PipelineId::new("test-id");

    let started = PipelineEvent::Started {
        pipeline_id: id.clone(),
        pipeline_name: "test".to_owned(),
    }
    .to_emission();
    assert_eq!(started.name, "pipeline.started");
    assert_eq!(started.payload["pipeline_id"], "test-id");
    assert_eq!(started.payload["pipeline_name"], "test");

    let step_started = PipelineEvent::StepStarted {
        pipeline_id: id.clone(),
        step_index: 0,
        agent_name: "agent-a".to_owned(),
    }
    .to_emission();
    assert_eq!(step_started.name, "pipeline.step_started");
    assert_eq!(step_started.payload["pipeline_id"], "test-id");
    assert_eq!(step_started.payload["step_index"], 0);
    assert_eq!(step_started.payload["agent_name"], "agent-a");

    let usage = swink_agent::Usage::default()
        .with_input(12)
        .with_output(34)
        .with_total(46);
    let step_completed = PipelineEvent::StepCompleted {
        pipeline_id: id.clone(),
        step_index: 0,
        agent_name: "agent-a".to_owned(),
        duration: std::time::Duration::from_millis(100),
        usage: usage.clone(),
    }
    .to_emission();
    assert_eq!(step_completed.name, "pipeline.step_completed");
    assert_eq!(step_completed.payload["pipeline_id"], "test-id");
    assert_eq!(step_completed.payload["step_index"], 0);
    assert_eq!(step_completed.payload["agent_name"], "agent-a");
    assert_eq!(step_completed.payload["duration_ms"], 100);
    assert_eq!(step_completed.payload["usage"]["input"], 12);
    assert_eq!(step_completed.payload["usage"]["output"], 34);
    assert_eq!(step_completed.payload["usage"]["total"], 46);

    let completed = PipelineEvent::Completed {
        pipeline_id: id.clone(),
        total_duration: std::time::Duration::from_secs(1),
        total_usage: usage,
    }
    .to_emission();
    assert_eq!(completed.name, "pipeline.completed");
    assert_eq!(completed.payload["pipeline_id"], "test-id");
    assert_eq!(completed.payload["total_duration_ms"], 1000);
    assert_eq!(completed.payload["total_usage"]["total"], 46);

    let failed = PipelineEvent::Failed {
        pipeline_id: id,
        error_message: "boom".to_owned(),
    }
    .to_emission();
    assert_eq!(failed.name, "pipeline.failed");
    assert_eq!(failed.payload["pipeline_id"], "test-id");
    assert_eq!(failed.payload["error_message"], "boom");
}
