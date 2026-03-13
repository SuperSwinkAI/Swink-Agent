//! Shared SSE (Server-Sent Events) stream parser.
//!
//! Provides a reusable byte-buffer parser that both Anthropic and `OpenAI`
//! adapters can use instead of duplicating SSE line parsing logic.

/// Parsed SSE line.
#[derive(Debug, PartialEq, Eq)]
pub enum SseLine {
    /// An event type label (e.g., `event: message_start`).
    Event(String),
    /// A data payload.
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
pub struct SseStreamParser {
    buffer: String,
}

impl SseStreamParser {
    /// Create a new empty parser.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
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
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return vec![];
        }
        // Process any remaining content as a final line
        let remaining = std::mem::take(&mut self.buffer);
        let mut lines = vec![];
        for line in remaining.lines() {
            if let Some(parsed) = Self::parse_line(line) {
                lines.push(parsed);
            }
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

            if let Some(parsed) = Self::parse_line(&line) {
                lines.push(parsed);
            }
        }
        lines
    }

    fn parse_line(line: &str) -> Option<SseLine> {
        if line.is_empty() {
            return Some(SseLine::Empty);
        }
        if line.starts_with(':') {
            // SSE comment, skip
            return None;
        }
        if let Some(event_type) = line.strip_prefix("event: ") {
            return Some(SseLine::Event(event_type.trim().to_string()));
        }
        if let Some(data) = line.strip_prefix("data: ") {
            let data = data.trim();
            if data == "[DONE]" {
                return Some(SseLine::Done);
            }
            if !data.is_empty() {
                return Some(SseLine::Data(data.to_string()));
            }
            return None;
        }
        // Unknown line type, skip
        None
    }
}

impl Default for SseStreamParser {
    fn default() -> Self {
        Self::new()
    }
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
    fn sse_parser_multiline_data() {
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
}
