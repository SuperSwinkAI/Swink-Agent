#![forbid(unsafe_code)]
//! Proc macros for `swink-agent`: `#[derive(ToolSchema)]` and `#[tool]`.
//!
//! This crate provides two proc macros:
//!
//! - `#[derive(ToolSchema)]` — generates a `ToolParameters` implementation from
//!   a struct's fields, mapping Rust types to JSON Schema types.
//! - `#[tool(name = "...", description = "...")]` — wraps an async function as
//!   an `AgentTool` implementation.

mod tool_attr;
mod tool_schema;

use proc_macro::TokenStream;

/// Derive macro that generates a `ToolParameters` implementation.
///
/// Maps struct fields to JSON Schema properties:
/// - `String` → `"string"`
/// - `u8`–`u128`, `i8`–`i128` → `"integer"`
/// - `f32`, `f64` → `"number"`
/// - `bool` → `"boolean"`
/// - `Option<T>` → type of T, not in `required`
/// - `Vec<T>` → `"array"` with items of T's type
///
/// Doc comments (`///`) become `description` fields. Use
/// `#[tool(description = "...")]` to override.
#[proc_macro_derive(ToolSchema, attributes(tool))]
pub fn derive_tool_schema(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    tool_schema::derive_tool_schema_impl(&input).into()
}

/// Attribute macro that generates an `AgentTool` implementation from an async
/// function.
///
/// # Attributes
///
/// - `name` — the tool's routing key (required)
/// - `description` — natural-language description for the LLM prompt
///
/// # Example
///
/// ```ignore
/// #[tool(name = "greet", description = "Greet someone")]
/// async fn greet(name: String) -> AgentToolResult {
///     AgentToolResult::text(format!("Hello, {name}!"))
/// }
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool_attr::tool_attr_impl(attr.into(), item.into()).into()
}
