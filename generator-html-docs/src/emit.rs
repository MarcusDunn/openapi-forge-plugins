//! Generation entry point. Builds the nav tree, walks every operation,
//! tag, and schema, and collects the pages plus the static assets into
//! a `GenerationOutput`.

use forge_plugin_sdk::ir::{Diagnostic, Ir};
use forge_plugin_sdk::output::OutputFile;
use forge_plugin_sdk::GenerationOutput;

use crate::config::Config;
use crate::{nav, render, schema_filter};

pub enum Outcome {
    Generated(GenerationOutput),
    Rejected(Vec<Diagnostic>),
}

pub fn all(spec: &Ir, cfg: &Config) -> Outcome {
    let nav = nav::build(spec);
    let used_in = render::build_used_in_index(spec);
    let env = render::env();
    let mut files: Vec<OutputFile> = Vec::new();
    let diagnostics: Vec<Diagnostic> = Vec::new();

    match render::landing(&env, spec, cfg, &nav) {
        Ok(page) => files.push(OutputFile::text(page.path, page.html)),
        Err(e) => return rejection("index.html", e, diagnostics),
    }

    if !spec.security_schemes.is_empty() {
        match render::security_page(&env, spec, cfg, &nav) {
            Ok(page) => files.push(OutputFile::text(page.path, page.html)),
            Err(e) => return rejection("security/index.html", e, diagnostics),
        }
    }

    if cfg.include_schemas {
        match render::schemas_index_page(&env, spec, cfg, &nav) {
            Ok(page) => files.push(OutputFile::text(page.path, page.html)),
            Err(e) => return rejection("schemas/index.html", e, diagnostics),
        }
    }

    for tag in nav.walk() {
        match render::tag_page(&env, spec, cfg, &nav, tag) {
            Ok(page) => files.push(OutputFile::text(page.path, page.html)),
            Err(e) => return rejection(&format!("tag '{}'", tag.name), e, diagnostics),
        }
    }

    for op in &spec.operations {
        match render::operation_page(&env, spec, cfg, &nav, op) {
            Ok(page) => files.push(OutputFile::text(page.path, page.html)),
            Err(e) => return rejection(&format!("operation '{}'", op.id), e, diagnostics),
        }
    }

    if cfg.include_schemas {
        // Synthetic IR types (`*_property_*`, `*_param_*`, etc.) do
        // NOT get their own page. They're inlined under the property /
        // parameter / response they belong to on the parent's page —
        // see `render::inline_schema_view`. Emitting one page per
        // synthetic blew the wasm hostcall fuel budget on real-world
        // specs (DialAI: ~3000 synthetic types).
        for t in &spec.types {
            if t.id == forge_plugin_sdk::ir::NULL_ID {
                continue;
            }
            if !schema_filter::is_user_facing(t) {
                continue;
            }
            match render::schema_page(&env, spec, cfg, &nav, &used_in, t) {
                Ok(page) => files.push(OutputFile::text(page.path, page.html)),
                Err(e) => return rejection(&format!("schema '{}'", t.id), e, diagnostics),
            }
        }
    }

    files.push(OutputFile::text(
        "_static/styles.css",
        include_str!("../assets/styles.css"),
    ));
    files.push(OutputFile::text(
        "_static/app.js",
        include_str!("../assets/app.js"),
    ));
    // The OAuth Authorization-Code popup redirects here. It's the
    // same on every site, so we just drop a verbatim static page.
    files.push(OutputFile::text(
        "auth/callback.html",
        include_str!("../assets/auth_callback.html"),
    ));

    Outcome::Generated(GenerationOutput { files, diagnostics })
}

fn rejection(page: &str, err: render::RenderError, mut diagnostics: Vec<Diagnostic>) -> Outcome {
    diagnostics.push(forge_plugin_sdk::diag::error(
        "generator-html-docs/render",
        format!("failed to render {page}: {}", err),
    ));
    Outcome::Rejected(diagnostics)
}
