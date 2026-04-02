//! Implementation of the `#[tool]` attribute macro.
//!
//! Generates a struct implementing `AgentTool` from an async function.

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

    let (param_names, property_tokens, required_tokens) = extract_params(&input_fn);

    let body = &input_fn.block;
    let fn_params: Vec<_> = input_fn.sig.inputs.iter().collect();

    quote! {
        pub struct #struct_name;

        impl #struct_name {
            fn _schema() -> serde_json::Value {
                let mut properties = serde_json::Map::new();
                let mut required = Vec::new();
                #(#property_tokens)*
                #(#required_tokens)*
                let mut schema = serde_json::Map::new();
                schema.insert("type".to_string(), serde_json::Value::String("object".to_string()));
                schema.insert("properties".to_string(), serde_json::Value::Object(properties));
                if !required.is_empty() {
                    schema.insert("required".to_string(), serde_json::Value::Array(required));
                }
                serde_json::Value::Object(schema)
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

                    async fn #fn_name(#(#fn_params),*) -> swink_agent::AgentToolResult #body

                    #fn_name(
                        #(
                            serde_json::from_value(
                                __params.get(stringify!(#param_names))
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null)
                            ).unwrap_or_default()
                        ),*
                    ).await
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

fn extract_params(input_fn: &ItemFn) -> (Vec<syn::Ident>, Vec<TokenStream>, Vec<TokenStream>) {
    let mut param_names = Vec::new();
    let mut property_tokens = Vec::new();
    let mut required_tokens = Vec::new();

    for arg in &input_fn.sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
            continue;
        };
        let param_name = &pat_ident.ident;
        let param_name_str = param_name.to_string();

        if is_cancellation_token(&pat_type.ty) {
            continue;
        }

        let json_type = rust_type_to_json_type(&pat_type.ty);
        let is_option = is_option_type(&pat_type.ty);

        param_names.push(param_name.clone());

        property_tokens.push(quote! {
            {
                let mut prop = serde_json::Map::new();
                prop.insert("type".to_string(), serde_json::Value::String(#json_type.to_string()));
                properties.insert(#param_name_str.to_string(), serde_json::Value::Object(prop));
            }
        });

        if !is_option {
            required_tokens.push(quote! {
                required.push(serde_json::Value::String(#param_name_str.to_string()));
            });
        }
    }

    (param_names, property_tokens, required_tokens)
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

fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        tp.path.segments.last().is_some_and(|s| s.ident == "Option")
    } else {
        false
    }
}

fn rust_type_to_json_type(ty: &Type) -> &'static str {
    let Type::Path(type_path) = ty else {
        return "string";
    };
    let Some(last) = type_path.path.segments.last() else {
        return "string";
    };
    match last.ident.to_string().as_str() {
        "bool" => "boolean",
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" => "integer",
        "f32" | "f64" => "number",
        "Vec" => "array",
        // String, str, Option, and any unknown types default to "string"
        _ => "string",
    }
}
