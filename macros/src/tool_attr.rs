//! Implementation of the `#[tool]` attribute macro.
//!
//! Generates a struct implementing `AgentTool` from an async function.
//!
//! Schema generation is delegated to [`schemars`] via a hidden params struct
//! that derives `serde::Deserialize` and `schemars::JsonSchema`. This replaces
//! the previous bespoke type mapper and eliminates the duplicate schema engine.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::{FnArg, ItemFn, Lit, Pat, Type};

pub fn tool_attr_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            input_fn.sig.fn_token,
            "#[tool] requires an async function",
        )
        .to_compile_error();
    }

    let (tool_name, tool_description) = match parse_tool_attrs(attr) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let fn_name = &input_fn.sig.ident;
    let struct_name = format_ident!("{}Tool", pascal_case(&fn_name.to_string()));

    // Hidden params struct name — unique per tool to avoid collisions.
    let params_struct_name = format_ident!("__{fn_name}Params");

    // Collect non-CancellationToken parameters for the generated params struct
    // and for deserialization at call time.
    let tool_params = collect_tool_params(&input_fn);

    let param_names: Vec<&syn::Ident> = tool_params.iter().map(|(n, _)| n).collect();
    let param_types: Vec<&Type> = tool_params.iter().map(|(_, t)| t).collect();

    let body = &input_fn.block;
    let fn_params: Vec<_> = input_fn.sig.inputs.iter().collect();

    // Build ordered call args matching the original function signature:
    // CancellationToken params → pass `__cancel`, others → pass `__p.field`.
    let call_args = build_call_args(&input_fn);

    quote! {
        // Hidden params struct with schemars + serde so schema_for works.
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        struct #params_struct_name {
            #(#param_names: #param_types,)*
        }

        pub struct #struct_name;

        impl #struct_name {
            fn _schema() -> serde_json::Value {
                swink_agent::schema_for::<#params_struct_name>()
            }
        }

        impl swink_agent::AgentTool for #struct_name {
            fn name(&self) -> &str { #tool_name }
            fn label(&self) -> &str { #tool_name }
            fn description(&self) -> &str { #tool_description }
            fn parameters_schema(&self) -> &serde_json::Value {
                static SCHEMA: std::sync::LazyLock<serde_json::Value> =
                    std::sync::LazyLock::new(#struct_name::_schema);
                &SCHEMA
            }
            fn execute(
                &self,
                _tool_call_id: &str,
                params: serde_json::Value,
                cancellation_token: tokio_util::sync::CancellationToken,
                _on_update: Option<Box<dyn Fn(swink_agent::AgentToolResult) + Send + Sync>>,
                _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
                _credential: Option<swink_agent::credential::ResolvedCredential>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = swink_agent::AgentToolResult> + Send + '_>> {
                Box::pin(async move {
                    #[allow(unused_variables)]
                    let __params = params;
                    #[allow(unused_variables)]
                    let __cancel = cancellation_token;

                    // Deserialize the whole params JSON into the generated struct.
                    let __p: #params_struct_name =
                        ::serde_json::from_value(__params).unwrap_or_else(|_| {
                            ::serde_json::from_value(::serde_json::Value::Object(
                                ::serde_json::Map::new()
                            )).unwrap_or_else(|_| panic!("failed to deserialize tool params"))
                        });

                    async fn #fn_name(#(#fn_params),*) -> swink_agent::AgentToolResult #body

                    // Call with args in the original parameter order:
                    // CancellationToken slots receive __cancel; others receive __p.field.
                    #fn_name(#(#call_args),*).await
                })
            }
        }
    }
}

fn parse_tool_attrs(attr: TokenStream) -> Result<(String, String), syn::Error> {
    let mut name = None;
    let mut description = None;

    let parser =
        syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated;
    let attrs = parser.parse2(attr)?;

    for nv in &attrs {
        let syn::Expr::Lit(expr_lit) = &nv.value else {
            continue;
        };
        let Lit::Str(s) = &expr_lit.lit else {
            continue;
        };
        if nv.path.is_ident("name") {
            name = Some(s.value());
        } else if nv.path.is_ident("description") {
            description = Some(s.value());
        }
    }

    Ok((
        name.unwrap_or_else(|| "unnamed_tool".to_string()),
        description.unwrap_or_else(|| "No description".to_string()),
    ))
}

fn pascal_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().to_string() + &chars.collect::<String>()
            })
        })
        .collect()
}

/// Build the ordered call-arg token list matching the original function signature.
///
/// `CancellationToken` parameters receive `__cancel` (the token from `execute`);
/// all other named parameters receive `__p.<name>` (deserialized from JSON).
fn build_call_args(input_fn: &ItemFn) -> Vec<TokenStream> {
    let mut args = Vec::new();
    for arg in &input_fn.sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
            continue;
        };
        if is_cancellation_token(&pat_type.ty) {
            args.push(quote! { __cancel });
        } else {
            let name = &pat_ident.ident;
            args.push(quote! { __p.#name });
        }
    }
    args
}

/// Collect (name, type) pairs for all non-`CancellationToken` named parameters.
fn collect_tool_params(input_fn: &ItemFn) -> Vec<(syn::Ident, Type)> {
    let mut params = Vec::new();
    for arg in &input_fn.sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
            continue;
        };
        if is_cancellation_token(&pat_type.ty) {
            continue;
        }
        params.push((pat_ident.ident.clone(), *pat_type.ty.clone()));
    }
    params
}

fn is_cancellation_token(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        tp.path
            .segments
            .last()
            .is_some_and(|s| s.ident == "CancellationToken")
    } else {
        false
    }
}
