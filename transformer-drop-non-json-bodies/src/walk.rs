//! Type-graph walker: enumerates every `TypeRef` reachable from an
//! `Operation` (params / request body / responses / response headers) and
//! from a `TypeDef` (object properties, array items, union variants,
//! `additionalProperties`, etc.). The transformer uses these helpers to
//! prune `Ir::types` down to a transitive-closure subset after dropping
//! non-JSON-body operations.
//!
//! Duplicated from `transformer-filter-operations` — the two transformers
//! both need this BFS and there's no shared utility crate yet. If a
//! third transformer needs it, lift to one.

use forge_plugin_sdk::ir;
use std::collections::VecDeque;

pub fn seed_from_operation(op: &ir::Operation, queue: &mut VecDeque<String>) {
    for p in op
        .path_params
        .iter()
        .chain(&op.query_params)
        .chain(&op.header_params)
        .chain(&op.cookie_params)
        .chain(&op.querystring_params)
    {
        queue.push_back(p.r#type.clone());
    }
    if let Some(body) = &op.request_body {
        for c in &body.content {
            queue.push_back(c.r#type.clone());
            if let Some(item) = &c.item_schema {
                queue.push_back(item.clone());
            }
        }
    }
    for resp in &op.responses {
        for c in &resp.content {
            queue.push_back(c.r#type.clone());
            if let Some(item) = &c.item_schema {
                queue.push_back(item.clone());
            }
        }
        for (_name, hdr) in &resp.headers {
            queue.push_back(hdr.r#type.clone());
        }
    }
}

pub fn seed_from_definition(def: &ir::TypeDef, queue: &mut VecDeque<String>) {
    match def {
        ir::TypeDef::Primitive(p) => {
            if let Some(content_schema) = &p.constraints.content_schema {
                queue.push_back(content_schema.clone());
            }
        }
        ir::TypeDef::Object(o) => {
            for prop in &o.properties {
                queue.push_back(prop.r#type.clone());
            }
            if let ir::AdditionalProperties::Typed { r#type } = &o.additional_properties {
                queue.push_back(r#type.clone());
            }
        }
        ir::TypeDef::Array(a) => {
            queue.push_back(a.items.clone());
        }
        ir::TypeDef::Union(u) => {
            for variant in &u.variants {
                queue.push_back(variant.r#type.clone());
            }
        }
        ir::TypeDef::EnumString(_) | ir::TypeDef::EnumInt(_) | ir::TypeDef::Null => {}
    }
}
