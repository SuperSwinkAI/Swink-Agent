//! Conversion functions between `rmcp` types and `swink-agent` types.
//!
//! Internal module: no `rmcp` type may cross this crate's public boundary,
//! so these conversions are applied before results leave the crate (see
//! [`McpConnection::call_tool`](crate::McpConnection::call_tool)).

use rmcp::model::{CallToolResult, ContentBlock as McpContentBlock, ResourceContents};
use swink_agent::{AgentToolResult, ContentBlock};

/// Convert an `rmcp` content block to a `swink-agent` `ContentBlock`.
pub fn content_to_block(content: &McpContentBlock) -> ContentBlock {
    #[allow(unreachable_patterns)]
    match content {
        McpContentBlock::Text(text) => ContentBlock::Text {
            text: text.text.clone(),
        },
        McpContentBlock::Image(image) => ContentBlock::Image {
            source: swink_agent::ImageSource::Base64 {
                data: image.data.clone(),
                media_type: image.mime_type.clone(),
            },
        },
        McpContentBlock::Resource(resource) => match &resource.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => ContentBlock::Text {
                text: format!("[MCP Resource: {uri}] {text}"),
            },
            ResourceContents::BlobResourceContents { uri, .. } => ContentBlock::Text {
                text: format!("[MCP Resource: {uri}] <binary content>"),
            },
            _ => ContentBlock::Text {
                text: "[MCP Resource: unsupported content]".to_string(),
            },
        },
        McpContentBlock::Audio(audio) => ContentBlock::Text {
            text: format!("[MCP Audio: {}]", audio.mime_type),
        },
        McpContentBlock::ResourceLink(link) => ContentBlock::Text {
            text: format!("[MCP ResourceLink: {}]", link.uri),
        },
        _ => ContentBlock::Text {
            text: "[MCP: unsupported content type]".to_string(),
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

    AgentToolResult::new(content, is_error)
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
