//! Conversion functions between `rmcp` types and `swink-agent` types.

use rmcp::model::{CallToolResult, Content, RawContent, ResourceContents};
use serde_json::Value;
use swink_agent::tool::AgentToolResult;
use swink_agent::types::ContentBlock;

/// Convert an `rmcp` `Content` item to a `swink-agent` `ContentBlock`.
pub fn content_to_block(content: &Content) -> ContentBlock {
    match &content.raw {
        RawContent::Text(text) => ContentBlock::Text {
            text: text.text.clone(),
        },
        RawContent::Image(image) => ContentBlock::Image {
            source: swink_agent::types::ImageSource::Base64 {
                data: image.data.clone(),
                media_type: image.mime_type.clone(),
            },
        },
        RawContent::Resource(resource) => match &resource.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => ContentBlock::Text {
                text: format!("[MCP Resource: {uri}] {text}"),
            },
            ResourceContents::BlobResourceContents { uri, .. } => ContentBlock::Text {
                text: format!("[MCP Resource: {uri}] <binary content>"),
            },
        },
    }
}

/// Convert an `rmcp` `CallToolResult` to a `swink-agent` `AgentToolResult`.
pub fn call_result_to_agent_result(result: &CallToolResult) -> AgentToolResult {
    let is_error = result.is_error.unwrap_or(false);
    let content: Vec<ContentBlock> = result.content.iter().map(content_to_block).collect();

    if content.is_empty() {
        if is_error {
            return AgentToolResult::error("MCP tool returned an error with no content");
        }
        return AgentToolResult::text("");
    }

    AgentToolResult {
        content,
        details: Value::Null,
        is_error,
        transfer_signal: None,
    }
}

/// Extract tool definition fields from an `rmcp` `Tool`.
///
/// Returns `(name, description, input_schema)`.
pub fn tool_definition(tool: &rmcp::model::Tool) -> (String, String, Value) {
    let name = tool.name.to_string();
    let description = tool.description.to_string();
    let input_schema = tool.schema_as_json_value();
    (name, description, input_schema)
}
