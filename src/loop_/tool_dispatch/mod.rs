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
use tracing::{info, warn};

use crate::tool::{AgentTool, AgentToolResult};
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

/// Build a tool lookup table that preserves the first registered tool for a
/// given name.
///
/// Public lookup paths such as `Agent::find_tool()` return the first matching
/// tool. Dispatch must use the same rule so duplicate tool names do not expose
/// one tool to the model while executing another.
fn build_tool_map(tools: &[Arc<dyn AgentTool>]) -> HashMap<&str, &Arc<dyn AgentTool>> {
    let mut tool_map: HashMap<&str, &Arc<dyn AgentTool>> = HashMap::with_capacity(tools.len());

    for tool in tools {
        if tool_map.contains_key(tool.name()) {
            warn!(
                tool_name = %tool.name(),
                "duplicate tool name detected during dispatch; keeping first registered tool"
            );
            continue;
        }

        tool_map.insert(tool.name(), tool);
    }

    tool_map
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

    let tool_map = build_tool_map(&config.tools);

    // Phase 1: Pre-process — policies, approval, argument rewriting.
    let preprocess::PreprocessResult {
        prepared,
        injected_messages,
    } = match preprocess::preprocess_tool_calls(
        config,
        tool_calls,
        &batch_token,
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

    if batch_token.is_cancelled() {
        return collect::build_aborted_outcome(
            tool_calls,
            results,
            tool_timings,
            injected_messages,
        )
        .await;
    }

    // Phase 2: Compute execution groups and dispatch.
    let groups = match execute::compute_execution_groups(
        &config.tool_execution_policy,
        tool_calls,
        &prepared,
    )
    .await
    {
        Ok(groups) => groups,
        Err(reason) => {
            for prep in &prepared {
                let tc = &tool_calls[prep.idx];
                shared::emit_error_result(
                    &tc.name,
                    &tc.id,
                    AgentToolResult::error(format!(
                        "custom tool execution strategy returned an invalid partition: {reason}"
                    )),
                    prep.idx,
                    &results,
                    tx,
                )
                .await;
            }

            let all_results = std::mem::take(&mut *results.lock().await);
            let ordered = order_results_by_tool_calls(tool_calls, &all_results);
            let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
            return ToolExecOutcome::Completed {
                results: ordered,
                tool_metrics: collected_timings,
                transfer_signal: None,
                injected_messages,
            };
        }
    };

    // Phase 3: Execute each group and collect results.
    for group in groups {
        if batch_token.is_cancelled() {
            return collect::build_aborted_outcome(
                tool_calls,
                Arc::clone(&results),
                Arc::clone(&tool_timings),
                injected_messages,
            )
            .await;
        }

        let mut handles: Vec<(usize, tokio::task::JoinHandle<()>)> = Vec::new();

        for &prepared_idx in &group {
            if batch_token.is_cancelled() {
                for (_, handle) in handles {
                    handle.abort();
                    let _ = handle.await;
                }

                return collect::build_aborted_outcome(
                    tool_calls,
                    Arc::clone(&results),
                    Arc::clone(&tool_timings),
                    injected_messages,
                )
                .await;
            }

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
                DispatchResult::ChannelClosed => {
                    // Cancel the batch and abort/join all already-spawned handles
                    // before returning to prevent orphaned side-effecting tasks.
                    batch_token.cancel();
                    for (_, h) in handles {
                        h.abort();
                        let _ = h.await;
                    }
                    return ToolExecOutcome::ChannelClosed;
                }
            }
        }

        let group_outcome = collect::collect_group_results(
            tool_calls,
            handles,
            &results,
            &steering_detected,
            &transfer_detected,
            &batch_token,
            tx,
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
            GroupOutcome::Aborted => {
                return collect::build_aborted_outcome(
                    tool_calls,
                    results,
                    tool_timings,
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

    use std::collections::HashMap;
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
    use crate::{
        DefaultRetryStrategy, StreamOptions, ToolApproval, ToolCallSummary, ToolExecutionPolicy,
        ToolExecutionStrategy,
    };

    struct BurstUpdatingTool {
        update_count: usize,
    }

    struct NonCancellingTool {
        started: Arc<AtomicBool>,
    }

    struct YieldingTool {
        name: &'static str,
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
        fn name(&self) -> &'static str {
            "burst_tool"
        }

        fn label(&self) -> &'static str {
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

    impl crate::tool::AgentTool for NonCancellingTool {
        fn name(&self) -> &'static str {
            "non_cancelling_tool"
        }

        fn label(&self) -> &'static str {
            "non_cancelling_tool"
        }

        fn description(&self) -> &'static str {
            "Ignores cancellation and waits forever until aborted"
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
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
            _credential: Option<crate::ResolvedCredential>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            self.started.store(true, Ordering::SeqCst);
            Box::pin(async move {
                std::future::pending::<()>().await;
                AgentToolResult::text("unreachable")
            })
        }
    }

    impl crate::tool::AgentTool for YieldingTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn label(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            "Yields once before returning a result"
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
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
            _credential: Option<crate::ResolvedCredential>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async move {
                tokio::task::yield_now().await;
                AgentToolResult::text("mock result")
            })
        }
    }

    struct ExecutionRootRecorder {
        saw_none: Arc<AtomicBool>,
        captured_roots: Arc<StdMutex<Vec<Option<PathBuf>>>>,
    }

    struct StopBatchPolicy;
    struct StopOnToolTwoPolicy;
    struct SkipRewrittenPathPolicy {
        evaluations: Arc<AtomicU32>,
    }
    struct OriginalIndexStrategy;
    struct DuplicateIndexStrategy;

    impl PreDispatchPolicy for StopBatchPolicy {
        fn name(&self) -> &'static str {
            "stop-batch"
        }

        fn evaluate(&self, _ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            PreDispatchVerdict::Stop("blocked by policy".to_string())
        }
    }

    impl PreDispatchPolicy for StopOnToolTwoPolicy {
        fn name(&self) -> &'static str {
            "stop-on-tool-two"
        }

        fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            if ctx.tool_name == "tool_two" {
                PreDispatchVerdict::Stop("blocked after an earlier tool was prepared".to_string())
            } else {
                PreDispatchVerdict::Continue
            }
        }
    }

    impl PreDispatchPolicy for ExecutionRootRecorder {
        fn name(&self) -> &'static str {
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

    impl PreDispatchPolicy for SkipRewrittenPathPolicy {
        fn name(&self) -> &'static str {
            "skip-rewritten-path"
        }

        fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            self.evaluations.fetch_add(1, Ordering::SeqCst);

            if ctx
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                == Some("rewritten.txt")
            {
                PreDispatchVerdict::Skip(
                    "rewritten approval arguments must still pass pre-dispatch policy".to_string(),
                )
            } else {
                PreDispatchVerdict::Continue
            }
        }
    }

    impl ToolExecutionStrategy for OriginalIndexStrategy {
        fn partition(
            &self,
            tool_calls: &[ToolCallSummary<'_>],
        ) -> Pin<Box<dyn Future<Output = Vec<Vec<usize>>> + Send + '_>> {
            let count = tool_calls.len();
            Box::pin(async move {
                if count >= 2 {
                    vec![vec![0], vec![2]]
                } else {
                    vec![vec![0]]
                }
            })
        }
    }

    impl ToolExecutionStrategy for DuplicateIndexStrategy {
        fn partition(
            &self,
            _tool_calls: &[ToolCallSummary<'_>],
        ) -> Pin<Box<dyn Future<Output = Vec<Vec<usize>>> + Send + '_>> {
            Box::pin(async move { vec![vec![0, 0]] })
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
            ToolExecutionPolicy::Concurrent,
        )
    }

    fn test_loop_config_with_options(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
        tools: Vec<Arc<dyn crate::tool::AgentTool>>,
        approve_tool: Option<Box<crate::agent_options::ApproveToolFn>>,
        approval_mode: ApprovalMode,
        tool_execution_policy: ToolExecutionPolicy,
    ) -> Arc<AgentLoopConfig> {
        test_loop_config_with_message_provider(
            pre_dispatch_policies,
            tools,
            approve_tool,
            approval_mode,
            tool_execution_policy,
            None,
        )
    }

    fn test_loop_config_with_message_provider(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
        tools: Vec<Arc<dyn crate::tool::AgentTool>>,
        approve_tool: Option<Box<crate::agent_options::ApproveToolFn>>,
        approval_mode: ApprovalMode,
        tool_execution_policy: ToolExecutionPolicy,
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
            loop_context_snapshot: Arc::default(),
            approve_tool,
            approval_mode,
            pre_turn_policies: vec![],
            pre_dispatch_policies,
            post_turn_policies: vec![],
            post_loop_policies: vec![],
            async_transform_context: None,
            metrics_collector: None,
            fallback: None,
            tool_execution_policy,
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

        let ToolExecOutcome::Stopped {
            results, reason, ..
        } = outcome
        else {
            panic!("expected stopped outcome");
        };
        assert_eq!(results.len(), 2, "each tool call should receive a result");
        assert_eq!(reason, "blocked by policy");
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
    async fn pre_dispatch_stop_backfills_prepared_tool_calls_without_results() {
        let config = test_loop_config(vec![Arc::new(StopOnToolTwoPolicy)]);
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

        let ToolExecOutcome::Stopped {
            results, reason, ..
        } = outcome
        else {
            panic!("expected stopped outcome");
        };
        assert_eq!(
            results.len(),
            2,
            "a later stop must still return one result per tool call"
        );
        assert_eq!(reason, "blocked after an earlier tool was prepared");
        assert_eq!(
            results
                .iter()
                .map(|result| result.tool_call_id.as_str())
                .collect::<Vec<_>>(),
            vec!["call_1", "call_2"]
        );
        assert!(
            results.iter().all(|result| result.is_error),
            "every unresolved tool call should surface as a synthetic error"
        );
        assert!(
            results.iter().all(|result| {
                matches!(
                    result.content.as_slice(),
                    [ContentBlock::Text { text }]
                        if text.contains("policy stopped tool batch before dispatch")
                )
            }),
            "backfilled results should explain the batch stop"
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
            "prepared-but-undispatched calls must not emit ToolExecutionStart"
        );
        assert_eq!(end_ids, vec!["call_1".to_string(), "call_2".to_string()]);
    }

    #[tokio::test]
    async fn pre_dispatch_stop_aborts_before_any_approval_side_effects() {
        let tool_one = Arc::new(MockTool::new("tool_one").with_requires_approval(true));
        let tool_two = Arc::new(MockTool::new("tool_two").with_requires_approval(true));
        let tool_one_ref = Arc::clone(&tool_one);
        let tool_two_ref = Arc::clone(&tool_two);
        let approval_calls = Arc::new(AtomicU32::new(0));
        let approval_calls_clone = Arc::clone(&approval_calls);

        let config = test_loop_config_with_options(
            vec![Arc::new(StopOnToolTwoPolicy)],
            vec![
                tool_one as Arc<dyn crate::tool::AgentTool>,
                tool_two as Arc<dyn crate::tool::AgentTool>,
            ],
            Some(Box::new(move |_request| {
                approval_calls_clone.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { ToolApproval::Approved })
            })),
            ApprovalMode::Enabled,
            ToolExecutionPolicy::Concurrent,
        );
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
        let (tx, mut rx) = mpsc::channel(16);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Stopped {
            results, reason, ..
        } = outcome
        else {
            panic!("expected stopped outcome");
        };
        assert_eq!(reason, "blocked after an earlier tool was prepared");
        assert_eq!(approval_calls.load(Ordering::SeqCst), 0);
        assert_eq!(tool_one_ref.execution_count(), 0);
        assert_eq!(tool_two_ref.execution_count(), 0);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.is_error));

        let events = drain_events(&mut rx);
        assert!(
            !events.iter().any(|event| matches!(
                event,
                AgentEvent::ToolApprovalRequested { .. }
                    | AgentEvent::ToolApprovalResolved { .. }
                    | AgentEvent::ToolExecutionStart { .. }
            )),
            "a later pre-dispatch stop must prevent earlier approval or execution events"
        );
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
            ToolExecutionPolicy::Concurrent,
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
            ToolExecutionPolicy::Concurrent,
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
    async fn synchronous_approval_panic_rejects_without_dispatch() {
        let tool = Arc::new(MockTool::new("delete_file").with_requires_approval(true));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> =
            Box::new(|_request| panic!("sync approval panic"));
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn crate::tool::AgentTool>],
            Some(approve_tool),
            ApprovalMode::Enabled,
            ToolExecutionPolicy::Concurrent,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_sync_panic".to_string(),
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
        assert!(matches!(
            results[0].content.as_slice(),
            [ContentBlock::Text { text }] if text.contains("approval callback panicked")
        ));

        let events = drain_events(&mut rx);
        let start_count = events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(
            start_count, 0,
            "synchronous approval panic must not look started"
        );
        assert!(matches!(
            events.as_slice(),
            [
                AgentEvent::ToolApprovalRequested { .. },
                AgentEvent::ToolApprovalResolved {
                    approved: false,
                    ..
                },
                AgentEvent::ToolExecutionEnd { .. }
            ]
        ));
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
            ToolExecutionPolicy::Concurrent,
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
    async fn approved_argument_rewrites_rerun_pre_dispatch_policies() {
        let tool = Arc::new(MockTool::new("write_file"));
        let policy_evaluations = Arc::new(AtomicU32::new(0));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> = Box::new(|_request| {
            Box::pin(async {
                ToolApproval::ApprovedWith(json!({
                    "path": "rewritten.txt",
                    "content": "updated"
                }))
            })
        });
        let config = test_loop_config_with_options(
            vec![Arc::new(SkipRewrittenPathPolicy {
                evaluations: Arc::clone(&policy_evaluations),
            })],
            vec![tool.clone() as Arc<dyn crate::tool::AgentTool>],
            Some(approve_tool),
            ApprovalMode::Enabled,
            ToolExecutionPolicy::Concurrent,
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
        assert!(results[0].is_error);
        assert_eq!(tool.execution_count(), 0);
        assert_eq!(
            policy_evaluations.load(Ordering::SeqCst),
            2,
            "pre-dispatch policies should run again on approval-rewritten arguments"
        );
        assert!(matches!(
            results[0].content.as_slice(),
            [ContentBlock::Text { text }]
                if text.contains("rewritten approval arguments must still pass pre-dispatch policy")
        ));

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(
            start_count, 0,
            "policy-rejected approval rewrites must not emit ToolExecutionStart"
        );
    }

    #[tokio::test]
    async fn invalid_custom_partition_after_filtering_returns_errors_without_dispatch() {
        let tool_a = Arc::new(MockTool::new("tool_a"));
        let tool_b = Arc::new(MockTool::new("tool_b").with_requires_approval(true));
        let tool_c = Arc::new(MockTool::new("tool_c"));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> = Box::new(|request| {
            let should_reject = request.tool_name == "tool_b";
            Box::pin(async move {
                if should_reject {
                    ToolApproval::Rejected
                } else {
                    ToolApproval::Approved
                }
            })
        });
        let config = test_loop_config_with_options(
            vec![],
            vec![
                tool_a.clone() as Arc<dyn crate::tool::AgentTool>,
                tool_b.clone() as Arc<dyn crate::tool::AgentTool>,
                tool_c.clone() as Arc<dyn crate::tool::AgentTool>,
            ],
            Some(approve_tool),
            ApprovalMode::Enabled,
            ToolExecutionPolicy::Custom(Arc::new(OriginalIndexStrategy)),
        );
        let tool_calls = vec![
            ToolCallInfo {
                id: "call_a".to_string(),
                name: "tool_a".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_c".to_string(),
                name: "tool_c".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
        ];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(16);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 3, "all tool calls should receive a result");
        assert_eq!(tool_a.execution_count(), 0);
        assert_eq!(tool_b.execution_count(), 0);
        assert_eq!(tool_c.execution_count(), 0);

        let result_texts: HashMap<_, _> = results
            .iter()
            .map(|result| {
                (
                    result.tool_call_id.as_str(),
                    ContentBlock::extract_text(&result.content),
                )
            })
            .collect();
        assert!(
            result_texts["call_a"].contains("invalid partition"),
            "prepared tool_a should surface the partition validation error"
        );
        assert!(
            result_texts["call_a"].contains("prepared index 2"),
            "error should explain the out-of-bounds prepared index"
        );
        assert!(
            result_texts["call_b"].contains("rejected by the approval gate"),
            "filtered tool_b should keep its approval rejection"
        );
        assert!(
            result_texts["call_c"].contains("invalid partition"),
            "prepared tool_c should surface the partition validation error"
        );

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(
            start_count, 0,
            "invalid custom partitions must not emit ToolExecutionStart"
        );
    }

    #[tokio::test]
    async fn duplicate_custom_partition_indices_return_deterministic_errors() {
        let tool_a = Arc::new(MockTool::new("tool_a"));
        let tool_b = Arc::new(MockTool::new("tool_b"));
        let config = test_loop_config_with_options(
            vec![],
            vec![
                tool_a.clone() as Arc<dyn crate::tool::AgentTool>,
                tool_b.clone() as Arc<dyn crate::tool::AgentTool>,
            ],
            None,
            ApprovalMode::Bypassed,
            ToolExecutionPolicy::Custom(Arc::new(DuplicateIndexStrategy)),
        );
        let tool_calls = vec![
            ToolCallInfo {
                id: "call_a".to_string(),
                name: "tool_a".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
                arguments: json!({}),
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
        assert_eq!(
            results.len(),
            2,
            "every prepared tool call should get an error"
        );
        assert_eq!(tool_a.execution_count(), 0);
        assert_eq!(tool_b.execution_count(), 0);
        assert!(
            results.iter().all(|result| result.is_error),
            "invalid partitions should synthesize error results"
        );
        assert!(
            results.iter().all(|result| {
                ContentBlock::extract_text(&result.content).contains("repeated prepared index 0")
            }),
            "duplicate prepared indices should be called out explicitly"
        );

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(
            start_count, 0,
            "duplicate custom partitions must fail before dispatch"
        );
    }

    #[tokio::test]
    async fn tool_execution_updates_include_identity() {
        let tool = Arc::new(BurstUpdatingTool { update_count: 4 });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool as Arc<dyn crate::tool::AgentTool>],
            None,
            ApprovalMode::Bypassed,
            ToolExecutionPolicy::Concurrent,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_updates".to_string(),
            name: "burst_tool".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);
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
        assert_eq!(updates.len(), 4, "partial updates should be forwarded");
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
            Some("partial-3")
        );
    }

    /// Regression test for #770: per-tool partial-update buffering must be
    /// bounded so a tool that emits updates faster than downstream drains
    /// them cannot grow the queue without limit.
    ///
    /// The tool emits `CAP + OVERFLOW` updates in a tight synchronous loop,
    /// which starves the forwarder task on the current-thread runtime. Under
    /// the old unbounded channel all updates would buffer; under the bounded
    /// channel excess updates are dropped by `try_send` and the observed count
    /// is capped at the buffer size.
    #[tokio::test]
    async fn partial_update_channel_is_bounded_under_backpressure() {
        use super::shared::TOOL_UPDATE_CHANNEL_CAPACITY;

        const OVERFLOW: usize = 64;
        let update_count = TOOL_UPDATE_CHANNEL_CAPACITY + OVERFLOW;

        let tool = Arc::new(BurstUpdatingTool { update_count });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool as Arc<dyn crate::tool::AgentTool>],
            None,
            ApprovalMode::Bypassed,
            ToolExecutionPolicy::Concurrent,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_overflow".to_string(),
            name: "burst_tool".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(TOOL_UPDATE_CHANNEL_CAPACITY * 4);
        let collected = StdArc::new(StdSyncMutex::new(Vec::new()));
        let collected_clone = StdArc::clone(&collected);
        let receiver = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                collected_clone.lock().unwrap().push(event);
            }
        });

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx),
        )
        .await
        .expect("bounded update channel must never stall the tool");
        drop(tx);
        receiver.await.unwrap();

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1, "terminal result still arrives");
        assert!(!results[0].is_error, "tool ran to completion");

        let events = collected.lock().unwrap();
        let updates: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionUpdate { partial, .. } => {
                    Some(ContentBlock::extract_text(&partial.content))
                }
                _ => None,
            })
            .collect();

        assert!(
            updates.len() <= TOOL_UPDATE_CHANNEL_CAPACITY,
            "observed {} updates but buffer capacity is {}",
            updates.len(),
            TOOL_UPDATE_CHANNEL_CAPACITY,
        );
        assert!(
            updates.len() < update_count,
            "at least some of the {OVERFLOW} overflow updates must be dropped \
             (observed {}, emitted {update_count})",
            updates.len(),
        );
        assert_eq!(
            updates.first().map(String::as_str),
            Some("partial-0"),
            "earliest updates land before the buffer fills"
        );
    }

    #[tokio::test]
    async fn steering_interrupt_preserves_worker_polled_messages() {
        let fast_tool = Arc::new(MockTool::new("fast_tool"));
        let slow_tool = Arc::new(YieldingTool { name: "slow_tool" });
        let config = test_loop_config_with_message_provider(
            vec![],
            vec![
                fast_tool as Arc<dyn crate::tool::AgentTool>,
                slow_tool as Arc<dyn crate::tool::AgentTool>,
            ],
            None,
            ApprovalMode::Bypassed,
            ToolExecutionPolicy::Concurrent,
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

    #[tokio::test]
    async fn parent_cancellation_aborts_non_cancelling_tool_batches() {
        let started = Arc::new(AtomicBool::new(false));
        let tool = Arc::new(NonCancellingTool {
            started: Arc::clone(&started),
        });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool as Arc<dyn crate::tool::AgentTool>],
            None,
            ApprovalMode::Bypassed,
            ToolExecutionPolicy::Concurrent,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_abort".to_string(),
            name: "non_cancelling_tool".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let cancel_clone = cancellation_token.clone();
        let (tx, _rx) = mpsc::channel(8);

        tokio::spawn(async move {
            while !started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            cancel_clone.cancel();
        });

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Aborted { results, .. } = outcome else {
            panic!("expected aborted outcome");
        };
        assert_eq!(
            results.len(),
            1,
            "aborted batches should preserve result parity"
        );
        assert_eq!(results[0].tool_call_id, "call_abort");
        assert!(results[0].is_error);
        assert!(matches!(
            results[0].content.as_slice(),
            [ContentBlock::Text { text }] if text.contains("operation aborted")
        ));
    }

    #[tokio::test]
    async fn cancellation_during_approval_wait_aborts_without_dispatch() {
        let tool = Arc::new(MockTool::new("delete_file").with_requires_approval(true));
        let tool_ref = Arc::clone(&tool);
        let config = test_loop_config_with_options(
            vec![],
            vec![tool as Arc<dyn crate::tool::AgentTool>],
            Some(Box::new(|_request| {
                Box::pin(async { std::future::pending::<crate::tool::ToolApproval>().await })
            })),
            ApprovalMode::Enabled,
            ToolExecutionPolicy::Concurrent,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_waiting".to_string(),
            name: "delete_file".to_string(),
            arguments: json!({ "path": "danger.txt" }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let cancel_clone = cancellation_token.clone();
        let (tx, mut rx) = mpsc::channel(8);
        let saw_requested = Arc::new(AtomicBool::new(false));
        let saw_start = Arc::new(AtomicBool::new(false));
        let saw_requested_clone = Arc::clone(&saw_requested);
        let saw_start_clone = Arc::clone(&saw_start);

        let receiver = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    AgentEvent::ToolApprovalRequested { .. } => {
                        saw_requested_clone.store(true, Ordering::SeqCst);
                        cancel_clone.cancel();
                    }
                    AgentEvent::ToolExecutionStart { .. } => {
                        saw_start_clone.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        });

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;
        drop(tx);
        receiver.await.unwrap();

        let ToolExecOutcome::Aborted { results, .. } = outcome else {
            panic!("expected aborted outcome");
        };
        assert!(saw_requested.load(Ordering::SeqCst));
        assert!(!saw_start.load(Ordering::SeqCst));
        assert_eq!(tool_ref.execution_count(), 0);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(matches!(
            results[0].content.as_slice(),
            [ContentBlock::Text { text }] if text.contains("operation aborted")
        ));
    }

    #[tokio::test]
    async fn cancellation_after_first_approval_does_not_touch_later_tools() {
        let tool_a = Arc::new(MockTool::new("tool_a").with_requires_approval(true));
        let tool_b = Arc::new(MockTool::new("tool_b").with_requires_approval(true));
        let tool_a_ref = Arc::clone(&tool_a);
        let tool_b_ref = Arc::clone(&tool_b);
        let approval_calls = Arc::new(AtomicU32::new(0));
        let approval_calls_clone = Arc::clone(&approval_calls);
        let cancellation_token = CancellationToken::new();
        let cancel_clone = cancellation_token.clone();

        let approve_tool: Box<crate::agent_options::ApproveToolFn> = Box::new(move |_request| {
            let call_index = approval_calls_clone.fetch_add(1, Ordering::SeqCst);
            let cancel = cancel_clone.clone();
            Box::pin(async move {
                if call_index == 0 {
                    cancel.cancel();
                }
                ToolApproval::Approved
            })
        });

        let config = test_loop_config_with_options(
            vec![],
            vec![
                tool_a as Arc<dyn crate::tool::AgentTool>,
                tool_b as Arc<dyn crate::tool::AgentTool>,
            ],
            Some(approve_tool),
            ApprovalMode::Enabled,
            ToolExecutionPolicy::Concurrent,
        );
        let tool_calls = vec![
            ToolCallInfo {
                id: "call_a".to_string(),
                name: "tool_a".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
        ];
        let (tx, mut rx) = mpsc::channel(16);
        let saw_start = Arc::new(AtomicBool::new(false));
        let saw_start_clone = Arc::clone(&saw_start);
        let receiver = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
                    saw_start_clone.store(true, Ordering::SeqCst);
                }
            }
        });

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;
        drop(tx);
        receiver.await.unwrap();

        let ToolExecOutcome::Aborted { results, .. } = outcome else {
            panic!("expected aborted outcome");
        };
        assert_eq!(approval_calls.load(Ordering::SeqCst), 1);
        assert!(!saw_start.load(Ordering::SeqCst));
        assert_eq!(tool_a_ref.execution_count(), 0);
        assert_eq!(tool_b_ref.execution_count(), 0);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.is_error));
    }

    #[tokio::test]
    async fn cancellation_between_sequential_groups_skips_later_dispatch() {
        let tool_a = Arc::new(MockTool::new("tool_a"));
        let tool_b = Arc::new(MockTool::new("tool_b"));
        let tool_a_ref = Arc::clone(&tool_a);
        let tool_b_ref = Arc::clone(&tool_b);
        let config = test_loop_config_with_options(
            vec![],
            vec![
                tool_a as Arc<dyn crate::tool::AgentTool>,
                tool_b as Arc<dyn crate::tool::AgentTool>,
            ],
            None,
            ApprovalMode::Bypassed,
            ToolExecutionPolicy::Sequential,
        );
        let tool_calls = vec![
            ToolCallInfo {
                id: "call_a".to_string(),
                name: "tool_a".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
        ];
        let cancellation_token = CancellationToken::new();
        let cancel_clone = cancellation_token.clone();
        let (tx, mut rx) = mpsc::channel(16);
        let saw_b_start = Arc::new(AtomicBool::new(false));
        let saw_b_start_clone = Arc::clone(&saw_b_start);

        let receiver = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    AgentEvent::ToolExecutionEnd { id, .. } if id == "call_a" => {
                        cancel_clone.cancel();
                    }
                    AgentEvent::ToolExecutionStart { id, .. } if id == "call_b" => {
                        saw_b_start_clone.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        });

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;
        drop(tx);
        receiver.await.unwrap();

        let ToolExecOutcome::Aborted { results, .. } = outcome else {
            panic!("expected aborted outcome");
        };
        assert_eq!(tool_a_ref.execution_count(), 1);
        assert_eq!(tool_b_ref.execution_count(), 0);
        assert!(!saw_b_start.load(Ordering::SeqCst));
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_call_id, "call_a");
        assert!(!results[0].is_error);
        assert_eq!(results[1].tool_call_id, "call_b");
        assert!(results[1].is_error);
        assert!(matches!(
            results[1].content.as_slice(),
            [ContentBlock::Text { text }] if text.contains("operation aborted")
        ));
    }

    /// Regression test for #556: when a later tool in a concurrent group returns
    /// `ChannelClosed`, already-spawned handles must be aborted before returning.
    ///
    /// Setup:
    /// - Channel capacity = 1.  Tool A's `ToolExecutionStart` event fills the buffer
    ///   and dispatch returns `Spawned`.
    /// - A companion task receives that one buffered event then drops the receiver,
    ///   ensuring tool B's `emit_tool_execution_start` send blocks on a full buffer
    ///   and then fails with `ChannelClosed` once the receiver is gone.
    /// - Tool A is `NonCancellingTool`, which loops forever unless aborted.  Without
    ///   the fix the test hangs; with the fix it completes within the timeout.
    #[tokio::test]
    async fn channel_closed_mid_group_aborts_already_spawned_handles() {
        let started = Arc::new(AtomicBool::new(false));
        let tool_a = Arc::new(NonCancellingTool {
            started: Arc::clone(&started),
        });
        let tool_b = Arc::new(MockTool::new("tool_b"));

        let config = test_loop_config_with_options(
            vec![],
            vec![
                tool_a as Arc<dyn crate::tool::AgentTool>,
                tool_b as Arc<dyn crate::tool::AgentTool>,
            ],
            None,
            ApprovalMode::Bypassed,
            // Concurrent: both tools land in one group and are dispatched
            // sequentially in the for-loop, so tool_a is Spawned before
            // tool_b returns ChannelClosed.
            ToolExecutionPolicy::Concurrent,
        );

        let tool_calls = vec![
            ToolCallInfo {
                id: "call_a".to_string(),
                name: "non_cancelling_tool".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
                arguments: json!({}),
                is_incomplete: false,
            },
        ];
        let cancellation_token = CancellationToken::new();

        // Capacity=1: tool_a's ToolExecutionStart fills the single-slot buffer
        // without blocking.  tool_b's send then blocks on a full buffer, yielding
        // to the companion task which drops the receiver so the send returns Err.
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(1);

        // Companion: receive the one buffered event, then drop the receiver so
        // subsequent sends fail immediately.
        tokio::spawn(async move {
            let _ = rx.recv().await;
        });

        // This path explicitly guards against orphaning a non-cancelling
        // spawned tool, so keep a generous hang detector for regression clarity.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx),
        )
        .await
        .expect("channel-closed mid-group must not leave orphaned handles that block shutdown");

        assert!(
            matches!(outcome, ToolExecOutcome::ChannelClosed),
            "expected ChannelClosed outcome"
        );
    }
}
