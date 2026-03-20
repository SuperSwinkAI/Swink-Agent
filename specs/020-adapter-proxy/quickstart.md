# Quickstart: Adapter: Proxy

**Feature**: 020-adapter-proxy | **Date**: 2026-03-20

## Prerequisites

- Rust 1.88+ (edition 2024)
- `swink-agent` core crate available as a path dependency
- A running proxy server that speaks the SSE protocol described in `contracts/public-api.md`

## Build & Test

```bash
# Build the adapters crate
cargo build -p swink-agent-adapters

# Run all proxy adapter tests
cargo test -p swink-agent-adapters proxy

# Run full adapters test suite
cargo test -p swink-agent-adapters

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Basic Setup

```rust
use swink_agent_adapters::ProxyStreamFn;

let proxy = ProxyStreamFn::new(
    "https://proxy.example.com",
    "my-bearer-token",
);

// Use with Agent via ModelConnection
let connection = ModelConnection::new(model_spec, Arc::new(proxy));
let agent = Agent::new(connection);
```

### Debug Output (Token Redacted)

```rust
let proxy = ProxyStreamFn::new("https://proxy.example.com", "secret");
println!("{proxy:?}");
// ProxyStreamFn { base_url: "https://proxy.example.com", bearer_token: "[redacted]", .. }
```

### Per-Request Token Override

```rust
// StreamOptions.api_key overrides the stored bearer token
let options = StreamOptions {
    api_key: Some("override-token".to_owned()),
    ..Default::default()
};
// The request uses "override-token" instead of the stored token
```

### Error Handling

```rust
// Errors are emitted as AssistantMessageEvent variants in the stream:
// - Connection failure  → error_network("network error: ...")
// - HTTP 401/403        → error_auth("authentication failure (401)")
// - HTTP 429            → error_throttled("rate limit (429)")
// - HTTP 5xx            → error_network("network error: HTTP 500")
// - Malformed JSON      → Error { error_message: "malformed SSE event JSON: ..." }
// - Stream cut off      → error_network("SSE stream ended unexpectedly")
// - Cancellation        → Error { stop_reason: Aborted, ... }
```

## Key Files

| File | Purpose |
|------|---------|
| `adapters/src/proxy.rs` | `ProxyStreamFn`, `SseEventData`, request types, SSE parsing, tests |
| `adapters/src/classify.rs` | `HttpErrorKind`, `classify_http_status` (shared, from spec 011) |
| `adapters/src/lib.rs` | Crate root — re-exports `ProxyStreamFn` |

## SSE Protocol Reference

The proxy endpoint (`POST {base_url}/v1/stream`) returns an SSE stream. Each event's `data:` field is a JSON object with a `type` discriminator:

```
data: {"type":"start"}
data: {"type":"text_start","content_index":0}
data: {"type":"text_delta","content_index":0,"delta":"Hello"}
data: {"type":"text_delta","content_index":0,"delta":" world"}
data: {"type":"text_end","content_index":0}
data: {"type":"done","stop_reason":"stop","usage":{...},"cost":{...}}
```

Tool calls follow the same pattern with `tool_call_start`, `tool_call_delta`, `tool_call_end`.
