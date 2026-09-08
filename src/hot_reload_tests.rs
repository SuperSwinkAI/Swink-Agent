//! Tests for `hot_reload`.
#![cfg(test)]

use serde_json::json;

use super::*;

#[test]
fn script_tool_from_toml() {
    let definition = r#"
name = "greet"
description = "Greet someone"
command = "echo Hello {name}"
"#;
    let tool = ScriptTool::from_toml(definition).unwrap();
    assert_eq!(tool.name(), "greet");
    assert_eq!(tool.description(), "Greet someone");
    assert!(tool.requires_approval());
}

#[test]
fn script_tool_from_toml_with_parameter_schema_section() {
    let definition = r#"
name = "greet"
description = "Greet a person by name"
command = "echo 'Hello, {name}!'"

[parameters_schema]
type = "object"
required = ["name"]

[parameters_schema.properties.name]
type = "string"
description = "The name to greet"
"#;

    let tool = ScriptTool::from_toml(definition).unwrap();
    assert_eq!(tool.parameters_schema()["required"], json!(["name"]));
    assert_eq!(
        tool.parameters_schema()["properties"]["name"]["description"],
        json!("The name to greet")
    );
}

#[test]
fn script_tool_from_json_definition() {
    let json_str = r#"{"name": "test", "description": "A test", "command": "echo test"}"#;
    let tool = ScriptTool::from_json(json_str).unwrap();
    assert_eq!(tool.name(), "test");
}

#[test]
fn script_tool_invalid_definition() {
    let result = ScriptTool::from_toml("invalid toml {{{}}}");
    assert!(result.is_err());
}

#[test]
fn script_tool_escapes_parameters() {
    let definition = r#"
name = "run"
description = "Run command"
command = "echo {input}"
"#;
    let tool = ScriptTool::from_toml(definition).unwrap();
    // Input contains a single quote to exercise the '\'' escape path
    let cmd = tool.interpolate_command(&json!({"input": "it's; rm -rf /"}));
    assert!(
        cmd.contains("'\\''"),
        "expected '\\'' escape sequence in {cmd}"
    );
    assert!(cmd.contains("'it'\\''s; rm -rf /'"));
}

#[tokio::test]
async fn script_tool_executes_command() {
    let definition = r#"
name = "echo_test"
description = "Echo test"
command = "echo hello"
"#;
    let tool = ScriptTool::from_toml(definition).unwrap();
    let result = tool
        .execute(
            "call_1",
            json!({}),
            CancellationToken::new(),
            None,
            std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::new())),
            None,
        )
        .await;
    assert!(!result.is_error);
}

#[test]
fn duplicate_tool_names_last_write_wins() {
    let tool1 = ScriptTool::from_toml(
        r#"
name = "dup"
description = "First"
command = "echo 1"
"#,
    )
    .unwrap();
    let tool2 = ScriptTool::from_toml(
        r#"
name = "dup"
description = "Second"
command = "echo 2"
"#,
    )
    .unwrap();

    let mut map: HashMap<PathBuf, ScriptTool> = HashMap::new();
    map.insert(PathBuf::from("/a.toml"), tool1);

    // Simulate last-write-wins
    let name = tool2.def.name.clone();
    map.retain(|_, t| t.def.name != name);
    map.insert(PathBuf::from("/b.toml"), tool2);

    assert_eq!(map.len(), 1);
    assert_eq!(map.values().next().unwrap().def.description, "Second");
}
