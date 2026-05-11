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
/// names in the emitted source. Shares its word-splitting rules with
/// [`pascal_case`] so the two stay consistent (e.g.
/// `updatePhoneManifestWithJSONV2` → `update_phone_manifest_with_json_v2`,
/// matching `UpdatePhoneManifestWithJsonV2` from `pascal_case`).
pub fn snake_case(s: &str) -> String {
    if s.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(s.len() + 4);
    for (i, word) in split_into_words(s).into_iter().enumerate() {
        if i > 0 {
            out.push('_');
        }
        for c in word.chars() {
            out.push(c.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        return "_".to_string();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    escape_keyword(out)
}

/// `camelCase` / `kebab-case` / `snake_case` → `PascalCase`.
///
/// Splits into words on:
/// - non-alphanumeric punctuation (`_-./ `)
/// - lower/digit → upper (`getUser` → `get`, `User`)
/// - upper → upper+lower (`HTTPClient` → `HTTP`, `Client`)
/// - upper → upper followed by a digit (`JSONV2` → `JSON`, `V2`)
/// - letter → digit (`v2` → `v`, `2`)
///
/// Each word is title-cased, so the third and fourth rules together turn
/// `updatePhoneManifestWithJSONV2` into `UpdatePhoneManifestWithJsonV2`
/// (not `Jsonv2`, which is the worst of both worlds).
pub fn pascal_case(s: &str) -> String {
    if s.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(s.len());
    for word in split_into_words(s) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            for c in chars {
                out.push(c.to_ascii_lowercase());
            }
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

fn split_into_words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let split = boundary(prev, c, next);
            if split {
                words.push(std::mem::take(&mut current));
            }
        }
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn boundary(prev: char, curr: char, next: Option<char>) -> bool {
    let prev_upper = prev.is_ascii_uppercase();
    let prev_lower_or_digit = prev.is_ascii_lowercase() || prev.is_ascii_digit();
    let curr_upper = curr.is_ascii_uppercase();
    let next_lower = matches!(next, Some(n) if n.is_ascii_lowercase());
    let next_digit = matches!(next, Some(n) if n.is_ascii_digit());

    // 1. lower/digit → upper: `getId` → `get`, `Id`
    (prev_lower_or_digit && curr_upper)
        // 2. upper → upper followed by lower: `HTTPClient` → `HTTP`, `Client`
        || (prev_upper && curr_upper && next_lower)
        // 3. trailing upper before a digit ends an acronym run: `JSONV2`
        //    → `JSON`, `V2` (split before `V`, not after it). Without this,
        //    `JSONV` swallows `V` and the result reads as `Jsonv2`.
        //    We deliberately don't split at letter→digit boundaries
        //    generally: `version2` and `v2` should stay as a single token
        //    (`Version2`/`v2`, not `Version_2`/`v_2`), which is what `heck`
        //    and the broader Rust ecosystem do.
        || (prev_upper && curr_upper && next_digit)
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
        assert_eq!(
            snake_case("updatePhoneManifestWithJSONV2"),
            "update_phone_manifest_with_json_v2"
        );
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

    #[test]
    fn pascal_acronyms_and_versions() {
        // The acronym + digit suffix is the case the previous algorithm got
        // wrong; the reviewer dubbed `Jsonv2` "the no-fly zone."
        assert_eq!(
            pascal_case("updatePhoneManifestWithJSONV2"),
            "UpdatePhoneManifestWithJsonV2"
        );
        assert_eq!(pascal_case("HTTPClient"), "HttpClient");
        assert_eq!(pascal_case("JSON"), "Json");
        assert_eq!(pascal_case("JSONV2"), "JsonV2");
        assert_eq!(pascal_case("version2"), "Version2");
    }
}
