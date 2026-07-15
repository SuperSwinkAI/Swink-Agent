//! TLS smoke test for the ring-backed rustls provider (#1110).
//!
//! The wiremock suites exercise the HTTP stack over plaintext localhost, so
//! nothing else in CI proves a real TLS handshake still works after the
//! switch from aws-lc-rs to ring (workspace reqwest is `rustls-no-provider`
//! plus a direct `rustls/ring` dependency in every reqwest consumer). This
//! test makes one HTTPS request to a real endpoint with no credentials:
//! getting any HTTP status back (a 401-class rejection is expected) proves
//! the handshake negotiated end-to-end via ring.
//!
//! Ignored by default — needs network.
//! Run with: `cargo test -p swink-agent-adapters --test tls_smoke_live -- --ignored`

use std::time::Duration;

#[tokio::test]
#[ignore = "needs network: performs a real TLS handshake against api.anthropic.com"]
async fn ring_provider_negotiates_tls() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client builds: exactly one rustls provider (ring) is enabled in the graph");

    let response = client
        .get("https://api.anthropic.com/v1/models")
        .send()
        .await
        .expect("TLS handshake + HTTP exchange succeeds via the ring provider");

    // No API key was sent, so the application layer must reject us — the
    // signal here is that rejection happened over a successfully negotiated
    // TLS session (transport-layer failure would have errored in `send`).
    assert!(
        response.status().is_client_error(),
        "expected a 4xx (unauthenticated) response, got {}",
        response.status()
    );
}
