//! Tests for `#[derive(ToolSchema)]`.
//!
//! Schema generation now delegates to [`schemars`]. Each struct must also
//! derive `schemars::JsonSchema` (available via `swink_agent::JsonSchema`).
//! Field descriptions come from doc comments or `#[schemars(description = "...")]`.

#![allow(dead_code)]

use swink_agent::JsonSchema;
use swink_agent::ToolParameters;
use swink_agent_macros::ToolSchema;

#[derive(ToolSchema, JsonSchema)]
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
    let schema = <BasicParams as ToolParameters>::json_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["age"]["type"], "integer");
    assert_eq!(schema["properties"]["active"]["type"], "boolean");

    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("name")));
    assert!(required.contains(&serde_json::json!("age")));
    assert!(required.contains(&serde_json::json!("active")));
}

#[derive(ToolSchema, JsonSchema)]
struct OptionalParams {
    /// Required field
    required_field: String,
    /// Optional field
    optional_field: Option<String>,
}

#[test]
fn derive_tool_schema_option() {
    let schema = <OptionalParams as ToolParameters>::json_schema();
    // required_field must be in required; optional_field must not be.
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("required_field")));
    assert!(!required.contains(&serde_json::json!("optional_field")));
    // The optional field is still present in properties.
    assert!(schema["properties"]["optional_field"].is_object());
}

#[derive(ToolSchema, JsonSchema)]
struct VecParams {
    /// List of tags
    tags: Vec<String>,
}

#[test]
fn derive_tool_schema_vec() {
    let schema = <VecParams as ToolParameters>::json_schema();
    assert_eq!(schema["properties"]["tags"]["type"], "array");
    assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
}

#[derive(ToolSchema, JsonSchema)]
struct DocCommentParams {
    /// This is the city name
    city: String,
}

#[test]
fn derive_tool_schema_doc_comments() {
    let schema = <DocCommentParams as ToolParameters>::json_schema();
    assert_eq!(
        schema["properties"]["city"]["description"],
        "This is the city name"
    );
}

// Field descriptions can be overridden with #[schemars(description = "...")] —
// the old #[tool(description = "...")] attribute is no longer processed.
#[derive(ToolSchema, JsonSchema)]
struct AttrOverrideParams {
    /// Doc comment description
    #[schemars(description = "Overridden description")]
    field: String,
}

#[test]
fn derive_tool_schema_attr_override() {
    let schema = <AttrOverrideParams as ToolParameters>::json_schema();
    assert_eq!(
        schema["properties"]["field"]["description"],
        "Overridden description"
    );
}

#[derive(ToolSchema, JsonSchema)]
#[allow(clippy::struct_field_names)]
struct NumericParams {
    count_i32: i32,
    count_f64: f64,
    count_usize: usize,
}

#[test]
fn derive_tool_schema_numeric_types() {
    let schema = <NumericParams as ToolParameters>::json_schema();
    assert_eq!(schema["properties"]["count_i32"]["type"], "integer");
    assert_eq!(schema["properties"]["count_f64"]["type"], "number");
    assert_eq!(schema["properties"]["count_usize"]["type"], "integer");
}

// Regression: nested/complex types beyond the old bespoke mapper's reach now work.
#[derive(ToolSchema, JsonSchema)]
struct NestedObjectParam {
    /// Outer field
    label: String,
    /// Inner params
    inner: InnerData,
}

#[derive(JsonSchema)]
struct InnerData {
    /// Inner value
    value: u32,
}

#[test]
fn derive_tool_schema_nested_object() {
    let schema = <NestedObjectParam as ToolParameters>::json_schema();
    assert_eq!(schema["type"], "object");
    // Both top-level fields must be present.
    assert!(schema["properties"]["label"].is_object());
    assert!(schema["properties"]["inner"].is_object());
    // Required must list both non-optional fields.
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("label")));
    assert!(required.contains(&serde_json::json!("inner")));
}
