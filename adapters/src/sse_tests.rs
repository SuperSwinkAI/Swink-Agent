//! Tests for `sse`.
#![cfg(test)]

use futures::StreamExt as _;

use super::*;

#[test]
fn sse_parser_basic_event_data() {
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"event: message_start\ndata: {}\n\n");
    assert_eq!(
        lines,
        vec![
            SseLine::Event("message_start".to_string()),
            SseLine::Data("{}".to_string()),
            SseLine::Empty,
        ]
    );
}

#[test]
fn sse_parser_accepts_optional_space_after_colon() {
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"event:message_start\ndata:{\"ok\":true}\n\n");
    assert_eq!(
        lines,
        vec![
            SseLine::Event("message_start".to_string()),
            SseLine::Data("{\"ok\":true}".to_string()),
            SseLine::Empty,
        ]
    );
}

#[test]
fn sse_parser_accepts_mixed_spaced_and_unspaced_fields() {
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"event: message_start\ndata:{\"one\":1}\ndata: {\"two\":2}\n\n");
    assert_eq!(
        lines,
        vec![
            SseLine::Event("message_start".to_string()),
            SseLine::Data("{\"one\":1}\n{\"two\":2}".to_string()),
            SseLine::Empty,
        ]
    );
}

#[test]
fn sse_parser_partial_chunk_buffering() {
    let mut parser = SseStreamParser::new();
    // First feed — partial, no newline yet at end
    let lines1 = parser.feed(b"event: content");
    assert!(lines1.is_empty(), "no newline yet, nothing to yield");

    // Second feed completes the first line and provides data
    let lines2 = parser.feed(b"_block_delta\ndata: {\"text\":\"hello\"}\n\n");
    assert_eq!(
        lines2,
        vec![
            SseLine::Event("content_block_delta".to_string()),
            SseLine::Data("{\"text\":\"hello\"}".to_string()),
            SseLine::Empty,
        ]
    );
}

#[test]
fn sse_parser_done_sentinel() {
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: [DONE]\n");
    assert_eq!(lines, vec![SseLine::Done]);
}

#[test]
fn sse_parser_empty_line() {
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"\n");
    assert_eq!(lines, vec![SseLine::Empty]);
}

#[test]
fn sse_parser_comment_skipped() {
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b": this is a comment\n");
    assert!(lines.is_empty());
}

#[test]
fn sse_parser_flush_remaining() {
    let mut parser = SseStreamParser::new();
    // Feed partial data without trailing newline
    let lines = parser.feed(b"data: {\"final\":true}");
    assert!(lines.is_empty(), "no newline, nothing drained yet");

    // Flush should yield the remaining buffered line
    let flushed = parser.flush();
    assert_eq!(flushed, vec![SseLine::Data("{\"final\":true}".to_string())]);
}

#[test]
fn sse_parser_multiline_data_concatenation() {
    // Per SSE spec, successive `data:` lines are joined with `\n`
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: line1\ndata: line2\ndata: line3\n\n");
    assert_eq!(
        lines,
        vec![
            SseLine::Data("line1\nline2\nline3".to_string()),
            SseLine::Empty,
        ]
    );
}

#[test]
fn sse_parser_multiline_data_flushed_on_event() {
    // Data lines should be flushed when a non-data line arrives
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: part1\ndata: part2\nevent: next\n");
    assert_eq!(
        lines,
        vec![
            SseLine::Data("part1\npart2".to_string()),
            SseLine::Event("next".to_string()),
        ]
    );
}

#[test]
fn sse_parser_multiline_data_across_feeds() {
    // Multi-line data split across feed() calls
    let mut parser = SseStreamParser::new();
    let lines1 = parser.feed(b"data: first\n");
    assert!(
        lines1.is_empty(),
        "pending data not emitted without separator"
    );

    let lines2 = parser.feed(b"data: second\n\n");
    assert_eq!(
        lines2,
        vec![SseLine::Data("first\nsecond".to_string()), SseLine::Empty,]
    );
}

#[test]
fn sse_parser_single_data_emitted_on_empty_line() {
    // Single data line followed by empty line
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: single\n\n");
    assert_eq!(
        lines,
        vec![SseLine::Data("single".to_string()), SseLine::Empty,]
    );
}

#[test]
fn sse_parser_pending_data_flushed_at_end() {
    // Data without a trailing empty line should be flushed
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: orphan\n");
    assert!(lines.is_empty());

    let flushed = parser.flush();
    assert_eq!(flushed, vec![SseLine::Data("orphan".to_string())]);
}

#[test]
fn sse_parser_split_utf8_across_chunks_is_lossless() {
    // Regression for #207: a multi-byte UTF-8 sequence split across two
    // feed() calls must decode losslessly, not produce replacement chars.
    // "héllo" — 'é' is 0xC3 0xA9; we split the chunk between those bytes.
    let payload = "data: héllo\n\n".as_bytes();
    let split_at = payload
        .windows(2)
        .position(|w| w == [0xC3, 0xA9])
        .expect("payload contains é")
        + 1;
    let (first, second) = payload.split_at(split_at);

    let mut parser = SseStreamParser::new();
    let lines1 = parser.feed(first);
    // The split byte should not have produced any line yet (no newline)
    // and must NOT contain a replacement character.
    for line in &lines1 {
        if let SseLine::Data(d) = line {
            assert!(!d.contains('\u{FFFD}'), "split byte produced U+FFFD: {d:?}");
        }
    }

    let lines2 = parser.feed(second);
    let combined: Vec<_> = lines1.into_iter().chain(lines2).collect();
    assert_eq!(
        combined,
        vec![SseLine::Data("héllo".to_string()), SseLine::Empty]
    );
}

#[test]
fn sse_parser_split_utf8_3byte_and_4byte() {
    // 3-byte char: '€' (0xE2 0x82 0xAC); 4-byte char: '🦀' (0xF0 0x9F 0xA6 0x80).
    // Feed each one byte at a time and confirm the parser reassembles
    // them without inserting replacement characters.
    let payload = "data: €🦀\n\n".as_bytes();
    let mut parser = SseStreamParser::new();
    let mut all = Vec::new();
    for b in payload {
        all.extend(parser.feed(&[*b]));
    }
    assert_eq!(all, vec![SseLine::Data("€🦀".to_string()), SseLine::Empty]);
}

#[test]
fn sse_parser_invalid_utf8_emits_protocol_error() {
    // A lone 0xFF is not the start of any valid UTF-8 sequence and is
    // not the continuation of a partial sequence. It should terminate the
    // parser instead of being lossy-decoded into provider-visible data.
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: a\xFFb\n\n");
    assert_eq!(
        lines,
        vec![SseLine::ProtocolError(
            "SSE stream contained invalid UTF-8 bytes".to_string()
        )]
    );
    assert!(parser.feed(b"data: later\n\n").is_empty());
}

#[test]
fn sse_parser_invalid_utf8_preserves_prior_complete_lines() {
    // Complete, newline-terminated lines decoded from the same chunk
    // before the corrupt byte (e.g. a message_stop or final usage delta)
    // must be emitted ahead of the terminal ProtocolError instead of
    // being discarded with the poisoned buffer.
    let mut parser = SseStreamParser::new();
    let lines =
        parser.feed(b"event: message_delta\ndata: {\"usage\":1}\n\ndata: stop\n\xFFgarbage");
    assert_eq!(
        lines,
        vec![
            SseLine::Event("message_delta".to_string()),
            SseLine::Data("{\"usage\":1}".to_string()),
            SseLine::Empty,
            SseLine::Data("stop".to_string()),
            SseLine::ProtocolError("SSE stream contained invalid UTF-8 bytes".to_string()),
        ]
    );
    // The parser stays poisoned: later feeds and flushes emit nothing.
    assert!(parser.feed(b"data: later\n\n").is_empty());
    assert!(parser.flush().is_empty());
}

#[test]
fn sse_parser_incomplete_utf8_at_eof_emits_protocol_error() {
    let mut parser = SseStreamParser::new();
    assert!(parser.feed(b"data: \xE2").is_empty());

    let lines = parser.flush();
    assert_eq!(
        lines,
        vec![SseLine::ProtocolError(
            "SSE stream ended with an incomplete UTF-8 sequence".to_string()
        )]
    );
}

#[test]
fn sse_parser_incomplete_utf8_at_eof_preserves_pending_data() {
    // A complete data line from an earlier feed must not be discarded
    // when the stream later ends mid-UTF-8-sequence; it is emitted just
    // as a clean-EOF flush would emit it, ahead of the ProtocolError.
    let mut parser = SseStreamParser::new();
    assert!(parser.feed(b"data: stop\n").is_empty());
    assert!(parser.feed(b"\xE2").is_empty());

    let lines = parser.flush();
    assert_eq!(
        lines,
        vec![
            SseLine::Data("stop".to_string()),
            SseLine::ProtocolError(
                "SSE stream ended with an incomplete UTF-8 sequence".to_string()
            ),
        ]
    );
    assert!(parser.flush().is_empty());
}

#[test]
fn sse_parser_done_flushes_pending_data() {
    // data: [DONE] should flush any pending data first
    let mut parser = SseStreamParser::new();
    let lines = parser.feed(b"data: last\ndata: [DONE]\n");
    assert_eq!(
        lines,
        vec![SseLine::Data("last".to_string()), SseLine::Done,]
    );
}

// ─── OnRawPayload tests ────────────────────────────────────────────────

#[tokio::test]
async fn on_raw_payload_fires_for_each_line() {
    use std::sync::{Arc, Mutex};

    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_clone = Arc::clone(&captured);
    let callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |data: &str| {
        captured_clone.lock().unwrap().push(data.to_owned());
    });

    let chunks = vec![Ok(bytes::Bytes::from("data: line1\n\ndata: line2\n\n"))];
    let byte_stream = futures::stream::iter(chunks);
    let mut data_stream = sse_data_lines_with_callback(byte_stream, Some(callback));

    let first = data_stream.next().await;
    assert_eq!(first, Some(SseLine::Data("line1".to_string())));
    let second = data_stream.next().await;
    assert_eq!(second, Some(SseLine::Data("line2".to_string())));

    let lines = {
        let guard = captured.lock().unwrap();
        guard.clone()
    };
    assert_eq!(lines, vec!["line1".to_string(), "line2".to_string()]);
}

#[tokio::test]
async fn on_raw_payload_none_no_overhead() {
    let chunks = vec![Ok(bytes::Bytes::from("data: hello\n\n"))];
    let byte_stream = futures::stream::iter(chunks);
    let mut data_stream = sse_data_lines_with_callback(byte_stream, None);

    let first = data_stream.next().await;
    assert_eq!(first, Some(SseLine::Data("hello".to_string())));
    let done = data_stream.next().await;
    assert!(done.is_none());
}

#[tokio::test]
async fn sse_data_lines_surfaces_invalid_utf8_as_protocol_error() {
    let chunks = vec![Ok(bytes::Bytes::from_static(b"data: \xFF\n\n"))];
    let byte_stream = futures::stream::iter(chunks);
    let mut data_stream = sse_data_lines(byte_stream);

    assert_eq!(
        data_stream.next().await,
        Some(SseLine::ProtocolError(
            "SSE stream contained invalid UTF-8 bytes".to_string()
        ))
    );
    assert_eq!(data_stream.next().await, None);
}

#[tokio::test]
async fn on_raw_payload_panic_caught() {
    use std::sync::Arc;
    let callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(|_data: &str| {
        panic!("callback panic!");
    });

    let chunks = vec![Ok(bytes::Bytes::from("data: safe\n\ndata: also_safe\n\n"))];
    let byte_stream = futures::stream::iter(chunks);
    let mut data_stream = sse_data_lines_with_callback(byte_stream, Some(callback));

    // Should not panic — the callback panic is caught
    let first = data_stream.next().await;
    assert_eq!(first, Some(SseLine::Data("safe".to_string())));
    let second = data_stream.next().await;
    assert_eq!(second, Some(SseLine::Data("also_safe".to_string())));
}

#[tokio::test]
async fn sse_lines_preserves_events_and_separators() {
    let chunks = vec![Ok(bytes::Bytes::from(
        "event: start\ndata: hello\n\ndata: [DONE]\n",
    ))];
    let byte_stream = futures::stream::iter(chunks);
    let lines: Vec<_> = sse_lines(byte_stream).collect().await;

    assert_eq!(
        lines,
        vec![
            SseLine::Event("start".to_string()),
            SseLine::Data("hello".to_string()),
            SseLine::Empty,
            SseLine::Done,
        ]
    );
}

// ─── sse_paired_events tests ──────────────────────────────────────────

#[tokio::test]
async fn paired_events_pairs_event_with_data() {
    let chunks = vec![Ok(bytes::Bytes::from(
        "event: message_start\ndata: {\"type\":\"start\"}\n\nevent: content_block_delta\ndata: {\"text\":\"hi\"}\n\n",
    ))];
    let byte_stream = futures::stream::iter(chunks);
    let events: Vec<_> = super::sse_paired_events(byte_stream).collect().await;

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "message_start");
    assert_eq!(events[0].data, "{\"type\":\"start\"}");
    assert_eq!(events[1].event_type, "content_block_delta");
    assert_eq!(events[1].data, "{\"text\":\"hi\"}");
}

#[tokio::test]
async fn paired_events_data_without_event_uses_unknown() {
    let chunks = vec![Ok(bytes::Bytes::from("data: orphan\n\n"))];
    let byte_stream = futures::stream::iter(chunks);
    let events: Vec<_> = super::sse_paired_events(byte_stream).collect().await;

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "unknown");
    assert_eq!(events[0].data, "orphan");
}

#[tokio::test]
async fn paired_events_maps_invalid_utf8_to_protocol_error_event() {
    let chunks = vec![Ok(bytes::Bytes::from_static(b"event: one\ndata: \xFF\n\n"))];
    let byte_stream = futures::stream::iter(chunks);
    let events: Vec<_> = super::sse_paired_events(byte_stream).collect().await;

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, SSE_PROTOCOL_ERROR_EVENT);
    assert_eq!(events[0].data, "SSE stream contained invalid UTF-8 bytes");
}

#[tokio::test]
async fn paired_events_empty_line_resets_event() {
    // event: foo, then empty line (separator), then data — should use "unknown"
    let chunks = vec![Ok(bytes::Bytes::from(
        "event: foo\n\ndata: after_reset\n\n",
    ))];
    let byte_stream = futures::stream::iter(chunks);
    let events: Vec<_> = super::sse_paired_events(byte_stream).collect().await;

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "unknown");
}

#[tokio::test]
async fn paired_events_on_raw_payload_fires_for_each_line() {
    use std::sync::{Arc, Mutex};

    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_clone = Arc::clone(&captured);
    let callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |data: &str| {
        captured_clone.lock().unwrap().push(data.to_owned());
    });

    let chunks = vec![Ok(bytes::Bytes::from(
        "event: one\ndata: first\n\nevent: two\ndata: second\n\n",
    ))];
    let byte_stream = futures::stream::iter(chunks);
    let events: Vec<_> = super::sse_paired_events_with_callback(byte_stream, Some(callback))
        .collect()
        .await;

    assert_eq!(events.len(), 2);
    assert_eq!(
        captured.lock().unwrap().clone(),
        vec!["first".to_string(), "second".to_string()]
    );
}

#[tokio::test]
async fn paired_events_on_raw_payload_panic_caught() {
    use std::sync::Arc;

    let callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(|_data: &str| {
        panic!("callback panic!");
    });

    let chunks = vec![Ok(bytes::Bytes::from(
        "event: one\ndata: safe\n\nevent: two\ndata: still_safe\n\n",
    ))];
    let byte_stream = futures::stream::iter(chunks);
    let events: Vec<_> = super::sse_paired_events_with_callback(byte_stream, Some(callback))
        .collect()
        .await;

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data, "safe");
    assert_eq!(events[1].data, "still_safe");
}

// ─── sse_adapter_stream tests ─────────────────────────────────────────

#[tokio::test]
async fn adapter_stream_emits_start_first() {
    use crate::finalize::{OpenBlock, StreamFinalize};

    struct EmptyState;
    impl StreamFinalize for EmptyState {
        fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
            vec![]
        }
    }

    let line_stream: Pin<Box<dyn Stream<Item = SseLine> + Send>> =
        Box::pin(futures::stream::empty());
    let token = CancellationToken::new();

    let events: Vec<_> = super::sse_adapter_stream(
        line_stream,
        token,
        EmptyState,
        "cancelled",
        |item, _state| match item {
            None => super::SseAction::Done(vec![]),
            Some(_) => super::SseAction::Skip,
        },
    )
    .collect()
    .await;

    assert!(!events.is_empty());
    assert!(matches!(events[0], AssistantMessageEvent::Start));
}

#[tokio::test]
async fn adapter_stream_finalizes_on_cancel() {
    use crate::finalize::{OpenBlock, StreamFinalize};

    struct TextState;
    impl StreamFinalize for TextState {
        fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
            vec![OpenBlock::Text { content_index: 0 }]
        }
    }

    let token = CancellationToken::new();
    token.cancel(); // Cancel immediately

    // Use a stream that never yields so the cancel branch fires
    let line_stream: Pin<Box<dyn Stream<Item = SseLine> + Send>> =
        Box::pin(futures::stream::pending());

    let events: Vec<_> = super::sse_adapter_stream(
        line_stream,
        token,
        TextState,
        "test cancelled",
        |_item, _state| super::SseAction::Skip,
    )
    .collect()
    .await;

    // Start + TextEnd (from finalize) + Error(Aborted)
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert!(matches!(
        events[2],
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        }
    ));
}

/// Regression for issue #230 — transport errors from `reqwest::bytes_stream`
/// must surface as a terminal `SseLine::TransportError` rather than being
/// silently dropped as EOF. We spin up a TCP listener that writes a valid
/// HTTP/1.1 chunked-encoded response header + a partial chunk and then
/// closes the connection mid-body; `reqwest` surfaces that as an error on
/// its byte stream.
#[tokio::test]
async fn sse_lines_surfaces_transport_error() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            // Read + discard the request headers.
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            // Write a chunked response header and one partial chunk, then
            // abruptly close the connection so reqwest's body stream errors.
            let header = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Transfer-Encoding: chunked\r\n\r\n\
                    10\r\ndata: partial\n";
            let _ = sock.write_all(header.as_bytes()).await;
            // Drop the socket without finishing the chunk — reqwest will
            // surface a transport-level error on its next byte-stream poll.
            drop(sock);
        }
    });

    crate::base::ensure_default_crypto_provider();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect("connect");
    let lines: Vec<_> = sse_lines(resp.bytes_stream()).collect().await;

    assert!(
        lines
            .iter()
            .any(|l| matches!(l, SseLine::TransportError(_))),
        "expected TransportError line, got {lines:?}"
    );
}

#[tokio::test]
async fn adapter_stream_delegates_lines_to_callback() {
    use crate::finalize::{OpenBlock, StreamFinalize};

    struct EmptyState;
    impl StreamFinalize for EmptyState {
        fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
            vec![]
        }
    }

    let line_stream: Pin<Box<dyn Stream<Item = SseLine> + Send>> = Box::pin(futures::stream::iter(
        vec![SseLine::Data("hello".to_string())],
    ));
    let token = CancellationToken::new();

    let events: Vec<_> = super::sse_adapter_stream(
        line_stream,
        token,
        EmptyState,
        "cancelled",
        |item, _state| match item {
            Some(SseLine::Data(text)) => {
                super::SseAction::Continue(vec![AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: text,
                }])
            }
            None => super::SseAction::Done(vec![]),
            _ => super::SseAction::Skip,
        },
    )
    .collect()
    .await;

    // Start + TextDelta
    assert!(events.len() >= 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            ..
        }
    ));
}
