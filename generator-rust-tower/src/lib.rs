//! `generator-rust-tower` — WIT/WASM plugin shim around the shared
//! [`codegen_rust_tower`] codegen library.
//!
//! All of the rendering logic lives in `codegen-rust-tower`; this crate
//! only adapts the canonical IR / `GenerationOutput` to/from the WIT
//! types and exports the WASM component the host loads.

#![forbid(unsafe_code)]

use codegen_rust_tower::Outcome;
use forge_plugin_sdk::convert::generator as conv;
use forge_plugin_sdk::generator::exports::forge::plugin::generator_api::{
    GenerationOutput as WitGenerationOutput, Guest,
};
use forge_plugin_sdk::generator::forge::plugin::stage::StageError;
use forge_plugin_sdk::generator::forge::plugin::types::{Ir as WitIr, PluginInfo as WitPluginInfo};
use forge_plugin_sdk::ir;

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
        match codegen_rust_tower::all(&canonical) {
            Outcome::Generated(out) => Ok(conv::generation_output_to_wit(out)),
            Outcome::Rejected(diagnostics) => Err(conv::rejected(
                "generator-rust-tower: one or more types are not representable in Rust",
                diagnostics,
            )),
        }
    }
}

forge_plugin_sdk::generator::export!(RustTower with_types_in forge_plugin_sdk::generator);
