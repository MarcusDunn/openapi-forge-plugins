//! `generator-rust-clap` — emits a Rust CLI crate (clap derive +
//! tower/hyper-util) for an OpenAPI spec.
//!
//! The CLI's HTTP client is a tower-driven module tree produced by
//! [`codegen_rust_tower`]; this crate adds the clap subcommand surface,
//! per-op dispatch into that client, optional OAuth 2.0 PKCE
//! login/logout, and optional RFC 8693 token exchange.

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

pub fn generate(spec: &ir::Ir, cfg: &Config) -> emit::Outcome {
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
        match generate(&canonical, &cfg) {
            emit::Outcome::Generated(out) => Ok(conv::generation_output_to_wit(out)),
            emit::Outcome::Rejected(diagnostics) => Err(conv::rejected(
                "generator-rust-clap: one or more types are not representable in Rust",
                diagnostics,
            )),
        }
    }
}

forge_plugin_sdk::generator::export!(RustClap with_types_in forge_plugin_sdk::generator);
