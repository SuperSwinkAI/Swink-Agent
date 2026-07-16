#![forbid(unsafe_code)]
//! Proc macros for `swink-agent`: `#[derive(ToolSchema)]` and `#[tool]`.
//!
//! This crate provides two proc macros:
//!
//! - `#[derive(ToolSchema)]` — generates a `ToolParameters` implementation that
//!   delegates schema generation to `schemars`. The struct must also derive
//!   `schemars::JsonSchema` (available as `swink_agent::JsonSchema`).
//! - `#[tool(name = "...", description = "...")]` — wraps an async function as
//!   an `AgentTool` implementation. Schema is derived from a hidden params struct
//!   via `schemars`, replacing the previous bespoke type mapper.

mod tool_attr;
mod tool_schema;

use proc_macro::TokenStream;

/// Derive macro that generates a `ToolParameters` implementation.
///
/// Delegates JSON Schema generation to `schemars` by implementing
/// `ToolParameters::json_schema` via `swink_agent::schema_for::<Self>()`.
///
/// **The annotated struct must also derive `JsonSchema`** (via
/// `swink_agent::JsonSchema` or `schemars::JsonSchema`). Doc comments
/// (`/// …`) are automatically picked up as field descriptions by schemars.
/// Use `#[schemars(description = "…")]` to override a field description.
///
/// `#[tool(...)]` is **not** a helper attribute of this derive: schema
/// customization goes through `schemars` attributes exclusively. Putting
/// `#[tool(...)]` on a field is a compile error rather than a silent no-op.
///
/// All Rust types supported by `schemars` are accepted — there is no
/// restricted subset of primitives.
///
/// # Example
///
/// ```ignore
/// use swink_agent::JsonSchema;
/// use swink_agent_macros::ToolSchema;
///
/// #[derive(ToolSchema, JsonSchema, serde::Deserialize)]
/// struct SearchParams {
///     /// The search query
///     query: String,
///     /// Maximum number of results
///     limit: Option<u32>,
/// }
/// ```
// No `attributes(tool)` here: the derive never read a `#[tool(...)]` helper
// attribute, so registering it made `#[tool(description = "...")]` on a field
// a silent no-op. Without the registration, misuse is a compile error and the
// user is pointed at `#[schemars(description = "...")]`, which works.
#[proc_macro_derive(ToolSchema)]
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
