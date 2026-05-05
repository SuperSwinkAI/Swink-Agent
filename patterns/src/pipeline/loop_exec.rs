//! Loop pipeline execution.

use std::sync::Arc;
use std::time::Instant;

use swink_agent::{AgentMessage, AgentResult, ContentBlock, LlmMessage, Usage};
use tokio_util::sync::CancellationToken;

use super::events::PipelineEvent;
use super::executor::AgentFactory;
use super::output::{PipelineError, PipelineOutput, StepResult};
use super::types::{ExitCondition, PipelineId};

/// Execute a loop pipeline: run the body agent repeatedly until an exit condition
/// is met or `max_iterations` is reached.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_loop(
    factory: &Arc<dyn AgentFactory>,
    event_handler: &Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>,
    id: PipelineId,
    name: String,
    body: String,
    exit_condition: ExitCondition,
    max_iterations: usize,
    input: String,
    cancellation_token: CancellationToken,
) -> Result<PipelineOutput, PipelineError> {
    if let Some(handler) = event_handler {
        handler(PipelineEvent::Started {
            pipeline_id: id.clone(),
            pipeline_name: name,
        });
    }

    let pipeline_start = Instant::now();
    let mut steps: Vec<StepResult> = Vec::new();
    let mut total_usage = Usage::default();
    let mut accumulated_responses: Vec<String> = Vec::new();

    for iteration in 0..max_iterations {
        // Check cancellation before each iteration.
        if cancellation_token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }

        // Emit step-started event.
        if let Some(handler) = event_handler {
            handler(PipelineEvent::StepStarted {
                pipeline_id: id.clone(),
                step_index: iteration,
                agent_name: body.clone(),
            });
        }

        let step_start = Instant::now();

        // Create a fresh agent for this iteration.
        let mut agent = factory.create(&body)?;

        // Build input messages: original input + accumulated context from prior iterations.
        let mut messages = Vec::new();
        if accumulated_responses.is_empty() {
            messages.push(make_user_message(&input));
        } else {
            let context = format!(
                "{}\n\nPrevious iterations:\n{}",
                input,
                accumulated_responses
                    .iter()
                    .enumerate()
                    .map(|(i, r)| format!("Iteration {}: {}", i + 1, r))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            messages.push(make_user_message(&context));
        }

        // Run the agent.
        let result = agent
            .prompt_async(messages)
            .await
            .map_err(|e| PipelineError::StepFailed {
                step_index: iteration,
                agent_name: body.clone(),
                source: Box::new(e),
            })?;

        let step_duration = step_start.elapsed();
        let response_text = extract_text(&result);

        // Accumulate usage.
        total_usage.merge(&result.usage);

        // Record step result.
        let step = StepResult {
            agent_name: body.clone(),
            response: response_text.clone(),
            duration: step_duration,
            usage: result.usage.clone(),
        };
        steps.push(step);

        // Emit step-completed event.
        if let Some(handler) = event_handler {
            handler(PipelineEvent::StepCompleted {
                pipeline_id: id.clone(),
                step_index: iteration,
                agent_name: body.clone(),
                duration: step_duration,
                usage: result.usage.clone(),
            });
        }

        accumulated_responses.push(response_text.clone());

        // Check exit condition.
        let should_exit = match &exit_condition {
            ExitCondition::ToolCalled { tool_name } => check_tool_called(&result, tool_name),
            ExitCondition::OutputContains { compiled, .. } => compiled.is_match(&response_text),
            ExitCondition::MaxIterations => false, // Never triggers early exit.
        };

        if should_exit {
            let total_duration = pipeline_start.elapsed();
            if let Some(handler) = event_handler {
                handler(PipelineEvent::Completed {
                    pipeline_id: id.clone(),
                    total_duration,
                    total_usage: total_usage.clone(),
                });
            }
            return Ok(PipelineOutput {
                pipeline_id: id,
                final_response: response_text,
                steps,
                total_duration,
                total_usage,
            });
        }
    }

    // All iterations exhausted.
    match exit_condition {
        ExitCondition::MaxIterations => {
            // MaxIterations exit condition: success after running all iterations.
            let total_duration = pipeline_start.elapsed();
            let final_response = accumulated_responses.last().cloned().unwrap_or_default();
            if let Some(handler) = event_handler {
                handler(PipelineEvent::Completed {
                    pipeline_id: id.clone(),
                    total_duration,
                    total_usage: total_usage.clone(),
                });
            }
            Ok(PipelineOutput {
                pipeline_id: id,
                final_response,
                steps,
                total_duration,
                total_usage,
            })
        }
        _ => Err(PipelineError::MaxIterationsReached {
            iterations: max_iterations,
        }),
    }
}

/// Extract the text response from the last assistant message in an `AgentResult`.
fn extract_text(result: &AgentResult) -> String {
    result
        .messages
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Llm(LlmMessage::Assistant(msg)) => Some(msg),
            _ => None,
        })
        .map(|msg| {
            msg.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Check whether the agent result contains a tool call with the given name.
fn check_tool_called(result: &AgentResult, tool_name: &str) -> bool {
    result.messages.iter().any(|m| match m {
        AgentMessage::Llm(LlmMessage::Assistant(msg)) => msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { name, .. } if name == tool_name)),
        _ => false,
    })
}

/// Build a user message from plain text.
fn make_user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(swink_agent::UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, PoisonError};

    use swink_agent::AgentOptions;
    use swink_agent::testing::{
        MockStreamFn, default_convert, default_model, text_events, tool_call_events,
    };

    use crate::pipeline::executor::SimpleAgentFactory;

    /// Build a factory that creates agents returning the given event sequences.
    fn factory_with_responses(
        name: &str,
        responses: Vec<Vec<swink_agent::AssistantMessageEvent>>,
    ) -> Arc<SimpleAgentFactory> {
        let name = name.to_string();
        let responses = Arc::new(Mutex::new(responses));
        let mut factory = SimpleAgentFactory::new();
        factory.register(name, move || {
            // Pop the first response set for each agent creation.
            let next = pop_next_response(&responses);
            let options = AgentOptions::new(
                "loop-body",
                default_model(),
                Arc::new(MockStreamFn::new(next)),
                default_convert,
            );
            Agent::new(options)
        });
        Arc::new(factory)
    }

    fn pop_next_response(
        responses: &Mutex<Vec<Vec<swink_agent::AssistantMessageEvent>>>,
    ) -> Vec<Vec<swink_agent::AssistantMessageEvent>> {
        let mut guard = responses.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_empty() {
            vec![]
        } else {
            vec![guard.remove(0)]
        }
    }

    use swink_agent::Agent;

    #[test]
    fn response_queue_recovers_from_poisoned_lock() {
        let responses = Arc::new(Mutex::new(vec![text_events("after poison")]));
        let poisoned = responses.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = poisoned.lock().expect("lock before poison");
            panic!("poison response queue");
        });

        let next = pop_next_response(&responses);

        assert_eq!(next.len(), 1);
        assert!(
            responses
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );
    }

    // T038: ToolCalled exit — mock returns tool call on iteration 2
    #[tokio::test]
    async fn loop_exits_on_tool_called() {
        let factory = factory_with_responses(
            "body",
            vec![
                text_events("iteration 1 output"),
                tool_call_events("tc-1", "done", "{}"),
            ],
        );

        let result = run_loop(
            &(factory as Arc<dyn AgentFactory>),
            &None,
            PipelineId::new("test-loop"),
            "test".to_string(),
            "body".to_string(),
            ExitCondition::ToolCalled {
                tool_name: "done".to_string(),
            },
            10,
            "do something".to_string(),
            CancellationToken::new(),
        )
        .await;

        let output = result.expect("should succeed");
        assert_eq!(output.steps.len(), 2);
        // First step has text, second triggered the tool call exit.
        assert!(!output.steps[0].response.is_empty());
    }

    // T039: OutputContains exit — mock returns "DONE" on iteration 2, regex matches
    #[tokio::test]
    async fn loop_exits_on_output_contains() {
        let factory = factory_with_responses(
            "body",
            vec![
                text_events("still working..."),
                text_events("all finished DONE"),
            ],
        );

        let exit_cond = ExitCondition::output_contains(r"DONE").unwrap();

        let result = run_loop(
            &(factory as Arc<dyn AgentFactory>),
            &None,
            PipelineId::new("test-loop"),
            "test".to_string(),
            "body".to_string(),
            exit_cond,
            10,
            "process data".to_string(),
            CancellationToken::new(),
        )
        .await;

        let output = result.expect("should succeed");
        assert_eq!(output.steps.len(), 2);
        assert!(output.steps[1].response.contains("DONE"));
    }

    // T040: MaxIterationsReached — exit condition never met
    #[tokio::test]
    async fn loop_errors_when_max_iterations_reached() {
        let factory = factory_with_responses(
            "body",
            vec![
                text_events("iter 1"),
                text_events("iter 2"),
                text_events("iter 3"),
            ],
        );

        let exit_cond = ExitCondition::output_contains(r"NEVER_MATCHES").unwrap();

        let result = run_loop(
            &(factory as Arc<dyn AgentFactory>),
            &None,
            PipelineId::new("test-loop"),
            "test".to_string(),
            "body".to_string(),
            exit_cond,
            3,
            "input".to_string(),
            CancellationToken::new(),
        )
        .await;

        match result {
            Err(PipelineError::MaxIterationsReached { iterations }) => {
                assert_eq!(iterations, 3);
            }
            other => panic!("expected MaxIterationsReached, got: {other:?}"),
        }
    }

    // T041: Body agent error halts loop
    #[tokio::test]
    async fn loop_halts_on_agent_error() {
        // Body agent not registered → AgentNotFound on first iteration.
        let factory: Arc<dyn AgentFactory> = Arc::new(SimpleAgentFactory::new());

        let result = run_loop(
            &factory,
            &None,
            PipelineId::new("test-loop"),
            "test".to_string(),
            "body".to_string(),
            ExitCondition::MaxIterations,
            5,
            "input".to_string(),
            CancellationToken::new(),
        )
        .await;

        assert!(
            matches!(result, Err(PipelineError::AgentNotFound { .. })),
            "expected AgentNotFound, got: {result:?}"
        );
    }

    // T042: Context accumulates across iterations
    #[tokio::test]
    async fn loop_accumulates_context() {
        // Use a context-capturing approach: we verify that the factory is called
        // 3 times (one per iteration) and each step records the response.
        let factory = factory_with_responses(
            "body",
            vec![
                text_events("response A"),
                text_events("response B"),
                text_events("response C DONE"),
            ],
        );

        let exit_cond = ExitCondition::output_contains(r"DONE").unwrap();

        let result = run_loop(
            &(factory as Arc<dyn AgentFactory>),
            &None,
            PipelineId::new("test-loop"),
            "test".to_string(),
            "body".to_string(),
            exit_cond,
            10,
            "original input".to_string(),
            CancellationToken::new(),
        )
        .await;

        let output = result.expect("should succeed");
        assert_eq!(output.steps.len(), 3);
        assert_eq!(output.steps[0].response, "response A");
        assert_eq!(output.steps[1].response, "response B");
        assert!(output.steps[2].response.contains("DONE"));
    }

    // T043: MaxIterations exit condition runs to cap successfully
    #[tokio::test]
    async fn loop_max_iterations_exit_condition_succeeds() {
        let factory = factory_with_responses(
            "body",
            vec![
                text_events("iter 1"),
                text_events("iter 2"),
                text_events("iter 3"),
            ],
        );

        let result = run_loop(
            &(factory as Arc<dyn AgentFactory>),
            &None,
            PipelineId::new("test-loop"),
            "test".to_string(),
            "body".to_string(),
            ExitCondition::MaxIterations,
            3,
            "input".to_string(),
            CancellationToken::new(),
        )
        .await;

        let output = result.expect("MaxIterations should succeed after running all iterations");
        assert_eq!(output.steps.len(), 3);
        assert_eq!(output.final_response, "iter 3");
    }
}
