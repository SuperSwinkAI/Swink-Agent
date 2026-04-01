//! Tests for `#[derive(ToolSchema)]`.

use swink_agent::tool::ToolParameters;
use swink_agent_macros::ToolSchema;

#[derive(ToolSchema)]
struct BasicParams {
    /// The user's name
    name: String,
    /// Age in years
    age: u64,
    /// Is active
    active: bool,
}

#[test]
fn derive_tool_schema_basic() {
    let schema = BasicParams::json_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["age"]["type"], "integer");
    assert_eq!(schema["properties"]["active"]["type"], "boolean");

    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("name")));
    assert!(required.contains(&serde_json::json!("age")));
    assert!(required.contains(&serde_json::json!("active")));
}

#[derive(ToolSchema)]
struct OptionalParams {
    /// Required field
    required_field: String,
    /// Optional field
    optional_field: Option<String>,
}

#[test]
fn derive_tool_schema_option() {
    let schema = OptionalParams::json_schema();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("required_field")));
    assert!(!required.contains(&serde_json::json!("optional_field")));
    // Optional field still has a type
    assert_eq!(schema["properties"]["optional_field"]["type"], "string");
}

#[derive(ToolSchema)]
struct VecParams {
    /// List of tags
    tags: Vec<String>,
}

#[test]
fn derive_tool_schema_vec() {
    let schema = VecParams::json_schema();
    assert_eq!(schema["properties"]["tags"]["type"], "array");
    assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
}

#[derive(ToolSchema)]
struct DocCommentParams {
    /// This is the city name
    city: String,
}

#[test]
fn derive_tool_schema_doc_comments() {
    let schema = DocCommentParams::json_schema();
    assert_eq!(
        schema["properties"]["city"]["description"],
        "This is the city name"
    );
}

#[derive(ToolSchema)]
struct AttrOverrideParams {
    /// Doc comment description
    #[tool(description = "Overridden description")]
    field: String,
}

#[test]
fn derive_tool_schema_attr_override() {
    let schema = AttrOverrideParams::json_schema();
    assert_eq!(
        schema["properties"]["field"]["description"],
        "Overridden description"
    );
}

#[derive(ToolSchema)]
struct NumericParams {
    count_i32: i32,
    count_f64: f64,
    count_usize: usize,
}

#[test]
fn derive_tool_schema_numeric_types() {
    let schema = NumericParams::json_schema();
    assert_eq!(schema["properties"]["count_i32"]["type"], "integer");
    assert_eq!(schema["properties"]["count_f64"]["type"], "number");
    assert_eq!(schema["properties"]["count_usize"]["type"], "integer");
}
