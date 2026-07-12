use swink_agent_macros::tool;

#[tool(name = "sync_tool", description = "This should fail")]
fn sync_tool() -> swink_agent::AgentToolResult {
    swink_agent::AgentToolResult::text("nope")
}

fn main() {}
