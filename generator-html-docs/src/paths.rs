//! URL/file-path helpers for the site layout. All site paths are
//! relative — pages compute an `asset_prefix` of `../`s to reach the
//! root.

/// Convert a free-form string into a URL-safe kebab-case slug.
///
/// Lowercases ASCII alphanumerics, replaces every run of other
/// characters with a single hyphen, and trims leading/trailing
/// hyphens. Empty input becomes `"_"` so we always produce a usable
/// path segment.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "_".into()
    } else {
        out
    }
}

/// Slash-joined tag-name slug chain, e.g. `pets/admin` for an `admin`
/// tag whose parent is `pets`.
pub fn tag_dir(slug_chain: &[String]) -> String {
    slug_chain.join("/")
}

/// Output path for a tag's index page (clean URL: `tags/.../index.html`).
pub fn tag_page_path(slug_chain: &[String]) -> String {
    format!("tags/{}/index.html", tag_dir(slug_chain))
}

/// Output path for an operation page.
///
/// OpenAPI requires `operationId` to be `[A-Za-z0-9_.-]+`, which is
/// already URL-safe; we use it verbatim so the path matches the
/// operationId character-for-character (no slug coercion to confuse
/// readers).
pub fn operation_page_path(op_id: &str) -> String {
    format!("operations/{}.html", op_id)
}

/// Output path for a schema page. The IR sanitizes schema ids to
/// `[A-Za-z0-9_]+` upstream, so we can use them verbatim.
pub fn schema_page_path(schema_id: &str) -> String {
    format!("schemas/{}.html", schema_id)
}

/// Output path for the security catalogue page.
pub const SECURITY_INDEX: &str = "security/index.html";

/// Output path for the schemas index page.
pub const SCHEMAS_INDEX: &str = "schemas/index.html";

/// `../` repeated to walk up to the site root from `path`. The path
/// is the file's relative location inside `out/`.
pub fn asset_prefix(path: &str) -> String {
    let depth = path.matches('/').count();
    "../".repeat(depth)
}
