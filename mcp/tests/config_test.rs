//! Tests for the config module.

mod common;

use std::collections::HashMap;

use swink_agent::CredentialType;
use swink_agent_mcp::{McpServerConfig, McpTransport, SseBearerAuth, ToolFilter};

#[test]
fn server_config_construction() {
    let config = McpServerConfig::new(
        "test-server",
        McpTransport::Stdio {
            command: "echo".into(),
            args: vec!["hello".into()],
            env: std::collections::HashMap::default(),
        },
    )
    .with_tool_prefix("test")
    .with_connect_timeout_ms(1_500)
    .with_discovery_timeout_ms(2_500);

    assert_eq!(config.name, "test-server");
    assert!(config.requires_approval);
    assert_eq!(config.tool_prefix.as_deref(), Some("test"));
    assert_eq!(config.connect_timeout_ms, Some(1_500));
    assert_eq!(config.discovery_timeout_ms, Some(2_500));
}

#[test]
fn sse_transport_construction() {
    let config = McpServerConfig::new(
        "remote",
        McpTransport::Sse {
            url: "http://localhost:8080/sse".into(),
            bearer_token: Some("secret".into()),
            bearer_auth: None,
            headers: HashMap::from([("x-api-key".into(), "abc123".into())]),
        },
    )
    .with_requires_approval(false);

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
        "requires_approval": false,
        "connect_timeout_ms": 250,
        "discovery_timeout_ms": 500
    }"#;

    let config: McpServerConfig = serde_json::from_str(json).expect("deserialize");
    assert_eq!(config.connect_timeout_ms, Some(250));
    assert_eq!(config.discovery_timeout_ms, Some(500));

    match config.transport {
        McpTransport::Sse {
            url,
            bearer_token,
            bearer_auth,
            headers,
        } => {
            assert_eq!(url, "http://localhost:8080/sse");
            assert_eq!(bearer_token.as_deref(), Some("secret"));
            assert!(
                bearer_auth.is_none(),
                "legacy configs should default bearer auth"
            );
            assert!(headers.is_empty(), "legacy configs should default headers");
        }
        other => panic!("expected SSE transport, got {other:?}"),
    }
}

#[test]
fn tool_filter_allow_only() {
    let filter = ToolFilter::new().with_allow(vec!["read".into(), "write".into()]);

    assert!(filter.matches("read"));
    assert!(filter.matches("write"));
    assert!(!filter.matches("delete"));
}

#[test]
fn tool_filter_deny_only() {
    let filter = ToolFilter::new().with_deny(vec!["delete".into()]);

    assert!(filter.matches("read"));
    assert!(filter.matches("write"));
    assert!(!filter.matches("delete"));
}

#[test]
fn tool_filter_both_allow_and_deny() {
    let filter = ToolFilter::new()
        .with_allow(vec!["read".into(), "write".into(), "delete".into()])
        .with_deny(vec!["delete".into()]);

    assert!(filter.matches("read"));
    assert!(filter.matches("write"));
    assert!(!filter.matches("delete")); // denied even though allowed
    assert!(!filter.matches("list")); // not in allow list
}

#[test]
fn tool_filter_neither() {
    let filter = ToolFilter::new();

    assert!(filter.matches("anything"));
    assert!(filter.matches("goes"));
}

#[test]
fn config_serialization_roundtrip() {
    let config = McpServerConfig::new(
        "test",
        McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "mcp-server".into()],
            env: std::collections::HashMap::default(),
        },
    )
    .with_tool_prefix("fs")
    .with_tool_filter(ToolFilter::new().with_allow(vec!["read".into()]))
    .with_connect_timeout_ms(750)
    .with_discovery_timeout_ms(1_250);

    let json = serde_json::to_string(&config).expect("serialize");
    let roundtrip: McpServerConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(roundtrip.name, config.name);
    assert_eq!(roundtrip.tool_prefix, config.tool_prefix);
    assert!(roundtrip.requires_approval);
    assert_eq!(roundtrip.connect_timeout_ms, Some(750));
    assert_eq!(roundtrip.discovery_timeout_ms, Some(1_250));
}

#[test]
fn sse_config_serialization_roundtrip_preserves_headers() {
    let config = McpServerConfig::new(
        "remote",
        McpTransport::Sse {
            url: "https://example.com/mcp".into(),
            bearer_token: Some("secret".into()),
            bearer_auth: Some(SseBearerAuth::new("mcp-sse", CredentialType::OAuth2)),
            headers: HashMap::from([
                ("x-api-key".into(), "key-123".into()),
                ("x-trace-id".into(), "trace-abc".into()),
            ]),
        },
    )
    .with_tool_prefix("remote")
    .with_requires_approval(false)
    .with_connect_timeout_ms(1_000)
    .with_discovery_timeout_ms(2_000);

    let json = serde_json::to_string(&config).expect("serialize");
    let roundtrip: McpServerConfig = serde_json::from_str(&json).expect("deserialize");

    match roundtrip.transport {
        McpTransport::Sse {
            url,
            bearer_token,
            bearer_auth,
            headers,
        } => {
            assert_eq!(url, "https://example.com/mcp");
            assert_eq!(bearer_token.as_deref(), Some("secret"));
            assert_eq!(
                bearer_auth,
                Some(SseBearerAuth::new("mcp-sse", CredentialType::OAuth2))
            );
            assert_eq!(
                headers.get("x-api-key").map(String::as_str),
                Some("key-123")
            );
            assert_eq!(
                headers.get("x-trace-id").map(String::as_str),
                Some("trace-abc")
            );
        }
        other => panic!("expected SSE transport, got {other:?}"),
    }
    assert_eq!(roundtrip.connect_timeout_ms, Some(1_000));
    assert_eq!(roundtrip.discovery_timeout_ms, Some(2_000));
}
