//! Cross-plugin invariants. These checks run against every generator
//! registered in `host_tests::plugins::ALL_GENERATORS`. Anything that
//! should hold for "any generator plugin we ship" lives here; anything
//! plugin-specific lives in `tests/<plugin>.rs`.

use host_tests::fixtures::ir_minimal;
use host_tests::plugins::ALL_GENERATORS;
use host_tests::paths;
use serde_json::json;

#[test]
fn emits_at_least_one_file_on_a_minimal_spec() {
    for plugin in ALL_GENERATORS {
        let out = plugin.run(ir_minimal(), json!({}));
        assert!(
            !out.files.is_empty(),
            "{} emitted zero files on the minimal spec",
            plugin.name
        );
    }
}

#[test]
fn no_fatal_diagnostics_on_a_minimal_spec() {
    for plugin in ALL_GENERATORS {
        let out = plugin.run(ir_minimal(), json!({}));
        assert!(
            out.diagnostics.is_empty(),
            "{} produced diagnostics on the minimal spec: {:?}",
            plugin.name,
            out.diagnostics
        );
    }
}

#[test]
fn no_duplicate_output_paths() {
    for plugin in ALL_GENERATORS {
        let out = plugin.run(ir_minimal(), json!({}));
        let mut seen: Vec<&str> = paths(&out);
        seen.sort();
        let before = seen.len();
        seen.dedup();
        assert_eq!(
            seen.len(),
            before,
            "{} emitted duplicate paths: {:?}",
            plugin.name,
            paths(&out)
        );
    }
}

#[test]
fn all_text_files_are_valid_utf8() {
    for plugin in ALL_GENERATORS {
        let out = plugin.run(ir_minimal(), json!({}));
        for f in &out.files {
            // Mode is plugin-controlled; we trust the plugin to mark
            // binary content as such. For everything declared text,
            // assert the bytes decode.
            if matches!(f.mode, forge_host::FileMode::Text) {
                std::str::from_utf8(&f.content).unwrap_or_else(|e| {
                    panic!(
                        "{} emitted non-UTF-8 text file {:?}: {e}",
                        plugin.name, f.path
                    )
                });
            }
        }
    }
}
