//! Tests for `error`.
#![cfg(test)]

use super::*;

#[test]
fn display_download_error() {
    let err = LocalModelError::download(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "repo not found",
    ));
    let msg = err.to_string();
    assert!(msg.contains("download failed"), "got: {msg}");
    assert!(msg.contains("repo not found"), "got: {msg}");
}

#[test]
fn display_loading_error() {
    let err = LocalModelError::loading(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "corrupt GGUF",
    ));
    let msg = err.to_string();
    assert!(msg.contains("loading failed"), "got: {msg}");
}

#[test]
fn display_inference_error() {
    let err = LocalModelError::inference("token limit exceeded");
    assert_eq!(err.to_string(), "inference error: token limit exceeded");
}

#[test]
fn display_embedding_error() {
    let err = LocalModelError::embedding("input exceeds max length");
    assert_eq!(err.to_string(), "embedding error: input exceeds max length");
}

#[test]
fn display_not_ready() {
    let err = LocalModelError::NotReady;
    assert!(err.to_string().contains("not ready"));
}

#[test]
fn source_chaining() {
    use std::error::Error as _;

    let inner = std::io::Error::other("inner");
    let err = LocalModelError::download(inner);
    assert!(err.source().is_some());

    let err = LocalModelError::NotReady;
    assert!(err.source().is_none());
}
