//! Integration tests for the pipeline crate: happy path, merge strategies,
//! exit conditions, error propagation, and lifecycle event ordering.
//!
//! These compile as an external crate, so they also verify that the
//! `#[non_exhaustive]` public types keep a complete construction path.

#![cfg(feature = "pipelines")]

mod support {
    use std::sync::{Arc, Mutex, PoisonError};

    use swink_agent::Agent;
    use swink_agent::AgentOptions;
    use swink_agent::AssistantMessageEvent;
    use swink_agent::testing::{MockStreamFn, default_convert, default_model, text_only_events};
    use swink_agent_patterns::{
        Pipeline, PipelineEvent, PipelineExecutor, PipelineId, PipelineRegistry, SimpleAgentFactory,
    };

    /// Build an agent that replies with a fixed text response.
    fn text_agent(text: &str) -> Agent {
        let options = AgentOptions::new(
            "test",
            default_model(),
            Arc::new(MockStreamFn::new(vec![text_only_events(text)])),
            default_convert,
        );
        Agent::new(options)
    }

    /// Build a factory with one fixed-response agent per `(name, response)` pair.
    pub fn text_factory(agents: &[(&str, &str)]) -> SimpleAgentFactory {
        let mut factory = SimpleAgentFactory::new();
        for (name, response) in agents {
            let response = (*response).to_owned();
            factory.register(*name, move || text_agent(&response));
        }
        factory
    }

    /// Build a factory whose single named agent pops one scripted event
    /// sequence per creation, so each loop iteration sees the next response.
    pub fn scripted_factory(
        name: &str,
        responses: Vec<Vec<AssistantMessageEvent>>,
    ) -> SimpleAgentFactory {
        let responses = Arc::new(Mutex::new(responses));
        let mut factory = SimpleAgentFactory::new();
        factory.register(name, move || {
            let next = {
                let mut guard = responses.lock().unwrap_or_else(PoisonError::into_inner);
                if guard.is_empty() {
                    vec![]
                } else {
                    vec![guard.remove(0)]
                }
            };
            let options = AgentOptions::new(
                "scripted",
                default_model(),
                Arc::new(MockStreamFn::new(next)),
                default_convert,
            );
            Agent::new(options)
        });
        factory
    }

    /// Register the pipeline and build an executor over the given factory.
    pub fn executor_for(
        factory: SimpleAgentFactory,
        pipeline: Pipeline,
    ) -> (PipelineExecutor, PipelineId) {
        let id = pipeline.id().clone();
        let registry = PipelineRegistry::new();
        registry.register(pipeline);
        let executor = PipelineExecutor::new(Arc::new(factory), Arc::new(registry));
        (executor, id)
    }

    /// Like [`executor_for`], but also records lifecycle events.
    pub fn executor_with_events(
        factory: SimpleAgentFactory,
        pipeline: Pipeline,
    ) -> (PipelineExecutor, PipelineId, Arc<Mutex<Vec<PipelineEvent>>>) {
        let id = pipeline.id().clone();
        let registry = PipelineRegistry::new();
        registry.register(pipeline);
        let events: Arc<Mutex<Vec<PipelineEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let executor = PipelineExecutor::new(Arc::new(factory), Arc::new(registry))
            .with_event_handler(move |event| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(event);
            });
        (executor, id, events)
    }
}

mod happy_path {
    use swink_agent_patterns::Pipeline;
    use tokio_util::sync::CancellationToken;

    use crate::support::{executor_for, text_factory};

    #[tokio::test]
    async fn sequential_pipeline_end_to_end() {
        let factory = text_factory(&[("researcher", "notes"), ("writer", "final draft")]);
        let pipeline = Pipeline::sequential(
            "research-then-write",
            vec!["researcher".into(), "writer".into()],
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "topic".into(), token).await.unwrap();

        assert_eq!(output.pipeline_id, id);
        assert_eq!(output.final_response, "final draft");
        assert_eq!(output.steps.len(), 2);
        assert_eq!(output.steps[0].agent_name, "researcher");
        assert_eq!(output.steps[0].response, "notes");
        assert_eq!(output.steps[1].agent_name, "writer");
        assert_eq!(output.steps[1].response, "final draft");
    }

    #[tokio::test]
    async fn sequential_with_context_end_to_end() {
        let factory = text_factory(&[("researcher", "notes"), ("writer", "final draft")]);
        let pipeline = Pipeline::sequential_with_context(
            "research-then-write",
            vec!["researcher".into(), "writer".into()],
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "topic".into(), token).await.unwrap();

        assert_eq!(output.final_response, "final draft");
        assert_eq!(output.steps.len(), 2);
    }
}

mod merge_strategies {
    use swink_agent_patterns::{MergeStrategy, Pipeline};
    use tokio_util::sync::CancellationToken;

    use crate::support::{executor_for, text_factory};

    #[tokio::test]
    async fn concat_merges_all_branches_in_declaration_order() {
        let factory = text_factory(&[("a", "alpha"), ("b", "bravo"), ("c", "charlie")]);
        let pipeline = Pipeline::parallel(
            "fan-out-concat",
            vec!["a".into(), "b".into(), "c".into()],
            MergeStrategy::Concat {
                separator: " | ".to_owned(),
            },
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "input".into(), token).await.unwrap();

        assert_eq!(output.final_response, "alpha | bravo | charlie");
        assert_eq!(output.steps.len(), 3);
        assert_eq!(output.steps[0].agent_name, "a");
        assert_eq!(output.steps[1].agent_name, "b");
        assert_eq!(output.steps[2].agent_name, "c");
    }

    #[tokio::test]
    async fn first_returns_exactly_one_branch() {
        let factory = text_factory(&[("a", "alpha"), ("b", "bravo")]);
        let pipeline = Pipeline::parallel(
            "fan-out-first",
            vec!["a".into(), "b".into()],
            MergeStrategy::First,
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "input".into(), token).await.unwrap();

        assert_eq!(output.steps.len(), 1);
        assert!(
            output.final_response == "alpha" || output.final_response == "bravo",
            "unexpected response: {}",
            output.final_response
        );
    }

    #[tokio::test]
    async fn fastest_returns_first_n_branches() {
        let factory = text_factory(&[("a", "alpha"), ("b", "bravo"), ("c", "charlie")]);
        let pipeline = Pipeline::parallel(
            "fan-out-fastest",
            vec!["a".into(), "b".into(), "c".into()],
            MergeStrategy::Fastest { n: 2 },
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "input".into(), token).await.unwrap();

        assert_eq!(output.steps.len(), 2);
    }

    #[tokio::test]
    async fn custom_routes_branch_outputs_through_aggregator() {
        let factory = text_factory(&[("a", "alpha"), ("b", "bravo"), ("agg", "aggregated")]);
        let pipeline = Pipeline::parallel(
            "fan-out-custom",
            vec!["a".into(), "b".into()],
            MergeStrategy::Custom {
                aggregator: "agg".to_owned(),
            },
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "input".into(), token).await.unwrap();

        assert_eq!(output.final_response, "aggregated");
        assert_eq!(
            output.steps.len(),
            3,
            "two branches plus the aggregator step"
        );
        assert_eq!(output.steps[2].agent_name, "agg");
    }
}

mod exit_conditions {
    use swink_agent::testing::{text_events, tool_call_events};
    use swink_agent_patterns::{ExitCondition, Pipeline, PipelineError};
    use tokio_util::sync::CancellationToken;

    use crate::support::{executor_for, scripted_factory, text_factory};

    #[tokio::test]
    async fn tool_called_exits_loop_on_matching_tool() {
        let factory = scripted_factory(
            "body",
            vec![
                text_events("still working"),
                tool_call_events("tc-1", "finish", "{}"),
            ],
        );
        let pipeline = Pipeline::loop_with_max(
            "loop-tool",
            "body",
            ExitCondition::ToolCalled {
                tool_name: "finish".to_owned(),
            },
            5,
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "go".into(), token).await.unwrap();

        assert_eq!(output.steps.len(), 2, "should exit on the second iteration");
    }

    #[tokio::test]
    async fn output_contains_exits_loop_on_regex_match() {
        let factory = scripted_factory(
            "body",
            vec![text_events("still working"), text_events("all done DONE")],
        );
        let exit = ExitCondition::output_contains(r"\bDONE\b").unwrap();
        let pipeline = Pipeline::loop_with_max("loop-regex", "body", exit, 5);
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "go".into(), token).await.unwrap();

        assert_eq!(output.steps.len(), 2);
        assert!(output.final_response.contains("DONE"));
    }

    #[tokio::test]
    async fn max_iterations_runs_to_cap_and_succeeds() {
        let factory = text_factory(&[("body", "same every time")]);
        let pipeline = Pipeline::loop_with_max("loop-cap", "body", ExitCondition::MaxIterations, 3);
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let output = executor.run(&id, "go".into(), token).await.unwrap();

        assert_eq!(output.steps.len(), 3);
        assert_eq!(output.final_response, "same every time");
    }

    #[tokio::test]
    async fn unmet_exit_condition_errors_at_cap() {
        let factory = text_factory(&[("body", "never matches")]);
        let exit = ExitCondition::output_contains("FINISHED").unwrap();
        let pipeline = Pipeline::loop_with_max("loop-unmet", "body", exit, 2);
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let result = executor.run(&id, "go".into(), token).await;

        assert!(
            matches!(
                result,
                Err(PipelineError::MaxIterationsReached { iterations: 2 })
            ),
            "expected MaxIterationsReached at the cap"
        );
    }
}

mod errors {
    use swink_agent_patterns::{MergeStrategy, Pipeline, PipelineError, PipelineId};
    use tokio_util::sync::CancellationToken;

    use crate::support::{executor_for, text_factory};

    #[tokio::test]
    async fn failing_step_surfaces_agent_not_found() {
        // The first step succeeds; the second step's agent is not registered.
        let factory = text_factory(&[("a", "alpha")]);
        let pipeline = Pipeline::sequential("halts", vec!["a".into(), "ghost".into()]);
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let result = executor.run(&id, "input".into(), token).await;

        assert!(
            matches!(result, Err(PipelineError::AgentNotFound { name }) if name == "ghost"),
            "expected AgentNotFound for the missing second step"
        );
    }

    #[tokio::test]
    async fn unknown_pipeline_id_surfaces_pipeline_not_found() {
        let pipeline = Pipeline::sequential("registered", vec![]);
        let (executor, _id) = executor_for(text_factory(&[]), pipeline);
        let missing = PipelineId::new("missing");
        let token = CancellationToken::new();

        let result = executor.run(&missing, "input".into(), token).await;

        assert!(
            matches!(result, Err(PipelineError::PipelineNotFound { id }) if id == missing),
            "expected PipelineNotFound for an unregistered id"
        );
    }

    #[tokio::test]
    async fn panicking_branch_surfaces_step_failed() {
        let mut factory = text_factory(&[]);
        factory.register("boom", || panic!("branch builder panic"));
        let pipeline = Pipeline::parallel(
            "panicking",
            vec!["boom".into()],
            MergeStrategy::Concat {
                separator: String::new(),
            },
        );
        let (executor, id) = executor_for(factory, pipeline);
        let token = CancellationToken::new();

        let result = executor.run(&id, "input".into(), token).await;

        assert!(
            matches!(result, Err(PipelineError::StepFailed { step_index: 0, agent_name, .. }) if agent_name == "boom"),
            "expected StepFailed for the panicking branch"
        );
    }
}

mod events {
    use swink_agent_patterns::{ExitCondition, MergeStrategy, Pipeline, PipelineEvent};
    use tokio_util::sync::CancellationToken;

    use crate::support::{executor_with_events, text_factory};

    #[tokio::test]
    async fn sequential_emits_ordered_lifecycle_events() {
        let factory = text_factory(&[("a", "alpha"), ("b", "bravo")]);
        let pipeline = Pipeline::sequential("two-step", vec!["a".into(), "b".into()]);
        let (executor, id, events) = executor_with_events(factory, pipeline);
        let token = CancellationToken::new();

        executor.run(&id, "input".into(), token).await.unwrap();

        let captured = events.lock().unwrap();
        assert_eq!(
            captured.len(),
            6,
            "expected Started + 2*(StepStarted + StepCompleted) + Completed"
        );
        assert!(
            matches!(&captured[0], PipelineEvent::Started { pipeline_name, .. } if pipeline_name == "two-step")
        );
        assert!(
            matches!(&captured[1], PipelineEvent::StepStarted { step_index: 0, agent_name, .. } if agent_name == "a")
        );
        assert!(
            matches!(&captured[2], PipelineEvent::StepCompleted { step_index: 0, agent_name, .. } if agent_name == "a")
        );
        assert!(
            matches!(&captured[3], PipelineEvent::StepStarted { step_index: 1, agent_name, .. } if agent_name == "b")
        );
        assert!(
            matches!(&captured[4], PipelineEvent::StepCompleted { step_index: 1, agent_name, .. } if agent_name == "b")
        );
        assert!(matches!(&captured[5], PipelineEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn parallel_emits_started_first_and_completed_last() {
        let factory = text_factory(&[("a", "alpha"), ("b", "bravo")]);
        let pipeline = Pipeline::parallel(
            "fan-out",
            vec!["a".into(), "b".into()],
            MergeStrategy::Concat {
                separator: " ".to_owned(),
            },
        );
        let (executor, id, events) = executor_with_events(factory, pipeline);
        let token = CancellationToken::new();

        executor.run(&id, "input".into(), token).await.unwrap();

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 6);
        assert!(
            matches!(&captured[0], PipelineEvent::Started { pipeline_name, .. } if pipeline_name == "fan-out"),
            "expected Started first, got: {captured:?}"
        );
        assert!(
            matches!(captured.last(), Some(PipelineEvent::Completed { .. })),
            "expected Completed last, got: {captured:?}"
        );
        for agent in ["a", "b"] {
            assert!(
                captured
                    .iter()
                    .any(|event| matches!(event, PipelineEvent::StepStarted { agent_name, .. } if agent_name == agent)),
                "missing StepStarted for {agent}"
            );
            assert!(
                captured
                    .iter()
                    .any(|event| matches!(event, PipelineEvent::StepCompleted { agent_name, .. } if agent_name == agent)),
                "missing StepCompleted for {agent}"
            );
        }
    }

    #[tokio::test]
    async fn loop_emits_step_events_per_iteration() {
        let factory = text_factory(&[("body", "spin")]);
        let pipeline =
            Pipeline::loop_with_max("loop-twice", "body", ExitCondition::MaxIterations, 2);
        let (executor, id, events) = executor_with_events(factory, pipeline);
        let token = CancellationToken::new();

        executor.run(&id, "input".into(), token).await.unwrap();

        let captured = events.lock().unwrap();
        assert_eq!(
            captured.len(),
            6,
            "expected Started + 2*(StepStarted + StepCompleted) + Completed"
        );
        assert!(
            matches!(&captured[0], PipelineEvent::Started { pipeline_name, .. } if pipeline_name == "loop-twice")
        );
        assert!(matches!(
            &captured[1],
            PipelineEvent::StepStarted { step_index: 0, .. }
        ));
        assert!(matches!(
            &captured[3],
            PipelineEvent::StepStarted { step_index: 1, .. }
        ));
        assert!(matches!(&captured[5], PipelineEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn failed_pipeline_emits_failed_event_and_no_completed() {
        let factory = text_factory(&[]);
        let pipeline = Pipeline::sequential("failing", vec!["ghost".into()]);
        let (executor, id, events) = executor_with_events(factory, pipeline);
        let token = CancellationToken::new();

        let result = executor.run(&id, "input".into(), token).await;
        assert!(result.is_err());

        let captured = events.lock().unwrap();
        assert!(matches!(&captured[0], PipelineEvent::Started { .. }));
        assert!(
            matches!(
                captured.last(),
                Some(PipelineEvent::Failed { error_message, .. }) if error_message.contains("ghost")
            ),
            "expected a trailing Failed event naming the missing agent, got: {captured:?}"
        );
        assert!(
            !captured
                .iter()
                .any(|event| matches!(event, PipelineEvent::Completed { .. })),
            "a failed pipeline must not emit Completed"
        );
    }
}
