//! `transformer-drop-non-json-bodies` — drops operations whose request
//! body has no JSON-shaped content type, then prunes `Ir::types` down to
//! the transitive closure of `TypeRef`s reachable from the survivors.
//!
//! Pair with JSON-only generators (e.g. `generator-rust-tower`) so a
//! mixed spec — JSON endpoints plus multipart uploads, image PUTs,
//! `text/csv` ingests — generates cleanly with the upload-shaped ops
//! filtered out. Each dropped op surfaces as an `info`-severity
//! diagnostic so the caller knows what was removed.
//!
//! "JSON-shaped" by default: `application/json` (case-insensitive,
//! ignoring media-type parameters) and `application/<x>+json` structured-
//! syntax suffix. Vendor-specific JSON dialects (`application/x-amz-json-
//! 1.1`, etc.) can be added via `extra_json_media_types` in the config.
//!
//! Operations with no request body are kept unconditionally — there's
//! nothing to reject.

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
use std::collections::{HashSet, VecDeque};

mod walk;

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Extra media types (exact match) to treat as JSON-shaped on top
    /// of the built-in `application/json` + `+json`-suffix recognition.
    #[serde(default)]
    pub extra_json_media_types: Vec<String>,
}

pub fn transform(mut spec: ir::Ir, cfg: &Config) -> forge_plugin_sdk::TransformOutput {
    let extras: HashSet<String> = cfg
        .extra_json_media_types
        .iter()
        .map(|m| m.to_ascii_lowercase())
        .collect();

    let mut diagnostics: Vec<ir::Diagnostic> = Vec::new();
    let kept_ops: Vec<ir::Operation> = std::mem::take(&mut spec.operations)
        .into_iter()
        .filter(|op| match op.request_body.as_ref() {
            None => true,
            Some(body) => {
                let media_types: Vec<&str> =
                    body.content.iter().map(|c| c.media_type.as_str()).collect();
                let has_json = media_types.iter().any(|m| is_json_media_type(m, &extras));
                if !has_json {
                    let id = op.original_id.as_deref().unwrap_or(&op.id);
                    diagnostics.push(ir::Diagnostic {
                        severity: ir::Severity::Info,
                        code: "transformer-drop-non-json-bodies/I-DROPPED".to_string(),
                        message: format!(
                            "dropped operation `{id}` — request body content types {media_types:?} \
                             have no JSON-shaped entry"
                        ),
                        location: None,
                        related: Vec::new(),
                        suggested_fix: None,
                    });
                }
                has_json
            }
        })
        .collect();

    // Prune `Ir::types` to types reachable from the kept operations.
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
    let kept_types: Vec<ir::NamedType> = std::mem::take(&mut spec.types)
        .into_iter()
        .filter(|t| reachable.contains(&t.id))
        .collect();

    spec.operations = kept_ops;
    spec.types = kept_types;
    forge_plugin_sdk::TransformOutput { spec, diagnostics }
}

/// Recognize JSON-shaped content types: `application/json`, the `+json`
/// structured-syntax suffix (`application/problem+json`,
/// `application/vnd.api+json`), trailing media-type parameters
/// (`; charset=utf-8`), and any explicitly-configured extras. Matches
/// the logic in `generator-rust-tower`'s `is_json_media_type` so the
/// transformer and the generator agree on "JSON-shaped."
fn is_json_media_type(media_type: &str, extras: &HashSet<String>) -> bool {
    let essence = media_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();
    if essence == "application/json" {
        return true;
    }
    if essence
        .strip_prefix("application/")
        .is_some_and(|rest| rest.ends_with("+json"))
    {
        return true;
    }
    extras.contains(&essence)
}

struct DropNonJsonBodies;

impl Guest for DropNonJsonBodies {
    fn info() -> WitPluginInfo {
        conv::plugin_info_to_wit(ir::PluginInfo {
            name: "transformer-drop-non-json-bodies".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    fn config_schema() -> String {
        include_str!("../schema.json").into()
    }

    fn transform(spec: WitIr, config: String) -> Result<WitTransformOutput, StageError> {
        let cfg: Config = if config.trim().is_empty() {
            Config::default()
        } else {
            serde_json::from_str(&config).map_err(|e| conv::config_invalid(e.to_string()))?
        };
        let canonical = conv::ir_from_wit(spec);
        let out = transform(canonical, &cfg);
        Ok(conv::transform_output_to_wit(out))
    }
}

forge_plugin_sdk::transformer::export!(DropNonJsonBodies with_types_in forge_plugin_sdk::transformer);
