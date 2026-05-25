//! `generator-html-docs` — emits a static HTML documentation site for
//! an OpenAPI spec.
//!
//! The site is multi-page: one page per endpoint, tag pages nested per
//! OAS 3.2 `tag.parent`, schema pages, and a landing page. Semantic
//! HTML throughout; CSS and JS are emitted alongside as `_static/`.

#![forbid(unsafe_code)]

mod config;
mod emit;
mod highlight;
mod markdown;
mod nav;
mod paths;
mod render;
mod schema_filter;

use forge_plugin_sdk::convert::generator as conv;
use forge_plugin_sdk::generator::exports::forge::plugin::generator_api::{
    GenerationOutput as WitGenerationOutput, Guest,
};
use forge_plugin_sdk::generator::forge::plugin::stage::StageError;
use forge_plugin_sdk::generator::forge::plugin::types::{Ir as WitIr, PluginInfo as WitPluginInfo};
use forge_plugin_sdk::ir;

pub use config::Config;

pub fn generate(spec: &ir::Ir, cfg: &Config) -> emit::Outcome {
    emit::all(spec, cfg)
}

struct HtmlDocs;

impl Guest for HtmlDocs {
    fn info() -> WitPluginInfo {
        conv::plugin_info_to_wit(ir::PluginInfo {
            name: "generator-html-docs".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    fn config_schema() -> String {
        include_str!("../schema.json").into()
    }

    fn generate(spec: WitIr, config: String) -> Result<WitGenerationOutput, StageError> {
        let cfg: Config = if config.trim().is_empty() {
            Config::default()
        } else {
            forge_plugin_sdk::serde_json::from_str(&config)
                .map_err(|e| conv::config_invalid(e.to_string()))?
        };
        let canonical = conv::ir_from_wit(spec);
        match generate(&canonical, &cfg) {
            emit::Outcome::Generated(out) => Ok(conv::generation_output_to_wit(out)),
            emit::Outcome::Rejected(diagnostics) => {
                let reason = diagnostics
                    .iter()
                    .find(|d| d.severity == ir::Severity::Error)
                    .map(|d| format!("generator-html-docs: {}", d.message))
                    .unwrap_or_else(|| "generator-html-docs: rendering failed".into());
                Err(conv::rejected(reason, diagnostics))
            }
        }
    }
}

forge_plugin_sdk::generator::export!(HtmlDocs with_types_in forge_plugin_sdk::generator);
