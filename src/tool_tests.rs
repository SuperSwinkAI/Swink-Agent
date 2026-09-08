//! Tests for `tool`.
#![cfg(test)]

use serde_json::json;

use super::*;
use crate::FnTool;

fn stub_tool(name: &str) -> FnTool {
    FnTool::new(name, name, "A test tool.")
}

// ─── ToolApprovalRequest Debug ──────────────────────────────────────────

#[test]
fn approval_request_debug_redacts_arguments_and_context() {
    let req = ToolApprovalRequest {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        arguments: json!({"command": "echo secret"}),
        requires_approval: true,
        context: Some(json!({
            "Authorization": "Bearer top-secret",
            "path": "/tmp/output.txt",
        })),
    };
    let debug = format!("{req:?}");
    assert!(debug.contains("tool_call_id: \"call_1\""));
    assert!(debug.contains("tool_name: \"bash\""));
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("echo secret"));
    assert!(!debug.contains("top-secret"));
    assert!(debug.contains("/tmp/output.txt"));
}

// ─── redact_sensitive_values ────────────────────────────────────────────

#[test]
fn redacts_sk_prefix() {
    let val = json!({"key": "sk-abc123"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["key"], json!("[REDACTED]"));
}

#[test]
fn redacts_key_prefix() {
    let val = json!({"data": "key-live-xyz"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["data"], json!("[REDACTED]"));
}

#[test]
fn redacts_token_prefix() {
    let val = json!({"tok": "token-abcdef"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["tok"], json!("[REDACTED]"));
}

#[test]
fn redacts_bearer_prefix_case_insensitive() {
    let val = json!({"auth": "Bearer eyJhbGciOi..."});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["auth"], json!("[REDACTED]"));

    let val2 = json!({"auth": "bearer xyz"});
    let redacted2 = redact_sensitive_values(&val2);
    assert_eq!(redacted2["auth"], json!("[REDACTED]"));
}

#[test]
fn redacts_basic_prefix_case_insensitive() {
    let val = json!({"auth": "Basic dXNlcjpwYXNz"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["auth"], json!("[REDACTED]"));

    let val2 = json!({"auth": "basic abc"});
    let redacted2 = redact_sensitive_values(&val2);
    assert_eq!(redacted2["auth"], json!("[REDACTED]"));
}

#[test]
fn redacts_env_var_dollar_sign() {
    let val = json!({"ref": "$SECRET"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["ref"], json!("[REDACTED]"));
}

#[test]
fn redacts_env_var_braced() {
    let val = json!({"ref": "${API_KEY}"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["ref"], json!("[REDACTED]"));
}

#[test]
fn redacts_sensitive_key_password() {
    let val = json!({"password": "hunter2"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["password"], json!("[REDACTED]"));
}

#[test]
fn redacts_sensitive_key_secret() {
    let val = json!({"secret": "mysecret"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["secret"], json!("[REDACTED]"));
}

#[test]
fn redacts_sensitive_key_token() {
    let val = json!({"Token": "abc"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["Token"], json!("[REDACTED]"));
}

#[test]
fn redacts_sensitive_key_api_key() {
    let val = json!({"api_key": "abc"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["api_key"], json!("[REDACTED]"));
}

#[test]
fn redacts_sensitive_key_apikey() {
    let val = json!({"apiKey": "abc"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["apiKey"], json!("[REDACTED]"));
}

#[test]
fn redacts_sensitive_key_authorization() {
    let val = json!({"Authorization": "something"});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["Authorization"], json!("[REDACTED]"));
}

#[test]
fn passes_through_non_sensitive_values() {
    let val = json!({
        "command": "echo hello",
        "path": "/tmp/file.txt",
        "count": 42,
        "verbose": true,
        "items": ["one", "two"]
    });
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted, val);
}

#[test]
fn redacts_nested_objects() {
    let val = json!({
        "config": {
            "password": "secret123",
            "host": "localhost"
        }
    });
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted["config"]["password"], json!("[REDACTED]"));
    assert_eq!(redacted["config"]["host"], json!("localhost"));
}

#[test]
fn redacts_values_in_arrays() {
    let val = json!(["normal", "sk-secret", "also normal"]);
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted, json!(["normal", "[REDACTED]", "also normal"]));
}

#[test]
fn handles_null_and_numbers() {
    let val = json!({"a": null, "b": 42, "c": 2.72});
    let redacted = redact_sensitive_values(&val);
    assert_eq!(redacted, val);
}

// ─── validate_schema ──────────────────────────────────────────────────

#[test]
fn valid_schema_passes() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"]
    });
    assert!(validate_schema(&schema).is_ok());
}

#[test]
fn invalid_schema_returns_error() {
    let schema = json!({
        "type": "not_a_real_type"
    });
    assert!(validate_schema(&schema).is_err());
}

#[test]
fn empty_object_schema_is_valid() {
    let schema = json!({
        "type": "object",
        "properties": {}
    });
    assert!(validate_schema(&schema).is_ok());
}

// ─── ApprovalMode ─────────────────────────────────────────────────────

#[test]
fn approval_mode_default_is_smart() {
    assert_eq!(ApprovalMode::default(), ApprovalMode::Smart);
}

#[test]
fn approval_mode_variants_are_distinct() {
    assert_ne!(ApprovalMode::Enabled, ApprovalMode::Smart);
    assert_ne!(ApprovalMode::Smart, ApprovalMode::Bypassed);
    assert_ne!(ApprovalMode::Enabled, ApprovalMode::Bypassed);
}

// ─── ToolMetadata ────────────────────────────────────────────────────

#[test]
fn tool_metadata_default_is_empty() {
    let meta = ToolMetadata::default();
    assert_eq!(meta.namespace, None);
    assert_eq!(meta.version, None);
}

#[test]
fn tool_metadata_builder() {
    let meta = ToolMetadata::with_namespace("filesystem").with_version("1.2.0");
    assert_eq!(meta.namespace.as_deref(), Some("filesystem"));
    assert_eq!(meta.version.as_deref(), Some("1.2.0"));
}

#[test]
fn agent_tool_metadata_defaults_to_none() {
    let tool = stub_tool("minimal");
    assert!(tool.metadata().is_none());
}

// T025: auth_config default returns None
#[test]
fn agent_tool_auth_config_defaults_to_none() {
    let tool = stub_tool("no-auth");
    assert!(tool.auth_config().is_none());
}

// ─── approval_context ────────────────────────────────────────────────

#[test]
fn approval_context_default_none() {
    let tool = stub_tool("plain");
    assert!(tool.approval_context(&json!({})).is_none());
}

#[test]
fn approval_context_returns_value() {
    use crate::FnTool;

    let tool = FnTool::new("ctx", "Ctx", "With context").with_approval_context(|params| {
        Some(json!({"preview": format!("Will process: {}", params)}))
    });

    let ctx = tool.approval_context(&json!({"file": "test.txt"}));
    assert!(ctx.is_some());
    assert!(
        ctx.unwrap()["preview"]
            .as_str()
            .unwrap()
            .contains("test.txt")
    );
}

#[test]
fn approval_context_panic_caught() {
    use crate::FnTool;

    let tool =
        FnTool::new("panicker", "Panicker", "Panics in context").with_approval_context(|_params| {
            panic!("oops");
        });

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tool.approval_context(&json!({}))
    }));
    // Panic is caught
    assert!(result.is_err());
}

#[test]
fn approval_request_includes_context() {
    let ctx = json!({"diff": "+new line"});
    let req = ToolApprovalRequest {
        tool_call_id: "call_1".into(),
        tool_name: "write_file".into(),
        arguments: json!({"path": "/tmp/test"}),
        requires_approval: true,
        context: Some(ctx.clone()),
    };
    assert_eq!(req.context, Some(ctx));
}

// ─── Transfer signal on AgentToolResult ─────────────────────────────

#[test]
fn transfer_constructor_sets_signal_and_text() {
    use crate::transfer::TransferSignal;

    let signal = TransferSignal::new("billing", "billing issue");
    let result = AgentToolResult::transfer(signal);

    assert!(result.is_transfer());
    assert!(!result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.as_str(),
        _ => panic!("expected text block"),
    };
    assert_eq!(text, "Transfer to billing initiated.");
    assert!(result.transfer_signal.is_some());
    let sig = result.transfer_signal.as_ref().unwrap();
    assert_eq!(sig.target_agent(), "billing");
    assert_eq!(sig.reason(), "billing issue");
}

#[test]
fn text_constructor_has_no_transfer_signal() {
    let result = AgentToolResult::text("hello");
    assert!(!result.is_transfer());
    assert!(result.transfer_signal.is_none());
}

#[test]
fn error_constructor_has_no_transfer_signal() {
    let result = AgentToolResult::error("something failed");
    assert!(!result.is_transfer());
    assert!(result.transfer_signal.is_none());
}

#[test]
fn deserialize_without_transfer_signal_defaults_to_none() {
    let json = r#"{
            "content": [{"type": "text", "text": "hello"}],
            "details": null,
            "is_error": false
        }"#;
    let result: AgentToolResult = serde_json::from_str(json).unwrap();
    assert!(!result.is_transfer());
    assert!(result.transfer_signal.is_none());
}

#[test]
fn transfer_signal_not_serialized_when_none() {
    let result = AgentToolResult::text("hello");
    let json = serde_json::to_value(&result).unwrap();
    assert!(!json.as_object().unwrap().contains_key("transfer_signal"));
}
