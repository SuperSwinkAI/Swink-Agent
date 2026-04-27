//! JSON-RPC 2.0 message types.

use serde::{Deserialize, Serialize};

// ─── RequestId ────────────────────────────────────────────────────────────────

/// A JSON-RPC 2.0 request identifier — either a number or a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    Str(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Str(s) => write!(f, "{s}"),
        }
    }
}

// ─── RpcError ─────────────────────────────────────────────────────────────────

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("JSON-RPC error {code}: {message}")]
pub struct RpcError {
    /// Standard or application-defined error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// JSON-RPC 2.0 standard: Parse error.
    pub const PARSE_ERROR: i64 = -32700;
    /// JSON-RPC 2.0 standard: Invalid request.
    pub const INVALID_REQUEST: i64 = -32600;
    /// JSON-RPC 2.0 standard: Method not found.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// JSON-RPC 2.0 standard: Invalid params.
    pub const INVALID_PARAMS: i64 = -32602;
    /// JSON-RPC 2.0 standard: Internal error.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// Application: Protocol version mismatch.
    pub const PROTOCOL_MISMATCH: i64 = -32099;
    /// Application: Session in use (single-client server).
    pub const SESSION_IN_USE: i64 = -32098;
    /// Application: Peer disconnected.
    pub const DISCONNECTED: i64 = -32097;
    /// Application: Transport not available on this platform.
    pub const UNAVAILABLE: i64 = -32096;

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self { code: Self::PARSE_ERROR, message: msg.into(), data: None }
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self { code: Self::INVALID_REQUEST, message: msg.into(), data: None }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: Self::METHOD_NOT_FOUND,
            message: format!("method not found: {method}"),
            data: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self { code: Self::INTERNAL_ERROR, message: msg.into(), data: None }
    }

    pub fn protocol_mismatch(msg: impl Into<String>) -> Self {
        Self { code: Self::PROTOCOL_MISMATCH, message: msg.into(), data: None }
    }

    pub fn session_in_use() -> Self {
        Self { code: Self::SESSION_IN_USE, message: "session in use".into(), data: None }
    }

    pub fn disconnected() -> Self {
        Self { code: Self::DISCONNECTED, message: "peer disconnected".into(), data: None }
    }

    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self { code: Self::UNAVAILABLE, message: msg.into(), data: None }
    }
}

// ─── RawMessage ───────────────────────────────────────────────────────────────

/// A flat JSON-RPC 2.0 message envelope used for both serialization and
/// deserialization. Classification (request / response / notification) is
/// determined by which optional fields are present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMessage {
    /// Must be exactly `"2.0"`.
    pub jsonrpc: String,
    /// Present in requests and responses; absent in notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Present in requests and notifications; absent in responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Request / notification parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Successful response result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error response payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RawMessage {
    const VERSION: &'static str = "2.0";

    pub fn request(id: RequestId, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: Self::VERSION.into(),
            id: Some(id),
            method: Some(method.into()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn notification(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: Self::VERSION.into(),
            id: None,
            method: Some(method.into()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    pub fn success(id: RequestId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: Self::VERSION.into(),
            id: Some(id),
            method: None,
            params: None,
            result: Some(result),
            error: None,
        }
    }

    pub fn error_response(id: RequestId, err: RpcError) -> Self {
        Self {
            jsonrpc: Self::VERSION.into(),
            id: Some(id),
            method: None,
            params: None,
            result: None,
            error: Some(err),
        }
    }

    /// Classify this message.
    pub fn classify(&self) -> MessageKind<'_> {
        match (&self.id, &self.method) {
            (Some(id), Some(method)) => MessageKind::Request { id, method },
            (None, Some(method)) => MessageKind::Notification { method },
            (Some(id), None) => MessageKind::Response { id },
            (None, None) => MessageKind::Invalid,
        }
    }
}

/// Logical classification of a [`RawMessage`].
#[derive(Debug)]
pub enum MessageKind<'a> {
    Request { id: &'a RequestId, method: &'a str },
    Notification { method: &'a str },
    Response { id: &'a RequestId },
    Invalid,
}
