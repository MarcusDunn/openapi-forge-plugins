//! `generator-kotlin-types` — emits Kotlin types (kotlinx.serialization data
//! classes + values4k value-object wrappers) for an OpenAPI spec.
//!
//! Every `NamedType` in the spec must declare an `x-kotlin-source` extension
//! of the form `"<path>#<fully.qualified.ClassName>"`. The path drives the
//! output file; the FQCN drives the package declaration, class name, and
//! cross-file `import`s. Missing annotations are a fatal error.

#![forbid(unsafe_code)]

mod config;
mod emit;
mod naming;
mod render;

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

struct KotlinTypes;

impl Guest for KotlinTypes {
    fn info() -> WitPluginInfo {
        conv::plugin_info_to_wit(ir::PluginInfo {
            name: "generator-kotlin-types".into(),
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
                    .map(|d| format!("generator-kotlin-types: {}", d.message))
                    .unwrap_or_else(|| "generator-kotlin-types: rejected".into());
                Err(conv::rejected(reason, diagnostics))
            }
        }
    }
}

forge_plugin_sdk::generator::export!(KotlinTypes with_types_in forge_plugin_sdk::generator);
