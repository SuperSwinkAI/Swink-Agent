use schemars::JsonSchema;
use swink_agent_macros::ToolSchema;

#[derive(ToolSchema, JsonSchema)]
struct Params(String);

fn main() {}
