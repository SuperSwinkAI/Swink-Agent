//! RL-compatible training-format trace export (feature: `training-export`).
//!
//! Exports [`Invocation`] traces collected during eval runs into formats
//! compatible with LLM fine-tuning pipelines: ChatML/SFT, DPO pairs, and
//! ShareGPT.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use swink_agent_eval::training::{
//!     ChatMlExporter, ExportOptions, ScoredTrace, TrainingExporter, TrainingFormat,
//! };
//!
//! let exporter = ChatMlExporter;
//! let opts = ExportOptions::default();
//! let bytes = exporter.export(&traces, &opts)?;
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::EvalCaseResult;
use crate::report::{Reporter, ReporterError, ReporterOutput};
use crate::types::Invocation;

// ─── Error ──────────────────────────────────────────────────────────────────

/// Errors produced by training-format exporters.
#[derive(Debug, Error)]
pub enum ExportError {
    /// JSON serialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The requested format is not yet fully implemented (stubs).
    #[error("format not fully implemented: {0:?}")]
    NotImplemented(TrainingFormat),
    /// No traces survived the quality threshold filter.
    #[error("no traces passed the quality threshold ({threshold})")]
    NothingToExport { threshold: f32 },
}

// ─── Format Enum ────────────────────────────────────────────────────────────

/// Supported training-data output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TrainingFormat {
    /// Conversation-style JSONL with system/user/assistant turns and tool calls.
    ChatMlSft,
    /// Chosen/rejected pairs from high-score vs low-score traces on the same case.
    DpoPairs,
    /// Community ShareGPT conversation format.
    ShareGpt,
}

// ─── Options ────────────────────────────────────────────────────────────────

/// Options controlling how traces are exported.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Target output format.
    pub format: TrainingFormat,
    /// Minimum score `[0.0, 1.0]` a trace must have to be included.
    /// Traces with `score < quality_threshold` are filtered out.
    pub quality_threshold: f32,
    /// When `true`, per-record metadata (model, temperature proxy, eval case
    /// ID, timestamp) is included in the exported records.
    pub include_metadata: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: TrainingFormat::ChatMlSft,
            quality_threshold: 0.0,
            include_metadata: true,
        }
    }
}

impl ExportOptions {
    /// Create options targeting ChatML/SFT format with a quality gate.
    #[must_use]
    pub fn chatml_sft(quality_threshold: f32) -> Self {
        Self {
            format: TrainingFormat::ChatMlSft,
            quality_threshold,
            include_metadata: true,
        }
    }

    /// Create options targeting DPO pairs with a quality gate.
    #[must_use]
    pub fn dpo_pairs(quality_threshold: f32) -> Self {
        Self {
            format: TrainingFormat::DpoPairs,
            quality_threshold,
            include_metadata: true,
        }
    }

    /// Create options targeting ShareGPT format.
    #[must_use]
    pub fn sharegpt() -> Self {
        Self {
            format: TrainingFormat::ShareGpt,
            quality_threshold: 0.0,
            include_metadata: true,
        }
    }
}

// ─── ScoredTrace ────────────────────────────────────────────────────────────

/// An [`Invocation`] paired with a quality score and the originating case.
///
/// Built from [`EvalCaseResult`] values collected during a run.
#[derive(Debug, Clone)]
pub struct ScoredTrace {
    /// The captured execution trace.
    pub invocation: Invocation,
    /// Aggregate quality score for this trace, typically the mean of all
    /// evaluator scores, in `[0.0, 1.0]`.
    pub score: f64,
    /// Identifier of the eval case this trace was produced by.
    pub case_id: String,
}

impl ScoredTrace {
    /// Construct a `ScoredTrace` from an [`EvalCaseResult`].
    ///
    /// `score` is the mean of all metric scores (0.0 when there are none).
    #[must_use]
    pub fn from_case_result(result: &EvalCaseResult) -> Self {
        let score = if result.metric_results.is_empty() {
            0.0
        } else {
            let sum: f64 = result.metric_results.iter().map(|m| m.score.value).sum();
            #[allow(clippy::cast_precision_loss)]
            let mean = sum / result.metric_results.len() as f64;
            mean
        };
        Self {
            invocation: result.invocation.clone(),
            score,
            case_id: result.case_id.clone(),
        }
    }
}

// ─── Trait ──────────────────────────────────────────────────────────────────

/// Converts a slice of scored traces into a training-data byte payload.
///
/// Implementations are stateless; all configuration is passed via
/// [`ExportOptions`].
pub trait TrainingExporter: Send + Sync {
    /// Export `traces` according to `opts`.
    ///
    /// Returns a `Vec<u8>` whose encoding depends on the implementation
    /// (typically UTF-8 JSONL). Returns [`ExportError::NothingToExport`] when
    /// every trace is below `opts.quality_threshold`.
    fn export(&self, traces: &[ScoredTrace], opts: &ExportOptions) -> Result<Vec<u8>, ExportError>;
}

// ─── ChatML/SFT ─────────────────────────────────────────────────────────────

/// Conversation-style JSONL exporter.
///
/// Each qualifying trace produces one JSON object per line.  The schema
/// follows OpenAI's ChatML convention used by many fine-tuning platforms:
///
/// ```json
/// {"messages": [
///   {"role": "system",    "content": "You are a helpful agent."},
///   {"role": "user",      "content": "What is 2+2?"},
///   {"role": "assistant", "content": "4", "tool_calls": [...]}
/// ], "metadata": {...}}
/// ```
///
/// Tool calls on an assistant turn are serialised as an array of
/// `{id, type, function: {name, arguments}}` objects, matching the OpenAI
/// tool-call schema so downstream fine-tuning pipelines can parse them
/// without additional transformation.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChatMlExporter;

impl TrainingExporter for ChatMlExporter {
    fn export(&self, traces: &[ScoredTrace], opts: &ExportOptions) -> Result<Vec<u8>, ExportError> {
        let threshold = f64::from(opts.quality_threshold);
        let qualified: Vec<&ScoredTrace> = traces.iter().filter(|t| t.score >= threshold).collect();

        if qualified.is_empty() {
            return Err(ExportError::NothingToExport {
                threshold: opts.quality_threshold,
            });
        }

        let mut out = Vec::new();
        for trace in qualified {
            let record = build_chatml_record(trace, opts);
            serde_json::to_writer(&mut out, &record)?;
            out.push(b'\n');
        }
        Ok(out)
    }
}

// ─── ChatML helpers ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatMlRecord<'a> {
    messages: Vec<ChatMlMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<ChatMlMetadata<'a>>,
}

#[derive(Serialize)]
struct ChatMlMessage {
    role: &'static str,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatMlToolCall>>,
}

#[derive(Serialize)]
struct ChatMlToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: &'static str,
    function: ChatMlFunction,
}

#[derive(Serialize)]
struct ChatMlFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct ChatMlMetadata<'a> {
    case_id: &'a str,
    score: f64,
    model_id: String,
    turns: usize,
}

fn build_chatml_record<'a>(trace: &'a ScoredTrace, opts: &ExportOptions) -> ChatMlRecord<'a> {
    let inv = &trace.invocation;
    let mut messages: Vec<ChatMlMessage> = Vec::new();

    // System message — derive from the first user message context if there is
    // relevant text, otherwise use an empty string.  The system prompt is not
    // stored on `Invocation` directly; we emit a placeholder so downstream
    // pipelines always have a system slot to fill from their own case data.
    messages.push(ChatMlMessage {
        role: "system",
        content: String::new(),
        tool_calls: None,
    });

    for turn in &inv.turns {
        // User turn: synthesise from tool results of the *previous* turn or
        // from the first turn where we have no tool results to carry.
        // For turn 0 the user message is implicit (not stored in Invocation).
        // We add a user placeholder only for turn 0.
        if turn.turn_index == 0 {
            messages.push(ChatMlMessage {
                role: "user",
                content: String::new(), // prompt not stored in Invocation
                tool_calls: None,
            });
        }

        // Assistant message
        let content = extract_assistant_text(&turn.assistant_message);
        let tool_calls: Vec<ChatMlToolCall> = turn
            .tool_calls
            .iter()
            .map(|tc| ChatMlToolCall {
                id: tc.id.clone(),
                call_type: "function",
                function: ChatMlFunction {
                    name: tc.name.clone(),
                    arguments: tc.arguments.to_string(),
                },
            })
            .collect();

        messages.push(ChatMlMessage {
            role: "assistant",
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        });
    }

    // Final response appended as a last assistant message if not already
    // captured via the last turn's assistant message.
    if let Some(response) = &inv.final_response {
        let needs_patch = messages
            .last()
            .is_some_and(|last| last.role == "assistant" && last.content.is_empty());
        let needs_append = messages.last().is_some_and(|last| last.role != "assistant");

        if needs_patch && !response.is_empty() {
            if let Some(last_mut) = messages.last_mut() {
                last_mut.content.clone_from(response);
            }
        } else if needs_append {
            messages.push(ChatMlMessage {
                role: "assistant",
                content: response.clone(),
                tool_calls: None,
            });
        }
    }

    let metadata = if opts.include_metadata {
        Some(ChatMlMetadata {
            case_id: &trace.case_id,
            score: trace.score,
            model_id: inv.model.model_id.clone(),
            turns: inv.turns.len(),
        })
    } else {
        None
    };

    ChatMlRecord { messages, metadata }
}

fn extract_assistant_text(msg: &swink_agent::AssistantMessage) -> String {
    use swink_agent::ContentBlock;
    msg.content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text { text } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

// ─── DPO Pairs ──────────────────────────────────────────────────────────────

/// Chosen/rejected pair exporter for DPO (Direct Preference Optimization).
///
/// Traces are grouped by `case_id`. Within each group, the highest-scoring
/// trace becomes the `chosen` side and the lowest-scoring trace becomes the
/// `rejected` side. Cases with fewer than two traces are skipped.
///
/// Output schema (one JSON object per line):
///
/// ```json
/// {"case_id": "...", "chosen": {...chatml record...}, "rejected": {...chatml record...}}
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct DpoExporter;

/// A single DPO pair (one JSONL record).
#[derive(Serialize)]
struct DpoPairRecord {
    case_id: String,
    chosen: serde_json::Value,
    rejected: serde_json::Value,
}

impl TrainingExporter for DpoExporter {
    fn export(&self, traces: &[ScoredTrace], opts: &ExportOptions) -> Result<Vec<u8>, ExportError> {
        let threshold = f64::from(opts.quality_threshold);
        let qualified: Vec<&ScoredTrace> = traces.iter().filter(|t| t.score >= threshold).collect();

        // Group by case_id
        let mut by_case: std::collections::HashMap<&str, Vec<&ScoredTrace>> =
            std::collections::HashMap::new();
        for trace in &qualified {
            by_case
                .entry(trace.case_id.as_str())
                .or_default()
                .push(trace);
        }

        let mut pairs: Vec<DpoPairRecord> = Vec::new();
        for (case_id, mut group) in by_case {
            if group.len() < 2 {
                continue;
            }
            // Sort by score descending
            group.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let chosen_trace = group[0];
            let rejected_trace = group[group.len() - 1];

            let chosen_record = build_chatml_record(chosen_trace, opts);
            let rejected_record = build_chatml_record(rejected_trace, opts);

            pairs.push(DpoPairRecord {
                case_id: case_id.to_string(),
                chosen: serde_json::to_value(chosen_record)?,
                rejected: serde_json::to_value(rejected_record)?,
            });
        }

        if pairs.is_empty() {
            return Err(ExportError::NothingToExport {
                threshold: opts.quality_threshold,
            });
        }

        let mut out = Vec::new();
        for pair in &pairs {
            serde_json::to_writer(&mut out, pair)?;
            out.push(b'\n');
        }
        Ok(out)
    }
}

// ─── ShareGPT ───────────────────────────────────────────────────────────────

/// Community ShareGPT conversation format exporter.
///
/// Output schema (one JSON object per line):
///
/// ```json
/// {"conversations": [
///   {"from": "system", "value": "..."},
///   {"from": "human",  "value": "..."},
///   {"from": "gpt",    "value": "..."}
/// ]}
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct ShareGptExporter;

#[derive(Serialize)]
struct ShareGptRecord {
    conversations: Vec<ShareGptTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ShareGptTurn {
    from: &'static str,
    value: String,
}

impl TrainingExporter for ShareGptExporter {
    fn export(&self, traces: &[ScoredTrace], opts: &ExportOptions) -> Result<Vec<u8>, ExportError> {
        let threshold = f64::from(opts.quality_threshold);
        let qualified: Vec<&ScoredTrace> = traces.iter().filter(|t| t.score >= threshold).collect();

        if qualified.is_empty() {
            return Err(ExportError::NothingToExport {
                threshold: opts.quality_threshold,
            });
        }

        let mut out = Vec::new();
        for trace in qualified {
            let record = build_sharegpt_record(trace, opts);
            serde_json::to_writer(&mut out, &record)?;
            out.push(b'\n');
        }
        Ok(out)
    }
}

fn build_sharegpt_record(trace: &ScoredTrace, opts: &ExportOptions) -> ShareGptRecord {
    let inv = &trace.invocation;
    let mut conversations: Vec<ShareGptTurn> = Vec::new();

    // System placeholder
    conversations.push(ShareGptTurn {
        from: "system",
        value: String::new(),
    });

    for turn in &inv.turns {
        if turn.turn_index == 0 {
            conversations.push(ShareGptTurn {
                from: "human",
                value: String::new(), // user prompt not stored in Invocation
            });
        }
        let content = extract_assistant_text(&turn.assistant_message);
        conversations.push(ShareGptTurn {
            from: "gpt",
            value: content,
        });
    }

    // Final response patch — same logic as ChatML.
    if let Some(response) = &inv.final_response {
        let needs_patch = conversations
            .last()
            .is_some_and(|last| last.from == "gpt" && last.value.is_empty());
        let needs_append = conversations.last().is_some_and(|last| last.from != "gpt");

        if needs_patch && !response.is_empty() {
            if let Some(last_mut) = conversations.last_mut() {
                last_mut.value.clone_from(response);
            }
        } else if needs_append {
            conversations.push(ShareGptTurn {
                from: "gpt",
                value: response.clone(),
            });
        }
    }

    let metadata = if opts.include_metadata {
        Some(serde_json::json!({
            "case_id": trace.case_id,
            "score": trace.score,
        }))
    } else {
        None
    };

    ShareGptRecord {
        conversations,
        metadata,
    }
}

// ─── Dispatch helper ─────────────────────────────────────────────────────────

/// Dispatch export to the appropriate exporter based on `opts.format`.
pub fn export_traces(traces: &[ScoredTrace], opts: &ExportOptions) -> Result<Vec<u8>, ExportError> {
    match opts.format {
        TrainingFormat::ChatMlSft => ChatMlExporter.export(traces, opts),
        TrainingFormat::DpoPairs => DpoExporter.export(traces, opts),
        TrainingFormat::ShareGpt => ShareGptExporter.export(traces, opts),
    }
}

// ─── TrainingReporter ────────────────────────────────────────────────────────

/// A [`Reporter`] that exports all eval results as training data.
///
/// Implements the existing [`Reporter`] trait so it can be composed with other
/// reporters in the eval runner pipeline.
///
/// The reporter converts each [`EvalCaseResult`] into a [`ScoredTrace`],
/// applies the configured [`ExportOptions`], and writes the export artifact to
/// the configured output path (or returns it as [`ReporterOutput::Artifact`]).
#[derive(Debug, Clone)]
pub struct TrainingReporter {
    opts: ExportOptions,
    /// Suggested output file path. Callers may override.
    output_path: PathBuf,
}

impl TrainingReporter {
    /// Create a new reporter with explicit options and output path.
    #[must_use]
    pub fn new(opts: ExportOptions, output_path: impl Into<PathBuf>) -> Self {
        Self {
            opts,
            output_path: output_path.into(),
        }
    }

    /// Create a ChatML/SFT reporter writing to `output_path`.
    #[must_use]
    pub fn chatml_sft(quality_threshold: f32, output_path: impl Into<PathBuf>) -> Self {
        Self::new(ExportOptions::chatml_sft(quality_threshold), output_path)
    }

    /// Create a DPO pairs reporter writing to `output_path`.
    #[must_use]
    pub fn dpo_pairs(quality_threshold: f32, output_path: impl Into<PathBuf>) -> Self {
        Self::new(ExportOptions::dpo_pairs(quality_threshold), output_path)
    }

    /// Create a ShareGPT reporter writing to `output_path`.
    #[must_use]
    pub fn sharegpt(output_path: impl Into<PathBuf>) -> Self {
        Self::new(ExportOptions::sharegpt(), output_path)
    }
}

impl Reporter for TrainingReporter {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError> {
        let traces: Vec<ScoredTrace> = result
            .case_results
            .iter()
            .map(ScoredTrace::from_case_result)
            .collect();

        let bytes =
            export_traces(&traces, &self.opts).map_err(|e| ReporterError::Format(e.to_string()))?;

        Ok(ReporterOutput::Artifact {
            path: self.output_path.clone(),
            bytes,
        })
    }
}

// Import needed for Reporter impl
use crate::types::EvalSetResult;
