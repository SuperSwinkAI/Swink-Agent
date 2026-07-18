use swink_agent::{
    Agent, AgentError, AgentTool, DefaultRetryStrategy, MessageProvider, RetryStrategy, StreamFn,
    ToolApproval, ToolApprovalRequest, from_fns,
};
#[cfg(feature = "builtin-tools")]
use swink_agent::{BashTool, ReadFileTool, WriteFileTool};
use swink_agent_adapters::ProxyStreamFn;

#[test]
fn top_level_reexports_remain_consumable() {
    let _agent_type = std::any::type_name::<Agent>();
    let _provider_type = std::any::type_name::<dyn MessageProvider>();
    let _stream_type = std::any::type_name::<dyn StreamFn>();
    let _tool_type = std::any::type_name::<dyn AgentTool>();
    let _retry_type = std::any::type_name::<DefaultRetryStrategy>();
    let _retry_trait = std::any::type_name::<dyn RetryStrategy>();
    let _proxy = ProxyStreamFn::new("https://example.com", "token");
    let _tool_approval_type = std::any::type_name::<ToolApproval>();
    let _tool_request_type = std::any::type_name::<ToolApprovalRequest>();
    let _agent_error_type = std::any::type_name::<AgentError>();
    let _provider = from_fns(Vec::new, Vec::new);
}

#[cfg(feature = "builtin-tools")]
#[test]
fn builtin_tools_reexported() {
    let _bash_type = std::any::type_name::<BashTool>();
    let _read_type = std::any::type_name::<ReadFileTool>();
    let _write_type = std::any::type_name::<WriteFileTool>();
}

#[test]
fn stream_types_re_exported() {
    let _ = std::any::type_name::<dyn StreamFn>();
    let _ = swink_agent::StreamOptions::default();
    let _ = std::any::type_name::<swink_agent::AssistantMessageEvent>();
    let _ = std::any::type_name::<swink_agent::AssistantMessageDelta>();
    let _ = std::any::type_name::<swink_agent::StreamErrorKind>();
}

#[test]
fn policy_types_re_exported() {
    fn _assert_pre_turn<T: swink_agent::PreTurnPolicy>() {}
    fn _assert_post_turn<T: swink_agent::PostTurnPolicy>() {}
    fn _assert_pre_dispatch<T: swink_agent::PreDispatchPolicy>() {}
    fn _assert_post_loop<T: swink_agent::PostLoopPolicy>() {}
    let _ = std::any::type_name::<swink_agent::PolicyVerdict>();
    let _ = std::any::type_name::<swink_agent::PreDispatchVerdict>();
}

#[test]
fn tool_types_re_exported() {
    fn _assert_tool<T: swink_agent::AgentTool>() {}
    let _ = swink_agent::ToolMetadata::default();
    let _ = std::any::type_name::<swink_agent::AgentToolResult>();
    let _ = std::any::type_name::<swink_agent::ToolApproval>();
    let _ = std::any::type_name::<swink_agent::ToolApprovalRequest>();
}

#[test]
fn convert_types_re_exported() {
    fn _assert_converter<T: swink_agent::MessageConverter>() {}
}

#[test]
fn model_types_re_exported() {
    let _ = std::any::type_name::<swink_agent::ModelSpec>();
    let _ = std::any::type_name::<swink_agent::ContentBlock>();
    let _ = std::any::type_name::<swink_agent::LlmMessage>();
    let _ = std::any::type_name::<swink_agent::Usage>();
    let _ = std::any::type_name::<swink_agent::Cost>();
    let _ = std::any::type_name::<swink_agent::StopReason>();
    let _ = std::any::type_name::<swink_agent::AssistantMessage>();
}

#[test]
fn display_types_re_exported() {
    fn _assert_display<T: swink_agent::IntoDisplayMessages>() {}
    let _ = std::any::type_name::<swink_agent::DisplayRole>();
    let _ = std::any::type_name::<swink_agent::CoreDisplayMessage>();
}

#[test]
fn context_and_config_types_re_exported() {
    let _ = std::any::type_name::<swink_agent::AgentContext>();
    let _ = std::any::type_name::<swink_agent::AgentConfig>();
    let _ = std::any::type_name::<swink_agent::AgentLoopConfig>();
    let _ = std::any::type_name::<swink_agent::AgentMessage>();
}

#[test]
fn agent_loop_config_constructor_hides_runtime_snapshots() {
    struct EmptyStreamFn;

    impl swink_agent::StreamFn for EmptyStreamFn {
        fn stream<'a>(
            &'a self,
            _model: &'a swink_agent::ModelSpec,
            _context: &'a swink_agent::AgentContext,
            _options: &'a swink_agent::StreamOptions,
            _cancellation_token: tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<
            Box<dyn futures::Stream<Item = swink_agent::AssistantMessageEvent> + Send + 'a>,
        > {
            Box::pin(futures::stream::iter(
                swink_agent::AssistantMessageEvent::text_response("ok"),
            ))
        }
    }

    let config = swink_agent::AgentLoopConfig::new(
        swink_agent::ModelSpec::new("test", "model"),
        std::sync::Arc::new(EmptyStreamFn),
        Box::new(swink_agent::default_convert),
    );

    assert!(config.tools.is_empty());
    assert!(config.message_provider.is_none());
}

#[test]
fn metrics_types_re_exported() {
    fn _assert_metrics<T: swink_agent::MetricsCollector>() {}
    let _ = std::any::type_name::<swink_agent::TurnMetrics>();
    let _ = std::any::type_name::<swink_agent::ToolExecMetrics>();
}

#[test]
fn stream_assembly_types_remain_available_via_narrow_module() {
    type BlockAccumulator = swink_agent::stream_assembly::BlockAccumulator;
    type OpenBlock = swink_agent::stream_assembly::OpenBlock;

    struct FakeState {
        blocks: Vec<OpenBlock>,
    }

    impl swink_agent::stream_assembly::StreamFinalize for FakeState {
        fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
            std::mem::take(&mut self.blocks)
        }
    }

    let _ = std::any::type_name::<BlockAccumulator>();

    let mut accumulator = BlockAccumulator::new();
    assert!(matches!(
        accumulator.ensure_text_open(),
        Some(swink_agent::AssistantMessageEvent::TextStart { content_index: 0 })
    ));
    assert!(matches!(
        accumulator.text_delta("alpha".to_string()),
        Some(swink_agent::AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta,
        }) if delta == "alpha"
    ));
    assert!(matches!(
        accumulator.close_text(),
        Some(swink_agent::AssistantMessageEvent::TextEnd { content_index: 0 })
    ));

    assert!(matches!(
        accumulator.ensure_thinking_open(),
        Some(swink_agent::AssistantMessageEvent::ThinkingStart { content_index: 1 })
    ));
    assert!(matches!(
        accumulator.thinking_delta("beta".to_string()),
        Some(swink_agent::AssistantMessageEvent::ThinkingDelta {
            content_index: 1,
            delta,
        }) if delta == "beta"
    ));
    assert!(matches!(
        accumulator.close_thinking(Some("sig".to_string())),
        Some(swink_agent::AssistantMessageEvent::ThinkingEnd {
            content_index: 1,
            signature,
        }) if signature.as_deref() == Some("sig")
    ));

    let (tool_call_index, tool_call_start) =
        accumulator.open_tool_call("tool-id".to_string(), "tool-name".to_string());
    assert_eq!(tool_call_index, 2);
    assert!(matches!(
        tool_call_start,
        swink_agent::AssistantMessageEvent::ToolCallStart {
            content_index: 2,
            id,
            name,
        } if id == "tool-id" && name == "tool-name"
    ));
    assert!(matches!(
        BlockAccumulator::tool_call_delta(tool_call_index, r#"{"ok":true}"#.to_string()),
        swink_agent::AssistantMessageEvent::ToolCallDelta {
            content_index: 2,
            delta,
        } if delta == r#"{"ok":true}"#
    ));
    assert!(matches!(
        accumulator.close_tool_call(tool_call_index),
        Some(swink_agent::AssistantMessageEvent::ToolCallEnd { content_index: 2 })
    ));

    assert!(matches!(
        accumulator.ensure_text_open(),
        Some(swink_agent::AssistantMessageEvent::TextStart { content_index: 3 })
    ));
    assert!(matches!(
        accumulator.close_text(),
        Some(swink_agent::AssistantMessageEvent::TextEnd { content_index: 3 })
    ));

    let mut state = FakeState {
        blocks: vec![
            OpenBlock::Text { content_index: 0 },
            OpenBlock::ToolCall { content_index: 1 },
        ],
    };
    let events = swink_agent::stream_assembly::finalize_blocks(&mut state);

    assert!(matches!(
        events.as_slice(),
        [
            swink_agent::AssistantMessageEvent::TextEnd { content_index: 0 },
            swink_agent::AssistantMessageEvent::ToolCallEnd { content_index: 1 }
        ]
    ));
}

#[test]
fn message_provider_types_re_exported() {
    fn _assert_provider<T: swink_agent::MessageProvider>() {}
    let _ = std::any::type_name::<swink_agent::ChannelMessageProvider>();
}

#[cfg(feature = "plugins")]
#[test]
fn plugin_types_re_exported() {
    fn _assert_plugin<T: swink_agent::Plugin>() {}
    let _ = std::any::type_name::<swink_agent::PluginRegistry>();
    let _ = std::any::type_name::<swink_agent::NamespacedTool>();
}
