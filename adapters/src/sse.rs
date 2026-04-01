//! Shared SSE (Server-Sent Events) stream parser.
//!
//! Provides a reusable byte-buffer parser that Anthropic, `OpenAI`, Azure, and
//! Google adapters use instead of duplicating SSE line parsing logic.
//!
//! **Stability note:** This module is a shared implementation detail for
//! built-in adapters. External `StreamFn` implementors should depend only
//! on `swink_agent` (core) types. Breaking changes to this module's API
//! may occur without a major version bump.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};

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
    /// Accumulates successive `data:` lines for multi-line concatenation.
    pending_data: Option<String>,
}

impl SseStreamParser {
    /// Create a new empty parser.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
            pending_data: None,
        }
    }

    /// Feed bytes into the parser, yielding complete SSE lines.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseLine> {
        match std::str::from_utf8(chunk) {
            Ok(s) => self.buffer.push_str(s),
            Err(_) => self.buffer.push_str(&String::from_utf8_lossy(chunk)),
        }
        self.drain_lines()
    }

    /// Flush remaining buffer at stream end.
    pub fn flush(&mut self) -> Vec<SseLine> {
        let mut lines = vec![];

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

/// Like [`sse_data_lines`] but with an optional raw-payload callback.
pub fn sse_data_lines_with_callback(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>> {
    Box::pin(stream::unfold(
        (
            Box::pin(byte_stream),
            SseStreamParser::new(),
            Vec::<SseLine>::new(),
            on_raw_payload,
        ),
        |(mut stream, mut parser, mut pending, callback)| async move {
            loop {
                // Drain any pending parsed lines, yielding only Data/Done.
                while let Some(line) = pending.first() {
                    if matches!(line, SseLine::Data(_) | SseLine::Done) {
                        let line = pending.remove(0);
                        // Fire callback for Data lines before yielding
                        if let (SseLine::Data(data), Some(cb)) = (&line, &callback) {
                            let cb = AssertUnwindSafe(cb);
                            let data = AssertUnwindSafe(data);
                            let _ = catch_unwind(|| (cb)(&data));
                        }
                        return Some((line, (stream, parser, pending, callback)));
                    }
                    pending.remove(0);
                }

                // Pull more bytes from the underlying stream.
                if let Some(result) = stream.next().await {
                    if let Ok(bytes) = result {
                        pending.extend(parser.feed(&bytes));
                    }
                    // On Err, skip the chunk and try the next one.
                    continue;
                }

                // Stream ended — flush remaining buffer.
                pending.extend(parser.flush());
                if pending.is_empty() {
                    return None;
                }
                // Loop back to drain the flushed lines.
            }
        },
    ))
}

#[cfg(test)]
mod tests {
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

        let chunks = vec![
            Ok(bytes::Bytes::from("data: line1\n\ndata: line2\n\n")),
        ];
        let byte_stream = futures::stream::iter(chunks);
        let mut data_stream = sse_data_lines_with_callback(byte_stream, Some(callback));

        use futures::StreamExt as _;
        let first = data_stream.next().await;
        assert_eq!(first, Some(SseLine::Data("line1".to_string())));
        let second = data_stream.next().await;
        assert_eq!(second, Some(SseLine::Data("line2".to_string())));

        let lines = captured.lock().unwrap();
        assert_eq!(*lines, vec!["line1".to_string(), "line2".to_string()]);
    }

    #[tokio::test]
    async fn on_raw_payload_none_no_overhead() {
        let chunks = vec![
            Ok(bytes::Bytes::from("data: hello\n\n")),
        ];
        let byte_stream = futures::stream::iter(chunks);
        let mut data_stream = sse_data_lines_with_callback(byte_stream, None);

        use futures::StreamExt as _;
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

        let chunks = vec![
            Ok(bytes::Bytes::from("data: safe\n\ndata: also_safe\n\n")),
        ];
        let byte_stream = futures::stream::iter(chunks);
        let mut data_stream = sse_data_lines_with_callback(byte_stream, Some(callback));

        use futures::StreamExt as _;
        // Should not panic — the callback panic is caught
        let first = data_stream.next().await;
        assert_eq!(first, Some(SseLine::Data("safe".to_string())));
        let second = data_stream.next().await;
        assert_eq!(second, Some(SseLine::Data("also_safe".to_string())));
    }
}
