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
mod tests {
    use rmcp::model::{CallToolResult, ContentBlock as McpContentBlock};
    use swink_agent::ContentBlock;

    use super::{call_result_to_agent_result, content_to_block};

    #[test]
    fn text_content_conversion() {
        let content = McpContentBlock::text("hello world");
        let block = content_to_block(&content);
        match block {
            ContentBlock::Text { text } => assert_eq!(text, "hello world"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn image_content_conversion() {
        let content = McpContentBlock::image("aW1hZ2VkYXRh", "image/png");
        let block = content_to_block(&content);
        match block {
            ContentBlock::Image { source } => match source {
                swink_agent::ImageSource::Base64 { data, media_type } => {
                    assert_eq!(data, "aW1hZ2VkYXRh");
                    assert_eq!(media_type, "image/png");
                }
                other => panic!("expected Base64 image source, got {other:?}"),
            },
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn error_result_conversion() {
        let result = {
            let mut r = CallToolResult::default();
            r.content = vec![McpContentBlock::text("something went wrong")];
            r.is_error = Some(true);
            r
        };
        let agent_result = call_result_to_agent_result(&result);
        assert!(agent_result.is_error);
        assert_eq!(agent_result.content.len(), 1);
        match &agent_result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "something went wrong"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn empty_content_handling() {
        let result = {
            let mut r = CallToolResult::default();
            r.content = vec![];
            r.is_error = Some(false);
            r
        };
        let agent_result = call_result_to_agent_result(&result);
        assert!(!agent_result.is_error);
        assert_eq!(agent_result.content.len(), 1);
        match &agent_result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, ""),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn empty_error_content_handling() {
        let result = {
            let mut r = CallToolResult::default();
            r.content = vec![];
            r.is_error = Some(true);
            r
        };
        let agent_result = call_result_to_agent_result(&result);
        assert!(agent_result.is_error);
    }

    #[test]
    fn success_result_conversion() {
        let result = {
            let mut r = CallToolResult::default();
            r.content = vec![McpContentBlock::text("success output")];
            r.is_error = Some(false);
            r
        };
        let agent_result = call_result_to_agent_result(&result);
        assert!(!agent_result.is_error);
        assert_eq!(agent_result.content.len(), 1);
    }

    #[test]
    fn resource_content_fallback() {
        let content = McpContentBlock::embedded_text("file:///tmp/test.txt", "file content here");
        let block = content_to_block(&content);
        match block {
            ContentBlock::Text { text } => {
                assert!(text.contains("file:///tmp/test.txt"));
                assert!(text.contains("file content here"));
            }
            other => panic!("expected Text fallback for resource, got {other:?}"),
        }
    }
}
