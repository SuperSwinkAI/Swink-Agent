# Public API Contract: swink-agent-adapters (Bedrock)

**Feature**: 019-adapter-bedrock | **Date**: 2026-04-02

## Feature Gate

```toml
[dependencies]
swink-agent-adapters = { features = ["bedrock"] }
```

Activates deps: `sha2`, `hmac`, `chrono`, `aws-smithy-eventstream`, `aws-smithy-types`

## Public Type

### `BedrockStreamFn`

```rust
pub struct BedrockStreamFn { /* private */ }

impl BedrockStreamFn {
    /// Create a new Bedrock stream function.
    ///
    /// # Arguments
    /// * `region` - AWS region (e.g., `us-east-1`)
    /// * `access_key_id` - AWS access key ID
    /// * `secret_access_key` - AWS secret access key
    /// * `session_token` - Optional session token for temporary credentials
    pub fn new(
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self;

    /// Create with a custom base URL (for testing or custom endpoints).
    pub fn new_with_base_url(
        base_url: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self;
}

impl StreamFn for BedrockStreamFn { /* ... */ }
impl Debug for BedrockStreamFn { /* redacted credentials */ }
// Send + Sync enforced at compile time
```

## Usage Example

```rust
use swink_agent_adapters::BedrockStreamFn;

let stream_fn = BedrockStreamFn::new(
    "us-east-1",
    std::env::var("AWS_ACCESS_KEY_ID").unwrap(),
    std::env::var("AWS_SECRET_ACCESS_KEY").unwrap(),
    std::env::var("AWS_SESSION_TOKEN").ok(),
);
// Use with Agent::new() or directly via stream_fn.stream()
```

## Wire Protocol

- **Endpoint**: `POST {base_url}/model/{model_id}/converse-stream`
- **Auth**: AWS SigV4 (`Authorization`, `x-amz-date`, `x-amz-content-sha256`, optional `x-amz-security-token`)
- **Request**: `Content-Type: application/json` — Bedrock ConverseStream request body
- **Response**: `Content-Type: application/vnd.amazon.eventstream` — binary event-stream frames
- **Events**: `messageStart` → `contentBlockStart/Delta/Stop`* → `messageStop` → `metadata`

## Behavioral Contract

1. Text deltas arrive as individual `AssistantMessageEvent::TextDelta` events (true streaming)
2. Tool calls emit `ToolCallStart` (with name/id), `ToolCallDelta` (partial JSON), then `ToolCallEnd`
3. HTTP errors classified via shared classifier (429→throttled, 403→auth, 5xx→network)
4. Network errors produce `AssistantMessageEvent::error_network()`
5. `GUARDRAIL_INTERVENED` stop reason → `ContentFiltered` error event
6. Cancellation token terminates the stream immediately
7. Usage data surfaced from `metadata` event in terminal `Done` event
8. All requests signed with AWS SigV4 (including session token if present)
