//! Decides which `NamedType`s warrant their own schema page.
//!
//! The IR explodes every property, parameter, response shape, and
//! union variant into its own `NamedType` so each can carry
//! independent docs. On a real-world spec that yields thousands of
//! synthetic entries — every `<schema>_property_<field>`,
//! `<op>_param_query_<name>`, `<op>_response_<status>`, etc.
//! Rendering one page per entry blows the wasm fuel budget and
//! produces near-empty pages whose content is already shown inline
//! on the parent's page.
//!
//! Same predicate gates schema-page emission (`emit.rs`) and
//! cross-reference link rendering (`render.rs`), so we never link
//! to a page we didn't emit.

use forge_plugin_sdk::ir::{NamedType, TypeDef};

/// True when this type deserves a dedicated `schemas/<id>.html` page.
pub fn is_user_facing(t: &NamedType) -> bool {
    match &t.definition {
        // Trivial leaf types render inline at their use sites (see
        // `render::render_typeref`); a dedicated page would be near-empty and
        // is never linked to.
        TypeDef::Primitive(_) | TypeDef::Null | TypeDef::Any => return false,
        _ => {}
    }
    !is_synthetic_id(&t.id)
}

/// True when this type id matches the IR's documented synthetic
/// naming patterns. Public so `render.rs` can suppress dead links.
pub fn is_synthetic_id(id: &str) -> bool {
    const MARKERS: &[&str] = &[
        "_property_",
        "_param_",
        "_response_",
        "_request_",
        "_variant_",
        "_part_",
        "_items",
        "_fallback",
        "_switchBranch",
    ];
    MARKERS.iter().any(|m| id.contains(m))
}
