//! Built-in tool for making surgical find-and-replace edits to a file.

use std::io::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use super::path::resolve_existing_path;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};
use crate::types::ContentBlock;

/// Built-in tool for making precise, surgical edits to a file.
///
/// Supports multiple edits per call, atomic writes, stale-read detection,
/// whitespace-normalised matching, and line-number-based disambiguation.
pub struct EditFileTool {
    schema: Value,
    execution_root: Option<PathBuf>,
}

impl EditFileTool {
    /// Create a new `EditFileTool`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: validated_schema_for::<Params>(),
            execution_root: None,
        }
    }

    /// Set the working directory used to resolve relative file paths.
    #[must_use]
    pub fn with_execution_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.execution_root = Some(root.into());
        self
    }
}

impl Default for EditFileTool {
    fn default() -> Self {
        Self::new()
    }
}

/// One find-and-replace operation.
#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct EditOp {
    /// Text to find in the file.  Exact match is tried first; if that fails a
    /// line-by-line match that ignores trailing whitespace is attempted.
    old_string: String,
    /// Replacement text.
    new_string: String,
    /// When `true`, every occurrence is replaced.  When `false` (the default)
    /// exactly one occurrence must exist, or `line_hint` must be provided.
    #[serde(default)]
    replace_all: bool,
    /// 1-based line number of the desired occurrence.  Used to pick among
    /// multiple matches when `replace_all` is `false`.
    line_hint: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Params {
    /// Absolute path to the file to edit.
    path: String,
    /// Edits to apply in order (top-to-bottom).
    edits: Vec<EditOp>,
    /// SHA-256 hex digest of the file content as previously read.  When
    /// provided the edit is rejected if the file has changed since.
    expected_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Matching helpers
// ---------------------------------------------------------------------------

/// Compute the SHA-256 hex digest of `data`.
fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Return `(byte_start, line_content_without_newline)` for every line.
///
/// Splits on `'\n'`; the `'\n'` itself is not included in the line slice.
/// Windows `\r\n` files: the `\r` will appear as trailing content in each
/// slice, which is stripped by [`str::trim_end`] during normalised matching.
fn line_spans(s: &str) -> Vec<(usize, &str)> {
    let mut spans = Vec::new();
    let mut pos = 0;
    for line in s.split('\n') {
        spans.push((pos, line));
        pos += line.len() + 1; // +1 for the '\n'
    }
    spans
}

/// Find all non-overlapping exact byte ranges of `pattern` in `content`.
fn find_exact(content: &str, pattern: &str) -> Vec<Range<usize>> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(pos) = content[start..].find(pattern) {
        let abs = start + pos;
        ranges.push(abs..abs + pattern.len());
        start = abs + pattern.len();
    }
    ranges
}

/// Find all non-overlapping byte ranges in `content` that match `pattern`
/// line-by-line, ignoring trailing whitespace on each line.
///
/// Leading and trailing blank lines in `pattern` are stripped before
/// matching.  The returned ranges refer to byte positions in the original
/// (un-normalised) `content`.
fn find_normalized(content: &str, pattern: &str) -> Vec<Range<usize>> {
    let pattern = pattern.trim_matches('\n');
    if pattern.is_empty() {
        return Vec::new();
    }
    let pattern_lines: Vec<&str> = pattern.split('\n').collect();
    let spans = line_spans(content);
    let n = pattern_lines.len();

    if n > spans.len() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut i = 0;
    while i + n <= spans.len() {
        let all_match = pattern_lines
            .iter()
            .enumerate()
            .all(|(j, &pl)| spans[i + j].1.trim_end() == pl.trim_end());

        if all_match {
            let byte_start = spans[i].0;
            let last = &spans[i + n - 1];
            let byte_end = last.0 + last.1.len();
            ranges.push(byte_start..byte_end);
            i += n; // skip past the match so occurrences don't overlap
        } else {
            i += 1;
        }
    }
    ranges
}

/// Return the 1-based line number of the character at `byte_pos`.
fn line_number_at(content: &str, byte_pos: usize) -> usize {
    content[..byte_pos].chars().filter(|&c| c == '\n').count() + 1
}

/// Replace all `ranges` in `content` with `replacement`.
///
/// `ranges` must be sorted ascending and non-overlapping.
fn replace_ranges(content: &str, ranges: &[Range<usize>], replacement: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0;
    for r in ranges {
        out.push_str(&content[cursor..r.start]);
        out.push_str(replacement);
        cursor = r.end;
    }
    out.push_str(&content[cursor..]);
    out
}

/// Apply a single [`EditOp`] to `content`, returning the modified string or
/// an error message.
fn apply_op(content: &str, op: &EditOp) -> Result<String, String> {
    if op.old_string.is_empty() {
        return Err("old_string must not be empty".to_owned());
    }

    // Prefer exact match; fall back to whitespace-normalised line matching.
    let candidates: Vec<Range<usize>> = {
        let exact = find_exact(content, &op.old_string);
        if exact.is_empty() {
            let norm = find_normalized(content, &op.old_string);
            if norm.is_empty() {
                return Err(format!(
                    "old_string not found (tried exact and whitespace-normalised match):\n{}",
                    op.old_string
                ));
            }
            norm
        } else {
            exact
        }
    };

    if op.replace_all {
        return Ok(replace_ranges(content, &candidates, &op.new_string));
    }

    match candidates.len() {
        0 => unreachable!("candidates is non-empty at this point"),
        1 => Ok(replace_ranges(content, &candidates, &op.new_string)),
        n => op.line_hint.map_or_else(
            || {
                Err(format!(
                    "old_string matched {n} times; set replace_all to replace every \
                     occurrence, or provide line_hint to select one"
                ))
            },
            |hint| {
                let best = candidates
                    .iter()
                    .min_by_key(|r| {
                        let line =
                            i64::try_from(line_number_at(content, r.start)).unwrap_or(i64::MAX);
                        (line - i64::from(hint)).abs()
                    })
                    .expect("candidates is non-empty");
                Ok(replace_ranges(
                    content,
                    std::slice::from_ref(best),
                    &op.new_string,
                ))
            },
        ),
    }
}

// ---------------------------------------------------------------------------
// Locked read-modify-write
// ---------------------------------------------------------------------------

/// Apply edits and write the result while holding the shared per-target lock.
fn edit_file_locked(
    path: &Path,
    expected_hash: Option<&str>,
    edits: &[EditOp],
) -> Result<(String, String), String> {
    crate::atomic_fs::with_target_lock(path, || Ok(edit_file_unlocked(path, expected_hash, edits)))
        .map_err(|error| format!("failed to lock {}: {error}", path.display()))?
}

fn edit_file_unlocked(
    path: &Path,
    expected_hash: Option<&str>,
    edits: &[EditOp],
) -> Result<(String, String), String> {
    let raw_bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;

    let original = std::str::from_utf8(&raw_bytes)
        .map_err(|_| format!("{} is not valid UTF-8", path.display()))?
        .to_owned();

    if let Some(expected) = expected_hash {
        let actual = sha256_hex(&raw_bytes);
        if actual != expected.to_ascii_lowercase() {
            return Err(format!(
                "{} has changed since it was last read (hash mismatch); \
                 re-read the file before editing",
                path.display()
            ));
        }
    }

    let mut content = original.clone();
    for (i, op) in edits.iter().enumerate() {
        content = apply_op(&content, op).map_err(|msg| format!("edit {}: {msg}", i + 1))?;
    }

    crate::atomic_fs::atomic_write_unlocked(path, |writer| writer.write_all(content.as_bytes()))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;

    Ok((original, content))
}

// ---------------------------------------------------------------------------
// AgentTool impl
// ---------------------------------------------------------------------------

#[allow(clippy::unnecessary_literal_bound)]
impl AgentTool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn label(&self) -> &str {
        "Edit File"
    }

    fn description(&self) -> &str {
        "Apply one or more surgical find-and-replace edits to a file. \
         Edits are applied top-to-bottom. Trailing whitespace is ignored \
         during matching when an exact match is not found. The write is \
         atomic: the file is never left in a partially-written state."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execution_root(&self) -> Option<&Path> {
        self.execution_root.as_deref()
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            let parsed: Params = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return AgentToolResult::error(format!("invalid parameters: {e}")),
            };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            let path =
                match resolve_existing_path(&parsed.path, self.execution_root.as_deref()).await {
                    Ok(path) => path,
                    Err(error) => return AgentToolResult::error(error),
                };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            let path = if self.execution_root.is_none() {
                match tokio::fs::canonicalize(&path).await {
                    Ok(path) => path,
                    Err(error) => {
                        return AgentToolResult::error(format!(
                            "failed to resolve path {}: {error}",
                            path.display()
                        ));
                    }
                }
            } else {
                path
            };

            if parsed.edits.is_empty() {
                return AgentToolResult::text("no edits specified; file unchanged");
            }

            let n = parsed.edits.len();
            let expected_hash = parsed.expected_hash;
            let edits = parsed.edits;
            let edit_path = path.clone();
            let edit_result = tokio::task::spawn_blocking(move || {
                edit_file_locked(&edit_path, expected_hash.as_deref(), &edits)
            })
            .await;
            let (original, content) = match edit_result {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => return AgentToolResult::error(error),
                Err(error) => {
                    return AgentToolResult::error(format!("edit task failed: {error}"));
                }
            };

            AgentToolResult {
                content: vec![ContentBlock::Text {
                    text: format!(
                        "Applied {} edit{} to {}",
                        n,
                        if n == 1 { "" } else { "s" },
                        path.display()
                    ),
                }],
                details: serde_json::json!({
                    "path": path,
                    "edits_applied": n,
                    "old_content": original,
                    "new_content": content,
                }),
                is_error: false,
                transfer_signal: None,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── apply_op unit tests ──────────────────────────────────────────────────

    #[test]
    fn exact_single_replacement() {
        let content = "hello world\n";
        let op = EditOp {
            old_string: "world".into(),
            new_string: "Rust".into(),
            replace_all: false,
            line_hint: None,
        };
        assert_eq!(apply_op(content, &op).unwrap(), "hello Rust\n");
    }

    #[test]
    fn normalised_trailing_whitespace_match() {
        // File has trailing spaces; old_string does not — should still match.
        let content = "fn foo() {   \n    let x = 1;\n}\n";
        let op = EditOp {
            old_string: "fn foo() {\n    let x = 1;\n}".into(),
            new_string: "fn foo() {\n    let x = 2;\n}".into(),
            replace_all: false,
            line_hint: None,
        };
        assert_eq!(
            apply_op(content, &op).unwrap(),
            "fn foo() {\n    let x = 2;\n}\n"
        );
    }

    #[test]
    fn replace_all_occurrences() {
        let content = "foo bar foo baz foo\n";
        let op = EditOp {
            old_string: "foo".into(),
            new_string: "qux".into(),
            replace_all: true,
            line_hint: None,
        };
        assert_eq!(apply_op(content, &op).unwrap(), "qux bar qux baz qux\n");
    }

    #[test]
    fn multiple_matches_without_hint_is_error() {
        let content = "fn foo() {}\nfn foo() {}\n";
        let op = EditOp {
            old_string: "fn foo() {}".into(),
            new_string: "fn bar() {}".into(),
            replace_all: false,
            line_hint: None,
        };
        let err = apply_op(content, &op).unwrap_err();
        assert!(err.contains("matched 2 times"), "unexpected error: {err}");
    }

    #[test]
    fn line_hint_picks_closest_match() {
        // "fn foo() {}" appears on lines 1 and 3; hint=3 should pick line 3.
        let content = "fn foo() {}\nfn bar() {}\nfn foo() {}\n";
        let op = EditOp {
            old_string: "fn foo() {}".into(),
            new_string: "fn baz() {}".into(),
            replace_all: false,
            line_hint: Some(3),
        };
        assert_eq!(
            apply_op(content, &op).unwrap(),
            "fn foo() {}\nfn bar() {}\nfn baz() {}\n"
        );
    }

    #[test]
    fn not_found_returns_error() {
        let content = "hello world\n";
        let op = EditOp {
            old_string: "missing".into(),
            new_string: "x".into(),
            replace_all: false,
            line_hint: None,
        };
        assert!(apply_op(content, &op).is_err());
    }

    #[test]
    fn empty_old_string_is_error() {
        let op = EditOp {
            old_string: String::new(),
            new_string: "x".into(),
            replace_all: false,
            line_hint: None,
        };
        assert!(apply_op("anything", &op).is_err());
    }

    #[test]
    fn multiple_edits_applied_in_order() {
        let mut content = "a b c\n".to_owned();
        let ops = [
            EditOp {
                old_string: "a".into(),
                new_string: "1".into(),
                replace_all: false,
                line_hint: None,
            },
            EditOp {
                old_string: "b".into(),
                new_string: "2".into(),
                replace_all: false,
                line_hint: None,
            },
            EditOp {
                old_string: "c".into(),
                new_string: "3".into(),
                replace_all: false,
                line_hint: None,
            },
        ];
        for op in &ops {
            content = apply_op(&content, op).unwrap();
        }
        assert_eq!(content, "1 2 3\n");
    }

    // ── sha256_hex ───────────────────────────────────────────────────────────

    #[test]
    fn sha256_hex_known_value() {
        // echo -n "abc" | sha256sum → ba7816bf…
        let digest = sha256_hex(b"abc");
        assert!(digest.starts_with("ba7816bf"), "got: {digest}");
        assert_eq!(digest.len(), 64);
    }

    // ── Integration: execute via tempfile ────────────────────────────────────

    #[tokio::test]
    async fn execute_edits_file_and_returns_diff() {
        use std::sync::{Arc, RwLock};

        use serde_json::json;

        use crate::SessionState;
        use crate::tool::AgentTool;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        tokio::fs::write(&file, "hello world\n").await.unwrap();

        let tool = EditFileTool::new();
        let params = json!({
            "path": file.to_str().unwrap(),
            "edits": [{ "old_string": "world", "new_string": "Rust" }]
        });

        let result = tool
            .execute(
                "id",
                params,
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::default())),
                None,
            )
            .await;

        assert!(!result.is_error);
        let on_disk = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(on_disk, "hello Rust\n");
        assert_eq!(result.details["old_content"], "hello world\n");
        assert_eq!(result.details["new_content"], "hello Rust\n");
    }

    #[tokio::test]
    async fn execute_rejects_stale_hash() {
        use std::sync::{Arc, RwLock};

        use serde_json::json;

        use crate::SessionState;
        use crate::tool::AgentTool;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        tokio::fs::write(&file, "hello world\n").await.unwrap();

        let tool = EditFileTool::new();
        let params = json!({
            "path": file.to_str().unwrap(),
            "edits": [{ "old_string": "world", "new_string": "Rust" }],
            "expected_hash": "0000000000000000000000000000000000000000000000000000000000000000"
        });

        let result = tool
            .execute(
                "id",
                params,
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::default())),
                None,
            )
            .await;

        assert!(result.is_error);
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text block"),
        };
        assert!(text.contains("hash mismatch"), "got: {text}");
    }

    #[tokio::test]
    async fn execute_rejects_relative_path_outside_execution_root() {
        use std::sync::{Arc, RwLock};

        use serde_json::json;

        use crate::SessionState;
        use crate::tool::AgentTool;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        tokio::fs::create_dir(&root).await.unwrap();
        let outside = dir.path().join("outside.txt");
        tokio::fs::write(&outside, "hello world\n").await.unwrap();

        let result = EditFileTool::new()
            .with_execution_root(&root)
            .execute(
                "id",
                json!({
                    "path": "../outside.txt",
                    "edits": [{ "old_string": "world", "new_string": "Rust" }]
                }),
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::default())),
                None,
            )
            .await;

        assert!(result.is_error);
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text block"),
        };
        assert!(text.contains("escapes execution root"), "got: {text}");
        let on_disk = tokio::fs::read_to_string(&outside).await.unwrap();
        assert_eq!(on_disk, "hello world\n");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_target_edits_preserve_disjoint_changes() {
        use std::sync::{Arc, RwLock};

        use serde_json::json;
        use tokio::sync::Barrier;

        use crate::SessionState;
        use crate::tool::AgentTool;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let original = (0..16)
            .map(|i| format!("line-{i:02}\n"))
            .collect::<String>();
        tokio::fs::write(&file, original).await.unwrap();

        let tool = Arc::new(EditFileTool::new());
        let barrier = Arc::new(Barrier::new(16));
        let mut handles = Vec::new();
        for i in 0..16 {
            let tool = Arc::clone(&tool);
            let barrier = Arc::clone(&barrier);
            let file = file.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                tool.execute(
                    "id",
                    json!({
                        "path": file.to_str().unwrap(),
                        "edits": [{
                            "old_string": format!("line-{i:02}"),
                            "new_string": format!("edited-{i:02}")
                        }]
                    }),
                    CancellationToken::new(),
                    None,
                    Arc::new(RwLock::new(SessionState::default())),
                    None,
                )
                .await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(
                !result.is_error,
                "unexpected edit failure: {:?}",
                result.content
            );
        }

        let on_disk = tokio::fs::read_to_string(&file).await.unwrap();
        for i in 0..16 {
            assert!(
                on_disk.contains(&format!("edited-{i:02}")),
                "missing concurrent edit {i}; final content:\n{on_disk}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_same_hash_edits_recheck_staleness_under_lock() {
        use std::sync::{Arc, RwLock};

        use serde_json::json;
        use tokio::sync::Barrier;

        use crate::SessionState;
        use crate::tool::AgentTool;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let original = "line-00\nline-01\n";
        tokio::fs::write(&file, original).await.unwrap();
        let expected_hash = sha256_hex(original.as_bytes());

        let tool = Arc::new(EditFileTool::new());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for i in 0..2 {
            let tool = Arc::clone(&tool);
            let barrier = Arc::clone(&barrier);
            let file = file.clone();
            let expected_hash = expected_hash.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                tool.execute(
                    "id",
                    json!({
                        "path": file.to_str().unwrap(),
                        "expected_hash": expected_hash,
                        "edits": [{
                            "old_string": format!("line-{i:02}"),
                            "new_string": format!("edited-{i:02}")
                        }]
                    }),
                    CancellationToken::new(),
                    None,
                    Arc::new(RwLock::new(SessionState::default())),
                    None,
                )
                .await
            }));
        }

        let mut successes = 0;
        let mut stale_rejections = 0;
        for handle in handles {
            let result = handle.await.unwrap();
            if result.is_error {
                let text = ContentBlock::extract_text(&result.content);
                assert!(text.contains("hash mismatch"), "unexpected error: {text}");
                stale_rejections += 1;
            } else {
                successes += 1;
            }
        }

        assert_eq!(successes, 1);
        assert_eq!(stale_rejections, 1);
    }
}
