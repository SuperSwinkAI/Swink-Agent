# Quickstart: Adapter Shared Infrastructure

**Feature**: 011-adapter-shared-infra | **Date**: 2026-03-20

## Prerequisites

- Rust 1.88+ (edition 2024)
- `swink-agent` core crate available as a path dependency

## Build & Test

```bash
# Build the adapters crate
cargo build -p swink-agent-adapters

# Run all unit tests for shared infrastructure
cargo test -p swink-agent-adapters

# Run only classify tests
cargo test -p swink-agent-adapters classify

# Run only SSE parser tests
cargo test -p swink-agent-adapters sse

# Run only remote preset tests
cargo test -p swink-agent-adapters remote_presets

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Message Conversion

```rust
use swink_agent_adapters::convert::{MessageConverter, convert_messages};

// Each adapter implements MessageConverter for its format.
// The generic convert_messages() drives the iteration.
let provider_messages = convert_messages(&my_converter, &agent_messages);
```

### HTTP Error Classification

```rust
use swink_agent_adapters::classify::{classify_http_status, HttpErrorKind};

match classify_http_status(status_code) {
    Some(HttpErrorKind::Throttled) => { /* back off and retry */ }
    Some(HttpErrorKind::Auth) => { /* terminal — bad credentials */ }
    Some(HttpErrorKind::Network) => { /* retry with jitter */ }
    None => { /* not an error status */ }
}
```

### SSE Stream Parsing

```rust
use swink_agent_adapters::sse::{SseStreamParser, SseLine, sse_data_lines};

// Low-level: feed bytes manually
let mut parser = SseStreamParser::new();
let lines = parser.feed(b"event: message_start\ndata: {}\n\n");

// High-level: stream combinator over a reqwest byte stream
let data_stream = sse_data_lines(response.bytes_stream());
// Yields only SseLine::Data and SseLine::Done
```

### Remote Preset Connections

```rust
use swink_agent_adapters::{build_remote_connection, remote_preset_keys};

// Build a connection from a compile-time preset key
let connection = build_remote_connection(
    remote_preset_keys::anthropic::SONNET_46,
)?;
// connection.model_spec() has the catalog model spec
// connection.stream_fn() is an Arc<dyn StreamFn>
```

## Key Files

| File | Purpose |
|------|---------|
| `adapters/src/convert.rs` | Re-exports `MessageConverter` trait from core |
| `adapters/src/classify.rs` | `HttpErrorKind` enum and classification functions |
| `adapters/src/sse.rs` | `SseStreamParser`, `SseLine`, `sse_data_lines` combinator |
| `adapters/src/remote_presets.rs` | `RemotePresetKey`, preset constants, `build_remote_connection` |
| `adapters/src/lib.rs` | Crate root with re-exports |
| `src/convert.rs` | Core `MessageConverter` trait definition |

### Configure Prompt Caching

```rust
use swink_agent::stream::{StreamOptions, CacheStrategy};

// Enable automatic caching (adapter determines optimal cache points)
let options = StreamOptions {
    cache_strategy: CacheStrategy::Auto,
    ..Default::default()
};

// Provider-specific: Anthropic cache_control blocks
let options = StreamOptions {
    cache_strategy: CacheStrategy::Anthropic,
    ..Default::default()
};

// Provider-specific: Google with TTL
use std::time::Duration;
let options = StreamOptions {
    cache_strategy: CacheStrategy::Google { ttl: Duration::from_secs(3600) },
    ..Default::default()
};
```

### Observe Raw Provider Payloads

```rust
use std::sync::Arc;
use swink_agent::stream::StreamOptions;

let options = StreamOptions {
    on_raw_payload: Some(Arc::new(|raw: &str| {
        eprintln!("[RAW] {raw}");
    })),
    ..Default::default()
};
// Each raw SSE data line is printed before event parsing
```

### Proxy Raw SSE Bytes

```rust
use swink_agent_adapters::ProxyStreamFn;

let proxy = ProxyStreamFn::new(base_url, api_key, "anthropic");
let byte_stream = proxy.stream_raw(model, messages, options).await;
// byte_stream yields raw Bytes — consumer parses events
```
