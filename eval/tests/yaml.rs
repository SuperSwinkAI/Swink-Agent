#![cfg(feature = "yaml")]

mod common;

use std::fs;
use std::io::Write;

use tempfile::{NamedTempFile, TempDir};

use swink_agent_eval::{load_eval_set_yaml, EvalStore, FsEvalStore};

#[test]
fn load_yaml_eval_set() {
    let yaml = r"
id: test-set
name: Test Set
cases:
  - id: case-1
    name: Case One
    system_prompt: You are a test agent.
    user_messages:
      - Hello
";
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(yaml.as_bytes()).unwrap();

    let set = load_eval_set_yaml(file.path()).unwrap();

    assert_eq!(set.id, "test-set");
    assert_eq!(set.name, "Test Set");
    assert_eq!(set.cases.len(), 1);
    assert_eq!(set.cases[0].id, "case-1");
    assert_eq!(set.cases[0].name, "Case One");
    assert_eq!(set.cases[0].system_prompt, "You are a test agent.");
    assert_eq!(set.cases[0].user_messages, vec!["Hello"]);
}

#[test]
fn store_load_set_prefers_yaml() {
    let dir = TempDir::new().unwrap();
    let sets_dir = dir.path().join("sets");
    fs::create_dir_all(&sets_dir).unwrap();

    let yaml_content = r"
id: test
name: From YAML
cases: []
";
    fs::write(sets_dir.join("test.yaml"), yaml_content).unwrap();

    let json_content = r#"{"id":"test","name":"From JSON","cases":[]}"#;
    fs::write(sets_dir.join("test.json"), json_content).unwrap();

    let store = FsEvalStore::new(dir.path());
    let set = store.load_set("test").unwrap();

    assert_eq!(set.name, "From YAML");
}

#[test]
fn store_falls_back_to_json() {
    let dir = TempDir::new().unwrap();
    let sets_dir = dir.path().join("sets");
    fs::create_dir_all(&sets_dir).unwrap();

    let json_content = r#"{"id":"test","name":"From JSON","cases":[]}"#;
    fs::write(sets_dir.join("test.json"), json_content).unwrap();

    let store = FsEvalStore::new(dir.path());
    let set = store.load_set("test").unwrap();

    assert_eq!(set.name, "From JSON");
}

#[test]
fn yaml_parse_error_returns_yaml_variant() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(b"[invalid: yaml: {{{").unwrap();

    let err = load_eval_set_yaml(file.path()).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("yaml"), "expected 'yaml' in error message, got: {msg}");
}
