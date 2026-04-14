//! Tool dispatch engine — split into explicit phases.
//!
//! - **Pre-process** (`preprocess`): pre-dispatch policies, approval gate, argument rewriting.
//! - **Execute** (`execute`): grouping, credential resolution, spawned tool execution.
//! - **Collect** (`collect`): result ordering, interrupt detection, outcome assembly.
//! - **Shared** (`shared`): helpers used across phases.

mod collect;
mod execute;
mod preprocess;
mod shared;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::tool::AgentTool;
use crate::types::ToolResultMessage;

use super::{AgentEvent, AgentLoopConfig, ToolCallInfo, ToolExecOutcome, emit};

use collect::GroupOutcome;
use execute::DispatchResult;

// ─── Shared types ───────────────────────────────────────────────────────────

/// A tool call that has passed approval, transformation, and validation gates.
struct PreparedToolCall {
    /// Index in the original `tool_calls` slice.
    idx: usize,
    /// Effective arguments after approval override and transformation.
    effective_arguments: serde_json::Value,
}

/// Order results to match the original `tool_calls` order, returning only
/// those whose IDs appear in the result set.
fn order_results_by_tool_calls(
    tool_calls: &[ToolCallInfo],
    all_results: &[(usize, ToolResultMessage)],
) -> Vec<ToolResultMessage> {
    let result_map: HashMap<&str, &ToolResultMessage> = all_results
        .iter()
        .map(|(_, r)| (r.tool_call_id.as_str(), r))
        .collect();
    let mut ordered: Vec<ToolResultMessage> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        if let Some(result) = result_map.get(tc.id.as_str()) {
            ordered.push((*result).clone());
        }
    }
    ordered
}

// ─── Public entry point ─────────────────────────────────────────────────────

/// Execute tool calls using the configured [`ToolExecutionPolicy`].
///
/// Pre-processing (approval, transformation, validation) runs for every tool
/// call regardless of policy. The policy controls how the actual execution
/// is dispatched:
///
/// - **Concurrent** — spawn all at once via `tokio::spawn` (default).
/// - **Sequential** — execute one at a time in order.
/// - **Priority** — group by priority, execute groups sequentially (concurrent
///   within each group).
/// - **Custom** — delegate grouping to a [`ToolExecutionStrategy`](crate::tool_execution_policy::ToolExecutionStrategy).
#[allow(clippy::too_many_lines)]
pub async fn execute_tools_concurrently(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> ToolExecOutcome {
    use tokio::sync::Mutex;

    let tool_names: Vec<&str> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
    info!(
        tool_count = tool_calls.len(),
        tools = ?tool_names,
        policy = ?config.tool_execution_policy,
        "dispatching tool batch"
    );

    let batch_token = cancellation_token.child_token();
    let results: Arc<Mutex<Vec<(usize, ToolResultMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let tool_timings: Arc<Mutex<Vec<crate::metrics::ToolExecMetrics>>> =
        Arc::new(Mutex::new(Vec::new()));
    let steering_messages: Arc<Mutex<Vec<crate::types::AgentMessage>>> =
        Arc::new(Mutex::new(Vec::new()));
    let steering_detected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transfer_detected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transfer_signal: Arc<Mutex<Option<crate::transfer::TransferSignal>>> =
        Arc::new(Mutex::new(None));

    let tool_map: HashMap<&str, &Arc<dyn AgentTool>> =
        config.tools.iter().map(|t| (t.name(), t)).collect();

    // Phase 1: Pre-process — policies, approval, argument rewriting.
    let preprocess::PreprocessResult {
        prepared,
        injected_messages,
    } = match preprocess::preprocess_tool_calls(
        config,
        tool_calls,
        &tool_map,
        &results,
        &tool_timings,
        tx,
    )
    .await
    {
        Ok(result) => result,
        Err(early_outcome) => return early_outcome,
    };

    // Phase 2: Compute execution groups and dispatch.
    let groups =
        execute::compute_execution_groups(&config.tool_execution_policy, tool_calls, &prepared)
            .await;

    // Phase 3: Execute each group and collect results.
    for group in groups {
        let mut handles = Vec::new();

        for &prepared_idx in &group {
            let prep = &prepared[prepared_idx];
            let tc = &tool_calls[prep.idx];

            let handle = execute::dispatch_single_tool(
                &tool_map,
                config,
                tc,
                &prep.effective_arguments,
                prep.idx,
                &batch_token,
                &results,
                &tool_timings,
                &steering_messages,
                &steering_detected,
                &transfer_detected,
                &transfer_signal,
                tx,
            )
            .await;

            match handle {
                DispatchResult::Spawned(h) => handles.push((prep.idx, h)),
                DispatchResult::Inline => {}
                DispatchResult::ChannelClosed => return ToolExecOutcome::ChannelClosed,
            }
        }

        let group_outcome = collect::collect_group_results(
            tool_calls,
            handles,
            &results,
            &steering_detected,
            &transfer_detected,
            &batch_token,
        )
        .await;

        match group_outcome {
            GroupOutcome::Continue => {}
            GroupOutcome::SteeringInterrupt => {
                return collect::build_steering_outcome(
                    config,
                    tool_calls,
                    results,
                    tool_timings,
                    steering_messages,
                    injected_messages,
                )
                .await;
            }
            GroupOutcome::TransferInterrupt => {
                return collect::build_transfer_outcome(
                    tool_calls,
                    results,
                    tool_timings,
                    transfer_signal,
                    injected_messages,
                )
                .await;
            }
        }
    }

    // All groups completed without interrupts.
    let all_results = std::mem::take(&mut *results.lock().await);
    let ordered = order_results_by_tool_calls(tool_calls, &all_results);

    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
    let captured_transfer = transfer_signal.lock().await.take();
    ToolExecOutcome::Completed {
        results: ordered,
        tool_metrics: collected_timings,
        transfer_signal: captured_transfer,
        injected_messages,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;

    use std::future::Future;
    use std::path::PathBuf;
    use std::sync::Arc as StdArc;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::{pin::Pin, sync::Mutex as StdSyncMutex};

    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use crate::MessageProvider;
    use crate::policy::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
    use crate::testing::{MockStreamFn, MockTool, default_convert, default_model};
    use crate::tool::{AgentToolResult, ApprovalMode};
    use crate::types::{AgentMessage, ContentBlock, LlmMessage, UserMessage};
    use crate::{DefaultRetryStrategy, StreamOptions, ToolApproval, ToolExecutionPolicy};

    struct BurstUpdatingTool {
        update_count: usize,
    }

    struct OneShotSteeringProvider {
        poll_count: AtomicU32,
    }

    impl MessageProvider for OneShotSteeringProvider {
        fn poll_steering(&self) -> Vec<AgentMessage> {
            if self.poll_count.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                    content: vec![ContentBlock::Text {
                        text: "redirect".to_string(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                }))]
            } else {
                vec![]
            }
        }

        fn poll_follow_up(&self) -> Vec<AgentMessage> {
            vec![]
        }

        fn has_steering(&self) -> bool {
            self.poll_count.load(Ordering::SeqCst) == 0
        }
    }

    impl crate::tool::AgentTool for BurstUpdatingTool {
        fn name(&self) -> &str {
            "burst_tool"
        }

        fn label(&self) -> &str {
            "burst_tool"
        }

        fn description(&self) -> &'static str {
            "Emits a burst of partial updates"
        }

        fn parameters_schema(&self) -> &serde_json::Value {
            static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": true
                })
            })
        }

        fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancellation_token: CancellationToken,
            on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
            _credential: Option<crate::ResolvedCredential>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            let update_count = self.update_count;
            Box::pin(async move {
                if let Some(on_update) = on_update {
                    for idx in 0..update_count {
                        on_update(AgentToolResult::text(format!("partial-{idx}")));
                    }
                }
                AgentToolResult::text("done")
            })
        }
    }

    struct ExecutionRootRecorder {
        saw_none: Arc<AtomicBool>,
        captured_roots: Arc<StdMutex<Vec<Option<PathBuf>>>>,
    }

    struct StopBatchPolicy;

    impl PreDispatchPolicy for StopBatchPolicy {
        fn name(&self) -> &str {
            "stop-batch"
        }

        fn evaluate(&self, _ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            PreDispatchVerdict::Stop("blocked by policy".to_string())
        }
    }

    impl PreDispatchPolicy for ExecutionRootRecorder {
        fn name(&self) -> &str {
            "execution-root-recorder"
        }

        fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            self.saw_none
                .store(ctx.execution_root.is_none(), Ordering::SeqCst);
            self.captured_roots
                .lock()
                .unwrap()
                .push(ctx.execution_root.map(std::path::Path::to_path_buf));
            PreDispatchVerdict::Continue
        }
    }

    fn test_loop_config(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
    ) -> Arc<AgentLoopConfig> {
        test_loop_config_with_options(
            pre_dispatch_policies,
            vec![],
            None,
            crate::ApprovalMode::Bypassed,
        )
    }

    fn test_loop_config_with_options(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
        tools: Vec<Arc<dyn crate::tool::AgentTool>>,
        approve_tool: Option<Box<crate::agent_options::ApproveToolFn>>,
        approval_mode: ApprovalMode,
    ) -> Arc<AgentLoopConfig> {
        test_loop_config_with_message_provider(
            pre_dispatch_policies,
            tools,
            approve_tool,
            approval_mode,
            None,
        )
    }

    fn test_loop_config_with_message_provider(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
        tools: Vec<Arc<dyn crate::tool::AgentTool>>,
        approve_tool: Option<Box<crate::agent_options::ApproveToolFn>>,
        approval_mode: ApprovalMode,
        message_provider: Option<Arc<dyn MessageProvider>>,
    ) -> Arc<AgentLoopConfig> {
        Arc::new(AgentLoopConfig {
            agent_name: None,
            transfer_chain: None,
            model: default_model(),
            stream_options: StreamOptions::default(),
            retry_strategy: Box::new(DefaultRetryStrategy::default()),
            stream_fn: Arc::new(MockStreamFn::new(vec![])),
            tools,
            convert_to_llm: Box::new(default_convert),
            transform_context: None,
            get_api_key: None,
            message_provider,
            pending_message_snapshot: Arc::default(),
            approve_tool,
            approval_mode,
            pre_turn_policies: vec![],
            pre_dispatch_policies,
            post_turn_policies: vec![],
            post_loop_policies: vec![],
            async_transform_context: None,
            metrics_collector: None,
            fallback: None,
            tool_execution_policy: ToolExecutionPolicy::Concurrent,
            session_state: Arc::new(std::sync::RwLock::new(crate::SessionState::new())),
            credential_resolver: None,
            cache_config: None,
            cache_state: std::sync::Mutex::new(crate::CacheState::default()),
            dynamic_system_prompt: None,
        })
    }

    fn drain_events(rx: &mut mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn pre_dispatch_execution_root_is_none_when_runtime_cannot_prove_it() {
        let saw_none = Arc::new(AtomicBool::new(false));
        let captured_roots = Arc::new(StdMutex::new(Vec::new()));
        let recorder = Arc::new(ExecutionRootRecorder {
            saw_none: Arc::clone(&saw_none),
            captured_roots: Arc::clone(&captured_roots),
        });
        let config = test_loop_config(vec![recorder]);
        let tool_calls = vec![ToolCallInfo {
            id: "call_1".to_string(),
            name: "unknown_tool".to_string(),
            arguments: serde_json::json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        assert!(
            matches!(outcome, ToolExecOutcome::Completed { .. }),
            "expected completed outcome"
        );
        assert!(
            saw_none.load(Ordering::SeqCst),
            "pre-dispatch policy should see execution_root=None"
        );
        assert_eq!(
            captured_roots.lock().unwrap().as_slice(),
            &[None],
            "execution_root should remain unknown until a tool-specific root is available"
        );
    }

    #[tokio::test]
    async fn pre_dispatch_stop_preserves_result_parity_for_remaining_tool_calls() {
        let config = test_loop_config(vec![Arc::new(StopBatchPolicy)]);
        let tool_calls = vec![
            ToolCallInfo {
                id: "call_1".to_string(),
                name: "tool_one".to_string(),
                arguments: serde_json::json!({ "first": true }),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_2".to_string(),
                name: "tool_two".to_string(),
                arguments: serde_json::json!({ "second": true }),
                is_incomplete: false,
            },
        ];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 2, "each tool call should receive a result");
        assert_eq!(
            results
                .iter()
                .map(|result| result.tool_call_id.as_str())
                .collect::<Vec<_>>(),
            vec!["call_1", "call_2"]
        );
        assert!(
            results.iter().all(|result| result.is_error),
            "stopped tool calls should surface as errors"
        );
        assert!(
            results.iter().all(|result| {
                matches!(
                    result.content.as_slice(),
                    [ContentBlock::Text { text }]
                        if text.contains("policy stopped tool batch before dispatch")
                )
            }),
            "synthetic results should explain the batch stop"
        );

        let mut start_ids = Vec::new();
        let mut end_ids = Vec::new();
        for event in drain_events(&mut rx) {
            match event {
                AgentEvent::ToolExecutionStart { id, .. } => start_ids.push(id),
                AgentEvent::ToolExecutionEnd { id, .. } => end_ids.push(id),
                _ => {}
            }
        }

        assert!(
            start_ids.is_empty(),
            "synthetic stop results should not emit ToolExecutionStart"
        );
        assert_eq!(end_ids, vec!["call_1".to_string(), "call_2".to_string()]);
    }

    #[tokio::test]
    async fn invalid_tool_arguments_do_not_emit_start_event() {
        let tool = Arc::new(MockTool::new("write_file").with_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"],
            "additionalProperties": false
        })));
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn crate::tool::AgentTool>],
            None,
            ApprovalMode::Bypassed,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_invalid".to_string(),
            name: "write_file".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert_eq!(tool.execution_count(), 0);

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(start_count, 0, "schema-invalid calls must not look started");
    }

    #[tokio::test]
    async fn unknown_tools_do_not_emit_start_event() {
        let config = test_loop_config(vec![]);
        let tool_calls = vec![ToolCallInfo {
            id: "call_unknown".to_string(),
            name: "unknown_tool".to_string(),
            arguments: json!({ "path": "ghost.txt" }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(start_count, 0, "unknown tools must not look started");
    }

    #[tokio::test]
    async fn approval_rejection_does_not_emit_start_event() {
        let tool = Arc::new(MockTool::new("delete_file").with_requires_approval(true));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> =
            Box::new(|_request| Box::pin(async { ToolApproval::Rejected }));
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn crate::tool::AgentTool>],
            Some(approve_tool),
            ApprovalMode::Enabled,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_rejected".to_string(),
            name: "delete_file".to_string(),
            arguments: json!({ "path": "danger.txt" }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert_eq!(tool.execution_count(), 0);

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(start_count, 0, "approval rejection must not look started");
    }

    #[tokio::test]
    async fn tool_execution_start_uses_approved_arguments() {
        let tool = Arc::new(MockTool::new("write_file"));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> = Box::new(|_request| {
            Box::pin(async {
                ToolApproval::ApprovedWith(json!({
                    "path": "rewritten.txt",
                    "content": "updated"
                }))
            })
        });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn crate::tool::AgentTool>],
            Some(approve_tool),
            ApprovalMode::Enabled,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_rewritten".to_string(),
            name: "write_file".to_string(),
            arguments: json!({
                "path": "original.txt",
                "content": "old"
            }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert_eq!(tool.execution_count(), 1);

        let start_events: Vec<_> = drain_events(&mut rx)
            .into_iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionStart {
                    id,
                    name,
                    arguments,
                } => Some((id, name, arguments)),
                _ => None,
            })
            .collect();
        assert_eq!(start_events.len(), 1);
        assert_eq!(start_events[0].0, "call_rewritten");
        assert_eq!(start_events[0].1, "write_file");
        assert_eq!(
            start_events[0].2,
            json!({
                "path": "rewritten.txt",
                "content": "updated"
            })
        );
    }

    #[tokio::test]
    async fn tool_execution_updates_include_identity_and_survive_backpressure() {
        let tool = Arc::new(BurstUpdatingTool { update_count: 32 });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool as Arc<dyn crate::tool::AgentTool>],
            None,
            ApprovalMode::Bypassed,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_updates".to_string(),
            name: "burst_tool".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(1);
        let collected = StdArc::new(StdSyncMutex::new(Vec::new()));
        let collected_clone = StdArc::clone(&collected);
        let receiver = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                collected_clone.lock().unwrap().push(event);
            }
        });

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;
        drop(tx);
        receiver.await.unwrap();

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);

        let events = collected.lock().unwrap();
        let updates: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionUpdate { id, name, partial } => Some((
                    id.clone(),
                    name.clone(),
                    ContentBlock::extract_text(&partial.content),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(updates.len(), 32, "partial updates should not be dropped");
        assert!(
            updates
                .iter()
                .all(|(id, name, _)| id == "call_updates" && name == "burst_tool"),
            "partial updates should carry the originating tool identity"
        );
        assert_eq!(
            updates.first().map(|(_, _, text)| text.as_str()),
            Some("partial-0")
        );
        assert_eq!(
            updates.last().map(|(_, _, text)| text.as_str()),
            Some("partial-31")
        );
    }

    #[tokio::test]
    async fn steering_interrupt_preserves_worker_polled_messages() {
        let fast_tool =
            Arc::new(MockTool::new("fast_tool").with_delay(std::time::Duration::from_millis(10)));
        let slow_tool =
            Arc::new(MockTool::new("slow_tool").with_delay(std::time::Duration::from_secs(5)));
        let config = test_loop_config_with_message_provider(
            vec![],
            vec![
                fast_tool as Arc<dyn crate::tool::AgentTool>,
                slow_tool as Arc<dyn crate::tool::AgentTool>,
            ],
            None,
            ApprovalMode::Bypassed,
            Some(Arc::new(OneShotSteeringProvider {
                poll_count: AtomicU32::new(0),
            })),
        );

        let tool_calls = vec![
            ToolCallInfo {
                id: "call_fast".to_string(),
                name: "fast_tool".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_slow".to_string(),
                name: "slow_tool".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
        ];
        let cancellation_token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::SteeringInterrupt {
            completed,
            cancelled,
            steering_messages,
            ..
        } = outcome
        else {
            panic!("expected steering interrupt outcome");
        };

        assert_eq!(
            completed.len(),
            1,
            "fast tool should complete before the interrupt"
        );
        assert_eq!(
            cancelled.len(),
            1,
            "slow tool should be cancelled by steering"
        );
        assert_eq!(
            steering_messages.len(),
            1,
            "drained steering must survive into the outcome"
        );
        assert!(matches!(
            &steering_messages[0],
            AgentMessage::Llm(LlmMessage::User(UserMessage { content, .. }))
                if ContentBlock::extract_text(content) == "redirect"
        ));
    }
}
