//! View models passed to MiniJinja, plus the `Environment` wiring.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use forge_plugin_sdk::ir::{
    AdditionalProperties, ApiKeyLocation, ApiKeyScheme, Body, BodyContent, Discriminator,
    ExternalDocs, Header, Ir, NamedType, OAuth2Flow, OAuth2FlowKind, OAuth2Scheme, Operation,
    Parameter, Property, Response, ResponseStatus, SecurityRequirement, SecurityScheme,
    SecuritySchemeKind, TypeDef, TypeRef,
};
use forge_plugin_sdk::values_ext;
use minijinja::{Environment, Value as JValue};
use serde::Serialize;

use crate::config::Config;
use crate::highlight;
use crate::markdown;
use crate::nav::{method_class, Nav};
use crate::paths;
use crate::schema_filter;

pub fn env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.add_template("base.html.j2", include_str!("../templates/base.html.j2"))
        .expect("base template parses");
    env.add_template("index.html.j2", include_str!("../templates/index.html.j2"))
        .expect("index template parses");
    env.add_template("tag.html.j2", include_str!("../templates/tag.html.j2"))
        .expect("tag template parses");
    env.add_template(
        "operation.html.j2",
        include_str!("../templates/operation.html.j2"),
    )
    .expect("operation template parses");
    env.add_template(
        "schema.html.j2",
        include_str!("../templates/schema.html.j2"),
    )
    .expect("schema template parses");
    env.add_template(
        "partials/_sidebar.html.j2",
        include_str!("../templates/partials/_sidebar.html.j2"),
    )
    .expect("sidebar partial parses");
    env.add_template(
        "partials/_breadcrumb.html.j2",
        include_str!("../templates/partials/_breadcrumb.html.j2"),
    )
    .expect("breadcrumb partial parses");
    env.add_template(
        "partials/_params.html.j2",
        include_str!("../templates/partials/_params.html.j2"),
    )
    .expect("params partial parses");
    env.add_template(
        "partials/_typeref.html.j2",
        include_str!("../templates/partials/_typeref.html.j2"),
    )
    .expect("typeref partial parses");
    env.add_template(
        "partials/_example.html.j2",
        include_str!("../templates/partials/_example.html.j2"),
    )
    .expect("example partial parses");
    env.add_template(
        "security.html.j2",
        include_str!("../templates/security.html.j2"),
    )
    .expect("security template parses");
    env.add_template(
        "schemas_index.html.j2",
        include_str!("../templates/schemas_index.html.j2"),
    )
    .expect("schemas index template parses");
    env.add_template(
        "partials/_auth_required.html.j2",
        include_str!("../templates/partials/_auth_required.html.j2"),
    )
    .expect("auth-required partial parses");
    env.add_template(
        "partials/_discriminator.html.j2",
        include_str!("../templates/partials/_discriminator.html.j2"),
    )
    .expect("discriminator partial parses");
    env.add_template(
        "partials/_inline_schema.html.j2",
        include_str!("../templates/partials/_inline_schema.html.j2"),
    )
    .expect("inline-schema partial parses");
    env.add_template(
        "partials/_tryit.html.j2",
        include_str!("../templates/partials/_tryit.html.j2"),
    )
    .expect("tryit partial parses");
    env
}

// ----- shared chrome -----

#[derive(Serialize)]
pub struct ChromeCtx<'a> {
    pub title: &'a str,
    pub page_title: String,
    pub page_description: Option<String>,
    pub theme: &'a str,
    pub canonical: Option<String>,
    pub current_path: &'a str,
    pub asset_prefix: String,
    pub home_href: String,
    pub security_href: String,
    pub schemas_href: String,
    pub has_security: bool,
    pub api_version: &'a str,
    pub nav: &'a Nav,
    /// Slim views fed to the header server picker. Stored on chrome so
    /// every page can render the picker without re-walking the IR.
    pub pickable_servers: Vec<PickableServer>,
    /// JSON-encoded OAuth-client config keyed by scheme id, emitted
    /// once per page in a `<meta>` tag so app.js can read it
    /// synchronously at boot. Empty `{}` when no oauth client is
    /// configured.
    pub oauth_client_config_json: String,
}

/// One entry in the header's server `<select>`. Stays lean: the full
/// editable variable form lives on the landing page (`ServerView`).
#[derive(Serialize)]
pub struct PickableServer {
    pub url: String,
    pub label: String,
}

impl<'a> ChromeCtx<'a> {
    pub fn new(
        spec: &'a Ir,
        cfg: &'a Config,
        nav: &'a Nav,
        current_path: &'a str,
        page_title: String,
        page_description: Option<String>,
    ) -> Self {
        let title = cfg.title.as_deref().unwrap_or(spec.info.title.as_str());
        let asset_prefix = paths::asset_prefix(current_path);
        let home_href = if asset_prefix.is_empty() {
            "index.html".to_string()
        } else {
            format!("{}index.html", asset_prefix)
        };
        let security_href = format!("{}{}", asset_prefix, paths::SECURITY_INDEX);
        let schemas_href = format!("{}{}", asset_prefix, paths::SCHEMAS_INDEX);
        let canonical = cfg
            .base_url
            .as_deref()
            .map(|b| format!("{}/{}", b.trim_end_matches('/'), current_path));
        let pickable_servers = spec
            .servers
            .iter()
            .map(|s| PickableServer {
                url: s.url.clone(),
                label: s
                    .name
                    .clone()
                    .or_else(|| s.description.clone())
                    .unwrap_or_else(|| s.url.clone()),
            })
            .collect();
        let oauth_client_config_json =
            forge_plugin_sdk::serde_json::to_string(&cfg.oauth).unwrap_or_else(|_| "{}".into());
        Self {
            title,
            page_title,
            page_description,
            theme: cfg.theme.as_str(),
            canonical,
            current_path,
            asset_prefix,
            home_href,
            security_href,
            schemas_href,
            has_security: !spec.security_schemes.is_empty(),
            api_version: spec.info.version.as_str(),
            nav,
            pickable_servers,
            oauth_client_config_json,
        }
    }
}

// ----- crumbs -----

#[derive(Serialize, Clone)]
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
}

pub fn crumbs_home(asset_prefix: &str) -> Vec<Crumb> {
    vec![Crumb {
        label: "API".into(),
        href: Some(format!("{}index.html", asset_prefix)),
    }]
}

pub fn crumbs_for_tag(nav: &Nav, slug_chain: &[String], asset_prefix: &str) -> Vec<Crumb> {
    let mut out = crumbs_home(asset_prefix);
    // Walk down the tree to find each ancestor's display name.
    let mut current: Option<&crate::nav::NavTag> = None;
    let mut search_in: &[crate::nav::NavTag] = &nav.roots;
    for (i, _slug) in slug_chain.iter().enumerate() {
        let depth_chain = &slug_chain[..=i];
        let found = search_in
            .iter()
            .find(|n| n.slug_chain.as_slice() == depth_chain);
        match found {
            Some(n) => {
                let href = if i + 1 == slug_chain.len() {
                    None
                } else {
                    Some(format!(
                        "{}{}",
                        asset_prefix,
                        paths::tag_page_path(&n.slug_chain)
                    ))
                };
                out.push(Crumb {
                    label: n.name.clone(),
                    href,
                });
                current = Some(n);
                search_in = &n.children;
            }
            None => break,
        }
    }
    let _ = current; // appease unused
    out
}

// ----- security -----

/// What a `<dl>` of OAuth2 scopes shows.
#[derive(Serialize, Clone, Debug)]
pub struct ScopeView {
    pub name: String,
    pub description: String,
}

/// One OAuth2 flow, rendered as a `<section>` on the security page.
#[derive(Serialize, Clone, Debug)]
pub struct OAuth2FlowView {
    pub kind: &'static str,
    pub authorization_url: Option<String>,
    pub token_url: Option<String>,
    pub refresh_url: Option<String>,
    pub scopes: Vec<ScopeView>,
}

fn oauth2_flow_view(f: &OAuth2Flow) -> OAuth2FlowView {
    OAuth2FlowView {
        kind: match f.kind {
            OAuth2FlowKind::Implicit => "implicit",
            OAuth2FlowKind::Password => "password",
            OAuth2FlowKind::ClientCredentials => "client-credentials",
            OAuth2FlowKind::AuthorizationCode => "authorization-code",
        },
        authorization_url: f.authorization_url.clone(),
        token_url: f.token_url.clone(),
        refresh_url: f.refresh_url.clone(),
        scopes: f
            .scopes
            .iter()
            .map(|(name, description)| ScopeView {
                name: name.clone(),
                description: description.clone(),
            })
            .collect(),
    }
}

/// Parsed `x-token-exchange` extension on a security scheme. The
/// generator-rust-clap plugin uses the same shape. Only one
/// placeholder in `audience_template` is supported for now; that
/// placeholder is a path-parameter name we substitute at request
/// time from the operation's `path_params` inputs.
#[derive(Serialize, Clone, Debug)]
pub struct TokenExchangeView {
    pub audience_template: String,
    pub placeholder: String,
    pub extra_scope: Vec<String>,
}

/// Tagged enum-style view of `SecuritySchemeKind` for templates. We
/// keep the variant's payload on the view directly so the template
/// doesn't have to switch on an inner sub-object.
#[derive(Serialize, Clone, Debug)]
pub struct SecuritySchemeView {
    pub id: String,
    /// One of: `api-key`, `http-basic`, `http-bearer`, `mutual-tls`,
    /// `oauth2`, `open-id-connect`. Templates branch on this.
    pub kind: &'static str,
    pub description_html: Option<String>,
    pub deprecated: bool,
    pub href: String,
    // -- per-kind extras (only the matching one is populated) --
    pub api_key_name: Option<String>,
    pub api_key_location: Option<&'static str>,
    pub bearer_format: Option<String>,
    pub oauth2_flows: Vec<OAuth2FlowView>,
    pub open_id_connect_url: Option<String>,
    pub token_exchange: Option<TokenExchangeView>,
}

impl SecuritySchemeView {
    fn from_scheme(spec: &Ir, asset_prefix: &str, s: &SecurityScheme) -> Self {
        let mut view = SecuritySchemeView {
            id: s.id.clone(),
            kind: "",
            description_html: markdown::render_opt(s.description.as_deref()),
            deprecated: s.deprecated,
            href: format!("{}{}#scheme-{}", asset_prefix, paths::SECURITY_INDEX, s.id),
            api_key_name: None,
            api_key_location: None,
            bearer_format: None,
            oauth2_flows: Vec::new(),
            open_id_connect_url: None,
            token_exchange: parse_token_exchange(spec, s),
        };
        match &s.kind {
            SecuritySchemeKind::ApiKey(ApiKeyScheme { name, location }) => {
                view.kind = "api-key";
                view.api_key_name = Some(name.clone());
                view.api_key_location = Some(match location {
                    ApiKeyLocation::Header => "header",
                    ApiKeyLocation::Query => "query",
                    ApiKeyLocation::Cookie => "cookie",
                });
            }
            SecuritySchemeKind::HttpBasic => view.kind = "http-basic",
            SecuritySchemeKind::HttpBearer { bearer_format } => {
                view.kind = "http-bearer";
                view.bearer_format = bearer_format.clone();
            }
            SecuritySchemeKind::MutualTls => view.kind = "mutual-tls",
            SecuritySchemeKind::Oauth2(OAuth2Scheme { flows }) => {
                view.kind = "oauth2";
                view.oauth2_flows = flows.iter().map(oauth2_flow_view).collect();
            }
            SecuritySchemeKind::OpenIdConnect { url } => {
                view.kind = "open-id-connect";
                view.open_id_connect_url = Some(url.clone());
            }
        }
        view
    }
}

/// Parse the `x-token-exchange` extension on a scheme. Same wire shape
/// as `generator-rust-clap`'s `parse_token_exchange`:
/// `{ "audience-template": "...", "scope": [...] }`.
fn parse_token_exchange(spec: &Ir, s: &SecurityScheme) -> Option<TokenExchangeView> {
    let (_, vref) = s.extensions.iter().find(|(k, _)| k == "x-token-exchange")?;
    let json = values_ext::resolve_to_serde(&spec.values, *vref);
    let obj = json.as_object()?;
    let audience_template = obj.get("audience-template")?.as_str()?.to_string();
    let placeholders = extract_template_placeholders(&audience_template);
    if placeholders.len() != 1 {
        // Multi-placeholder audience templates are out of scope for
        // this milestone; the clap generator has the same restriction.
        return None;
    }
    let placeholder = placeholders.into_iter().next().unwrap();
    let extra_scope: Vec<String> = obj
        .get("scope")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Some(TokenExchangeView {
        audience_template,
        placeholder,
        extra_scope,
    })
}

fn extract_template_placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    break;
                }
                name.push(c2);
            }
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// "Requires <X> with scopes [...]" — what's listed on an op page.
#[derive(Serialize, Clone, Debug)]
pub struct SecurityRequirementView {
    pub scheme_id: String,
    pub scheme_kind: &'static str,
    pub scopes: Vec<String>,
    pub href: String,
}

/// Per-op auth block: a list of `[req]` alternatives (each is the
/// AND-combination — meet all of these to access the op) plus an
/// `inherited` flag when the op falls back to document-level security.
#[derive(Serialize, Clone, Debug, Default)]
pub struct OpSecurityView {
    pub alternatives: Vec<Vec<SecurityRequirementView>>,
    pub inherited: bool,
}

/// Resolves an operation's effective security: its own `security` if
/// declared, else the document-level fallback.
pub fn op_security_view(asset_prefix: &str, spec: &Ir, op: &Operation) -> Option<OpSecurityView> {
    let (reqs, inherited) = if !op.security.is_empty() {
        (op.security.as_slice(), false)
    } else if !spec_doc_security_is_empty(spec) {
        // OAS expresses doc-level security on the Ir's root, but the
        // current `forge-ir` 0.1.13 doesn't surface it as a dedicated
        // field. We mirror the behaviour by treating the *first*
        // operation that explicitly declares an empty `security: []`
        // override as "anonymous"; everything else inherits whichever
        // doc-level set the parser flattened onto each op (since the
        // parser already inlines inheritance, `op.security` is the
        // effective list — see forge-parser).
        (op.security.as_slice(), true)
    } else {
        return None;
    };
    if reqs.is_empty() {
        return None;
    }
    let scheme_by_id: BTreeMap<&str, &SecurityScheme> = spec
        .security_schemes
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    // OAS security is a list of OR alternatives, each AND'd from one
    // or more requirements. forge-ir flattens it into a single
    // `Vec<SecurityRequirement>`; per the WIT shape we don't get the
    // outer OR structure. Treat each requirement as its own
    // alternative for now (the most permissive read; matches what
    // SwaggerUI shows when in doubt).
    let alternatives: Vec<Vec<SecurityRequirementView>> = reqs
        .iter()
        .map(|r| vec![requirement_view(asset_prefix, &scheme_by_id, r)])
        .collect();
    Some(OpSecurityView {
        alternatives,
        inherited,
    })
}

fn spec_doc_security_is_empty(_spec: &Ir) -> bool {
    // Placeholder — see op_security_view's comment. The parser already
    // flattens doc-level security onto each op, so there's no separate
    // root field to inspect on `Ir` itself.
    true
}

fn requirement_view(
    asset_prefix: &str,
    scheme_by_id: &BTreeMap<&str, &SecurityScheme>,
    r: &SecurityRequirement,
) -> SecurityRequirementView {
    let scheme_kind = scheme_by_id
        .get(r.scheme_id.as_str())
        .map_or("unknown", |s| match s.kind {
            SecuritySchemeKind::ApiKey(_) => "api-key",
            SecuritySchemeKind::HttpBasic => "http-basic",
            SecuritySchemeKind::HttpBearer { .. } => "http-bearer",
            SecuritySchemeKind::MutualTls => "mutual-tls",
            SecuritySchemeKind::Oauth2(_) => "oauth2",
            SecuritySchemeKind::OpenIdConnect { .. } => "open-id-connect",
        });
    SecurityRequirementView {
        scheme_id: r.scheme_id.clone(),
        scheme_kind,
        scopes: r.scopes.clone(),
        href: format!(
            "{}{}#scheme-{}",
            asset_prefix,
            paths::SECURITY_INDEX,
            r.scheme_id
        ),
    }
}

// ----- type refs -----

#[derive(Serialize, Clone, Debug)]
pub struct TypeRefView {
    pub display: String,
    /// Relative href to the schema page if this resolves to a named
    /// non-primitive type; else `None`.
    pub href: Option<String>,
    pub is_link: bool,
}

/// Maximum depth at which we inline synthetic schemas under a parent.
/// Beyond this, we fall back to the bare typeref (rendering deeper
/// chains would balloon page size; the depth cap also kills any
/// pathological self-recursion that slipped past the IR's
/// acyclic-types invariant).
const INLINE_DEPTH_LIMIT: usize = 4;

/// One inline property inside an inlined synthetic object.
#[derive(Serialize, Clone, Debug)]
pub struct InlineProperty {
    pub name: String,
    pub required: bool,
    pub deprecated: bool,
    pub description_html: Option<String>,
    pub r#type: TypeRefView,
    /// Nested inline shape when this property's type is itself
    /// synthetic. Bounded by [`INLINE_DEPTH_LIMIT`].
    pub inline: Option<Box<InlineSchemaView>>,
}

/// Inlined shape of a synthetic schema — rendered under a `<details>`
/// on its parent's page so the reader sees "what is this type" without
/// a page round-trip.
#[derive(Serialize, Clone, Debug, Default)]
pub struct InlineSchemaView {
    pub kind: &'static str,
    pub description_html: Option<String>,
    pub properties: Vec<InlineProperty>,
    /// `"forbidden"` / `"any"` / `"typed"` / `""` (the empty string
    /// when the kind is not object). Mirrors the schema page's
    /// rendering of `AdditionalProperties`.
    pub additional_properties_kind: &'static str,
    pub additional_properties_type: Option<TypeRefView>,
    /// For arrays: the items' typeref + (optionally) its own inline shape.
    pub items: Option<TypeRefView>,
    pub items_inline: Option<Box<InlineSchemaView>>,
    pub enum_values: Vec<String>,
    pub union_variants: Vec<TypeRefView>,
    pub union_kind: &'static str,
    pub discriminator: Option<DiscriminatorView>,
}

/// Display label for a synthetic NamedType, derived from its
/// underlying kind. We don't emit pages for synthetics (their ids
/// like `ErrorResponseV2_property_traceId` would just confuse a
/// reader), so the label is structural: `object`, `enum`,
/// `string | null`, etc. The structural detail lives in the
/// `inline_type` partial.
fn synthetic_display(spec: &Ir, asset_prefix: &str, t: &NamedType) -> String {
    match &t.definition {
        TypeDef::Primitive(p) => {
            // Defensive — `render_typeref` already handles primitive
            // synthetics via its Primitive arm.
            let kind = match p.kind {
                forge_plugin_sdk::ir::PrimitiveKind::String => "string",
                forge_plugin_sdk::ir::PrimitiveKind::Integer => "integer",
                forge_plugin_sdk::ir::PrimitiveKind::Number => "number",
                forge_plugin_sdk::ir::PrimitiveKind::Bool => "boolean",
            };
            kind.to_string()
        }
        TypeDef::Null => "null".to_string(),
        TypeDef::Object(_) => "object".to_string(),
        TypeDef::Array(a) => {
            let inner = render_typeref(spec, asset_prefix, &a.items);
            format!("[{}]", inner.display)
        }
        TypeDef::EnumString(_) => "enum<string>".to_string(),
        TypeDef::EnumInt(_) => "enum<integer>".to_string(),
        TypeDef::Union(u) => {
            // List the variants joined by `|`. Each variant's display
            // recurses through `render_typeref`, so a nested `T | null`
            // shows as `T | null` and not the synthetic id.
            u.variants
                .iter()
                .map(|v| render_typeref(spec, asset_prefix, &v.r#type).display)
                .collect::<Vec<_>>()
                .join(" | ")
        }
    }
}

/// Resolve `tref`. When it's a synthetic non-primitive, also return
/// the inlined shape so the parent template can expand it.
pub fn render_typeref_with_inline(
    spec: &Ir,
    asset_prefix: &str,
    tref: &TypeRef,
) -> (TypeRefView, Option<InlineSchemaView>) {
    let view = render_typeref(spec, asset_prefix, tref);
    let inline = build_inline_for(spec, asset_prefix, tref, 0);
    (view, inline)
}

/// Build an inline view of `tref` if it's a synthetic non-primitive
/// (and we're under the depth limit).
fn build_inline_for(
    spec: &Ir,
    asset_prefix: &str,
    tref: &TypeRef,
    depth: usize,
) -> Option<InlineSchemaView> {
    if depth >= INLINE_DEPTH_LIMIT {
        return None;
    }
    let t = spec.types.iter().find(|t| t.id == *tref)?;
    if !schema_filter::is_synthetic_id(&t.id) {
        return None;
    }
    // Primitives and null have no structure to expand — the bare
    // type display in the parent already says everything.
    if matches!(t.definition, TypeDef::Primitive(_) | TypeDef::Null) {
        return None;
    }
    Some(inline_schema_view(spec, asset_prefix, t, depth))
}

fn inline_schema_view(
    spec: &Ir,
    asset_prefix: &str,
    t: &NamedType,
    depth: usize,
) -> InlineSchemaView {
    let mut view = InlineSchemaView {
        kind: "",
        // The parent (property / parameter / response) already
        // renders the description in its <dd>; rendering it again
        // inside the inline expansion duplicates it.
        description_html: None,
        properties: Vec::new(),
        additional_properties_kind: "",
        additional_properties_type: None,
        items: None,
        items_inline: None,
        enum_values: Vec::new(),
        union_variants: Vec::new(),
        union_kind: "",
        discriminator: None,
    };
    match &t.definition {
        TypeDef::Primitive(_) | TypeDef::Null => {
            // Caller shouldn't ask for inline expansion on primitives /
            // null, but be defensive: empty view means "nothing extra
            // to show".
        }
        TypeDef::Object(o) => {
            view.kind = "object";
            view.properties = o
                .properties
                .iter()
                .map(|p| InlineProperty {
                    name: p.name.clone(),
                    required: p.required,
                    deprecated: p.deprecated,
                    description_html: markdown::render_opt(p.description.as_deref()),
                    r#type: render_typeref(spec, asset_prefix, &p.r#type),
                    inline: build_inline_for(spec, asset_prefix, &p.r#type, depth + 1)
                        .map(Box::new),
                })
                .collect();
            match &o.additional_properties {
                AdditionalProperties::Forbidden => {
                    view.additional_properties_kind = "forbidden";
                }
                AdditionalProperties::Any => {
                    view.additional_properties_kind = "any";
                }
                AdditionalProperties::Typed { r#type } => {
                    view.additional_properties_kind = "typed";
                    view.additional_properties_type =
                        Some(render_typeref(spec, asset_prefix, r#type));
                }
            }
        }
        TypeDef::Array(a) => {
            view.kind = "array";
            view.items = Some(render_typeref(spec, asset_prefix, &a.items));
            view.items_inline =
                build_inline_for(spec, asset_prefix, &a.items, depth + 1).map(Box::new);
        }
        TypeDef::EnumString(e) => {
            view.kind = "enum-string";
            view.enum_values = e.values.iter().map(|v| v.value.clone()).collect();
        }
        TypeDef::EnumInt(e) => {
            view.kind = "enum-int";
            view.enum_values = e.values.iter().map(|v| v.value.to_string()).collect();
        }
        TypeDef::Union(u) => {
            view.kind = "union";
            view.union_kind = match u.kind {
                forge_plugin_sdk::ir::UnionKind::OneOf => "one-of",
                forge_plugin_sdk::ir::UnionKind::AnyOf => "any-of",
            };
            view.union_variants = u
                .variants
                .iter()
                .map(|v| render_typeref(spec, asset_prefix, &v.r#type))
                .collect();
            view.discriminator = u
                .discriminator
                .as_ref()
                .map(|d| discriminator_view(spec, asset_prefix, d));
        }
    }
    view
}

pub fn render_typeref(spec: &Ir, asset_prefix: &str, tref: &TypeRef) -> TypeRefView {
    if tref.is_empty() {
        return TypeRefView {
            display: "any".into(),
            href: None,
            is_link: false,
        };
    }
    let named = spec.types.iter().find(|t| t.id == *tref);
    match named {
        Some(t) => match &t.definition {
            TypeDef::Primitive(p) => {
                let kind = match p.kind {
                    forge_plugin_sdk::ir::PrimitiveKind::String => "string",
                    forge_plugin_sdk::ir::PrimitiveKind::Integer => "integer",
                    forge_plugin_sdk::ir::PrimitiveKind::Number => "number",
                    forge_plugin_sdk::ir::PrimitiveKind::Bool => "boolean",
                };
                let display = match p.constraints.format_extension.as_deref() {
                    Some(fmt) => format!("{}<{}>", kind, fmt),
                    None => kind.to_string(),
                };
                TypeRefView {
                    display,
                    href: None,
                    is_link: false,
                }
            }
            TypeDef::Array(a) => {
                let inner = render_typeref(spec, asset_prefix, &a.items);
                TypeRefView {
                    display: format!("[{}]", inner.display),
                    href: inner.href,
                    is_link: inner.is_link,
                }
            }
            TypeDef::Null => TypeRefView {
                display: "null".into(),
                href: None,
                is_link: false,
            },
            _ => {
                if schema_filter::is_synthetic_id(&t.id) {
                    // Synthetic types don't get their own page; the
                    // structure renders in an inline `<details>`
                    // under the parent (`inline_type`). The label
                    // here is derived from the *underlying kind*, not
                    // the synthetic id, so readers see "object" /
                    // "string | null" rather than
                    // "ErrorResponseV2_property_traceId".
                    TypeRefView {
                        display: synthetic_display(spec, asset_prefix, t),
                        href: None,
                        is_link: false,
                    }
                } else {
                    let label = t.title.as_deref().unwrap_or(t.id.as_str()).to_owned();
                    let href = format!("{}{}", asset_prefix, paths::schema_page_path(&t.id));
                    TypeRefView {
                        display: label,
                        href: Some(href),
                        is_link: true,
                    }
                }
            }
        },
        None => TypeRefView {
            display: tref.clone(),
            href: None,
            is_link: false,
        },
    }
}

// ----- parameters / examples -----

#[derive(Serialize)]
pub struct ParamView {
    pub name: String,
    pub location: &'static str,
    pub r#type: TypeRefView,
    pub inline_type: Option<InlineSchemaView>,
    pub required: bool,
    pub deprecated: bool,
    pub description_html: Option<String>,
    pub example: Option<ExampleView>,
}

/// One example, pre-rendered: the raw payload as a `String` plus the
/// same payload with JSON-token spans applied. Templates emit both —
/// the highlighted HTML inside `<pre>`, the raw string in a
/// `data-copy-source` attribute on the copy button.
#[derive(Serialize, Clone)]
pub struct ExampleView {
    pub raw: String,
    pub html: String,
}

fn first_example(
    spec: &Ir,
    examples: &[(String, forge_plugin_sdk::ir::Example)],
) -> Option<ExampleView> {
    let (_, ex) = examples.first()?;
    if let Some(s) = &ex.serialized_value {
        return Some(example_view(s.clone()));
    }
    if let Some(r) = ex.data_value.or(ex.value) {
        return Some(example_view(values_ext::to_json_pretty(&spec.values, r)));
    }
    None
}

fn example_view(raw: String) -> ExampleView {
    let html = highlight::highlight_json(&raw);
    ExampleView { raw, html }
}

fn param_view(spec: &Ir, asset_prefix: &str, p: &Parameter, location: &'static str) -> ParamView {
    let (r#type, inline_type) = render_typeref_with_inline(spec, asset_prefix, &p.r#type);
    ParamView {
        name: p.name.clone(),
        location,
        r#type,
        inline_type,
        required: p.required,
        deprecated: p.deprecated,
        description_html: markdown::render_opt(p.description.as_deref()),
        example: first_example(spec, &p.examples),
    }
}

// ----- body / response -----

#[derive(Serialize)]
pub struct MediaTypeView {
    pub media_type: String,
    pub r#type: TypeRefView,
    pub inline_type: Option<InlineSchemaView>,
    pub example: Option<ExampleView>,
    /// When the media type's schema is a discriminated union, the
    /// disambiguation rule is inlined here so callers see it without
    /// clicking through to the schema page.
    pub discriminator: Option<DiscriminatorView>,
}

fn body_content_view(spec: &Ir, asset_prefix: &str, c: &BodyContent) -> MediaTypeView {
    let (r#type, inline_type) = render_typeref_with_inline(spec, asset_prefix, &c.r#type);
    MediaTypeView {
        media_type: c.media_type.clone(),
        r#type,
        inline_type,
        example: first_example(spec, &c.examples),
        discriminator: discriminator_for_typeref(spec, asset_prefix, &c.r#type),
    }
}

#[derive(Serialize)]
pub struct BodyView {
    pub required: bool,
    pub description_html: Option<String>,
    pub content: Vec<MediaTypeView>,
}

fn body_view(spec: &Ir, asset_prefix: &str, b: &Body) -> BodyView {
    BodyView {
        required: b.required,
        description_html: markdown::render_opt(b.description.as_deref()),
        content: b
            .content
            .iter()
            .map(|c| body_content_view(spec, asset_prefix, c))
            .collect(),
    }
}

#[derive(Serialize)]
pub struct HeaderView {
    pub name: String,
    pub r#type: TypeRefView,
    pub inline_type: Option<InlineSchemaView>,
    pub required: bool,
    pub deprecated: bool,
    pub description_html: Option<String>,
}

fn header_view(spec: &Ir, asset_prefix: &str, name: &str, h: &Header) -> HeaderView {
    let (r#type, inline_type) = render_typeref_with_inline(spec, asset_prefix, &h.r#type);
    HeaderView {
        name: name.into(),
        r#type,
        inline_type,
        required: h.required,
        deprecated: h.deprecated,
        description_html: markdown::render_opt(h.description.as_deref()),
    }
}

#[derive(Serialize)]
pub struct ResponseView {
    pub status: String,
    pub status_class: &'static str,
    pub summary: Option<String>,
    pub description_html: Option<String>,
    pub content: Vec<MediaTypeView>,
    pub headers: Vec<HeaderView>,
}

fn response_view(spec: &Ir, asset_prefix: &str, r: &Response) -> ResponseView {
    let (status, status_class) = match &r.status {
        ResponseStatus::Explicit { code } => (code.to_string(), status_class(*code)),
        ResponseStatus::Default => ("default".into(), "status-default"),
        ResponseStatus::Range { class } => (
            format!("{}xx", class),
            match class {
                1 => "status-1xx",
                2 => "status-2xx",
                3 => "status-3xx",
                4 => "status-4xx",
                5 => "status-5xx",
                _ => "status-default",
            },
        ),
    };
    ResponseView {
        status,
        status_class,
        summary: r.summary.clone(),
        description_html: markdown::render_opt(r.description.as_deref()),
        content: r
            .content
            .iter()
            .map(|c| body_content_view(spec, asset_prefix, c))
            .collect(),
        headers: r
            .headers
            .iter()
            .map(|(name, h)| header_view(spec, asset_prefix, name, h))
            .collect(),
    }
}

fn status_class(code: u16) -> &'static str {
    match code / 100 {
        1 => "status-1xx",
        2 => "status-2xx",
        3 => "status-3xx",
        4 => "status-4xx",
        5 => "status-5xx",
        _ => "status-default",
    }
}

// ----- operation -----

#[derive(Serialize)]
pub struct PathSegment {
    pub text: String,
    pub is_param: bool,
}

fn split_path(template: &str) -> Vec<PathSegment> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_param = false;
    for ch in template.chars() {
        match (ch, in_param) {
            ('{', false) => {
                if !buf.is_empty() {
                    out.push(PathSegment {
                        text: std::mem::take(&mut buf),
                        is_param: false,
                    });
                }
                in_param = true;
                buf.push(ch);
            }
            ('}', true) => {
                buf.push(ch);
                out.push(PathSegment {
                    text: std::mem::take(&mut buf),
                    is_param: true,
                });
                in_param = false;
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        out.push(PathSegment {
            text: buf,
            is_param: in_param,
        });
    }
    out
}

#[derive(Serialize)]
pub struct ExternalDocsView {
    pub url: String,
    pub description: Option<String>,
}

fn ext_docs_view(d: &ExternalDocs) -> ExternalDocsView {
    ExternalDocsView {
        url: d.url.clone(),
        description: d.description.clone(),
    }
}

#[derive(Serialize)]
pub struct TagLink {
    pub name: String,
    pub href: String,
}

#[derive(Serialize)]
pub struct OperationView {
    pub id: String,
    pub method: String,
    pub method_class: String,
    pub path_segments: Vec<PathSegment>,
    pub path_template: String,
    pub summary: Option<String>,
    pub description_html: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<ParamView>,
    pub request_body: Option<BodyView>,
    pub responses: Vec<ResponseView>,
    pub external_docs: Option<ExternalDocsView>,
    pub tag_links: Vec<TagLink>,
    pub security: Option<OpSecurityView>,
    /// When `enableTryIt` is on, this is the request-body example
    /// (if any) the form pre-fills its textarea with.
    pub try_it_body_seed: Option<String>,
    /// Honor the `enableTryIt` config knob.
    pub try_it_enabled: bool,
    /// When the op declares a security requirement on a scheme with an
    /// `x-token-exchange` extension AND the op's path declares a param
    /// matching the audience-template placeholder, the try-it form
    /// will RFC-8693 the user's access token before sending.
    pub token_exchange_marker: Option<OpTokenExchangeView>,
}

/// Per-op marker for the try-it form: which scheme to read the
/// subject_token from, which path-param to substitute into the
/// audience template, and the template itself.
#[derive(Serialize, Clone, Debug)]
pub struct OpTokenExchangeView {
    pub scheme_id: String,
    pub audience_template: String,
    pub placeholder: String,
    pub extra_scope: Vec<String>,
}

pub fn operation_view(
    spec: &Ir,
    nav: &Nav,
    cfg: &Config,
    asset_prefix: &str,
    op: &Operation,
) -> OperationView {
    let mut parameters = Vec::new();
    for p in &op.path_params {
        parameters.push(param_view(spec, asset_prefix, p, "path"));
    }
    for p in &op.query_params {
        parameters.push(param_view(spec, asset_prefix, p, "query"));
    }
    for p in &op.header_params {
        parameters.push(param_view(spec, asset_prefix, p, "header"));
    }
    for p in &op.cookie_params {
        parameters.push(param_view(spec, asset_prefix, p, "cookie"));
    }
    for p in &op.querystring_params {
        parameters.push(param_view(spec, asset_prefix, p, "querystring"));
    }
    let tag_links = op
        .tags
        .iter()
        .filter_map(|name| find_tag(&nav.roots, name).map(|t| (name, t)))
        .map(|(name, t)| TagLink {
            name: name.clone(),
            href: format!("{}{}", asset_prefix, paths::tag_page_path(&t.slug_chain)),
        })
        .collect();
    OperationView {
        id: op.id.clone(),
        method: op.method.as_str().to_owned(),
        method_class: method_class(op.method.as_str()).to_owned(),
        path_segments: split_path(&op.path_template),
        path_template: op.path_template.clone(),
        summary: op.summary.clone(),
        description_html: markdown::render_opt(op.description.as_deref()),
        deprecated: op.deprecated,
        parameters,
        request_body: op
            .request_body
            .as_ref()
            .map(|b| body_view(spec, asset_prefix, b)),
        responses: op
            .responses
            .iter()
            .map(|r| response_view(spec, asset_prefix, r))
            .collect(),
        external_docs: op.external_docs.as_ref().map(ext_docs_view),
        tag_links,
        security: op_security_view(asset_prefix, spec, op),
        try_it_enabled: cfg.enable_try_it,
        try_it_body_seed: op.request_body.as_ref().and_then(|b| {
            b.content
                .iter()
                .find_map(|c| first_example(spec, &c.examples).map(|ex| ex.raw))
        }),
        token_exchange_marker: op_token_exchange_marker(spec, op),
    }
}

fn op_token_exchange_marker(spec: &Ir, op: &Operation) -> Option<OpTokenExchangeView> {
    // Walk the op's declared security and find the first scheme with
    // an x-token-exchange extension whose placeholder is satisfied by
    // one of the op's path params (case-insensitive). Without a
    // path-param match the exchange has nothing to substitute, so we
    // skip — the bare scheme token will get sent unchanged.
    for req in &op.security {
        let Some(scheme) = spec.security_schemes.iter().find(|s| s.id == req.scheme_id) else {
            continue;
        };
        let Some(ex) = parse_token_exchange(spec, scheme) else {
            continue;
        };
        let normalized = ex.placeholder.to_lowercase();
        let matches_path = op
            .path_params
            .iter()
            .any(|p| p.name.to_lowercase() == normalized);
        if !matches_path {
            continue;
        }
        return Some(OpTokenExchangeView {
            scheme_id: scheme.id.clone(),
            audience_template: ex.audience_template,
            placeholder: ex.placeholder,
            extra_scope: ex.extra_scope,
        });
    }
    None
}

pub fn find_tag<'a>(roots: &'a [crate::nav::NavTag], name: &str) -> Option<&'a crate::nav::NavTag> {
    for r in roots {
        if r.name == name {
            return Some(r);
        }
        if let Some(found) = find_tag(&r.children, name) {
            return Some(found);
        }
    }
    None
}

// ----- schema -----

#[derive(Serialize)]
pub struct PropertyView {
    pub name: String,
    pub r#type: TypeRefView,
    pub inline_type: Option<InlineSchemaView>,
    pub required: bool,
    pub deprecated: bool,
    pub description_html: Option<String>,
}

#[derive(Serialize, Default)]
pub struct SchemaView {
    pub id: String,
    pub title: Option<String>,
    pub description_html: Option<String>,
    pub deprecated: bool,
    pub kind: &'static str,
    pub properties: Vec<PropertyView>,
    pub additional_properties_kind: &'static str,
    pub additional_properties_type: Option<TypeRefView>,
    pub array_items: Option<TypeRefView>,
    pub enum_values: Vec<String>,
    pub union_variants: Vec<UnionVariantView>,
    pub union_kind: &'static str,
    pub discriminator: Option<DiscriminatorView>,
    pub example: Option<ExampleView>,
    pub used_in: Vec<OperationLink>,
}

/// One row in a discriminated union's mapping table.
#[derive(Serialize, Clone, Debug)]
pub struct DiscriminatorMappingEntry {
    pub tag: String,
    pub r#type: TypeRefView,
}

/// View for a `discriminator` block. Carried on schemas AND inlined
/// into request/response sections when the body type is a
/// discriminated union, so callers immediately see the disambiguation
/// rule without bouncing to the schema page.
#[derive(Serialize, Clone, Debug)]
pub struct DiscriminatorView {
    pub property_name: String,
    pub mapping: Vec<DiscriminatorMappingEntry>,
    /// `true` when the union's discriminator block lists a mapping
    /// table; `false` for unions that name a discriminator property
    /// without enumerating tag→type — those still render the call-out
    /// but with no mapping table.
    pub has_mapping: bool,
}

fn discriminator_view(spec: &Ir, asset_prefix: &str, d: &Discriminator) -> DiscriminatorView {
    let mapping: Vec<DiscriminatorMappingEntry> = d
        .mapping
        .iter()
        .map(|(tag, tref)| DiscriminatorMappingEntry {
            tag: tag.clone(),
            r#type: render_typeref(spec, asset_prefix, tref),
        })
        .collect();
    DiscriminatorView {
        property_name: d.property_name.clone(),
        has_mapping: !mapping.is_empty(),
        mapping,
    }
}

/// A variant of a union, with its optional explicit tag (`UnionVariant.tag`).
#[derive(Serialize, Clone, Debug)]
pub struct UnionVariantView {
    pub r#type: TypeRefView,
    pub tag: Option<String>,
}

/// If `tref` resolves to a discriminated union — directly OR through
/// one level of array wrapping (`[PetEvent]` is just as worth
/// showing the discriminator as `PetEvent`) — return the view. Used
/// by the operation template to inline the discriminator callout on
/// request/response sections.
pub fn discriminator_for_typeref(
    spec: &Ir,
    asset_prefix: &str,
    tref: &TypeRef,
) -> Option<DiscriminatorView> {
    let mut current: &str = tref.as_str();
    // Peel through up to a few array wrappers — guards against silly
    // self-referential chains while still handling the common
    // `[Wrapper]` and `[[Item]]` cases.
    for _ in 0..4 {
        let t = spec.types.iter().find(|t| t.id == current)?;
        match &t.definition {
            TypeDef::Union(u) => {
                let d = u.discriminator.as_ref()?;
                return Some(discriminator_view(spec, asset_prefix, d));
            }
            TypeDef::Array(a) => {
                current = a.items.as_str();
            }
            _ => return None,
        }
    }
    None
}

#[derive(Serialize)]
pub struct OperationLink {
    pub id: String,
    pub method: String,
    pub method_class: String,
    pub path_template: String,
    pub href: String,
}

pub fn schema_view(
    spec: &Ir,
    asset_prefix: &str,
    used_in: &UsedInIndex,
    t: &NamedType,
) -> SchemaView {
    let mut view = SchemaView {
        id: t.id.clone(),
        title: t.title.clone(),
        description_html: markdown::render_opt(t.description.as_deref()),
        deprecated: t.deprecated,
        kind: "",
        properties: Vec::new(),
        additional_properties_kind: "",
        additional_properties_type: None,
        array_items: None,
        enum_values: Vec::new(),
        union_variants: Vec::new(),
        union_kind: "",
        discriminator: None,
        example: first_example(spec, &t.examples),
        used_in: used_in_links(spec, asset_prefix, used_in.ops_referencing(&t.id).collect()),
    };
    match &t.definition {
        TypeDef::Primitive(p) => {
            view.kind = "primitive";
            let kind_name = match p.kind {
                forge_plugin_sdk::ir::PrimitiveKind::String => "string",
                forge_plugin_sdk::ir::PrimitiveKind::Integer => "integer",
                forge_plugin_sdk::ir::PrimitiveKind::Number => "number",
                forge_plugin_sdk::ir::PrimitiveKind::Bool => "boolean",
            };
            view.array_items = Some(TypeRefView {
                display: kind_name.into(),
                href: None,
                is_link: false,
            });
        }
        TypeDef::Object(o) => {
            view.kind = "object";
            view.properties = o
                .properties
                .iter()
                .map(|p: &Property| {
                    let (r#type, inline_type) =
                        render_typeref_with_inline(spec, asset_prefix, &p.r#type);
                    PropertyView {
                        name: p.name.clone(),
                        r#type,
                        inline_type,
                        required: p.required,
                        deprecated: p.deprecated,
                        description_html: markdown::render_opt(p.description.as_deref()),
                    }
                })
                .collect();
            match &o.additional_properties {
                AdditionalProperties::Forbidden => {
                    view.additional_properties_kind = "forbidden";
                }
                AdditionalProperties::Any => {
                    view.additional_properties_kind = "any";
                }
                AdditionalProperties::Typed { r#type } => {
                    view.additional_properties_kind = "typed";
                    view.additional_properties_type =
                        Some(render_typeref(spec, asset_prefix, r#type));
                }
            }
        }
        TypeDef::Array(a) => {
            view.kind = "array";
            view.array_items = Some(render_typeref(spec, asset_prefix, &a.items));
        }
        TypeDef::EnumString(e) => {
            view.kind = "enum-string";
            view.enum_values = e.values.iter().map(|v| v.value.clone()).collect();
        }
        TypeDef::EnumInt(e) => {
            view.kind = "enum-int";
            view.enum_values = e.values.iter().map(|v| v.value.to_string()).collect();
        }
        TypeDef::Union(u) => {
            view.kind = "union";
            view.union_kind = match u.kind {
                forge_plugin_sdk::ir::UnionKind::OneOf => "one-of",
                forge_plugin_sdk::ir::UnionKind::AnyOf => "any-of",
            };
            view.union_variants = u
                .variants
                .iter()
                .map(|v| UnionVariantView {
                    r#type: render_typeref(spec, asset_prefix, &v.r#type),
                    tag: v.tag.clone(),
                })
                .collect();
            view.discriminator = u
                .discriminator
                .as_ref()
                .map(|d| discriminator_view(spec, asset_prefix, d));
        }
        TypeDef::Null => {
            view.kind = "null";
        }
    }
    view
}

/// Reverse index from "type id" to "operation ids that transitively
/// reach this type". Computed once per generation pass via one BFS per
/// operation. Lookup is then O(log n) on the schema page, replacing a
/// per-page O(operations × types) walk that previously blew the wasm
/// fuel budget on real-world specs.
#[derive(Debug, Default)]
pub struct UsedInIndex {
    by_type: BTreeMap<String, BTreeSet<String>>,
}

impl UsedInIndex {
    pub fn ops_referencing(&self, type_id: &str) -> impl Iterator<Item = &String> {
        self.by_type.get(type_id).into_iter().flatten()
    }
}

pub fn build_used_in_index(spec: &Ir) -> UsedInIndex {
    let type_by_id: BTreeMap<&str, &NamedType> =
        spec.types.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut index = UsedInIndex::default();
    for op in &spec.operations {
        let mut frontier: VecDeque<&str> = VecDeque::new();
        for ps in [
            &op.path_params,
            &op.query_params,
            &op.header_params,
            &op.cookie_params,
            &op.querystring_params,
        ] {
            for p in ps {
                frontier.push_back(p.r#type.as_str());
            }
        }
        if let Some(b) = &op.request_body {
            for c in &b.content {
                frontier.push_back(c.r#type.as_str());
            }
        }
        for r in &op.responses {
            for c in &r.content {
                frontier.push_back(c.r#type.as_str());
            }
        }

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(tref) = frontier.pop_front() {
            if tref.is_empty() || !seen.insert(tref) {
                continue;
            }
            index
                .by_type
                .entry(tref.to_string())
                .or_default()
                .insert(op.id.clone());
            let Some(t) = type_by_id.get(tref) else {
                continue;
            };
            match &t.definition {
                TypeDef::Array(a) => frontier.push_back(a.items.as_str()),
                TypeDef::Object(o) => {
                    for p in &o.properties {
                        frontier.push_back(p.r#type.as_str());
                    }
                    if let AdditionalProperties::Typed { r#type } = &o.additional_properties {
                        frontier.push_back(r#type.as_str());
                    }
                }
                TypeDef::Union(u) => {
                    for v in &u.variants {
                        frontier.push_back(v.r#type.as_str());
                    }
                }
                _ => {}
            }
        }
    }
    index
}

fn used_in_links(spec: &Ir, asset_prefix: &str, op_ids: BTreeSet<&String>) -> Vec<OperationLink> {
    let mut out = Vec::new();
    for op in &spec.operations {
        if !op_ids.contains(&op.id) {
            continue;
        }
        out.push(OperationLink {
            id: op.id.clone(),
            method: op.method.as_str().into(),
            method_class: method_class(op.method.as_str()).into(),
            path_template: op.path_template.clone(),
            href: format!("{}{}", asset_prefix, paths::operation_page_path(&op.id)),
        });
    }
    out
}

// ----- info / server views -----

#[derive(Serialize)]
pub struct ServerView {
    pub url: String,
    pub name: Option<String>,
    pub description_html: Option<String>,
    pub variables: Vec<ServerVariableView>,
}

#[derive(Serialize)]
pub struct ServerVariableView {
    pub name: String,
    pub default: String,
    pub description_html: Option<String>,
    pub allowed: Vec<String>,
}

pub fn server_views(servers: &[forge_plugin_sdk::ir::Server]) -> Vec<ServerView> {
    servers
        .iter()
        .map(|s| ServerView {
            url: s.url.clone(),
            name: s.name.clone(),
            description_html: markdown::render_opt(s.description.as_deref()),
            variables: s
                .variables
                .iter()
                .map(|(name, v)| ServerVariableView {
                    name: name.clone(),
                    default: v.default.clone(),
                    description_html: markdown::render_opt(v.description.as_deref()),
                    allowed: v.r#enum.clone().unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
}

#[derive(Serialize)]
pub struct InfoView {
    pub title: String,
    pub version: String,
    pub summary: Option<String>,
    pub description_html: Option<String>,
    pub terms_of_service: Option<String>,
    pub contact_name: Option<String>,
    pub contact_url: Option<String>,
    pub contact_email: Option<String>,
    pub license_name: Option<String>,
    pub license_url: Option<String>,
    pub license_identifier: Option<String>,
}

pub fn info_view(spec: &Ir) -> InfoView {
    let info = &spec.info;
    InfoView {
        title: info.title.clone(),
        version: info.version.clone(),
        summary: info.summary.clone(),
        description_html: markdown::render_opt(info.description.as_deref()),
        terms_of_service: info.terms_of_service.clone(),
        contact_name: info.contact.as_ref().and_then(|c| c.name.clone()),
        contact_url: info.contact.as_ref().and_then(|c| c.url.clone()),
        contact_email: info.contact.as_ref().and_then(|c| c.email.clone()),
        license_name: info.license_name.clone(),
        license_url: info.license_url.clone(),
        license_identifier: info.license_identifier.clone(),
    }
}

// ----- rendering entry points -----

pub struct RenderError(pub minijinja::Error);

impl From<minijinja::Error> for RenderError {
    fn from(value: minijinja::Error) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::error::Error;
        write!(f, "{}", self.0)?;
        let mut source: Option<&dyn Error> = self.0.source();
        while let Some(s) = source {
            write!(f, " — caused by: {}", s)?;
            source = s.source();
        }
        Ok(())
    }
}

/// One page rendered to UTF-8 HTML.
pub struct Page {
    pub path: String,
    pub html: String,
}

#[derive(Serialize)]
struct LandingCtx<'a> {
    chrome: ChromeCtx<'a>,
    info: InfoView,
    servers: Vec<ServerView>,
    external_docs: Option<ExternalDocsView>,
    crumbs: Vec<Crumb>,
}

pub fn landing(
    env: &Environment<'_>,
    spec: &Ir,
    cfg: &Config,
    nav: &Nav,
) -> Result<Page, RenderError> {
    let current_path = "index.html";
    let info = info_view(spec);
    let description = spec
        .info
        .description
        .as_deref()
        .map(markdown::first_paragraph_text);
    let chrome = ChromeCtx::new(
        spec,
        cfg,
        nav,
        current_path,
        info.title.clone(),
        description,
    );
    let asset_prefix = chrome.asset_prefix.clone();
    let crumbs = vec![Crumb {
        label: "API".into(),
        href: None,
    }];
    let ctx = LandingCtx {
        chrome,
        info,
        servers: server_views(&spec.servers),
        external_docs: spec.external_docs.as_ref().map(ext_docs_view),
        crumbs,
    };
    let _ = asset_prefix;
    let html = env
        .get_template("index.html.j2")?
        .render(JValue::from_serialize(&ctx))?;
    Ok(Page {
        path: current_path.into(),
        html,
    })
}

#[derive(Serialize)]
struct TagCtx<'a> {
    chrome: ChromeCtx<'a>,
    tag: &'a crate::nav::NavTag,
    crumbs: Vec<Crumb>,
    child_links: Vec<ChildLink>,
}

#[derive(Serialize)]
pub struct ChildLink {
    pub name: String,
    pub summary: Option<String>,
    pub href: String,
    pub op_count: usize,
}

pub fn tag_page(
    env: &Environment<'_>,
    spec: &Ir,
    cfg: &Config,
    nav: &Nav,
    tag: &crate::nav::NavTag,
) -> Result<Page, RenderError> {
    let current_path = paths::tag_page_path(&tag.slug_chain);
    let description_text = tag
        .description_html
        .as_deref()
        .map(strip_tags)
        .filter(|s| !s.is_empty());
    let chrome = ChromeCtx::new(
        spec,
        cfg,
        nav,
        &current_path,
        tag.name.clone(),
        description_text,
    );
    let asset_prefix = chrome.asset_prefix.clone();
    let crumbs = crumbs_for_tag(nav, &tag.slug_chain, &asset_prefix);
    let child_links = tag
        .children
        .iter()
        .map(|c| ChildLink {
            name: c.name.clone(),
            summary: c.summary.clone(),
            href: format!("{}{}", asset_prefix, paths::tag_page_path(&c.slug_chain)),
            op_count: c.total_op_count,
        })
        .collect();
    let ctx = TagCtx {
        chrome,
        tag,
        crumbs,
        child_links,
    };
    let html = env
        .get_template("tag.html.j2")?
        .render(JValue::from_serialize(&ctx))?;
    Ok(Page {
        path: current_path,
        html,
    })
}

#[derive(Serialize)]
struct OperationCtx<'a> {
    chrome: ChromeCtx<'a>,
    op: OperationView,
    crumbs: Vec<Crumb>,
}

pub fn operation_page(
    env: &Environment<'_>,
    spec: &Ir,
    cfg: &Config,
    nav: &Nav,
    op: &Operation,
) -> Result<Page, RenderError> {
    let current_path = paths::operation_page_path(&op.id);
    let asset_prefix = paths::asset_prefix(&current_path);
    let view = operation_view(spec, nav, cfg, &asset_prefix, op);
    let title = view.summary.clone().unwrap_or_else(|| view.id.clone());
    let description = op
        .description
        .as_deref()
        .map(markdown::first_paragraph_text);
    let chrome = ChromeCtx::new(spec, cfg, nav, &current_path, title, description);
    let mut crumbs = crumbs_home(&asset_prefix);
    if let Some(first_tag) = op.tags.first() {
        if let Some(t) = find_tag(&nav.roots, first_tag) {
            for (i, _slug) in t.slug_chain.iter().enumerate() {
                if let Some(ancestor) = tag_at_depth(nav, &t.slug_chain[..=i]) {
                    crumbs.push(Crumb {
                        label: ancestor.name.clone(),
                        href: Some(format!(
                            "{}{}",
                            asset_prefix,
                            paths::tag_page_path(&ancestor.slug_chain)
                        )),
                    });
                }
            }
        }
    }
    crumbs.push(Crumb {
        label: op.id.clone(),
        href: None,
    });
    let ctx = OperationCtx {
        chrome,
        op: view,
        crumbs,
    };
    let html = env
        .get_template("operation.html.j2")?
        .render(JValue::from_serialize(&ctx))?;
    Ok(Page {
        path: current_path,
        html,
    })
}

fn tag_at_depth<'a>(nav: &'a Nav, chain: &[String]) -> Option<&'a crate::nav::NavTag> {
    let mut current: Option<&'a crate::nav::NavTag> = None;
    let mut search: &'a [crate::nav::NavTag] = &nav.roots;
    for (i, _slug) in chain.iter().enumerate() {
        let prefix = &chain[..=i];
        let found = search.iter().find(|n| n.slug_chain.as_slice() == prefix)?;
        current = Some(found);
        search = &found.children;
    }
    current
}

#[derive(Serialize)]
struct SchemaCtx<'a> {
    chrome: ChromeCtx<'a>,
    schema: SchemaView,
    crumbs: Vec<Crumb>,
}

pub fn schema_page(
    env: &Environment<'_>,
    spec: &Ir,
    cfg: &Config,
    nav: &Nav,
    used_in: &UsedInIndex,
    t: &NamedType,
) -> Result<Page, RenderError> {
    let current_path = paths::schema_page_path(&t.id);
    let asset_prefix = paths::asset_prefix(&current_path);
    let view = schema_view(spec, &asset_prefix, used_in, t);
    let title = view.title.clone().unwrap_or_else(|| view.id.clone());
    let description = t.description.as_deref().map(markdown::first_paragraph_text);
    let chrome = ChromeCtx::new(spec, cfg, nav, &current_path, title.clone(), description);
    let mut crumbs = crumbs_home(&asset_prefix);
    crumbs.push(Crumb {
        label: "Schemas".into(),
        href: None,
    });
    crumbs.push(Crumb {
        label: title,
        href: None,
    });
    let ctx = SchemaCtx {
        chrome,
        schema: view,
        crumbs,
    };
    let html = env
        .get_template("schema.html.j2")?
        .render(JValue::from_serialize(&ctx))?;
    Ok(Page {
        path: current_path,
        html,
    })
}

// ----- security page -----

#[derive(Serialize)]
struct SecurityCtx<'a> {
    chrome: ChromeCtx<'a>,
    schemes: Vec<SecuritySchemeView>,
    crumbs: Vec<Crumb>,
}

pub fn security_page(
    env: &Environment<'_>,
    spec: &Ir,
    cfg: &Config,
    nav: &Nav,
) -> Result<Page, RenderError> {
    let current_path = paths::SECURITY_INDEX;
    let asset_prefix = paths::asset_prefix(current_path);
    let chrome = ChromeCtx::new(
        spec,
        cfg,
        nav,
        current_path,
        "Security".into(),
        Some("Authentication schemes accepted by this API.".into()),
    );
    let schemes: Vec<SecuritySchemeView> = spec
        .security_schemes
        .iter()
        .map(|s| SecuritySchemeView::from_scheme(spec, &asset_prefix, s))
        .collect();
    let mut crumbs = crumbs_home(&asset_prefix);
    crumbs.push(Crumb {
        label: "Security".into(),
        href: None,
    });
    let ctx = SecurityCtx {
        chrome,
        schemes,
        crumbs,
    };
    let html = env
        .get_template("security.html.j2")?
        .render(JValue::from_serialize(&ctx))?;
    Ok(Page {
        path: current_path.into(),
        html,
    })
}

// ----- schemas index page -----

#[derive(Serialize)]
pub struct SchemaIndexEntry {
    pub id: String,
    pub title: String,
    pub kind: &'static str,
    pub description_text: Option<String>,
    pub href: String,
    pub deprecated: bool,
}

#[derive(Serialize)]
struct SchemasIndexCtx<'a> {
    chrome: ChromeCtx<'a>,
    entries: Vec<SchemaIndexEntry>,
    crumbs: Vec<Crumb>,
}

pub fn schemas_index_page(
    env: &Environment<'_>,
    spec: &Ir,
    cfg: &Config,
    nav: &Nav,
) -> Result<Page, RenderError> {
    let current_path = paths::SCHEMAS_INDEX;
    let asset_prefix = paths::asset_prefix(current_path);
    let chrome = ChromeCtx::new(
        spec,
        cfg,
        nav,
        current_path,
        "Schemas".into(),
        Some("Every named type declared by this API.".into()),
    );
    let mut entries: Vec<SchemaIndexEntry> = spec
        .types
        .iter()
        .filter(|t| schema_filter::is_user_facing(t))
        .map(|t| {
            let kind = match &t.definition {
                TypeDef::Object(_) => "object",
                TypeDef::Array(_) => "array",
                TypeDef::EnumString(_) | TypeDef::EnumInt(_) => "enum",
                TypeDef::Union(_) => "union",
                TypeDef::Primitive(_) => "primitive",
                TypeDef::Null => "null",
            };
            let title = t.title.clone().unwrap_or_else(|| t.id.clone());
            let description_text = t
                .description
                .as_deref()
                .map(markdown::first_paragraph_text)
                .filter(|s| !s.is_empty());
            SchemaIndexEntry {
                id: t.id.clone(),
                title,
                kind,
                description_text,
                href: format!("{}{}", asset_prefix, paths::schema_page_path(&t.id)),
                deprecated: t.deprecated,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    let mut crumbs = crumbs_home(&asset_prefix);
    crumbs.push(Crumb {
        label: "Schemas".into(),
        href: None,
    });
    let ctx = SchemasIndexCtx {
        chrome,
        entries,
        crumbs,
    };
    let html = env
        .get_template("schemas_index.html.j2")?
        .render(JValue::from_serialize(&ctx))?;
    Ok(Page {
        path: current_path.into(),
        html,
    })
}

/// Cheap tag stripper for synthesising `<meta name="description">` text
/// from rendered HTML. Not a security boundary — input is the
/// generator's own pulldown-cmark output.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match (ch, in_tag) {
            ('<', _) => in_tag = true,
            ('>', true) => in_tag = false,
            (_, false) => out.push(ch),
            _ => {}
        }
    }
    let trimmed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    trimmed
}
