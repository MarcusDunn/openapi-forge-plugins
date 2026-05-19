//! Shared host-side test harness for openapi-forge plugins.
//!
//! Each test file declares which plugin(s) it runs against using the
//! `GeneratorPlugin` handles below. Behind each handle, the wasm
//! component is built once per test run (via `cargo build` shelled out
//! by `forge-test-harness::PluginRunner`) and cached in a module-level
//! `OnceLock`, so tests running concurrently share a single warm
//! runner. Tests pull the plugin's `GenerationOutput` and assert on
//! the emitted `OutputFile`s — file presence, contents, no fatal
//! diagnostics.
//!
//! Two scopes of test live here:
//!
//! - `tests/<plugin>.rs` — assertions specific to a single plugin's
//!   emit shape (e.g. clap-specific clap-derive struct annotations).
//! - `tests/invariants.rs` — cross-plugin checks that every generator
//!   in `plugins::ALL_GENERATORS` should satisfy on a minimal spec.
//!
//! To register a new generator plugin: add it as a `const` in
//! `plugins`, and append it to `ALL_GENERATORS` so it picks up the
//! generic invariants for free.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use forge_host::GenerationOutput;
pub use forge_ir::Ir;
use forge_test_harness::PluginRunner;
use serde_json::Value;

pub mod fixtures;

/// A generator plugin under test. Built lazily via `runner_fn` on first
/// use and reused across tests for the duration of `cargo test`.
///
/// The function-pointer indirection lets each plugin own its own
/// `OnceLock<PluginRunner>` at module scope while still being usable in
/// `const` contexts (so `ALL_GENERATORS` is a plain `&[&GeneratorPlugin]`).
pub struct GeneratorPlugin {
    pub name: &'static str,
    runner_fn: fn() -> &'static PluginRunner,
}

impl GeneratorPlugin {
    pub fn run(&self, ir: Ir, cfg: Value) -> GenerationOutput {
        (self.runner_fn)()
            .generate(ir, cfg)
            .unwrap_or_else(|e| panic!("{} returned StageError: {e:?}", self.name))
    }
}

pub mod plugins {
    use super::*;

    fn plugin_dir(name: &str) -> PathBuf {
        // CARGO_MANIFEST_DIR is `<workspace>/host-tests`; the plugin
        // sits as a sibling under the workspace root.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("host-tests has a parent directory")
            .join(name)
    }

    fn clap_runner() -> &'static PluginRunner {
        static R: OnceLock<PluginRunner> = OnceLock::new();
        R.get_or_init(|| {
            PluginRunner::build_and_load(&plugin_dir("generator-rust-clap"))
                .expect("build and load generator-rust-clap")
        })
    }

    fn tower_runner() -> &'static PluginRunner {
        static R: OnceLock<PluginRunner> = OnceLock::new();
        R.get_or_init(|| {
            PluginRunner::build_and_load(&plugin_dir("generator-rust-tower"))
                .expect("build and load generator-rust-tower")
        })
    }

    pub const CLAP: GeneratorPlugin = GeneratorPlugin {
        name: "generator-rust-clap",
        runner_fn: clap_runner,
    };
    pub const TOWER: GeneratorPlugin = GeneratorPlugin {
        name: "generator-rust-tower",
        runner_fn: tower_runner,
    };

    /// Every generator plugin that should pick up the cross-plugin
    /// invariants in `tests/invariants.rs`. Add a new plugin here once
    /// it's a workspace member and these invariants ought to hold for
    /// it.
    pub const ALL_GENERATORS: &[&GeneratorPlugin] = &[&CLAP, &TOWER];
}

// File lookup helpers --------------------------------------------------------

pub fn paths(out: &GenerationOutput) -> Vec<&str> {
    out.files.iter().map(|f| f.path.as_str()).collect()
}

pub fn file_named<'a>(out: &'a GenerationOutput, path: &str) -> &'a str {
    let f = out.files.iter().find(|f| f.path == path).unwrap_or_else(|| {
        panic!("expected output file {path:?}, got {:?}", paths(out))
    });
    std::str::from_utf8(&f.content).expect("output file is UTF-8")
}
