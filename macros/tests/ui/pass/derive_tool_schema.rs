use schemars::JsonSchema;
use swink_agent::ToolParameters;
use swink_agent_macros::ToolSchema;

#[derive(ToolSchema, JsonSchema)]
struct SearchParams {
    query: String,
    limit: Option<u32>,
}

fn main() {
    let schema = <SearchParams as ToolParameters>::json_schema();
    assert_eq!(schema["type"], "object");
}
