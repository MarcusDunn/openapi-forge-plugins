//! Naming helpers. Pure functions, no deps. Style choices match clap's
//! derive defaults (kebab-case for subcommands, kebab-case for flags).
//!
//! All helpers must be deterministic and round-trip-stable: the same
//! input maps to the same output, and the output is a valid Rust ident
//! when the helper returns one.

/// PascalCase Rust identifier. Suitable for type names, enum variants.
/// Non-alphanumeric runs become a single boundary; leading-digit inputs
/// are prefixed with `_`. Reserved-keyword collisions are escaped with
/// a trailing `_`.
pub fn pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper_next = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if upper_next {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            upper_next = false;
        } else {
            upper_next = true;
        }
    }
    if out.is_empty() {
        return "_".into();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if is_rust_keyword(&out) {
        out.push('_');
    }
    out
}

/// snake_case Rust identifier. Inserts `_` between camelCase boundaries.
pub fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() {
                if prev_lower {
                    out.push('_');
                }
                out.extend(c.to_lowercase());
                prev_lower = false;
            } else {
                out.push(c);
                prev_lower = c.is_ascii_lowercase();
            }
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
            prev_lower = false;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        return "_".into();
    }
    let mut t = trimmed;
    if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        t.insert(0, '_');
    }
    if is_rust_keyword(&t) {
        t.push('_');
    }
    t
}

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

fn is_rust_keyword(s: &str) -> bool {
    matches!(
        s,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
    )
}
