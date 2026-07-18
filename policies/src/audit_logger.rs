//! Audit logger policy — records every turn to a pluggable sink.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use unicode_truncate::UnicodeTruncateStr;

use swink_agent::{ContentBlock, PolicyContext, PolicyVerdict, PostTurnPolicy, TurnPolicyContext};

// ─── Types ──────────────────────────────────────────────────────────────────

/// Summary of a single turn, suitable for serialization to an audit log.
#[non_exhaustive]
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

impl AuditRecord {
    /// Create a record from its parts, covering every field.
    ///
    /// [`AuditLogger`] builds records itself; this constructor exists so
    /// custom [`AuditSink`] implementations can be unit-tested with
    /// hand-built records.
    #[must_use]
    pub fn new(
        timestamp: impl Into<String>,
        turn_index: usize,
        content_summary: impl Into<String>,
        tool_calls: Vec<String>,
        usage: AuditUsage,
        cost: AuditCost,
    ) -> Self {
        Self {
            timestamp: timestamp.into(),
            turn_index,
            content_summary: content_summary.into(),
            tool_calls,
            usage,
            cost,
        }
    }
}

/// Subset of token usage relevant for audit records.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
pub struct AuditUsage {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

impl AuditUsage {
    /// Create a usage summary from input, output, and total token counts.
    #[must_use]
    pub const fn new(input: u64, output: u64, total: u64) -> Self {
        Self {
            input,
            output,
            total,
        }
    }
}

/// Subset of cost relevant for audit records.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
pub struct AuditCost {
    pub total: f64,
}

impl AuditCost {
    /// Create a cost summary from a total cost in USD.
    #[must_use]
    pub const fn new(total: f64) -> Self {
        Self { total }
    }
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

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
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
            let json = serde_json::to_string(record).map_err(std::io::Error::other)?;
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
        AssistantMessage::new(content, "", "")
            .with_usage(
                Usage::default()
                    .with_input(100)
                    .with_output(50)
                    .with_total(150),
            )
            .with_cost(Cost::default().with_total(0.005))
            .with_timestamp(0)
    }

    fn make_ctx<'a>(
        usage: &'a Usage,
        cost: &'a Cost,
        state: &'a swink_agent::SessionState,
    ) -> PolicyContext<'a> {
        PolicyContext::new(3, usage, cost, 10, false, &[], state)
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
        static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
            std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
        let turn = TurnPolicyContext::new(&msg, &[], StopReason::Stop, "", &MODEL, &[]);

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
        static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
            std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
        let turn = TurnPolicyContext::new(&msg, &[], StopReason::Stop, "", &MODEL, &[]);

        logger.evaluate(&ctx, &turn);

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].content_summary, "hello world");
        drop(records);
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
        static MODEL2: std::sync::LazyLock<swink_agent::ModelSpec> =
            std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
        let turn = TurnPolicyContext::new(&msg, &[], StopReason::ToolUse, "", &MODEL2, &[]);

        logger.evaluate(&ctx, &turn);

        let records = records_handle.lock().unwrap();
        assert_eq!(records.len(), 1);

        let timestamp = records[0].timestamp.clone();
        let turn_index = records[0].turn_index;
        let content_summary = records[0].content_summary.clone();
        let tool_calls = records[0].tool_calls.clone();
        let usage = records[0].usage.clone();
        let cost = records[0].cost.clone();
        drop(records);

        assert!(!timestamp.is_empty());
        assert_eq!(turn_index, 3);
        assert_eq!(content_summary, "I will run a tool");
        assert_eq!(tool_calls, vec!["bash", "read_file"]);
        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 50);
        assert_eq!(usage.total, 150);
        assert!((cost.total - 0.005).abs() < f64::EPSILON);
    }

    #[test]
    fn jsonl_sink_writes_valid_json() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let sink = JsonlAuditSink::new(&path);
        let record = AuditRecord::new(
            "2026-03-24T00:00:00+00:00",
            0,
            "test",
            vec!["bash".into()],
            AuditUsage::new(10, 5, 15),
            AuditCost::new(0.001),
        );

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
        let record = AuditRecord::new(
            "2026-03-24T00:00:00+00:00",
            0,
            String::new(),
            vec![],
            AuditUsage::new(0, 0, 0),
            AuditCost::new(0.0),
        );

        // Should not panic.
        sink.write(&record);
    }

    #[test]
    fn hand_built_record_exercises_custom_sink() {
        // The constructors let a custom AuditSink impl be unit-tested with a
        // hand-built record, without running the policy at all.
        let sink = MockSink::new();
        let records = sink.shared_records();

        let record = AuditRecord::new(
            "2026-07-16T00:00:00+00:00",
            42,
            "summary text",
            vec!["bash".into(), "read_file".into()],
            AuditUsage::new(100, 50, 150),
            AuditCost::new(0.005),
        );
        sink.write(&record);

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp, "2026-07-16T00:00:00+00:00");
        assert_eq!(records[0].turn_index, 42);
        assert_eq!(records[0].content_summary, "summary text");
        assert_eq!(records[0].tool_calls, vec!["bash", "read_file"]);
        assert_eq!(records[0].usage.input, 100);
        assert_eq!(records[0].usage.output, 50);
        assert_eq!(records[0].usage.total, 150);
        assert!((records[0].cost.total - 0.005).abs() < f64::EPSILON);
    }
}
