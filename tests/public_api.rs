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
fn metrics_types_re_exported() {
    fn _assert_metrics<T: swink_agent::MetricsCollector>() {}
    let _ = std::any::type_name::<swink_agent::TurnMetrics>();
    let _ = std::any::type_name::<swink_agent::ToolExecMetrics>();
}

#[test]
fn block_accumulator_re_exported() {
    let _ = std::any::type_name::<swink_agent::BlockAccumulator>();
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
