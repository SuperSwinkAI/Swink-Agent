//! Tests for `plugin`.
#![cfg(test)]

use super::{WebEventClass, classify_web_event};
use serde_json::json;
use swink_agent::AgentEvent;
use swink_agent::AgentToolResult;

fn tool_end(name: &str, is_error: bool) -> AgentEvent {
    AgentEvent::ToolExecutionEnd {
        id: "tc1".into(),
        name: name.into(),
        result: if is_error {
            AgentToolResult::error("boom")
        } else {
            AgentToolResult::text("ok")
        },
        is_error,
    }
}

fn tool_start(name: &str) -> AgentEvent {
    AgentEvent::ToolExecutionStart {
        id: "tc1".into(),
        name: name.into(),
        arguments: json!({}),
    }
}

#[test]
fn non_web_tool_error_is_not_attributed_to_web_plugin() {
    // Regression for #237: the plugin previously matched every failing
    // ToolExecutionEnd as a web-tool failure, including tools from other
    // namespaces (e.g., `bash.run`).
    assert_eq!(
        classify_web_event(&tool_end("bash.run", true)),
        WebEventClass::Ignored
    );
    assert_eq!(
        classify_web_event(&tool_end("unrelated_tool", true)),
        WebEventClass::Ignored
    );
}

#[test]
fn web_tool_error_is_attributed_to_web_plugin() {
    assert_eq!(
        classify_web_event(&tool_end("web_fetch", true)),
        WebEventClass::Error("web_fetch")
    );
}

#[test]
fn successful_web_tool_end_is_ignored() {
    assert_eq!(
        classify_web_event(&tool_end("web_fetch", false)),
        WebEventClass::Ignored
    );
}

#[test]
fn web_tool_start_is_classified() {
    assert_eq!(
        classify_web_event(&tool_start("web_search")),
        WebEventClass::Start("web_search")
    );
    assert_eq!(
        classify_web_event(&tool_start("bash.run")),
        WebEventClass::Ignored
    );
}
