//! Repo-level guard for FR-050 (T165) — no test file sets a live-judge
//! API key without gating itself behind the `live-judges` cargo feature.
//!
//! This test greps every `*.rs` file under `eval/tests/` and
//! `eval-judges/tests/` looking for `std::env::set_var("ANTHROPIC_API_KEY"` /
//! `OPENAI_API_KEY` / the other eight provider keys. If a match is found
//! in a file that lacks `feature = "live-judges"` (either in a `#![cfg(...)]`
//! or `#[cfg(...)]`), the test fails with the offending path + line number.
//!
//! The suite stays hermetic — no live LLM calls can slip in by accident.

use std::fs;
use std::path::{Path, PathBuf};

const PROVIDER_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "BEDROCK_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "XAI_API_KEY",
    "OLLAMA_API_KEY",
];

const LIVE_JUDGES_GATE: &str = "live-judges";

fn test_dirs() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    vec![
        root.join("eval").join("tests"),
        root.join("eval-judges").join("tests"),
    ]
}

fn walk_rs(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, files);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn no_test_sets_provider_key_without_live_judges_gate() {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in test_dirs() {
        walk_rs(&dir, &mut files);
    }
    assert!(
        !files.is_empty(),
        "found no test files to scan — sanity check failed"
    );

    let mut offenses: Vec<String> = Vec::new();
    for file in files {
        let body = match fs::read_to_string(&file) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mentions_gate = body.contains(LIVE_JUDGES_GATE);
        for (lineno, line) in body.lines().enumerate() {
            for key in PROVIDER_KEYS {
                if line.contains("set_var") && line.contains(key) && !mentions_gate {
                    offenses.push(format!("{}:{}: {}", file.display(), lineno + 1, key));
                }
            }
        }
    }

    assert!(
        offenses.is_empty(),
        "FR-050 violation — test files setting provider API keys without a `{}` cargo gate:\n{}",
        LIVE_JUDGES_GATE,
        offenses.join("\n")
    );
}
