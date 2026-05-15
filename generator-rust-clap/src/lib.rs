//! `generator-rust-clap` — emits a Rust CLI crate (clap derive +
//! reqwest) for an OpenAPI spec.
//!
//! Status: Phase 0 iteration 1 — emits a buildable flat clap CLI with
//! one subcommand per operation, but handlers print TODO instead of
//! making HTTP calls. Real HTTP, model emission, tag grouping, and
//! OAuth land in subsequent iterations (see plan).

#![forbid(unsafe_code)]

mod config;
mod emit;
mod naming;
mod schema;
mod tags;

use forge_plugin_sdk::convert::generator as conv;
use forge_plugin_sdk::generator::exports::forge::plugin::generator_api::{
    GenerationOutput as WitGenerationOutput, Guest,
};
use forge_plugin_sdk::generator::forge::plugin::stage::StageError;
use forge_plugin_sdk::generator::forge::plugin::types::{Ir as WitIr, PluginInfo as WitPluginInfo};
use forge_plugin_sdk::ir;

pub use config::Config;

pub fn generate(spec: &ir::Ir, cfg: &Config) -> forge_plugin_sdk::GenerationOutput {
    emit::all(spec, cfg)
}

struct RustClap;

impl Guest for RustClap {
    fn info() -> WitPluginInfo {
        conv::plugin_info_to_wit(ir::PluginInfo {
            name: "generator-rust-clap".into(),
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
        let out = generate(&canonical, &cfg);
        Ok(conv::generation_output_to_wit(out))
    }
}

forge_plugin_sdk::generator::export!(RustClap with_types_in forge_plugin_sdk::generator);
