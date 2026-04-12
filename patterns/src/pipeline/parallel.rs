//! Parallel pipeline execution.

use std::sync::Arc;
use std::time::Instant;

use swink_agent::{AgentMessage, ContentBlock, LlmMessage, Usage, UserMessage, now_timestamp};
use tokio_util::sync::CancellationToken;

use super::events::PipelineEvent;
use super::executor::AgentFactory;
use super::output::{PipelineError, PipelineOutput, StepResult};
use super::types::{MergeStrategy, PipelineId};

/// Result from a single branch execution.
struct BranchResult {
    index: usize,
    agent_name: String,
    response: String,
    duration: std::time::Duration,
    usage: Usage,
}

/// Execute branches in parallel and merge results according to the merge strategy.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_parallel(
    factory: &Arc<dyn AgentFactory>,
    event_handler: &Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>,
    id: PipelineId,
    _name: String,
    branches: Vec<String>,
    merge_strategy: MergeStrategy,
    input: String,
    cancellation_token: CancellationToken,
) -> Result<PipelineOutput, PipelineError> {
    if cancellation_token.is_cancelled() {
        return Err(PipelineError::Cancelled);
    }

    let pipeline_start = Instant::now();
    let child_token = cancellation_token.child_token();
    let branch_count = branches.len();
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<Result<BranchResult, PipelineError>>(branch_count);

    // Spawn a task for each branch.
    for (index, branch_name) in branches.iter().enumerate() {
        let factory = Arc::clone(factory);
        let branch_name = branch_name.clone();
        let input = input.clone();
        let tx = tx.clone();
        let token = child_token.clone();
        let id = id.clone();
        let handler = event_handler.clone();

        tokio::spawn(async move {
            if token.is_cancelled() {
                let _ = tx.send(Err(PipelineError::Cancelled)).await;
                return;
            }

            // Emit step-started event.
            if let Some(ref h) = handler {
                h(PipelineEvent::StepStarted {
                    pipeline_id: id.clone(),
                    step_index: index,
                    agent_name: branch_name.clone(),
                });
            }

            let step_start = Instant::now();

            let mut agent = match factory.create(&branch_name) {
                Ok(a) => a,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };

            let messages = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text { text: input }],
                timestamp: now_timestamp(),
                cache_hint: None,
            }))];

            let result = tokio::select! {
                _ = token.cancelled() => {
                    let _ = tx.send(Err(PipelineError::Cancelled)).await;
                    return;
                }
                res = agent.prompt_async(messages) => res,
            };

            let duration = step_start.elapsed();

            match result {
                Ok(agent_result) => {
                    let response = agent_result.assistant_text();
                    let usage = agent_result.usage.clone();

                    // Emit step-completed event.
                    if let Some(ref h) = handler {
                        h(PipelineEvent::StepCompleted {
                            pipeline_id: id,
                            step_index: index,
                            agent_name: branch_name.clone(),
                            duration,
                            usage: usage.clone(),
                        });
                    }

                    let _ = tx
                        .send(Ok(BranchResult {
                            index,
                            agent_name: branch_name,
                            response,
                            duration,
                            usage,
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(PipelineError::StepFailed {
                            step_index: index,
                            agent_name: branch_name,
                            source: Box::new(e),
                        }))
                        .await;
                }
            }
        });
    }

    // Drop our copy so the channel closes when all tasks finish.
    drop(tx);

    match merge_strategy {
        MergeStrategy::Concat { separator } => {
            merge_concat(&mut rx, branch_count, separator, id, pipeline_start).await
        }
        MergeStrategy::First => merge_first(&mut rx, id, pipeline_start, child_token).await,
        MergeStrategy::Fastest { n } => {
            merge_fastest(&mut rx, n, id, pipeline_start, child_token).await
        }
        MergeStrategy::Custom { aggregator } => {
            merge_custom(
                &mut rx,
                branch_count,
                aggregator,
                factory,
                event_handler,
                id,
                pipeline_start,
            )
            .await
        }
    }
}

/// Concat: wait for all branches, fail if any errors.
async fn merge_concat(
    rx: &mut tokio::sync::mpsc::Receiver<Result<BranchResult, PipelineError>>,
    branch_count: usize,
    separator: String,
    id: PipelineId,
    pipeline_start: Instant,
) -> Result<PipelineOutput, PipelineError> {
    let mut results: Vec<Option<BranchResult>> = (0..branch_count).map(|_| None).collect();

    while let Some(item) = rx.recv().await {
        let branch = item?;
        let idx = branch.index;
        results[idx] = Some(branch);
    }

    let mut steps = Vec::with_capacity(branch_count);
    let mut responses = Vec::with_capacity(branch_count);
    let mut total_usage = Usage::default();

    for slot in results {
        let branch = slot.expect("all branches should have completed");
        total_usage.merge(&branch.usage);
        responses.push(branch.response.clone());
        steps.push(StepResult {
            agent_name: branch.agent_name,
            response: branch.response,
            duration: branch.duration,
            usage: branch.usage,
        });
    }

    let final_response = responses.join(&separator);
    let total_duration = pipeline_start.elapsed();

    Ok(PipelineOutput {
        pipeline_id: id,
        final_response,
        steps,
        total_duration,
        total_usage,
    })
}

/// First: return the first completed branch, cancel the rest.
async fn merge_first(
    rx: &mut tokio::sync::mpsc::Receiver<Result<BranchResult, PipelineError>>,
    id: PipelineId,
    pipeline_start: Instant,
    child_token: CancellationToken,
) -> Result<PipelineOutput, PipelineError> {
    let mut first_error = None;

    // Wait for the first successful result.
    while let Some(item) = rx.recv().await {
        match item {
            Ok(branch) => {
                // Cancel remaining branches.
                child_token.cancel();

                let total_duration = pipeline_start.elapsed();
                let total_usage = branch.usage.clone();

                let step = StepResult {
                    agent_name: branch.agent_name,
                    response: branch.response.clone(),
                    duration: branch.duration,
                    usage: branch.usage,
                };

                return Ok(PipelineOutput {
                    pipeline_id: id,
                    final_response: step.response.clone(),
                    steps: vec![step],
                    total_duration,
                    total_usage,
                });
            }
            Err(e) => {
                tracing::warn!("parallel branch failed: {e}");
                first_error.get_or_insert(e);
                continue;
            }
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    Err(PipelineError::StepFailed {
        step_index: 0,
        agent_name: "parallel".to_owned(),
        source: "all parallel branches failed".into(),
    })
}

/// Fastest(n): collect first N results, cancel the rest.
async fn merge_fastest(
    rx: &mut tokio::sync::mpsc::Receiver<Result<BranchResult, PipelineError>>,
    n: usize,
    id: PipelineId,
    pipeline_start: Instant,
    child_token: CancellationToken,
) -> Result<PipelineOutput, PipelineError> {
    let mut collected: Vec<BranchResult> = Vec::with_capacity(n);
    let mut first_error = None;

    while let Some(item) = rx.recv().await {
        match item {
            Ok(branch) => {
                collected.push(branch);
                if collected.len() >= n {
                    // Cancel remaining branches.
                    child_token.cancel();
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("parallel branch failed during fastest: {e}");
                first_error.get_or_insert(e);
                continue;
            }
        }
    }

    if collected.is_empty() {
        return Err(first_error.unwrap_or_else(|| PipelineError::StepFailed {
            step_index: 0,
            agent_name: "parallel".to_owned(),
            source: "no parallel branches completed successfully".into(),
        }));
    }

    // Sort by declaration order for deterministic output.
    collected.sort_by_key(|r| r.index);

    let mut steps = Vec::with_capacity(collected.len());
    let mut responses = Vec::with_capacity(collected.len());
    let mut total_usage = Usage::default();

    for branch in collected {
        total_usage.merge(&branch.usage);
        responses.push(branch.response.clone());
        steps.push(StepResult {
            agent_name: branch.agent_name,
            response: branch.response,
            duration: branch.duration,
            usage: branch.usage,
        });
    }

    let final_response = responses.join("\n");
    let total_duration = pipeline_start.elapsed();

    Ok(PipelineOutput {
        pipeline_id: id,
        final_response,
        steps,
        total_duration,
        total_usage,
    })
}

/// Custom: wait for all branches, format outputs, pass to aggregator agent.
#[allow(clippy::too_many_arguments)]
async fn merge_custom(
    rx: &mut tokio::sync::mpsc::Receiver<Result<BranchResult, PipelineError>>,
    branch_count: usize,
    aggregator_name: String,
    factory: &Arc<dyn AgentFactory>,
    _event_handler: &Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>,
    id: PipelineId,
    pipeline_start: Instant,
) -> Result<PipelineOutput, PipelineError> {
    let mut results: Vec<Option<BranchResult>> = (0..branch_count).map(|_| None).collect();

    while let Some(item) = rx.recv().await {
        let branch = item?;
        let idx = branch.index;
        results[idx] = Some(branch);
    }

    // Format outputs as labeled text sections
    let mut formatted_parts = Vec::with_capacity(branch_count);
    let mut steps = Vec::with_capacity(branch_count);
    let mut total_usage = Usage::default();

    for slot in results {
        let branch = slot.expect("all branches should have completed");
        formatted_parts.push(format!("[{}]: {}", branch.agent_name, branch.response));
        total_usage += branch.usage.clone();
        steps.push(StepResult {
            agent_name: branch.agent_name,
            response: branch.response,
            duration: branch.duration,
            usage: branch.usage,
        });
    }

    let formatted = formatted_parts.join("\n\n");

    // Create aggregator agent and pass formatted outputs
    let mut aggregator = factory.create(&aggregator_name)?;
    let messages = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text: formatted }],
        timestamp: now_timestamp(),
        cache_hint: None,
    }))];

    let agg_result =
        aggregator
            .prompt_async(messages)
            .await
            .map_err(|e| PipelineError::StepFailed {
                step_index: branch_count,
                agent_name: aggregator_name.clone(),
                source: Box::new(e),
            })?;

    let final_response = agg_result.assistant_text();
    total_usage += agg_result.usage.clone();

    steps.push(StepResult {
        agent_name: aggregator_name,
        response: final_response.clone(),
        duration: pipeline_start.elapsed(),
        usage: agg_result.usage,
    });

    Ok(PipelineOutput {
        pipeline_id: id,
        final_response,
        steps,
        total_duration: pipeline_start.elapsed(),
        total_usage,
    })
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use std::sync::Arc;

    use tokio_util::sync::CancellationToken;

    use swink_agent::testing::{MockStreamFn, default_convert, default_model, text_only_events};
    use swink_agent::{Agent, AgentOptions};

    use super::super::executor::SimpleAgentFactory;
    use super::super::types::{MergeStrategy, PipelineId};

    fn make_factory(agents: Vec<(&str, &str)>) -> Arc<SimpleAgentFactory> {
        let mut factory = SimpleAgentFactory::new();
        for (name, response) in agents {
            let response = response.to_owned();
            let name = name.to_owned();
            factory.register(name, move || {
                let events = text_only_events(&response);
                let options = AgentOptions::new(
                    "test",
                    default_model(),
                    Arc::new(MockStreamFn::new(vec![events])),
                    default_convert,
                );
                Agent::new(options)
            });
        }
        Arc::new(factory)
    }

    // T028: Concat merges all outputs in declaration order
    #[tokio::test]
    async fn concat_merges_all_outputs_in_order() {
        let factory = make_factory(vec![
            ("agent-a", "alpha"),
            ("agent-b", "bravo"),
            ("agent-c", "charlie"),
        ]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-concat"),
            "test".to_owned(),
            vec!["agent-a".into(), "agent-b".into(), "agent-c".into()],
            MergeStrategy::Concat {
                separator: " | ".to_owned(),
            },
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.final_response, "alpha | bravo | charlie");
        assert_eq!(result.steps.len(), 3);
        assert_eq!(result.steps[0].agent_name, "agent-a");
        assert_eq!(result.steps[1].agent_name, "agent-b");
        assert_eq!(result.steps[2].agent_name, "agent-c");
    }

    // T029: First returns first completed
    #[tokio::test]
    async fn first_returns_one_result() {
        let factory = make_factory(vec![("agent-a", "alpha"), ("agent-b", "bravo")]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-first"),
            "test".to_owned(),
            vec!["agent-a".into(), "agent-b".into()],
            MergeStrategy::First,
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        // First strategy returns exactly one step.
        assert_eq!(result.steps.len(), 1);
        // The response should be from one of the agents.
        assert!(
            result.final_response == "alpha" || result.final_response == "bravo",
            "unexpected response: {}",
            result.final_response
        );
    }

    // T030: Fastest(2) returns two results
    #[tokio::test]
    async fn fastest_returns_n_results() {
        let factory = make_factory(vec![
            ("agent-a", "alpha"),
            ("agent-b", "bravo"),
            ("agent-c", "charlie"),
        ]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-fastest"),
            "test".to_owned(),
            vec!["agent-a".into(), "agent-b".into(), "agent-c".into()],
            MergeStrategy::Fastest { n: 2 },
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.steps.len(), 2);
    }

    #[tokio::test]
    async fn first_returns_real_branch_error_when_all_branches_fail() {
        let factory = make_factory(vec![]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-first-all-fail"),
            "test".to_owned(),
            vec!["agent-a".into(), "agent-b".into()],
            MergeStrategy::First,
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await;

        assert!(
            matches!(result, Err(super::PipelineError::AgentNotFound { ref name }) if name == "agent-a" || name == "agent-b"),
            "expected a real branch error, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn fastest_returns_real_branch_error_when_all_branches_fail() {
        let factory = make_factory(vec![]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-fastest-all-fail"),
            "test".to_owned(),
            vec!["agent-a".into(), "agent-b".into()],
            MergeStrategy::Fastest { n: 2 },
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await;

        assert!(
            matches!(result, Err(super::PipelineError::AgentNotFound { ref name }) if name == "agent-a" || name == "agent-b"),
            "expected a real branch error, got: {result:?}"
        );
    }

    // T031: Concat fails if any branch errors
    #[tokio::test]
    async fn concat_fails_if_any_branch_errors() {
        // Register only "agent-a" — "agent-missing" is absent from factory.
        let factory = make_factory(vec![("agent-a", "alpha")]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-fail"),
            "test".to_owned(),
            vec!["agent-a".into(), "agent-missing".into()],
            MergeStrategy::Concat {
                separator: "\n".to_owned(),
            },
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("agent-missing") || msg.contains("not found"),
            "error should mention the missing agent: {msg}"
        );
    }

    // T032: Cancellation propagates
    #[tokio::test]
    async fn cancellation_before_run_returns_cancelled() {
        let factory = make_factory(vec![("agent-a", "alpha")]);
        let token = CancellationToken::new();
        token.cancel(); // Cancel before running.

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-cancel"),
            "test".to_owned(),
            vec!["agent-a".into()],
            MergeStrategy::First,
            "hello".to_owned(),
            token,
        )
        .await;

        assert!(matches!(result, Err(super::PipelineError::Cancelled)));
    }

    // T033: Single branch works
    #[tokio::test]
    async fn single_branch_works() {
        let factory = make_factory(vec![("solo", "only-one")]);

        let result = super::run_parallel(
            &(factory as Arc<dyn super::super::executor::AgentFactory>),
            &None,
            PipelineId::new("test-single"),
            "test".to_owned(),
            vec!["solo".into()],
            MergeStrategy::Concat {
                separator: "".to_owned(),
            },
            "hello".to_owned(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.final_response, "only-one");
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].agent_name, "solo");
    }
}
