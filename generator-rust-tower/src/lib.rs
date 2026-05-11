//! `generator-rust-tower` — emits a tower-based Rust client module tree from
//! an IR produced by the openapi-forge parser.
//!
//! The output is a self-contained module tree (no Cargo.toml): every
//! operation lands as `impl Operation` against the trait defined in
//! `runtime.rs`, which is emitted inline. Consumers drop the tree into an
//! existing crate via `pub mod gen;` (or any name) and supply the
//! `tower::Service<http::Request<…>>` instance to drive operations through.

#![forbid(unsafe_code)]

mod emit;
mod models;
mod naming;
mod operation_impl;
mod types;

use forge_plugin_sdk::convert::generator as conv;
use forge_plugin_sdk::generator::exports::forge::plugin::generator_api::{
    GenerationOutput as WitGenerationOutput, Guest,
};
use forge_plugin_sdk::generator::forge::plugin::stage::StageError;
use forge_plugin_sdk::generator::forge::plugin::types::{Ir as WitIr, PluginInfo as WitPluginInfo};
use forge_plugin_sdk::ir;

/// Pure entry point. Operates on the canonical IR.
pub fn generate(spec: &ir::Ir) -> forge_plugin_sdk::GenerationOutput {
    emit::all(spec)
}

struct RustTower;

impl Guest for RustTower {
    fn info() -> WitPluginInfo {
        conv::plugin_info_to_wit(ir::PluginInfo {
            name: "generator-rust-tower".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    fn config_schema() -> String {
        include_str!("../schema.json").into()
    }

    fn generate(spec: WitIr, _config: String) -> Result<WitGenerationOutput, StageError> {
        let canonical = conv::ir_from_wit(spec);
        let out = generate(&canonical);
        Ok(conv::generation_output_to_wit(out))
    }
}

forge_plugin_sdk::generator::export!(RustTower with_types_in forge_plugin_sdk::generator);
