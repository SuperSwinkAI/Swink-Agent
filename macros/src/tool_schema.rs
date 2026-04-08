//! Implementation of `#[derive(ToolSchema)]`.
//!
//! Delegates JSON Schema generation to [`schemars`] by implementing
//! `ToolParameters::json_schema` via `swink_agent::schema_for::<Self>()`.
//!
//! The annotated struct must also implement [`schemars::JsonSchema`] (e.g. via
//! `#[derive(schemars::JsonSchema)]` or `#[derive(swink_agent::JsonSchema)]`).
//! Doc comments become `description` fields automatically via schemars.
//! Use `#[schemars(description = "...")]` to override a field description.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

pub fn derive_tool_schema_impl(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;

    // Validate that the input is a named struct so error messages remain helpful.
    let Data::Struct(data_struct) = &input.data else {
        return syn::Error::new_spanned(&input.ident, "ToolSchema can only be derived for structs")
            .to_compile_error();
    };
    let Fields::Named(_) = &data_struct.fields else {
        return syn::Error::new_spanned(&input.ident, "ToolSchema requires named fields")
            .to_compile_error();
    };

    // Delegate entirely to schemars. The struct must also derive `JsonSchema`
    // (available as `swink_agent::JsonSchema` or `schemars::JsonSchema`).
    quote! {
        impl swink_agent::tool::ToolParameters for #name {
            fn json_schema() -> serde_json::Value {
                swink_agent::schema_for::<Self>()
            }
        }
    }
}
