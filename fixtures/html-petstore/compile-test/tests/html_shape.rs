//! Structural assertions on the HTML site emitted by
//! `generator-html-docs` for the `html-petstore` fixture.
//!
//! These tests follow the same pattern as
//! `fixtures/clap-petstore/compile-test/tests/codegen.rs`: read the
//! generated artifacts off disk with plain `std::fs`, then assert on
//! substring and stable `data-*` hook presence. No third-party HTML
//! parser — templates emit deterministic, well-formed HTML and tag
//! every load-bearing element with a `data-*` attribute so we can
//! pin behaviour with simple `&str` operations.
//!
//! The fixture's spec exercises nested OAS 3.2 tags
//! (`pets-public` and `pets-admin` under `pets`), a deprecated
//! operation (`legacySearchPets`), and a top-level tag (`owners`)
//! that lives outside the nested tree.

use std::fs;
use std::path::{Path, PathBuf};

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compile-test dir has a parent fixture dir")
        .join("out")
}

fn read(rel: &str) -> String {
    let p = out_dir().join(rel);
    fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("expected emitted file {} ({})", p.display(), e))
}

fn exists(rel: &str) -> bool {
    out_dir().join(rel).exists()
}

const OPERATIONS: &[&str] = &[
    "listPets",
    "getPet",
    "deletePet",
    "importPets",
    "legacySearchPets",
    "listOwners",
    "getOwner",
    "listPetEvents",
];

const DEPRECATED_OP: &str = "legacySearchPets";

/// The op whose `security` overrides the document-level default.
const OP_WITH_AUTH_OVERRIDE: &str = "deletePet";

/// An op that inherits the document-level `bearerAuth` requirement.
const OP_WITH_INHERITED_AUTH: &str = "listPets";

const SECURITY_SCHEMES: &[&str] = &["bearerAuth", "petsAdminOauth"];

const UNION_SCHEMA: &str = "PetEvent";
const UNION_DISCRIMINATOR_PROPERTY: &str = "type";

const TAGS_FLAT: &[(&str, &str)] = &[
    // (slug path under tags/, tag name as it appears in the spec)
    ("pets", "pets"),
    ("pets/pets-public", "pets-public"),
    ("pets/pets-admin", "pets-admin"),
    ("owners", "owners"),
];

const SCHEMAS: &[&str] = &[
    "Pet",
    "Pets",
    "Owner",
    "ImportJob",
    "Error",
    "PetEvent",
    "PetCreatedEvent",
    "PetDeletedEvent",
];

// ---------- file-tree existence ----------

#[test]
fn every_operation_has_a_page() {
    for op in OPERATIONS {
        assert!(
            exists(&format!("operations/{op}.html")),
            "missing operations/{op}.html"
        );
    }
}

#[test]
fn every_tag_has_a_page() {
    for (slug_path, _) in TAGS_FLAT {
        assert!(
            exists(&format!("tags/{slug_path}/index.html")),
            "missing tags/{slug_path}/index.html"
        );
    }
}

#[test]
fn schema_pages_exist() {
    for s in SCHEMAS {
        assert!(
            exists(&format!("schemas/{s}.html")),
            "missing schemas/{s}.html"
        );
    }
}

#[test]
fn static_assets_emitted() {
    assert!(exists("_static/styles.css"), "missing _static/styles.css");
    assert!(exists("_static/app.js"), "missing _static/app.js");
}

#[test]
fn landing_page_exists() {
    assert!(exists("index.html"), "missing index.html");
}

// ---------- semantic structure (sampled across page types) ----------

fn assert_well_formed(rel: &str) {
    let html = read(rel);
    assert!(
        html.contains("<!DOCTYPE html>"),
        "{rel}: missing <!DOCTYPE html>"
    );
    assert!(
        html.contains("<html lang=\"en\""),
        "{rel}: <html> tag missing lang=\"en\""
    );
    assert_eq!(
        count_substr(&html, "<h1"),
        1,
        "{rel}: should have exactly one <h1>"
    );
    for tag in &["<main", "<nav", "<aside"] {
        assert!(
            html.contains(tag),
            "{rel}: missing semantic landmark `{tag}`"
        );
    }
    assert!(
        html.contains("aria-label=\"Breadcrumb\""),
        "{rel}: missing breadcrumb nav"
    );
}

#[test]
fn pages_use_semantic_markup() {
    assert_well_formed("index.html");
    assert_well_formed("operations/getPet.html");
    assert_well_formed("tags/pets/index.html");
    assert_well_formed("tags/pets/pets-public/index.html");
    assert_well_formed("schemas/Pet.html");
}

#[test]
fn no_role_redundant_on_landmarks() {
    let pages = ["index.html", "operations/getPet.html"];
    for rel in pages {
        let html = read(rel);
        // <nav role="navigation"> and <main role="main"> are redundant
        // and we lint them out at template-review time. This is a
        // regression pin.
        assert!(
            !html.contains("role=\"navigation\""),
            "{rel}: redundant role=\"navigation\" on <nav>"
        );
        assert!(
            !html.contains("role=\"main\""),
            "{rel}: redundant role=\"main\" on <main>"
        );
    }
}

// ---------- nested tags ----------

#[test]
fn landing_sidebar_nests_admin_under_pets() {
    let html = read("index.html");
    let aside_start = html
        .find("<aside")
        .expect("landing page must have an <aside>");
    let aside_end = html[aside_start..]
        .find("</aside>")
        .expect("the <aside> must close")
        + aside_start;
    let aside = &html[aside_start..aside_end];

    let pets_open = aside
        .find("data-tag-path=\"pets\"")
        .expect("sidebar should mark the pets tag with data-tag-path");
    let pets_window = li_window(aside, pets_open);
    assert!(
        pets_window.contains("data-tag-path=\"pets/pets-admin\""),
        "pets-admin should be nested inside the pets <li> on the landing sidebar; window was:\n{pets_window}"
    );
    assert!(
        pets_window.contains("data-tag-path=\"pets/pets-public\""),
        "pets-public should be nested inside the pets <li> on the landing sidebar; window was:\n{pets_window}"
    );
}

/// Given a `<li>` opening tag's byte offset, return the slice that
/// covers from that `<li>` through its matching `</li>` — handling
/// arbitrary nested `<li>` depth so callers can assert "child element
/// X appears inside this list item".
fn li_window(hay: &str, anchor: usize) -> &str {
    // Walk backwards from `anchor` to find the `<li` that opens this
    // element. The anchor is an attribute inside the opening tag, so
    // the `<li` must precede it within the same tag.
    let li_open = hay[..anchor]
        .rfind("<li")
        .expect("anchor must be inside a <li> opening tag");
    let mut depth = 0usize;
    let mut i = li_open;
    let bytes = hay.as_bytes();
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<li") {
            // Only count if it's really an `<li` element (next char is
            // whitespace or `>` or `/`).
            let next = bytes.get(i + 3).copied().unwrap_or(b' ');
            if next == b' ' || next == b'>' || next == b'\n' || next == b'\t' {
                depth += 1;
                i += 3;
                continue;
            }
        }
        if bytes[i..].starts_with(b"</li>") {
            depth -= 1;
            i += 5;
            if depth == 0 {
                return &hay[li_open..i];
            }
            continue;
        }
        i += 1;
    }
    panic!("unbalanced <li> nesting starting at byte {li_open}");
}

#[test]
fn tag_overview_section_also_nests() {
    let html = read("index.html");
    // The main-content <section aria-labelledby="tags-heading"> also
    // renders the tag tree; it must show admin/public nested under pets.
    let section_start = html
        .find("aria-labelledby=\"tags-heading\"")
        .expect("tags overview section is present");
    let tail = &html[section_start..];
    assert!(
        tail.contains("data-tag-path=\"pets/pets-admin\""),
        "tags overview must render pets-admin nested under pets"
    );
}

#[test]
fn pets_tag_page_lists_its_children() {
    let html = read("tags/pets/index.html");
    assert!(
        html.contains("Sub-tags"),
        "pets tag page should list its sub-tags"
    );
    assert!(html.contains("pets-public"), "missing pets-public sub-tag link");
    assert!(html.contains("pets-admin"), "missing pets-admin sub-tag link");
}

// ---------- deprecation banner ----------

#[test]
fn deprecated_op_banner_visible() {
    let html = read(&format!("operations/{DEPRECATED_OP}.html"));
    // The banner element is always emitted; on deprecated ops it has
    // no `hidden` attribute.
    let banner = html
        .find("deprecated-banner")
        .expect("deprecated-banner class missing");
    let window = &html[banner..banner + 200.min(html.len() - banner)];
    assert!(
        !window.contains(" hidden"),
        "deprecated-banner on {DEPRECATED_OP} should be visible (no `hidden` attr): {window}"
    );
}

#[test]
fn live_op_banner_hidden() {
    let html = read("operations/getPet.html");
    let banner = html
        .find("deprecated-banner")
        .expect("deprecated-banner class present on all op pages");
    let window = &html[banner..banner + 200.min(html.len() - banner)];
    assert!(
        window.contains(" hidden"),
        "deprecated-banner on live op should carry `hidden`: {window}"
    );
}

// ---------- markdown rendering ----------

#[test]
fn markdown_renders_inline_code_in_descriptions() {
    // listPets's description mentions `limit` in backticks — must
    // surface as <code>limit</code>.
    let html = read("operations/listPets.html");
    assert!(
        html.contains("<code>limit</code>"),
        "listPets description should render `limit` -> <code>limit</code>"
    );
}

#[test]
fn markdown_renders_strong_in_landing_description() {
    let html = read("index.html");
    // info.description has a **bold** phrase that pulldown-cmark turns
    // into <strong>...</strong>.
    assert!(
        html.contains("<strong>"),
        "info.description markdown should render at least one <strong>"
    );
}

// ---------- cross-link integrity ----------

#[test]
fn landing_links_to_every_tag() {
    let html = read("index.html");
    for (slug_path, _) in TAGS_FLAT {
        let href = format!("tags/{slug_path}/index.html");
        assert!(
            html.contains(&href),
            "landing page should link to {href}"
        );
    }
}

#[test]
fn tag_pages_link_to_every_operation_they_own() {
    // pets-public houses listPets, getPet, legacySearchPets.
    let html = read("tags/pets/pets-public/index.html");
    for op in ["listPets", "getPet", "legacySearchPets"] {
        let href = format!("../../../operations/{op}.html");
        assert!(
            html.contains(&href),
            "pets-public tag page should link to {href}"
        );
    }
}

#[test]
fn schema_back_links_present() {
    // Pet is referenced by listPets / getPet / importPets / legacySearch.
    let html = read("schemas/Pet.html");
    assert!(html.contains("Used in"), "Pet schema page should have a 'Used in' section");
    for op in ["listPets", "getPet"] {
        assert!(
            html.contains(&format!("operations/{op}.html")),
            "Pet schema 'Used in' should link to {op}"
        );
    }
}

// ---------- M1: security ----------

#[test]
fn security_page_exists_and_lists_each_scheme() {
    let html = read("security/index.html");
    for scheme in SECURITY_SCHEMES {
        assert!(
            html.contains(&format!("id=\"scheme-{scheme}\"")),
            "security page should anchor scheme `{scheme}`"
        );
        assert!(
            html.contains(&format!("data-scheme-id=\"{scheme}\"")),
            "security page should mark scheme `{scheme}` with data-scheme-id"
        );
    }
    // Each known kind is rendered with its data-scheme-kind hook.
    assert!(html.contains("data-scheme-kind=\"http-bearer\""));
    assert!(html.contains("data-scheme-kind=\"oauth2\""));
    // Client-credentials flow specifics surface.
    assert!(html.contains("data-flow=\"client-credentials\""));
    // MiniJinja auto-escape encodes `/` in attribute / text content as
    // `&#x2f;`, which browsers transparently decode. Assert against
    // the host substring rather than the full URL to stay neutral.
    assert!(html.contains("auth.example.com"));
    assert!(html.contains("pets:write"));
}

#[test]
fn op_override_renders_override_not_inherited() {
    let html = read(&format!("operations/{OP_WITH_AUTH_OVERRIDE}.html"));
    assert!(
        html.contains("data-auth-required"),
        "{OP_WITH_AUTH_OVERRIDE} op page must render an Authorization section"
    );
    assert!(
        html.contains("data-scheme-id=\"petsAdminOauth\""),
        "{OP_WITH_AUTH_OVERRIDE} op should require petsAdminOauth"
    );
    assert!(
        !auth_section_marked_inherited(&html),
        "{OP_WITH_AUTH_OVERRIDE} op declares its own security and must not be marked (inherited)"
    );
    assert!(
        html.contains("pets:write"),
        "{OP_WITH_AUTH_OVERRIDE} should show its scope requirement"
    );
}

#[test]
fn op_inherits_doc_level_security() {
    let html = read(&format!("operations/{OP_WITH_INHERITED_AUTH}.html"));
    assert!(
        html.contains("data-auth-required"),
        "{OP_WITH_INHERITED_AUTH} should inherit the document-level requirement"
    );
    assert!(
        html.contains("data-scheme-id=\"bearerAuth\""),
        "{OP_WITH_INHERITED_AUTH} should resolve to bearerAuth"
    );
}

/// True when the rendered auth section is annotated with the
/// `(inherited from API default)` qualifier.
fn auth_section_marked_inherited(html: &str) -> bool {
    let Some(start) = html.find("id=\"auth-heading\"") else {
        return false;
    };
    let window_end = (start + 400).min(html.len());
    html[start..window_end].contains("(inherited")
}

// ---------- M1: schemas index ----------

#[test]
fn schemas_index_page_lists_every_emitted_schema() {
    let html = read("schemas/index.html");
    for s in SCHEMAS {
        assert!(
            html.contains(&format!("data-schema-id=\"{s}\"")),
            "schemas index should list schema `{s}`"
        );
    }
    // The discriminated union carries its kind badge. Templates emit
    // attributes on multiple lines, so check each independently within
    // a tight window around the schema's entry.
    let id_anchor = html
        .find(&format!("data-schema-id=\"{UNION_SCHEMA}\""))
        .expect("union schema entry present");
    let li_window = &html[id_anchor..(id_anchor + 400).min(html.len())];
    assert!(
        li_window.contains("data-schema-kind=\"union\""),
        "schemas index should tag {UNION_SCHEMA} as kind=union"
    );
}

#[test]
fn sidebar_links_to_security_and_schemas() {
    let html = read("index.html");
    assert!(
        html.contains("href=\"security/index.html\""),
        "sidebar should link to the security page"
    );
    assert!(
        html.contains("href=\"schemas/index.html\""),
        "sidebar should link to the schemas index page"
    );
}

// ---------- M1: discriminated union ----------

#[test]
fn discriminated_union_schema_renders_discriminator() {
    let html = read(&format!("schemas/{UNION_SCHEMA}.html"));
    assert!(
        html.contains(&format!(
            "data-discriminator-property=\"{UNION_DISCRIMINATOR_PROPERTY}\""
        )),
        "{UNION_SCHEMA} schema page should mark the discriminator property"
    );
    // Mapping table shows both tags.
    assert!(
        html.contains(&format!(
            "<code>{UNION_DISCRIMINATOR_PROPERTY}: \"created\"</code>"
        )),
        "{UNION_SCHEMA} should render the 'created' tag in its discriminator mapping"
    );
    assert!(
        html.contains(&format!(
            "<code>{UNION_DISCRIMINATOR_PROPERTY}: \"deleted\"</code>"
        )),
        "{UNION_SCHEMA} should render the 'deleted' tag in its discriminator mapping"
    );
}

#[test]
fn op_returning_discriminated_union_inlines_discriminator() {
    let html = read("operations/listPetEvents.html");
    assert!(
        html.contains(&format!(
            "data-discriminator-property=\"{UNION_DISCRIMINATOR_PROPERTY}\""
        )),
        "operation page should inline the discriminator on the response body"
    );
}

// ---------- synthetic-name typeref links land on the ancestor page ----------

#[test]
fn schema_page_property_dt_has_id_anchor() {
    // Pet has `id`, `name`, `tag` properties — each <dt> should carry
    // a stable `id="property-<name>"` anchor that synthetic-link
    // hrefs can target.
    let html = read("schemas/Pet.html");
    for name in ["id", "name", "tag"] {
        assert!(
            html.contains(&format!("id=\"property-{name}\"")),
            "Pet schema <dt> for property `{name}` must carry an id anchor"
        );
    }
}

#[test]
fn op_page_param_dt_has_id_anchor() {
    // listPets's `limit` query param.
    let html = read("operations/listPets.html");
    assert!(
        html.contains("id=\"param-query-limit\""),
        "listPets `<dt>` for the limit query param must carry an id anchor"
    );
}

#[test]
fn op_page_response_article_has_id_anchor() {
    let html = read("operations/listPets.html");
    assert!(
        html.contains("id=\"response-200\""),
        "listPets 200 response <article> must carry an id anchor"
    );
}

// ---------- M1: server variables ----------

#[test]
fn server_variables_render_on_landing() {
    let html = read("index.html");
    assert!(
        html.contains("class=\"server-variables\""),
        "landing should render <dl class=\"server-variables\"> for servers with variables"
    );
    assert!(html.contains("<code>tenant</code>"));
    assert!(html.contains("<code>stage</code>"));
    assert!(html.contains("<code>v1beta</code>"));
}

// ---------- helpers ----------

fn count_substr(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

#[allow(dead_code)]
fn dump(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("out")
        .join(rel);
    p.display().to_string()
}
