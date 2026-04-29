use schemars::JsonSchema;
use swink_agent_macros::ToolSchema;

#[derive(ToolSchema, JsonSchema)]
enum Params {
    Query { query: String },
}

fn main() {}
