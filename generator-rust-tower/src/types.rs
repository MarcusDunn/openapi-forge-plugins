//! IR `TypeRef` / `TypeDef` → Rust type expression as a `TokenStream`.
//!
//! Conservative: anything we can't model faithfully falls back to
//! `serde_json::Value` *and* emits a [`Diagnostic`](forge_plugin_sdk::ir::Diagnostic)
//! through [`crate::diagnostics`] so the consumer sees what got dropped.

use forge_plugin_sdk::diag;
use forge_plugin_sdk::ir;
use proc_macro2::{Span, TokenStream, TokenTree};
use quote::quote;

use crate::diagnostics;
use crate::naming;

/// Token-tree path prefix for named types. Empty inside `models.rs`
/// (sibling types resolve by bare name); `super::super::models` inside
/// per-operation modules.
pub type ModelsPath = TokenStream;

/// Resolve a `TypeRef` (string id into `Ir::types`) to a Rust type
/// expression. Looks up the named type and consults its `TypeDef`.
pub fn type_ref_to_rust(spec: &ir::Ir, type_ref: &str, models_path: &ModelsPath) -> TokenStream {
    if type_ref == ir::NULL_ID {
        diagnostics::report(diag::warning(
            "rust-tower/type-fallback-null",
            "bare `null`-typed reference at use site; emitting `serde_json::Value`",
        ));
        return quote! { serde_json::Value };
    }
    let Some(named) = spec.types.iter().find(|t| t.id == type_ref) else {
        diagnostics::report(diag::warning(
            "rust-tower/type-fallback-unresolved",
            format!("unresolved type reference `{type_ref}`; emitting `serde_json::Value`"),
        ));
        return quote! { serde_json::Value };
    };
    match &named.definition {
        ir::TypeDef::Object(o) => {
            // Objects with only `additionalProperties` and no named fields
            // are maps. Inline them as `HashMap<String, T>` at the use site
            // — emitting a one-field newtype struct forces every caller
            // through a `.additional.get(...)` dance for no semantic gain.
            if let Some(value_ty) = additional_properties_only(o) {
                let inner = type_ref_to_rust(spec, value_ty, models_path);
                return quote! { std::collections::HashMap<String, #inner> };
            }
            named_type_path(models_path, &naming::rust_type_name(named))
        }
        ir::TypeDef::EnumString(_) | ir::TypeDef::EnumInt(_) => {
            named_type_path(models_path, &naming::rust_type_name(named))
        }
        ir::TypeDef::Primitive(p) => primitive_to_rust(p),
        ir::TypeDef::Array(a) => {
            let inner = type_ref_to_rust(spec, &a.items, models_path);
            quote! { Vec<#inner> }
        }
        ir::TypeDef::Union(u) => union_to_rust(spec, u, models_path),
        ir::TypeDef::Null => {
            diagnostics::report(diag::warning(
                "rust-tower/type-fallback-null-def",
                format!(
                    "named type `{}` resolves to `null`; emitting `serde_json::Value`",
                    named.id
                ),
            ));
            quote! { serde_json::Value }
        }
    }
}

fn named_type_path(models_path: &ModelsPath, name: &str) -> TokenStream {
    let ident = ident(name);
    if models_path.is_empty() {
        quote! { #ident }
    } else {
        quote! { #models_path::#ident }
    }
}

/// Detect "this object has no named properties; it's just a typed map."
/// Returns the value `TypeRef` if so. The JSON-Schema shape is
/// `{ "type": "object", "additionalProperties": <X> }` with no
/// `properties`; we translate that to `HashMap<String, X>` at the use site.
pub fn additional_properties_only(o: &ir::ObjectType) -> Option<&str> {
    if !o.properties.is_empty() {
        return None;
    }
    match &o.additional_properties {
        ir::AdditionalProperties::Typed { r#type } => Some(r#type.as_str()),
        _ => None,
    }
}

fn primitive_to_rust(p: &ir::PrimitiveType) -> TokenStream {
    // `format_extension` could refine the type further (`uuid`, `date-time`,
    // `binary`); we stick with broad mappings — anything richer needs a
    // per-target opinion and the consumer can write a follow-up transformer.
    match p.kind {
        ir::PrimitiveKind::String => quote! { String },
        ir::PrimitiveKind::Integer => match p.constraints.format_extension.as_deref() {
            Some("int32") => quote! { i32 },
            _ => quote! { i64 },
        },
        ir::PrimitiveKind::Number => match p.constraints.format_extension.as_deref() {
            Some("float") => quote! { f32 },
            _ => quote! { f64 },
        },
        ir::PrimitiveKind::Bool => quote! { bool },
    }
}

/// `T | null` (the only union shape this generator models faithfully)
/// becomes `Option<T>`. Everything else is `serde_json::Value`.
fn union_to_rust(spec: &ir::Ir, u: &ir::UnionType, models_path: &ModelsPath) -> TokenStream {
    if u.variants.len() == 2 {
        let null_pos = u.variants.iter().position(|v| v.r#type == ir::NULL_ID);
        if let Some(idx) = null_pos {
            let other = &u.variants[1 - idx];
            let inner = type_ref_to_rust(spec, &other.r#type, models_path);
            // Avoid `Option<Option<...>>` if the inner already nests.
            return if is_option(&inner) {
                inner
            } else {
                quote! { Option<#inner> }
            };
        }
    }
    let variants: Vec<&str> = u.variants.iter().map(|v| v.r#type.as_str()).collect();
    diagnostics::report(diag::warning(
        "rust-tower/type-fallback-union",
        format!(
            "union of {} variants {:?} not modeled (only `T | null` is); emitting `serde_json::Value`",
            u.variants.len(),
            variants
        ),
    ));
    quote! { serde_json::Value }
}

/// Cheap check: is the first token of `ts` the identifier `Option`?
/// Used to short-circuit `Option<Option<T>>` when a property is both
/// `not required` *and* schema-nullable.
pub fn is_option(ts: &TokenStream) -> bool {
    matches!(ts.clone().into_iter().next(), Some(TokenTree::Ident(id)) if id == "Option")
}

/// Build a possibly-raw `Ident` from a name. `naming::snake_case` returns
/// `"r#type"` for Rust-keyword field names; `proc_macro2::Ident::new` rejects
/// `"r#type"` so we route through `new_raw` after stripping the prefix.
pub fn ident(s: &str) -> proc_macro2::Ident {
    if let Some(stripped) = s.strip_prefix("r#") {
        proc_macro2::Ident::new_raw(stripped, Span::call_site())
    } else {
        proc_macro2::Ident::new(s, Span::call_site())
    }
}

/// Render a `name = name` binding for `format!` named args.
pub fn format_arg(name: &proc_macro2::Ident) -> TokenStream {
    quote! { #name = #name }
}
