//! Identifier sanitization for Rust output.
//!
//! Prefer the spec's `original_name` (e.g. `UpdatePhoneManifestWithJSONRequestV2`)
//! over the parser's canonical id (`schemas__UpdatePhoneManifestWithJSONRequestV2`)
//! when generating Rust type names — the canonical id carries parser
//! bookkeeping prefixes that read poorly in user code. See `rust_type_name`
//! for the resolution.
//!
//! Three transforms:
//! - `snake_case` for `fn`/field/local names. Splits on lowercase→uppercase
//!   boundaries, lowercases. Reserved keywords get the `r#` raw-identifier
//!   prefix; identifiers that conflict with built-ins (`self`, `crate`,
//!   `super`, `Self`) get a `_` suffix because raw idents aren't legal for
//!   those names.
//! - `pascal_case` for type / variant names. Splits on case + underscore +
//!   hyphen, capitalizes each segment.
//! - `escape_str` for placing untrusted strings into Rust `"..."` literals.

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

/// `self`, `crate`, `super`, `Self` cannot be raw-identifier-escaped.
/// Anything that lowers to one of these gets a trailing underscore instead.
const RUST_KEYWORDS_NO_RAW: &[&str] = &["crate", "self", "super", "Self"];

/// Lower-camel / Pascal / kebab → `snake_case`. Used for `fn`/field/local
/// names in the emitted source.
pub fn snake_case(s: &str) -> String {
    if s.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let valid = c.is_ascii_alphanumeric() || c == '_';
        if !valid {
            out.push('_');
            continue;
        }
        if c.is_ascii_uppercase() {
            let prev = i.checked_sub(1).map(|j| chars[j]);
            let next = chars.get(i + 1).copied();
            let prev_lower_or_digit =
                matches!(prev, Some(p) if p.is_ascii_lowercase() || p.is_ascii_digit());
            let prev_upper = matches!(prev, Some(p) if p.is_ascii_uppercase());
            let next_lower = matches!(next, Some(n) if n.is_ascii_lowercase());
            // Insert `_` at lower→upper boundaries, and at the end of an
            // acronym (`URLPath` → `url_path`, not `urlp_ath`).
            if i > 0 && (prev_lower_or_digit || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    // Leading digit → prefix with `_`.
    let first = out.chars().next().unwrap_or('_');
    if first.is_ascii_digit() {
        out.insert(0, '_');
    }
    escape_keyword(out)
}

/// `camelCase` / `kebab-case` / `snake_case` → `PascalCase`. Used for type
/// and variant names.
pub fn pascal_case(s: &str) -> String {
    if s.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut next_upper = true;
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' || c == ' ' || c == '.' || c == '/' {
            next_upper = true;
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            // Drop punctuation; the next valid char starts a new segment.
            next_upper = true;
            continue;
        }
        if i > 0 && c.is_ascii_uppercase() {
            // Preserve existing boundaries (e.g. `getUser` → `GetUser`,
            // `JSONBody` → `JsonBody` — we don't try to preserve acronyms
            // in caps, which matches `heck` and most generators).
            let prev = chars.get(i - 1).copied();
            if matches!(prev, Some(p) if p.is_ascii_lowercase() || p.is_ascii_digit()) {
                next_upper = true;
            }
        }
        if next_upper {
            out.push(c.to_ascii_uppercase());
            next_upper = false;
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        return "_".to_string();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Rust type / variant name for an IR `NamedType`. Prefer the spec's
/// `original_name` (clean) over the parser's `id` (carries `schemas__`
/// prefixes and hoisting suffixes); fall back to `id` when no original name
/// is available (synthetic hoisted types).
pub fn rust_type_name(named: &forge_plugin_sdk::ir::NamedType) -> String {
    let source = named.original_name.as_deref().unwrap_or(named.id.as_str());
    pascal_case(source)
}

fn escape_keyword(name: String) -> String {
    if RUST_KEYWORDS_NO_RAW.contains(&name.as_str()) {
        format!("{name}_")
    } else if RUST_KEYWORDS.contains(&name.as_str()) {
        format!("r#{name}")
    } else {
        name
    }
}

/// Escape a string for placement inside a Rust `"..."` literal. Conservative
/// — only the four characters that actually break the literal get escaped.
pub fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_basic() {
        assert_eq!(snake_case("getUser"), "get_user");
        assert_eq!(snake_case("HTTPSConnection"), "https_connection");
        assert_eq!(
            snake_case("camelCaseWithACRONYMInside"),
            "camel_case_with_acronym_inside"
        );
        assert_eq!(snake_case("already_snake"), "already_snake");
        assert_eq!(snake_case("kebab-case"), "kebab_case");
        assert_eq!(snake_case("with space"), "with_space");
    }

    #[test]
    fn snake_keywords() {
        assert_eq!(snake_case("type"), "r#type");
        assert_eq!(snake_case("self"), "self_");
        assert_eq!(snake_case("crate"), "crate_");
        assert_eq!(snake_case("Super"), "super_");
    }

    #[test]
    fn pascal_basic() {
        assert_eq!(pascal_case("getUser"), "GetUser");
        assert_eq!(pascal_case("get_user"), "GetUser");
        assert_eq!(pascal_case("get-user"), "GetUser");
        assert_eq!(pascal_case("ALREADY_PASCAL"), "AlreadyPascal");
        assert_eq!(pascal_case("1leading"), "_1leading");
    }
}
