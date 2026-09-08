//! Tests for `loopback`.
#![cfg(test)]

use super::*;
use std::io::{Read as _, Write as _};

fn free_port() -> SocketAddr {
    // Bind then drop: the port is free again for the handler to claim.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn handler(addr: SocketAddr) -> LoopbackAuthorizationHandler {
    LoopbackAuthorizationHandler::new(Arc::new(|_| {}))
        .with_bind_addr(addr)
        .with_timeout(Duration::from_secs(5))
}

/// Send one GET and return the status line.
fn hit(addr: SocketAddr, target: &str) -> String {
    let mut attempts = 0;
    let mut stream = loop {
        match std::net::TcpStream::connect(addr) {
            Ok(s) => break s,
            Err(_) if attempts < 50 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("connect: {e}"),
        }
    };
    stream
        .write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
        .unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_returns_the_code_and_checks_state() {
    let addr = free_port();
    let h = handler(addr);
    let auth =
        tokio::spawn(async move { h.authorize("https://auth.example/authorize", "st-1").await });
    let status =
        tokio::task::spawn_blocking(move || hit(addr, "/auth/callback?code=abc123&state=st-1"))
            .await
            .unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(auth.await.unwrap().unwrap(), "abc123");
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_ignores_stray_requests_then_rejects_state_mismatch() {
    let addr = free_port();
    let h = handler(addr);
    let auth = tokio::spawn(async move { h.authorize("u", "expected").await });
    let stray = tokio::task::spawn_blocking(move || hit(addr, "/favicon.ico"))
        .await
        .unwrap();
    assert!(stray.starts_with("HTTP/1.1 404"), "{stray}");
    let bad = tokio::task::spawn_blocking(move || hit(addr, "/auth/callback?code=x&state=wrong"))
        .await
        .unwrap();
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    let err = auth.await.unwrap().unwrap_err();
    assert!(
        matches!(err, CredentialError::AuthorizationFailed { ref reason, .. } if reason.contains("state")),
        "{err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_error_is_surfaced_by_code_only() {
    let addr = free_port();
    let h = handler(addr);
    let auth = tokio::spawn(async move { h.authorize("u", "s").await });
    tokio::task::spawn_blocking(move || {
        hit(
            addr,
            "/cb?error=access_denied&error_description=SECRET_DETAIL&state=s",
        )
    })
    .await
    .unwrap();
    let err = auth.await.unwrap().unwrap_err();
    let rendered = format!("{err:?}");
    assert!(rendered.contains("access_denied"), "{rendered}");
    assert!(
        !rendered.contains("SECRET_DETAIL"),
        "error_description leaked: {rendered}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_fallback_used_when_port_is_busy() {
    // Keep the port occupied so the handler cannot bind.
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = occupied.local_addr().unwrap();
    let h = handler(addr).with_manual_code(Arc::new(|| {
        Some("http://localhost:1455/auth/callback?code=pasted&state=s".to_owned())
    }));
    assert_eq!(h.authorize("u", "s").await.unwrap(), "pasted");

    let bare = handler(addr).with_manual_code(Arc::new(|| Some("  bare-code \n".to_owned())));
    assert_eq!(bare.authorize("u", "s").await.unwrap(), "bare-code");
}

#[tokio::test]
async fn busy_port_without_fallback_fails_fast() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let err = handler(occupied.local_addr().unwrap())
        .authorize("u", "s")
        .await
        .unwrap_err();
    assert!(
        matches!(err, CredentialError::AuthorizationFailed { .. }),
        "{err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn times_out_when_nothing_arrives() {
    let h = handler(free_port()).with_timeout(Duration::from_millis(50));
    let err = h.authorize("u", "s").await.unwrap_err();
    assert!(matches!(err, CredentialError::Timeout { .. }), "{err:?}");
}

#[test]
fn manual_input_shapes() {
    assert_eq!(parse_manual_input("code=x&state=s", "s").unwrap(), "x");
    assert_eq!(
        parse_manual_input("http://l/cb?code=y&state=s", "s").unwrap(),
        "y"
    );
    assert!(parse_manual_input("http://l/cb?code=y&state=other", "s").is_err());
    assert_eq!(parse_manual_input("plain", "s").unwrap(), "plain");
    assert!(parse_manual_input("   ", "s").is_err());
}
