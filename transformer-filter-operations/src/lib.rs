//! `transformer-filter-operations` — drops operations whose `operationId`
//! isn't in the configured `keep` list, then prunes `Ir::types` down to the
//! transitive closure of `TypeRef`s reachable from the survivors.
//!
//! Useful when a consumer needs only a small slice of a huge spec (a Rust
//! lambda that hits one of 400+ documented endpoints, say). The downstream
//! generator's emitted code stays focused on operations + types the caller
//! actually uses, instead of dragging in everything from the source spec.
//!
//! Matches operations by `original_id` first (the literal `operationId`
//! from the spec), then by `id` (the sanitized canonical id) as a
//! fallback. Anything in `keep` that doesn't hit either pool surfaces as a
//! `warning`-severity diagnostic — the transform still succeeds, leaving
//! the caller to decide whether unmatched ids are typos or stale config.

#![forbid(unsafe_code)]

use forge_plugin_sdk::convert::transformer as conv;
use forge_plugin_sdk::ir;
use forge_plugin_sdk::transformer::exports::forge::plugin::transformer_api::{
    Guest, TransformOutput as WitTransformOutput,
};
use forge_plugin_sdk::transformer::forge::plugin::stage::StageError;
use forge_plugin_sdk::transformer::forge::plugin::types::{
    Ir as WitIr, PluginInfo as WitPluginInfo,
};
use std::collections::{BTreeSet, HashSet, VecDeque};

mod walk;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub keep: Vec<String>,
}

/// Pure entry point — operates on the canonical IR. Returns the filtered
/// IR plus diagnostics for unmatched ids.
pub fn transform(mut spec: ir::Ir, cfg: &Config) -> forge_plugin_sdk::TransformOutput {
    let requested: BTreeSet<&str> = cfg.keep.iter().map(String::as_str).collect();

    // 1. Pick the operations whose original_id or id is in `keep`.
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let kept_ops: Vec<ir::Operation> = std::mem::take(&mut spec.operations)
        .into_iter()
        .filter(|op| {
            let by_original = op
                .original_id
                .as_deref()
                .map(|s| requested.contains(s))
                .unwrap_or(false);
            let by_id = requested.contains(op.id.as_str());
            let hit = by_original || by_id;
            if hit {
                if let Some(orig) = op.original_id.as_deref() {
                    matched.insert(orig.to_string());
                }
                matched.insert(op.id.clone());
            }
            hit
        })
        .collect();

    // 2. Diagnostics for any requested id that didn't land.
    let mut diagnostics = Vec::new();
    for requested_id in &cfg.keep {
        if !matched.contains(requested_id.as_str()) {
            diagnostics.push(ir::Diagnostic {
                severity: ir::Severity::Warning,
                code: "transformer-filter-operations/W-NO-MATCH".to_string(),
                message: format!(
                    "no operation matched `keep` id `{requested_id}` — typo or stale config?"
                ),
                location: None,
                related: Vec::new(),
                suggested_fix: None,
            });
        }
    }

    // 3. Walk the type graph from each kept op; collect every reachable id.
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for op in &kept_ops {
        walk::seed_from_operation(op, &mut queue);
    }
    let type_by_id: std::collections::HashMap<&str, &ir::NamedType> =
        spec.types.iter().map(|t| (t.id.as_str(), t)).collect();
    while let Some(type_ref) = queue.pop_front() {
        if !reachable.insert(type_ref.clone()) {
            continue;
        }
        if let Some(named) = type_by_id.get(type_ref.as_str()) {
            walk::seed_from_definition(&named.definition, &mut queue);
        }
    }

    // 4. Prune `spec.types` to only reachable entries; preserve order.
    let kept_types: Vec<ir::NamedType> = std::mem::take(&mut spec.types)
        .into_iter()
        .filter(|t| reachable.contains(&t.id))
        .collect();

    spec.operations = kept_ops;
    spec.types = kept_types;
    forge_plugin_sdk::TransformOutput { spec, diagnostics }
}

struct FilterOperations;

impl Guest for FilterOperations {
    fn info() -> WitPluginInfo {
        conv::plugin_info_to_wit(ir::PluginInfo {
            name: "transformer-filter-operations".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    fn config_schema() -> String {
        include_str!("../schema.json").into()
    }

    fn transform(spec: WitIr, config: String) -> Result<WitTransformOutput, StageError> {
        let cfg: Config =
            serde_json::from_str(&config).map_err(|e| conv::config_invalid(e.to_string()))?;
        let canonical = conv::ir_from_wit(spec);
        let out = transform(canonical, &cfg);
        Ok(conv::transform_output_to_wit(out))
    }
}

forge_plugin_sdk::transformer::export!(FilterOperations with_types_in forge_plugin_sdk::transformer);
