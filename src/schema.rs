//! JSON Schema generation from Rust types via [`schemars`].
//!
//! Provides [`schema_for`] to derive a `serde_json::Value` JSON Schema from
//! any type implementing [`schemars::JsonSchema`].

use serde_json::Value;

/// Generate a JSON Schema as a [`serde_json::Value`] from a type implementing
/// [`JsonSchema`](schemars::JsonSchema).
///
/// This is a thin wrapper around [`schemars::schema_for!`] that returns a
/// plain `Value` instead of a `Schema` struct, making it easy to pass directly
/// to [`AgentTool::parameters_schema`](crate::AgentTool::parameters_schema).
///
/// # Example
///
/// ```
/// use schemars::JsonSchema;
/// use serde::Deserialize;
/// use swink_agent::schema_for;
///
/// #[derive(Deserialize, JsonSchema)]
/// struct Params {
///     city: String,
///     units: Option<String>,
/// }
///
/// let schema = schema_for::<Params>();
/// assert_eq!(schema["type"], "object");
/// assert!(schema["required"].as_array().unwrap().contains(&"city".into()));
/// ```
#[must_use]
pub fn schema_for<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serialization cannot fail")
}
