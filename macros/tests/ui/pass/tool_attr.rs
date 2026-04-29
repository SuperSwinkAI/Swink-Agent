use swink_agent::{AgentTool, AgentToolResult};
use swink_agent_macros::tool;
use tokio_util::sync::CancellationToken;

#[tool(name = "echo", description = "Echo a message")]
async fn echo(message: String, cancel: CancellationToken) -> AgentToolResult {
    let _ = cancel;
    AgentToolResult::text(message)
}

fn main() {
    let tool = EchoTool;
    assert_eq!(tool.name(), "echo");
    assert!(tool.parameters_schema()["properties"]["message"].is_object());
    assert!(tool.parameters_schema()["properties"].get("cancel").is_none());
}
