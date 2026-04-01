//! Implementation of `#[derive(ToolSchema)]`.
//!
//! Generates a `ToolParameters` implementation that returns a JSON Schema
//! for the annotated struct.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Lit, Meta, Type};

pub fn derive_tool_schema_impl(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;

    let Data::Struct(data_struct) = &input.data else {
        return syn::Error::new_spanned(&input.ident, "ToolSchema can only be derived for structs")
            .to_compile_error();
    };

    let Fields::Named(fields) = &data_struct.fields else {
        return syn::Error::new_spanned(&input.ident, "ToolSchema requires named fields")
            .to_compile_error();
    };

    let mut property_tokens = Vec::new();
    let mut required_tokens = Vec::new();

    for field in &fields.named {
        let field_name = field.ident.as_ref().expect("named field");
        let field_name_str = field_name.to_string();

        let description = get_tool_description(field).or_else(|| get_doc_comment(field));

        let Some((json_type, is_option)) = map_type(&field.ty) else {
            return syn::Error::new_spanned(
                &field.ty,
                format!(
                    "Unsupported type for ToolSchema. Supported: String, integers, floats, \
                     bool, Option<T>, Vec<T>. Found field `{field_name_str}`."
                ),
            )
            .to_compile_error();
        };

        let desc_token = description.map_or_else(
            || quote! {},
            |desc| quote! { prop.insert("description".to_string(), serde_json::Value::String(#desc.to_string())); },
        );

        let type_token = if json_type == "array" {
            let inner_type = vec_inner_json_type(&field.ty).unwrap_or("string");
            quote! {
                prop.insert("type".to_string(), serde_json::Value::String("array".to_string()));
                {
                    let mut items = serde_json::Map::new();
                    items.insert("type".to_string(), serde_json::Value::String(#inner_type.to_string()));
                    prop.insert("items".to_string(), serde_json::Value::Object(items));
                }
            }
        } else {
            quote! {
                prop.insert("type".to_string(), serde_json::Value::String(#json_type.to_string()));
            }
        };

        property_tokens.push(quote! {
            {
                let mut prop = serde_json::Map::new();
                #type_token
                #desc_token
                properties.insert(#field_name_str.to_string(), serde_json::Value::Object(prop));
            }
        });

        if !is_option {
            required_tokens.push(quote! {
                required.push(serde_json::Value::String(#field_name_str.to_string()));
            });
        }
    }

    quote! {
        impl swink_agent::tool::ToolParameters for #name {
            fn json_schema() -> serde_json::Value {
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
    }
}

/// Extract `#[tool(description = "...")]` attribute.
fn get_tool_description(field: &syn::Field) -> Option<String> {
    for attr in &field.attrs {
        if !attr.path().is_ident("tool") {
            continue;
        }
        let Meta::List(meta_list) = &attr.meta else {
            continue;
        };
        let Ok(name_value) = syn::parse2::<syn::MetaNameValue>(meta_list.tokens.clone()) else {
            continue;
        };
        if !name_value.path.is_ident("description") {
            continue;
        }
        if let syn::Expr::Lit(expr_lit) = &name_value.value
            && let Lit::Str(lit_str) = &expr_lit.lit
        {
            return Some(lit_str.value());
        }
    }
    None
}

/// Extract doc comment from `/// ...` attributes.
fn get_doc_comment(field: &syn::Field) -> Option<String> {
    let lines: Vec<String> = field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| {
            if let Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(expr_lit) = &nv.value
                && let Lit::Str(lit_str) = &expr_lit.lit
            {
                Some(lit_str.value().trim().to_string())
            } else {
                None
            }
        })
        .collect();

    if lines.is_empty() { None } else { Some(lines.join(" ")) }
}

/// Map a Rust type to `(json_schema_type, is_option)`. Returns `None` for unsupported types.
fn map_type(ty: &Type) -> Option<(&'static str, bool)> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let last = type_path.path.segments.last()?;
    let ident = last.ident.to_string();

    match ident.as_str() {
        "String" | "str" => Some(("string", false)),
        "bool" => Some(("boolean", false)),
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
        | "i128" | "isize" => Some(("integer", false)),
        "f32" | "f64" => Some(("number", false)),
        "Option" => {
            if let syn::PathArguments::AngleBracketed(args) = &last.arguments
                && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
            {
                let inner = map_type(inner_ty).map_or("string", |(t, _)| t);
                return Some((inner, true));
            }
            Some(("string", true))
        }
        "Vec" => Some(("array", false)),
        _ => None,
    }
}

/// Get the inner JSON type for `Vec<T>`.
fn vec_inner_json_type(ty: &Type) -> Option<&'static str> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let last = type_path.path.segments.last()?;
    if last.ident != "Vec" {
        return None;
    }
    if let syn::PathArguments::AngleBracketed(args) = &last.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return map_type(inner).map(|(t, _)| t);
    }
    Some("string")
}
