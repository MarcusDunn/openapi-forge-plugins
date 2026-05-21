//! IR `TypeRef` / `TypeDef` → Rust type expression as a `TokenStream`.
//!
//! Strict by default: anything we can't model faithfully reports a *fatal*
//! diagnostic through [`crate::diagnostics::report_fatal`] and the
//! generator returns a `StageError::Rejected` from `emit::all`.
//!
//! Unions in particular: the canonical `T | null` shape lowers to
//! `Option<T>` inline at the use site; anything else must be a
//! user-named union, which becomes a `#[serde(untagged)]` enum in
//! `models.rs` and is referred to by name from use sites. See
//! [`crate::models::render_union_enum`] for the alias/enum split.

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
        diagnostics::report_fatal(diag::error(
            "rust-serde/type-fallback-null",
            "bare `null`-typed reference at use site has no Rust representation",
        ));
        return quote! { serde_json::Value };
    }
    let Some(named) = spec.types.iter().find(|t| t.id == type_ref) else {
        diagnostics::report_fatal(diag::error(
            "rust-serde/type-fallback-unresolved",
            format!("unresolved type reference `{type_ref}`"),
        ));
        return quote! { serde_json::Value };
    };
    match &named.definition {
        ir::TypeDef::Object(o) => {
            // Objects with no named fields fall into three groups; all
            // three inline at the use site rather than minting a struct
            // (a one-field newtype just forces every caller through a
            // `.additional.get(...)` dance for no semantic gain, and the
            // fieldless variants silently swallow the wire body):
            //
            //  - Typed{T} → `HashMap<String, T>` (keys arbitrary, values
            //    are a known schema).
            //  - Any      → `serde_json::Value`. The IR has no "any JSON"
            //    primitive, so the parser lowers a bare `{}` schema (and
            //    `additionalProperties: true`) to `Object{props:[],
            //    AP=Any}`. That shape carries *no* constraint that values
            //    be objects — a string or number on the wire is just as
            //    valid — so a `HashMap<String, Value>` would over-restrict
            //    and reject those wires at deserialize time.
            //  - Forbidden → `serde_json::Value`. The literal reading is
            //    "only the empty object," but in practice spec authors
            //    reach for `additionalProperties: false` (with no
            //    `properties` block) when they mean *"don't care, pass
            //    it through"* and never check that the result is `{}`.
            //    Emitting a fieldless `pub struct Foo {}` deserializes
            //    every wire payload into nothing and silently drops the
            //    body (issue #26); `Value` surfaces what the server
            //    actually sent, mirroring the resolution of #20.
            if let Some(value_ty) = additional_properties_only(o) {
                return match value_ty {
                    AdditionalMapValue::Typed(t) => {
                        let inner = type_ref_to_rust(spec, t, models_path);
                        quote! { std::collections::HashMap<String, #inner> }
                    }
                    AdditionalMapValue::Any | AdditionalMapValue::Forbidden => {
                        quote! { serde_json::Value }
                    }
                };
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
        ir::TypeDef::Union(u) => {
            // `T | null` collapses to `Option<T>` and can be inlined at any
            // use site (or used as the RHS of a `pub type` alias).
            if let Some(inner) = nullable_inner(spec, u, models_path) {
                return inner;
            }
            // Multi-variant unions become a `#[serde(untagged)]` enum at
            // the def site. User-named unions use their original name;
            // synthesized inline unions get hoisted under the parser's
            // assigned id (e.g. `<op>_response_<code>_variant_<n>`) and
            // emit a warning so the consumer knows the name is generator-
            // derived rather than from their spec.
            if named.original_name.is_none() {
                let variants: Vec<&str> = u.variants.iter().map(|v| v.r#type.as_str()).collect();
                diagnostics::report(diag::warning(
                    "rust-serde/inline-union-synthesized",
                    format!(
                        "inline multi-variant union of {} variants {:?} has no schema name; \
                         hoisting to a Rust enum named `{}`. Promote this `oneOf` to a named \
                         schema in `components/schemas` to control the type name.",
                        u.variants.len(),
                        variants,
                        naming::rust_type_name(named),
                    ),
                ));
            }
            named_type_path(models_path, &naming::rust_type_name(named))
        }
        ir::TypeDef::Null => {
            diagnostics::report_fatal(diag::error(
                "rust-serde/type-fallback-null-def",
                format!("named type `{}` resolves to `null`", named.id),
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

/// Value-shape marker for an object that has no named properties.
/// [`additional_properties_only`] returns one of these when an object's
/// `properties` list is empty:
///  - `Typed(T)` — typed map; resolves through the spec to a concrete
///    Rust type at the use site (`HashMap<String, T>`).
///  - `Any` — `additionalProperties: {}` or `: true`; becomes
///    `serde_json::Value` without a type-table lookup, since the IR
///    permits any JSON value here (string, number, array, object…).
///  - `Forbidden` — `additionalProperties: false` with no properties.
///    Literally this means "only the empty object," but in practice
///    spec authors use it as a passthrough; lowering to
///    `serde_json::Value` matches that intent and avoids the
///    silent-body-drop in issue #26.
pub enum AdditionalMapValue<'a> {
    Typed(&'a str),
    Any,
    Forbidden,
}

/// Detect "this object has no named properties; it's a map-shaped or
/// otherwise-empty type that should inline at the use site." Returns a
/// shape marker if so, `None` if there are real fields to render.
///
/// The JSON-Schema shapes covered:
///  - `{ "type": "object", "additionalProperties": <X> }` — typed map.
///  - `{ "type": "object", "additionalProperties": {} | true }` — any
///    JSON value, since the IR permits non-object wires here.
///  - `{ "type": "object", "additionalProperties": false }` — closed
///    empty. Spec-literally this is "only `{}`," but emitting it as a
///    fieldless struct silently swallows whatever the server actually
///    sends (issue #26). The pragmatic call — matching the resolution
///    direction of #20 — is to surface the wire as `serde_json::Value`
///    and let the caller see it.
pub fn additional_properties_only(o: &ir::ObjectType) -> Option<AdditionalMapValue<'_>> {
    if !o.properties.is_empty() {
        return None;
    }
    match &o.additional_properties {
        ir::AdditionalProperties::Typed { r#type } => {
            Some(AdditionalMapValue::Typed(r#type.as_str()))
        }
        ir::AdditionalProperties::Any => Some(AdditionalMapValue::Any),
        ir::AdditionalProperties::Forbidden => Some(AdditionalMapValue::Forbidden),
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

/// If `u` is the two-variant `T | null` shape, return the Rust
/// `Option<T>` (or `T` if `T` is already an `Option`). Otherwise `None`.
pub fn nullable_inner(
    spec: &ir::Ir,
    u: &ir::UnionType,
    models_path: &ModelsPath,
) -> Option<TokenStream> {
    if u.variants.len() != 2 {
        return None;
    }
    let null_pos = u.variants.iter().position(|v| v.r#type == ir::NULL_ID)?;
    let other = &u.variants[1 - null_pos];
    let inner = type_ref_to_rust(spec, &other.r#type, models_path);
    Some(if is_option(&inner) {
        inner
    } else {
        quote! { Option<#inner> }
    })
}

/// Variant identifier for a member of a `#[serde(untagged)]` enum.
///
/// Untagged means the wire never sees these names — they're an
/// ergonomics choice. Strategy:
///  - if the variant's named type has an `original_name`, Pascal-case it;
///  - else derive from the kind (`String`, `Integer`, `Number`, `Bool`,
///    `Array`, `Object`, `Null`, `Enum`);
///  - the caller is responsible for de-duplication.
pub fn variant_ident_for(spec: &ir::Ir, type_ref: &str) -> String {
    if type_ref == ir::NULL_ID {
        return "Null".into();
    }
    let Some(named) = spec.types.iter().find(|t| t.id == type_ref) else {
        return "Variant".into();
    };
    if let Some(orig) = &named.original_name {
        return naming::pascal_case(orig);
    }
    match &named.definition {
        ir::TypeDef::Primitive(p) => match p.kind {
            ir::PrimitiveKind::String => "String".into(),
            ir::PrimitiveKind::Integer => "Integer".into(),
            ir::PrimitiveKind::Number => "Number".into(),
            ir::PrimitiveKind::Bool => "Bool".into(),
        },
        ir::TypeDef::Array(_) => "Array".into(),
        ir::TypeDef::Object(o) => {
            if additional_properties_only(o).is_some() {
                "Object".into()
            } else {
                naming::pascal_case(&named.id)
            }
        }
        ir::TypeDef::EnumString(_) | ir::TypeDef::EnumInt(_) | ir::TypeDef::Union(_) => {
            naming::pascal_case(&named.id)
        }
        ir::TypeDef::Null => "Null".into(),
    }
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
