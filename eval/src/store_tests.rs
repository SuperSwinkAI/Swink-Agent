//! Tests for `store`.
#![cfg(test)]

use super::{EvalStore, FsEvalStore};
use std::fs;
use std::io::{self, Write};

#[test]
fn failed_atomic_rewrite_preserves_existing_eval_json() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sets").join("suite.json");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "{\"stable\":true}\n").unwrap();

    let error = FsEvalStore::write_atomically(&target, |writer| {
        writer.write_all(b"{\"stable\":false")?;
        Err(io::Error::other("boom"))
    })
    .unwrap_err();

    assert!(matches!(error, crate::error::EvalError::Io { .. }));
    assert_eq!(fs::read_to_string(&target).unwrap(), "{\"stable\":true}\n");

    let temp_files: Vec<_> = fs::read_dir(target.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".suite.json.tmp."))
        .collect();
    assert!(temp_files.is_empty());
}

#[test]
fn list_results_returns_only_json_timestamp_files_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let result_dir = dir.path().join("results").join("suite");
    fs::create_dir_all(&result_dir).unwrap();
    fs::write(result_dir.join("20.json"), "{}").unwrap();
    fs::write(result_dir.join("10.json"), "{}").unwrap();
    fs::write(result_dir.join("not-a-timestamp.json"), "{}").unwrap();
    fs::write(result_dir.join("30.tmp"), "{}").unwrap();
    fs::write(result_dir.join("40"), "{}").unwrap();

    let store = FsEvalStore::new(dir.path());

    assert_eq!(store.list_results("suite").unwrap(), vec![10, 20]);
}
