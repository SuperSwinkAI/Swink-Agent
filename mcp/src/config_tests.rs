//! Tests for `config`.
#![cfg(test)]

use super::*;

#[test]
fn sse_transport_debug_redacts_bearer_and_sensitive_headers() {
    let transport = McpTransport::StreamableHttp {
        url: "https://mcp.example/sse".to_string(),
        bearer_token: Some("bearer-secret-token".to_string()),
        bearer_auth: None,
        headers: HashMap::from([
            (
                "Authorization".to_string(),
                "Bearer auth-secret".to_string(),
            ),
            ("x-api-key".to_string(), "api-secret".to_string()),
            ("x-trace-id".to_string(), "trace-123".to_string()),
        ]),
    };

    let debug = format!("{transport:?}");

    assert!(
        !debug.contains("bearer-secret-token"),
        "Debug leaks bearer token"
    );
    assert!(
        !debug.contains("auth-secret"),
        "Debug leaks Authorization header"
    );
    assert!(!debug.contains("api-secret"), "Debug leaks API key header");
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("trace-123"));
}

#[test]
fn stdio_transport_debug_redacts_sensitive_env_values() {
    let transport = McpTransport::Stdio {
        command: "server".to_string(),
        args: vec![],
        env: HashMap::from([
            ("API_TOKEN".to_string(), "env-secret".to_string()),
            ("RUST_LOG".to_string(), "debug".to_string()),
        ]),
    };

    let debug = format!("{transport:?}");

    assert!(
        !debug.contains("env-secret"),
        "Debug leaks sensitive env value"
    );
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("debug"));
}
