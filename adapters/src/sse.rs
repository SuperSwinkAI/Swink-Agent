//! Shared SSE (Server-Sent Events) stream parser and adapter helpers.
//!
//! Provides a reusable byte-buffer parser that Anthropic, `OpenAI`, Azure, and
//! Google adapters use instead of duplicating SSE line parsing logic.
//!
//! ## Helper hierarchy
//!
//! ```text
//! sse_lines()              ← raw SseLine stream (Event, Data, Done, Empty)
//!   ├── sse_data_lines()   ← filters to Data + Done only (Google, OpenAI, Proxy)
//!   └── sse_paired_events()← pairs event: + data: into SseEvent (Anthropic)
//!
//! sse_adapter_stream()     ← shared Start/cancel/finalize scaffolding
//!                            (Anthropic, Google, OpenAI-compat)
//! ```
//!
//! Adapters with simple or fundamentally different streaming models (Proxy,
//! Bedrock) use `sse_data_lines()` or raw byte streams directly.
//!
//! **Stability note:** This module is a shared implementation detail for
//! built-in adapters. External `StreamFn` implementors should depend only
//! on `swink_agent` (core) types. Breaking changes to this module's API
//! may occur without a major version bump.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;

use swink_agent::stream::AssistantMessageEvent;
use swink_agent::types::StopReason;

use crate::finalize::StreamFinalize;

/// Parsed SSE line.
#[derive(Debug, PartialEq, Eq)]
pub enum SseLine {
    /// An event type label (e.g., `event: message_start`).
    Event(String),
    /// A data payload (successive `data:` lines are concatenated with `\n`
    /// per the SSE specification).
    Data(String),
    /// End-of-stream signal (`data: [DONE]`).
    Done,
    /// Empty line (event separator).
    Empty,
}

/// Streaming SSE parser that buffers bytes and yields parsed lines.
///
/// Handles partial UTF-8 chunks and splits on newline boundaries,
/// producing [`SseLine`] values as complete lines become available.
/// Successive `data:` fields are concatenated with `\n` per the SSE
/// specification (FR-006).
pub struct SseStreamParser {
    buffer: String,
    /// Bytes carried across `feed()` calls when a chunk ends mid-UTF-8
    /// sequence. Up to 3 bytes may be held until the continuation arrives.
    byte_carry: Vec<u8>,
    /// Accumulates successive `data:` lines for multi-line concatenation.
    pending_data: Option<String>,
}

impl SseStreamParser {
    /// Create a new empty parser.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
            byte_carry: Vec::new(),
            pending_data: None,
        }
    }

    /// Feed bytes into the parser, yielding complete SSE lines.
    ///
    /// Multi-byte UTF-8 sequences split across chunk boundaries are held
    /// in an internal carry buffer until the continuation bytes arrive,
    /// so split characters are decoded losslessly rather than replaced.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseLine> {
        // Combine any carried trailing bytes from the previous feed with
        // the new chunk before attempting UTF-8 decoding.
        let combined: Vec<u8> = if self.byte_carry.is_empty() {
            chunk.to_vec()
        } else {
            let mut v = std::mem::take(&mut self.byte_carry);
            v.extend_from_slice(chunk);
            v
        };

        let bytes = combined.as_slice();
        let mut cursor = 0;
        while cursor < bytes.len() {
            match std::str::from_utf8(&bytes[cursor..]) {
                Ok(s) => {
                    self.buffer.push_str(s);
                    cursor = bytes.len();
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        // SAFETY: `valid_up_to()` is guaranteed to point at
                        // the end of a valid UTF-8 prefix.
                        let s =
                            std::str::from_utf8(&bytes[cursor..cursor + valid]).expect("valid utf-8 prefix");
                        self.buffer.push_str(s);
                    }
                    cursor += valid;
                    match e.error_len() {
                        None => {
                            // Trailing bytes are an *incomplete* UTF-8
                            // sequence — carry them to the next feed.
                            self.byte_carry.extend_from_slice(&bytes[cursor..]);
                            cursor = bytes.len();
                        }
                        Some(n) => {
                            // Genuinely invalid byte(s) — substitute the
                            // Unicode replacement char and skip past them.
                            self.buffer.push('\u{FFFD}');
                            cursor += n;
                        }
                    }
                }
            }
        }

        self.drain_lines()
    }

    /// Flush remaining buffer at stream end.
    pub fn flush(&mut self) -> Vec<SseLine> {
        let mut lines = vec![];

        // Any leftover incomplete UTF-8 bytes at end-of-stream are no
        // longer recoverable — emit them lossily so callers still see them.
        if !self.byte_carry.is_empty() {
            let carry = std::mem::take(&mut self.byte_carry);
            self.buffer.push_str(&String::from_utf8_lossy(&carry));
        }

        if !self.buffer.trim().is_empty() {
            let remaining = std::mem::take(&mut self.buffer);
            for line in remaining.lines() {
                self.process_raw_line(line, &mut lines);
            }
        }
        self.buffer.clear();

        // Emit any remaining pending data
        if let Some(data) = self.pending_data.take() {
            lines.push(SseLine::Data(data));
        }

        lines
    }

    fn drain_lines(&mut self) -> Vec<SseLine> {
        let mut lines = vec![];
        while let Some(pos) = self.buffer.find('\n') {
            let line_end = if pos > 0 && self.buffer.as_bytes().get(pos - 1) == Some(&b'\r') {
                pos - 1
            } else {
                pos
            };
            let line = self.buffer[..line_end].to_string();
            self.buffer.drain(..=pos);
            self.process_raw_line(&line, &mut lines);
        }
        lines
    }

    /// Process a single raw line, accumulating successive `data:` fields
    /// and flushing pending data when a non-data line is encountered.
    fn process_raw_line(&mut self, line: &str, output: &mut Vec<SseLine>) {
        if line.is_empty() {
            // Empty line = event separator. Flush pending data first.
            if let Some(data) = self.pending_data.take() {
                output.push(SseLine::Data(data));
            }
            output.push(SseLine::Empty);
            return;
        }
        if line.starts_with(':') {
            // SSE comment — skip, but don't flush pending data
            return;
        }
        if let Some(event_type) = line.strip_prefix("event: ") {
            // Flush any pending data before yielding the event
            if let Some(data) = self.pending_data.take() {
                output.push(SseLine::Data(data));
            }
            output.push(SseLine::Event(event_type.trim().to_string()));
            return;
        }
        if let Some(data) = line.strip_prefix("data: ") {
            let data = data.trim();
            if data == "[DONE]" {
                // Flush pending data, then yield Done
                if let Some(pending) = self.pending_data.take() {
                    output.push(SseLine::Data(pending));
                }
                output.push(SseLine::Done);
                return;
            }
            if !data.is_empty() {
                // Accumulate into pending_data for multi-line concatenation
                if let Some(ref mut pending) = self.pending_data {
                    pending.push('\n');
                    pending.push_str(data);
                } else {
                    self.pending_data = Some(data.to_string());
                }
            }
            return;
        }
        // Unknown line type — flush pending data, skip the line
        if let Some(data) = self.pending_data.take() {
            output.push(SseLine::Data(data));
        }
    }
}

impl Default for SseStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a byte stream into a stream of parsed SSE data lines.
///
/// Buffers incoming bytes through [`SseStreamParser`], filters to only
/// [`SseLine::Data`] and [`SseLine::Done`] variants (skipping events,
/// comments, and empty lines), and flushes any remaining buffer when
/// the byte stream ends.
///
/// If `on_raw_payload` is provided, it is called with each raw data line
/// string before the line is yielded. Panics in the callback are caught
/// and the stream continues uninterrupted.
pub fn sse_data_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>> {
    sse_data_lines_with_callback(byte_stream, None)
}

/// Convert a byte stream into a stream of parsed SSE lines.
///
/// Unlike [`sse_data_lines`], this preserves `event:` labels and empty-line
/// separators so callers with provider-specific pairing logic can reuse the
/// shared parser instead of maintaining their own byte-buffer state machine.
pub fn sse_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>> {
    Box::pin(stream::unfold(
        (
            Box::pin(byte_stream),
            SseStreamParser::new(),
            VecDeque::<SseLine>::new(),
        ),
        |(mut stream, mut parser, mut pending)| async move {
            loop {
                if let Some(line) = pending.pop_front() {
                    return Some((line, (stream, parser, pending)));
                }

                if let Some(result) = stream.next().await {
                    if let Ok(bytes) = result {
                        pending.extend(parser.feed(&bytes));
                    }
                    continue;
                }

                pending.extend(parser.flush());
                if pending.is_empty() {
                    return None;
                }
            }
        },
    ))
}

/// Like [`sse_data_lines`] but with an optional raw-payload callback.
pub fn sse_data_lines_with_callback(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(sse_lines(byte_stream)), on_raw_payload),
        |(mut stream, callback)| async move {
            loop {
                if let Some(line) = stream.next().await {
                    if !matches!(line, SseLine::Data(_) | SseLine::Done) {
                        continue;
                    }
                    if let (SseLine::Data(data), Some(cb)) = (&line, &callback) {
                        let cb = AssertUnwindSafe(cb);
                        let data = AssertUnwindSafe(data);
                        let _ = catch_unwind(|| (cb)(&data));
                    }
                    return Some((line, (stream, callback)));
                }
                return None;
            }
        },
    ))
}

// ─── Event/data pairing ────────────────────────────────────────────────────

/// A paired SSE event: an `event:` type label matched with its `data:` payload.
///
/// Providers like Anthropic emit `event: <type>\ndata: <json>\n\n` sequences.
/// This type captures the pairing so adapters receive structured events instead
/// of interleaved raw lines.
#[derive(Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The event type (e.g., `message_start`, `content_block_delta`).
    pub event_type: String,
    /// The JSON data payload associated with this event.
    pub data: String,
}

/// Pair `event:` and `data:` lines from an SSE byte stream.
///
/// Uses [`sse_lines`] internally to parse bytes, then applies a state machine
/// that tracks the most recent `event:` label and yields an [`SseEvent`] when
/// a `data:` line follows. Empty lines and `Done` sentinels reset the pairing
/// state. Data lines without a preceding event are attributed to `"unknown"`.
///
/// This is the appropriate entry point for providers that use both `event:` and
/// `data:` headers (e.g., Anthropic). Providers that emit only `data:` lines
/// (e.g., `OpenAI`, Google) should use [`sse_data_lines`] instead.
pub fn sse_paired_events(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = SseEvent> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(sse_lines(byte_stream)), Option::<String>::None),
        |(mut stream, mut current_event)| async move {
            loop {
                match stream.next().await {
                    Some(SseLine::Empty | SseLine::Done) => {
                        current_event = None;
                    }
                    Some(SseLine::Event(event_type)) => {
                        current_event = Some(event_type);
                    }
                    Some(SseLine::Data(data)) => {
                        if !data.is_empty() {
                            let event_type = current_event
                                .take()
                                .unwrap_or_else(|| "unknown".to_string());
                            return Some((SseEvent { event_type, data }, (stream, current_event)));
                        }
                    }
                    None => return None,
                }
            }
        },
    ))
}

// ─── Shared stream scaffolding ─────────────────────────────────────────────

/// Outcome of processing one SSE line (or stream end) in an adapter.
pub enum SseAction {
    /// Emit events and continue streaming.
    Continue(Vec<AssistantMessageEvent>),
    /// Emit events and terminate the stream.
    Done(Vec<AssistantMessageEvent>),
    /// No events to emit, continue streaming.
    Skip,
}

/// Build an event stream with shared Start-emission, cancellation, and
/// finalization scaffolding.
///
/// The common adapter streaming pattern is:
/// 1. Emit `Start` on the first iteration.
/// 2. On cancellation, finalize open blocks and emit an `Aborted` error.
/// 3. Delegate per-line processing to the adapter via `on_item`.
///
/// `on_item` receives `None` when the underlying line stream ends (allowing
/// adapter-specific cleanup, e.g., emitting `Done` vs an unexpected-end error)
/// and `Some(line)` for each SSE line.
///
/// Adapters with no block-tracking state (e.g., Proxy) should use
/// `sse_data_lines` directly rather than this helper, since they have
/// nothing to finalize on cancellation.
pub fn sse_adapter_stream<S, L, F>(
    line_stream: Pin<Box<dyn Stream<Item = L> + Send>>,
    cancellation_token: CancellationToken,
    state: S,
    cancel_message: &'static str,
    on_item: F,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>
where
    S: StreamFinalize + Send + 'static,
    L: Send + 'static,
    F: FnMut(Option<L>, &mut S) -> SseAction + Send + 'static,
{
    Box::pin(
        stream::unfold(
            (line_stream, cancellation_token, state, false, true, on_item),
            move |(mut lines, token, mut state, mut done, first, mut on_item)| async move {
                if done {
                    return None;
                }

                if first {
                    return Some((
                        vec![AssistantMessageEvent::Start],
                        (lines, token, state, done, false, on_item),
                    ));
                }

                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        let mut events = crate::finalize::finalize_blocks(&mut state);
                        events.push(AssistantMessageEvent::Error {
                            stop_reason: StopReason::Aborted,
                            error_message: cancel_message.to_string(),
                            usage: None,
                            error_kind: None,
                        });
                        done = true;
                        Some((events, (lines, token, state, done, false, on_item)))
                    }
                    item = lines.next() => {
                        let action = on_item(item, &mut state);
                        match action {
                            SseAction::Continue(events) => {
                                Some((events, (lines, token, state, done, false, on_item)))
                            }
                            SseAction::Done(events) => {
                                done = true;
                                Some((events, (lines, token, state, done, false, on_item)))
                            }
                            SseAction::Skip => {
                                Some((vec![], (lines, token, state, done, false, on_item)))
                            }
                        }
                    }
                }
            },
        )
        .flat_map(stream::iter),
    )
}

#[cfg(test)]
mod tests {
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
        assert_eq!(
            all,
            vec![SseLine::Data("€🦀".to_string()), SseLine::Empty]
        );
    }

    #[test]
    fn sse_parser_truly_invalid_byte_uses_replacement_char() {
        // A lone 0xFF is not the start of any valid UTF-8 sequence and is
        // not the continuation of a partial sequence — it should be replaced
        // with U+FFFD, not held in the carry buffer forever.
        let mut parser = SseStreamParser::new();
        let lines = parser.feed(b"data: a\xFFb\n\n");
        assert_eq!(
            lines,
            vec![
                SseLine::Data("a\u{FFFD}b".to_string()),
                SseLine::Empty,
            ]
        );
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

    #[tokio::test]
    async fn adapter_stream_delegates_lines_to_callback() {
        use crate::finalize::{OpenBlock, StreamFinalize};

        struct EmptyState;
        impl StreamFinalize for EmptyState {
            fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
                vec![]
            }
        }

        let line_stream: Pin<Box<dyn Stream<Item = SseLine> + Send>> = Box::pin(
            futures::stream::iter(vec![SseLine::Data("hello".to_string())]),
        );
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
}
