//! Audit logger policy — records every turn to a pluggable sink.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use unicode_truncate::UnicodeTruncateStr;

use swink_agent::{
    ContentBlock, PolicyContext, PolicyVerdict, PostTurnPolicy, TurnPolicyContext,
};

// ─── Types ──────────────────────────────────────────────────────────────────

/// Summary of a single turn, suitable for serialization to an audit log.
#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    /// ISO 8601 timestamp of when the record was created.
    pub timestamp: String,
    /// Zero-based turn index.
    pub turn_index: usize,
    /// First 200 characters of the assistant's text output.
    pub content_summary: String,
    /// Names of tools invoked in this turn.
    pub tool_calls: Vec<String>,
    /// Token usage for this turn.
    pub usage: AuditUsage,
    /// Cost for this turn.
    pub cost: AuditCost,
}

/// Subset of token usage relevant for audit records.
#[derive(Debug, Clone, Serialize)]
pub struct AuditUsage {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

/// Subset of cost relevant for audit records.
#[derive(Debug, Clone, Serialize)]
pub struct AuditCost {
    pub total: f64,
}

impl From<&swink_agent::Usage> for AuditUsage {
    fn from(u: &swink_agent::Usage) -> Self {
        Self {
            input: u.input,
            output: u.output,
            total: u.total,
        }
    }
}

impl From<&swink_agent::Cost> for AuditCost {
    fn from(c: &swink_agent::Cost) -> Self {
        Self { total: c.total }
    }
}

// ─── Sink Trait ─────────────────────────────────────────────────────────────

/// Pluggable destination for audit records.
pub trait AuditSink: Send + Sync {
    /// Write a single audit record. Implementations should not panic.
    fn write(&self, record: &AuditRecord);
}

// ─── AuditLogger ────────────────────────────────────────────────────────────

/// `PostTurnPolicy` that builds an [`AuditRecord`] for every turn and writes
/// it to the configured [`AuditSink`].
#[derive(Clone)]
pub struct AuditLogger {
    sink: Arc<dyn AuditSink>,
}

impl std::fmt::Debug for AuditLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLogger").finish_non_exhaustive()
    }
}

impl AuditLogger {
    /// Create a new `AuditLogger` wrapping the given sink.
    pub fn new(sink: impl AuditSink + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
        }
    }
}

impl PostTurnPolicy for AuditLogger {
    fn name(&self) -> &'static str {
        "audit-logger"
    }

    fn evaluate(
        &self,
        ctx: &PolicyContext<'_>,
        turn: &TurnPolicyContext<'_>,
    ) -> PolicyVerdict {
        let full_text = ContentBlock::extract_text(&turn.assistant_message.content);
        let content_summary = truncate_to_chars(&full_text, 200);

        let tool_calls: Vec<String> = turn
            .assistant_message
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolCall { name, .. } = block {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        let record = AuditRecord {
            timestamp: Utc::now().to_rfc3339(),
            turn_index: ctx.turn_index,
            content_summary,
            tool_calls,
            usage: AuditUsage::from(&turn.assistant_message.usage),
            cost: AuditCost::from(&turn.assistant_message.cost),
        };

        self.sink.write(&record);

        PolicyVerdict::Continue
    }
}

/// Truncate a string to at most `max` display-width columns, respecting
/// grapheme cluster boundaries (e.g. emoji with zero-width joiners).
fn truncate_to_chars(s: &str, max: usize) -> String {
    let (truncated, _width) = s.unicode_truncate(max);
    truncated.to_string()
}

// ─── JSONL Sink ─────────────────────────────────────────────────────────────

/// Appends one JSON line per audit record to a file on disk.
#[derive(Debug, Clone)]
pub struct JsonlAuditSink {
    path: PathBuf,
}

impl JsonlAuditSink {
    /// Create a new `JsonlAuditSink` that writes to the given file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl AuditSink for JsonlAuditSink {
    fn write(&self, record: &AuditRecord) {
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            let json = serde_json::to_string(record)
                .map_err(std::io::Error::other)?;
            writeln!(file, "{json}")?;
            Ok(())
        })();

        if let Err(err) = result {
            tracing::warn!("audit write failed: {err}");
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use swink_agent::{
        AssistantMessage, ContentBlock, Cost, PolicyContext, PolicyVerdict, PostTurnPolicy,
        StopReason, TurnPolicyContext, Usage,
    };

    use super::*;

    // ── Mock Sink ───────────────────────────────────────────────────────

    struct MockSink {
        records: Arc<Mutex<Vec<AuditRecord>>>,
    }

    impl MockSink {
        fn new() -> Self {
            Self {
                records: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn shared_records(&self) -> Arc<Mutex<Vec<AuditRecord>>> {
            Arc::clone(&self.records)
        }
    }

    impl AuditSink for MockSink {
        fn write(&self, record: &AuditRecord) {
            self.records.lock().unwrap().push(record.clone());
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    fn make_assistant_message(content: Vec<ContentBlock>) -> AssistantMessage {
        AssistantMessage {
            content,
            provider: String::new(),
            model_id: String::new(),
            usage: Usage {
                input: 100,
                output: 50,
                total: 150,
                ..Default::default()
            },
            cost: Cost {
                total: 0.005,
                ..Default::default()
            },
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            cache_hint: None,
        }
    }

    fn make_ctx<'a>(usage: &'a Usage, cost: &'a Cost, state: &'a swink_agent::SessionState) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 3,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 10,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    // ── Tests ───────────────────────────────────────────────────────────

    #[test]
    fn always_returns_continue() {
        let sink = MockSink::new();
        let logger = AuditLogger::new(sink);

        let msg = make_assistant_message(vec![ContentBlock::Text {
            text: "hello".into(),
        }]);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
        };

        let verdict = logger.evaluate(&ctx, &turn);
        assert!(matches!(verdict, PolicyVerdict::Continue));
    }

    #[test]
    fn sink_receives_record() {
        let sink = MockSink::new();
        let records = sink.shared_records();
        let logger = AuditLogger::new(sink);

        let msg = make_assistant_message(vec![ContentBlock::Text {
            text: "hello world".into(),
        }]);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
        };

        logger.evaluate(&ctx, &turn);

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].content_summary, "hello world");
    }

    #[test]
    fn record_has_all_fields() {
        let sink = MockSink::new();
        let records_handle = sink.shared_records();
        let logger = AuditLogger::new(sink);

        let msg = make_assistant_message(vec![
            ContentBlock::Text {
                text: "I will run a tool".into(),
            },
            ContentBlock::ToolCall {
                id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "ls"}),
                partial_json: None,
            },
            ContentBlock::ToolCall {
                id: "tc_2".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "/tmp/f"}),
                partial_json: None,
            },
        ]);

        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::ToolUse,
        };

        logger.evaluate(&ctx, &turn);

        let records = records_handle.lock().unwrap();
        assert_eq!(records.len(), 1);

        let r = &records[0];
        assert!(!r.timestamp.is_empty());
        assert_eq!(r.turn_index, 3);
        assert_eq!(r.content_summary, "I will run a tool");
        assert_eq!(r.tool_calls, vec!["bash", "read_file"]);
        assert_eq!(r.usage.input, 100);
        assert_eq!(r.usage.output, 50);
        assert_eq!(r.usage.total, 150);
        assert!((r.cost.total - 0.005).abs() < f64::EPSILON);
    }

    #[test]
    fn jsonl_sink_writes_valid_json() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let sink = JsonlAuditSink::new(&path);
        let record = AuditRecord {
            timestamp: "2026-03-24T00:00:00+00:00".into(),
            turn_index: 0,
            content_summary: "test".into(),
            tool_calls: vec!["bash".into()],
            usage: AuditUsage {
                input: 10,
                output: 5,
                total: 15,
            },
            cost: AuditCost { total: 0.001 },
        };

        sink.write(&record);

        let contents = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(parsed["turn_index"], 0);
        assert_eq!(parsed["content_summary"], "test");
        assert_eq!(parsed["tool_calls"][0], "bash");
    }

    #[test]
    fn jsonl_sink_handles_write_error_gracefully() {
        // Writing to an impossible path should not panic — error is logged.
        let sink = JsonlAuditSink::new("/dev/null/impossible");
        let record = AuditRecord {
            timestamp: "2026-03-24T00:00:00+00:00".into(),
            turn_index: 0,
            content_summary: String::new(),
            tool_calls: vec![],
            usage: AuditUsage {
                input: 0,
                output: 0,
                total: 0,
            },
            cost: AuditCost { total: 0.0 },
        };

        // Should not panic.
        sink.write(&record);
    }
}
