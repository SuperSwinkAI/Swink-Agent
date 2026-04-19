//! Tests for the config module.

mod common;

use std::collections::HashMap;

use swink_agent_mcp::{McpServerConfig, McpTransport, ToolFilter};

#[test]
fn server_config_construction() {
    let config = McpServerConfig {
        name: "test-server".into(),
        transport: McpTransport::Stdio {
            command: "echo".into(),
            args: vec!["hello".into()],
            env: std::collections::HashMap::default(),
        },
        tool_prefix: Some("test".into()),
        tool_filter: None,
        requires_approval: true,
    };

    assert_eq!(config.name, "test-server");
    assert!(config.requires_approval);
    assert_eq!(config.tool_prefix.as_deref(), Some("test"));
}

#[test]
fn sse_transport_construction() {
    let config = McpServerConfig {
        name: "remote".into(),
        transport: McpTransport::Sse {
            url: "http://localhost:8080/sse".into(),
            bearer_token: Some("secret".into()),
            headers: HashMap::from([("x-api-key".into(), "abc123".into())]),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    assert_eq!(config.name, "remote");
    assert!(!config.requires_approval);
}

#[test]
fn sse_transport_deserialization_defaults_headers() {
    let json = r#"{
        "name": "remote",
        "transport": {
            "type": "sse",
            "url": "http://localhost:8080/sse",
            "bearer_token": "secret"
        },
        "tool_prefix": null,
        "tool_filter": null,
        "requires_approval": false
    }"#;

    let config: McpServerConfig = serde_json::from_str(json).expect("deserialize");

    match config.transport {
        McpTransport::Sse {
            url,
            bearer_token,
            headers,
        } => {
            assert_eq!(url, "http://localhost:8080/sse");
            assert_eq!(bearer_token.as_deref(), Some("secret"));
            assert!(headers.is_empty(), "legacy configs should default headers");
        }
        other @ McpTransport::Stdio { .. } => panic!("expected SSE transport, got {other:?}"),
    }
}

#[test]
fn tool_filter_allow_only() {
    let filter = ToolFilter {
        allow: Some(vec!["read".into(), "write".into()]),
        deny: None,
    };

    assert!(filter.matches("read"));
    assert!(filter.matches("write"));
    assert!(!filter.matches("delete"));
}

#[test]
fn tool_filter_deny_only() {
    let filter = ToolFilter {
        allow: None,
        deny: Some(vec!["delete".into()]),
    };

    assert!(filter.matches("read"));
    assert!(filter.matches("write"));
    assert!(!filter.matches("delete"));
}

#[test]
fn tool_filter_both_allow_and_deny() {
    let filter = ToolFilter {
        allow: Some(vec!["read".into(), "write".into(), "delete".into()]),
        deny: Some(vec!["delete".into()]),
    };

    assert!(filter.matches("read"));
    assert!(filter.matches("write"));
    assert!(!filter.matches("delete")); // denied even though allowed
    assert!(!filter.matches("list")); // not in allow list
}

#[test]
fn tool_filter_neither() {
    let filter = ToolFilter {
        allow: None,
        deny: None,
    };

    assert!(filter.matches("anything"));
    assert!(filter.matches("goes"));
}

#[test]
fn config_serialization_roundtrip() {
    let config = McpServerConfig {
        name: "test".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "mcp-server".into()],
            env: std::collections::HashMap::default(),
        },
        tool_prefix: Some("fs".into()),
        tool_filter: Some(ToolFilter {
            allow: Some(vec!["read".into()]),
            deny: None,
        }),
        requires_approval: true,
    };

    let json = serde_json::to_string(&config).expect("serialize");
    let roundtrip: McpServerConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(roundtrip.name, config.name);
    assert_eq!(roundtrip.tool_prefix, config.tool_prefix);
    assert!(roundtrip.requires_approval);
}

#[test]
fn sse_config_serialization_roundtrip_preserves_headers() {
    let config = McpServerConfig {
        name: "remote".into(),
        transport: McpTransport::Sse {
            url: "https://example.com/mcp".into(),
            bearer_token: Some("secret".into()),
            headers: HashMap::from([
                ("x-api-key".into(), "key-123".into()),
                ("x-trace-id".into(), "trace-abc".into()),
            ]),
        },
        tool_prefix: Some("remote".into()),
        tool_filter: None,
        requires_approval: false,
    };

    let json = serde_json::to_string(&config).expect("serialize");
    let roundtrip: McpServerConfig = serde_json::from_str(&json).expect("deserialize");

    match roundtrip.transport {
        McpTransport::Sse {
            url,
            bearer_token,
            headers,
        } => {
            assert_eq!(url, "https://example.com/mcp");
            assert_eq!(bearer_token.as_deref(), Some("secret"));
            assert_eq!(
                headers.get("x-api-key").map(String::as_str),
                Some("key-123")
            );
            assert_eq!(
                headers.get("x-trace-id").map(String::as_str),
                Some("trace-abc")
            );
        }
        other @ McpTransport::Stdio { .. } => panic!("expected SSE transport, got {other:?}"),
    }
}
