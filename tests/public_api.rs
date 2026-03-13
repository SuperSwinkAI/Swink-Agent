use swink_agent::{
    Agent, AgentError, AgentTool, DefaultRetryStrategy, MessageProvider, RetryStrategy, StreamFn,
    ToolApproval, ToolApprovalRequest, ToolCallTransformer, from_fns,
};
#[cfg(feature = "builtin-tools")]
use swink_agent::{BashTool, ReadFileTool, WriteFileTool};
use swink_agent_adapters::ProxyStreamFn;
use swink_agent_tui::{App, TuiConfig};

#[test]
fn top_level_reexports_remain_consumable() {
    let _agent_type = std::any::type_name::<Agent>();
    let _provider_type = std::any::type_name::<dyn MessageProvider>();
    let _stream_type = std::any::type_name::<dyn StreamFn>();
    let _tool_type = std::any::type_name::<dyn AgentTool>();
    let _retry_type = std::any::type_name::<DefaultRetryStrategy>();
    let _retry_trait = std::any::type_name::<dyn RetryStrategy>();
    let _proxy = ProxyStreamFn::new("https://example.com", "token");
    let _: fn(TuiConfig) -> App = App::new;
    let _tool_approval_type = std::any::type_name::<ToolApproval>();
    let _tool_request_type = std::any::type_name::<ToolApprovalRequest>();
    let _transformer_type = std::any::type_name::<dyn ToolCallTransformer>();
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
