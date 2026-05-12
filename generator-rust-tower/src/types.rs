//! IR `TypeRef` / `TypeDef` → Rust type expression.
//!
//! Conservative: anything we can't model faithfully falls back to
//! `serde_json::Value`. We used to inline `/* TODO ... */` comments at the
//! type position; reviewers correctly pointed out this is the worst of both
//! worlds — code compiles, "looks fine," silently lossy. Use diagnostics or
//! refuse to emit instead.

use forge_plugin_sdk::diag;
use forge_plugin_sdk::ir;

use crate::diagnostics;
use crate::naming;

/// Resolve a `TypeRef` (string id into `Ir::types`) to a Rust type
/// expression. Looks up the named type and consults its `TypeDef`.
pub fn type_ref_to_rust(spec: &ir::Ir, type_ref: &str, models_path: &str) -> String {
    if type_ref == ir::NULL_ID {
        diagnostics::report(diag::warning(
            "rust-tower/type-fallback-null",
            "bare `null`-typed reference at use site; emitting `serde_json::Value`",
        ));
        return "serde_json::Value".to_string();
    }
    let Some(named) = spec.types.iter().find(|t| t.id == type_ref) else {
        diagnostics::report(diag::warning(
            "rust-tower/type-fallback-unresolved",
            format!("unresolved type reference `{type_ref}`; emitting `serde_json::Value`"),
        ));
        return "serde_json::Value".to_string();
    };
    match &named.definition {
        ir::TypeDef::Object(o) => {
            // Objects with only `additionalProperties` and no named fields
            // are maps. Inline them as `HashMap<String, T>` at the use site
            // — emitting a one-field newtype struct forces every caller
            // through a `.additional.get(...)` dance for no semantic gain.
            if let Some(value_ty) = additional_properties_only(o) {
                let inner = type_ref_to_rust(spec, value_ty, models_path);
                return format!("std::collections::HashMap<String, {inner}>");
            }
            named_type_path(models_path, &naming::rust_type_name(named))
        }
        ir::TypeDef::EnumString(_) | ir::TypeDef::EnumInt(_) => {
            named_type_path(models_path, &naming::rust_type_name(named))
        }
        ir::TypeDef::Primitive(p) => primitive_to_rust(p),
        ir::TypeDef::Array(a) => {
            format!("Vec<{}>", type_ref_to_rust(spec, &a.items, models_path))
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
            "serde_json::Value".to_string()
        }
    }
}

fn named_type_path(models_path: &str, name: &str) -> String {
    if models_path.is_empty() {
        name.to_string()
    } else {
        format!("{models_path}::{name}")
    }
}

/// Detect "this object has no named properties; it's just a typed map."
/// Returns the value `TypeRef` if so.
///
/// The JSON-Schema shape is `{ "type": "object", "additionalProperties": <X> }`
/// with no `properties`. We translate that to `HashMap<String, X>` at the
/// use site (see [`type_ref_to_rust`]) so callers don't have to walk through
/// a one-field newtype.
pub fn additional_properties_only(o: &ir::ObjectType) -> Option<&str> {
    if !o.properties.is_empty() {
        return None;
    }
    match &o.additional_properties {
        ir::AdditionalProperties::Typed { r#type } => Some(r#type.as_str()),
        _ => None,
    }
}

fn primitive_to_rust(p: &ir::PrimitiveType) -> String {
    // `format_extension` could refine the type (`int32` → `i32`, `int64` →
    // `i64`, `uuid` → `String`, `date-time` → `String`/`chrono::DateTime`,
    // `binary` → `Vec<u8>`). We stick with broad mappings — anything richer
    // needs a per-target opinion and the consumer can write a follow-up
    // transformer.
    match p.kind {
        ir::PrimitiveKind::String => "String".to_string(),
        ir::PrimitiveKind::Integer => match p.constraints.format_extension.as_deref() {
            Some("int32") => "i32".to_string(),
            _ => "i64".to_string(),
        },
        ir::PrimitiveKind::Number => match p.constraints.format_extension.as_deref() {
            Some("float") => "f32".to_string(),
            _ => "f64".to_string(),
        },
        ir::PrimitiveKind::Bool => "bool".to_string(),
    }
}

/// `T | null` (the only union shape this generator models faithfully)
/// becomes `Option<T>`. Everything else is `serde_json::Value`.
fn union_to_rust(spec: &ir::Ir, u: &ir::UnionType, models_path: &str) -> String {
    if u.variants.len() == 2 {
        let null_pos = u.variants.iter().position(|v| v.r#type == ir::NULL_ID);
        if let Some(idx) = null_pos {
            let other = &u.variants[1 - idx];
            let inner = type_ref_to_rust(spec, &other.r#type, models_path);
            // Avoid `Option<Option<...>>` if the inner already nests.
            return if inner.starts_with("Option<") {
                inner
            } else {
                format!("Option<{inner}>")
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
    "serde_json::Value".to_string()
}
