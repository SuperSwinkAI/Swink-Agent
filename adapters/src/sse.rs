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

use swink_agent::{AssistantMessageEvent, StopReason};

use crate::finalize::StreamFinalize;

/// Parsed SSE line.
#[non_exhaustive]
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
    /// A transport-level error emitted when the underlying `reqwest`
    /// byte stream yields an `Err`. Surfaces network failures so adapters
    /// can classify them as retryable instead of misreporting them as a
    /// clean EOF.
    TransportError(String),
    /// A protocol-level error produced by malformed SSE bytes. Adapters
    /// should surface this as non-retryable stream corruption.
    ProtocolError(String),
}

/// Synthetic event type that [`sse_paired_events`] emits when the upstream
/// byte stream reports a transport error. Adapters consuming `SseEvent`
/// should treat this as a terminal network error.
pub const SSE_TRANSPORT_ERROR_EVENT: &str = "__swink_transport_error__";

/// Synthetic event type that [`sse_paired_events`] emits when the shared
/// parser detects malformed SSE bytes.
pub const SSE_PROTOCOL_ERROR_EVENT: &str = "__swink_protocol_error__";

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
    /// Set after a terminal protocol error so later feed/flush calls cannot
    /// emit data from a desynchronized parser.
    poisoned: bool,
}

impl SseStreamParser {
    /// Create a new empty parser.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
            byte_carry: Vec::new(),
            pending_data: None,
            poisoned: false,
        }
    }

    /// Feed bytes into the parser, yielding complete SSE lines.
    ///
    /// Multi-byte UTF-8 sequences split across chunk boundaries are held
    /// in an internal carry buffer until the continuation bytes arrive,
    /// so split characters are decoded losslessly rather than replaced.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseLine> {
        if self.poisoned {
            return Vec::new();
        }

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
                        let s = std::str::from_utf8(&bytes[cursor..cursor + valid])
                            .expect("valid utf-8 prefix");
                        self.buffer.push_str(s);
                    }
                    cursor += valid;
                    if e.error_len().is_none() {
                        // Trailing bytes are an *incomplete* UTF-8
                        // sequence — carry them to the next feed.
                        self.byte_carry.extend_from_slice(&bytes[cursor..]);
                        cursor = bytes.len();
                    } else {
                        // Emit complete lines already decoded from this
                        // chunk before the corrupt byte (e.g. a final
                        // message_stop or usage delta), flushing pending
                        // data as at EOF, then terminate with the
                        // non-retryable protocol error.
                        let mut lines = self.drain_lines();
                        if let Some(data) = self.pending_data.take() {
                            lines.push(SseLine::Data(data));
                        }
                        lines.extend(
                            self.protocol_error("SSE stream contained invalid UTF-8 bytes"),
                        );
                        return lines;
                    }
                }
            }
        }

        self.drain_lines()
    }

    /// Flush remaining buffer at stream end.
    pub fn flush(&mut self) -> Vec<SseLine> {
        if self.poisoned {
            return Vec::new();
        }

        let mut lines = vec![];

        if !self.byte_carry.is_empty() {
            // The buffer holds no complete lines here (feed() drains them),
            // but pending_data may hold the payload of a complete data line
            // from an earlier feed. Emit it — as a clean-EOF flush would —
            // before terminating with the non-retryable protocol error.
            if let Some(data) = self.pending_data.take() {
                lines.push(SseLine::Data(data));
            }
            lines.extend(self.protocol_error("SSE stream ended with an incomplete UTF-8 sequence"));
            return lines;
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

    fn protocol_error(&mut self, message: &'static str) -> Vec<SseLine> {
        self.buffer.clear();
        self.byte_carry.clear();
        self.pending_data = None;
        self.poisoned = true;
        vec![SseLine::ProtocolError(message.to_string())]
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
        if let Some(event_type) = parse_sse_field_value(line, "event:") {
            // Flush any pending data before yielding the event
            if let Some(data) = self.pending_data.take() {
                output.push(SseLine::Data(data));
            }
            output.push(SseLine::Event(event_type.to_string()));
            return;
        }
        if let Some(data) = parse_sse_field_value(line, "data:") {
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

fn parse_sse_field_value<'a>(line: &'a str, field_prefix: &str) -> Option<&'a str> {
    let value = line.strip_prefix(field_prefix)?;
    Some(value.strip_prefix(' ').unwrap_or(value))
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
                    match result {
                        Ok(bytes) => {
                            pending.extend(parser.feed(&bytes));
                        }
                        Err(err) => {
                            // Transport failure — flush any buffered content,
                            // then append a terminal TransportError so downstream
                            // adapters classify this as a network error instead
                            // of a clean EOF.
                            pending.extend(parser.flush());
                            pending.push_back(SseLine::TransportError(format!(
                                "SSE transport error: {err}"
                            )));
                        }
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
                    if !matches!(
                        line,
                        SseLine::Data(_)
                            | SseLine::Done
                            | SseLine::TransportError(_)
                            | SseLine::ProtocolError(_)
                    ) {
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
#[non_exhaustive]
#[derive(Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The event type (e.g., `message_start`, `content_block_delta`).
    pub event_type: String,
    /// The JSON data payload associated with this event.
    pub data: String,
}

impl SseEvent {
    /// Create a new paired SSE event from an event type label and its data
    /// payload.
    #[must_use]
    pub fn new(event_type: String, data: String) -> Self {
        Self { event_type, data }
    }
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
    sse_paired_events_with_callback(byte_stream, None)
}

/// Like [`sse_paired_events`] but with an optional raw-payload callback.
pub fn sse_paired_events_with_callback(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> Pin<Box<dyn Stream<Item = SseEvent> + Send + 'static>> {
    Box::pin(stream::unfold(
        (
            Box::pin(sse_lines(byte_stream)),
            Option::<String>::None,
            on_raw_payload,
        ),
        |(mut stream, mut current_event, callback)| async move {
            loop {
                match stream.next().await {
                    Some(SseLine::Empty | SseLine::Done) => {
                        current_event = None;
                    }
                    Some(SseLine::TransportError(message)) => {
                        return Some((
                            SseEvent::new(SSE_TRANSPORT_ERROR_EVENT.to_string(), message),
                            (stream, current_event, callback),
                        ));
                    }
                    Some(SseLine::ProtocolError(message)) => {
                        return Some((
                            SseEvent::new(SSE_PROTOCOL_ERROR_EVENT.to_string(), message),
                            (stream, current_event, callback),
                        ));
                    }
                    Some(SseLine::Event(event_type)) => {
                        current_event = Some(event_type);
                    }
                    Some(SseLine::Data(data)) => {
                        if !data.is_empty() {
                            if let Some(cb) = &callback {
                                let cb = AssertUnwindSafe(cb);
                                let data = AssertUnwindSafe(&data);
                                let _ = catch_unwind(|| (cb)(&data));
                            }
                            let event_type = current_event
                                .take()
                                .unwrap_or_else(|| "unknown".to_string());
                            return Some((
                                SseEvent::new(event_type, data),
                                (stream, current_event, callback),
                            ));
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
#[non_exhaustive]
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
    sse_adapter_stream_with_cancel_finalizer(
        line_stream,
        cancellation_token,
        state,
        on_item,
        move |state| {
            let mut events = crate::finalize::finalize_blocks(state);
            events.push(AssistantMessageEvent::Error {
                stop_reason: StopReason::Aborted,
                error_message: cancel_message.to_string(),
                usage: None,
                error_kind: None,
                retry_after: None,
            });
            events
        },
    )
}

/// Like [`sse_adapter_stream`] with adapter-specific cancellation finalization.
///
/// Use this when an adapter must flush provider-specific buffered state before
/// the shared block finalizer drains open blocks.
pub fn sse_adapter_stream_with_cancel_finalizer<S, L, F, C>(
    line_stream: Pin<Box<dyn Stream<Item = L> + Send>>,
    cancellation_token: CancellationToken,
    state: S,
    on_item: F,
    on_cancel: C,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>
where
    S: StreamFinalize + Send + 'static,
    L: Send + 'static,
    F: FnMut(Option<L>, &mut S) -> SseAction + Send + 'static,
    C: FnMut(&mut S) -> Vec<AssistantMessageEvent> + Send + 'static,
{
    Box::pin(
        stream::unfold(
            (
                line_stream,
                cancellation_token,
                state,
                false,
                true,
                on_item,
                on_cancel,
            ),
            move |(
                mut lines,
                token,
                mut state,
                mut done,
                first,
                mut on_item,
                mut on_cancel,
            )| async move {
                if done {
                    return None;
                }

                if first {
                    return Some((
                        vec![AssistantMessageEvent::Start],
                        (lines, token, state, done, false, on_item, on_cancel),
                    ));
                }

                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        let events = on_cancel(&mut state);
                        done = true;
                        Some((events, (lines, token, state, done, false, on_item, on_cancel)))
                    }
                    item = lines.next() => {
                        let action = on_item(item, &mut state);
                        match action {
                            SseAction::Continue(events) => {
                                Some((events, (lines, token, state, done, false, on_item, on_cancel)))
                            }
                            SseAction::Done(events) => {
                                done = true;
                                Some((events, (lines, token, state, done, false, on_item, on_cancel)))
                            }
                            SseAction::Skip => {
                                Some((vec![], (lines, token, state, done, false, on_item, on_cancel)))
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
#[path = "sse_tests.rs"]
mod tests;
