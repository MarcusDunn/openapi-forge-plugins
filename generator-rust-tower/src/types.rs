//! IR `TypeRef` / `TypeDef` → Rust type expression.
//!
//! Conservative: anything we can't model faithfully falls back to
//! `serde_json::Value` with a `// TODO` comment so the spec author notices.

use forge_plugin_sdk::ir;

use crate::naming;

/// Resolve a `TypeRef` (string id into `Ir::types`) to a Rust type
/// expression. Looks up the named type and consults its `TypeDef`.
pub fn type_ref_to_rust(spec: &ir::Ir, type_ref: &str, models_path: &str) -> String {
    if type_ref == ir::NULL_ID {
        return "serde_json::Value".to_string();
    }
    let Some(named) = spec.types.iter().find(|t| t.id == type_ref) else {
        return format!("serde_json::Value /* TODO unknown type ref `{type_ref}` */");
    };
    match &named.definition {
        // Named types we emit as `struct` / `enum` resolve to a name under
        // `models::`. When `models_path` is empty, callers are *inside*
        // `models.rs` and sibling types resolve by bare name.
        ir::TypeDef::Object(_) | ir::TypeDef::EnumString(_) | ir::TypeDef::EnumInt(_) => {
            let name = naming::rust_type_name(named);
            if models_path.is_empty() {
                name
            } else {
                format!("{models_path}::{name}")
            }
        }
        ir::TypeDef::Primitive(p) => primitive_to_rust(p),
        ir::TypeDef::Array(a) => {
            format!("Vec<{}>", type_ref_to_rust(spec, &a.items, models_path))
        }
        ir::TypeDef::Union(u) => union_to_rust(spec, u, models_path),
        ir::TypeDef::Null => "serde_json::Value".to_string(),
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
/// becomes `Option<T>`. Everything else is `serde_json::Value` + TODO.
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
    format!(
        "serde_json::Value /* TODO union (variants: {}) */",
        u.variants.len()
    )
}
