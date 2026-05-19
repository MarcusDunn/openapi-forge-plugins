//! Clap-only naming helpers. `snake_case` / `pascal_case` come from
//! [`codegen_rust_serde::naming`] so the CLI's identifiers stay in
//! lockstep with the tower request-struct / model names generated
//! under `src/gen/`. The kebab / SCREAMING_SNAKE forms here are used
//! only for CLI surface (subcommand display names, flag names, env-var
//! prefixes) and have no shared-crate analogue.

use codegen_rust_serde::naming::snake_case;

/// kebab-case slug. Used for clap subcommand display names and flag
/// names. `&` and other non-alphanumerics collapse to single `-`.
/// `"Self & Scopes"` → `"self-scopes"`.
pub fn kebab_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() {
                if prev_lower {
                    out.push('-');
                }
                out.extend(c.to_lowercase());
                prev_lower = false;
            } else {
                out.push(c);
                prev_lower = c.is_ascii_lowercase();
            }
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
            prev_lower = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// SCREAMING_SNAKE for env var prefixes.
pub fn screaming_snake(s: &str) -> String {
    snake_case(s).to_uppercase()
}
