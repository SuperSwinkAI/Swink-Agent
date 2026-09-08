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
#[path = "audit_logger_tests.rs"]
mod tests;
