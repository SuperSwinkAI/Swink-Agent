//! Tests for `bash`.
#![cfg(test)]

use tokio::io::AsyncWriteExt;

use super::*;
use crate::types::ContentBlock;

fn result_text(result: &AgentToolResult) -> &str {
    match result.content.first() {
        Some(ContentBlock::Text { text }) => text.as_str(),
        _ => panic!("expected text content"),
    }
}

#[test]
fn format_output_truncates_multibyte_stdout_on_char_boundary() {
    let stdout = "€".repeat((MAX_OUTPUT_BYTES / "€".len()) + 1);

    let result = format_output(Some(0), stdout.as_bytes(), &[]);
    let text = result_text(&result);

    assert!(text.contains("[truncated]"), "expected marker in: {text}");
    assert!(text.is_char_boundary(text.len()));
}

#[tokio::test]
async fn read_stream_stops_after_output_budget_sentinel() {
    let (reader, mut writer) = tokio::io::duplex(1024);
    let writer_task = tokio::spawn(async move {
        let bytes = vec![b'a'; MAX_OUTPUT_BYTES + 2];
        let _ = writer.write_all(&bytes).await;
    });

    let output = read_stream(Some(reader)).await;
    let _ = writer_task.await;

    assert_eq!(output.len(), MAX_OUTPUT_BYTES + 1);
}
