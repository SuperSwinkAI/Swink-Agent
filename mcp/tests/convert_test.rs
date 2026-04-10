//! Tests for the convert module.

mod common;

use rmcp::model::{CallToolResult, Content};
use swink_agent::ContentBlock;

#[test]
fn text_content_conversion() {
    let content = Content::text("hello world");
    let block = swink_agent_mcp::convert::content_to_block(&content);
    match block {
        ContentBlock::Text { text } => assert_eq!(text, "hello world"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn image_content_conversion() {
    let content = Content::image("aW1hZ2VkYXRh", "image/png");
    let block = swink_agent_mcp::convert::content_to_block(&content);
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
        r.content = vec![Content::text("something went wrong")];
        r.is_error = Some(true);
        r
    };
    let agent_result = swink_agent_mcp::convert::call_result_to_agent_result(&result);
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
    let agent_result = swink_agent_mcp::convert::call_result_to_agent_result(&result);
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
    let agent_result = swink_agent_mcp::convert::call_result_to_agent_result(&result);
    assert!(agent_result.is_error);
}

#[test]
fn success_result_conversion() {
    let result = {
        let mut r = CallToolResult::default();
        r.content = vec![Content::text("success output")];
        r.is_error = Some(false);
        r
    };
    let agent_result = swink_agent_mcp::convert::call_result_to_agent_result(&result);
    assert!(!agent_result.is_error);
    assert_eq!(agent_result.content.len(), 1);
}

#[test]
fn resource_content_fallback() {
    let content = Content::embedded_text("file:///tmp/test.txt", "file content here");
    let block = swink_agent_mcp::convert::content_to_block(&content);
    match block {
        ContentBlock::Text { text } => {
            assert!(text.contains("file:///tmp/test.txt"));
            assert!(text.contains("file content here"));
        }
        other => panic!("expected Text fallback for resource, got {other:?}"),
    }
}
