//! View models passed to MiniJinja, plus the `Environment` wiring.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use forge_plugin_sdk::ir::{
    AdditionalProperties, Body, BodyContent, ExternalDocs, Header, Ir, NamedType, Operation,
    Parameter, Property, Response, ResponseStatus, TypeDef, TypeRef,
};
use forge_plugin_sdk::values_ext;
use minijinja::{Environment, Value as JValue};
use serde::Serialize;

use crate::config::Config;
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
    pub api_version: &'a str,
    pub nav: &'a Nav,
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
        let canonical = cfg
            .base_url
            .as_deref()
            .map(|b| format!("{}/{}", b.trim_end_matches('/'), current_path));
        Self {
            title,
            page_title,
            page_description,
            theme: cfg.theme.as_str(),
            canonical,
            current_path,
            asset_prefix,
            home_href,
            api_version: spec.info.version.as_str(),
            nav,
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

// ----- type refs -----

#[derive(Serialize, Clone, Debug)]
pub struct TypeRefView {
    pub display: String,
    /// Relative href to the schema page if this resolves to a named
    /// non-primitive type; else `None`.
    pub href: Option<String>,
    pub is_link: bool,
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
                let label = t.title.as_deref().unwrap_or(t.id.as_str()).to_owned();
                if schema_filter::is_synthetic_id(&t.id) {
                    // No emitted page — render as a non-link code span
                    // so we don't produce a dead href.
                    TypeRefView {
                        display: label,
                        href: None,
                        is_link: false,
                    }
                } else {
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
    pub required: bool,
    pub deprecated: bool,
    pub description_html: Option<String>,
    pub example_json: Option<String>,
}

fn first_example_json(
    spec: &Ir,
    examples: &[(String, forge_plugin_sdk::ir::Example)],
) -> Option<String> {
    let (_, ex) = examples.first()?;
    if let Some(s) = &ex.serialized_value {
        return Some(s.clone());
    }
    if let Some(r) = ex.data_value.or(ex.value) {
        return Some(values_ext::to_json_pretty(&spec.values, r));
    }
    None
}

fn param_view(spec: &Ir, asset_prefix: &str, p: &Parameter, location: &'static str) -> ParamView {
    ParamView {
        name: p.name.clone(),
        location,
        r#type: render_typeref(spec, asset_prefix, &p.r#type),
        required: p.required,
        deprecated: p.deprecated,
        description_html: markdown::render_opt(p.description.as_deref()),
        example_json: first_example_json(spec, &p.examples),
    }
}

// ----- body / response -----

#[derive(Serialize)]
pub struct MediaTypeView {
    pub media_type: String,
    pub r#type: TypeRefView,
    pub example_json: Option<String>,
}

fn body_content_view(spec: &Ir, asset_prefix: &str, c: &BodyContent) -> MediaTypeView {
    MediaTypeView {
        media_type: c.media_type.clone(),
        r#type: render_typeref(spec, asset_prefix, &c.r#type),
        example_json: first_example_json(spec, &c.examples),
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
    pub required: bool,
    pub deprecated: bool,
    pub description_html: Option<String>,
}

fn header_view(spec: &Ir, asset_prefix: &str, name: &str, h: &Header) -> HeaderView {
    HeaderView {
        name: name.into(),
        r#type: render_typeref(spec, asset_prefix, &h.r#type),
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
}

pub fn operation_view(spec: &Ir, nav: &Nav, asset_prefix: &str, op: &Operation) -> OperationView {
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
    }
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
    pub union_variants: Vec<TypeRefView>,
    pub union_kind: &'static str,
    pub example_json: Option<String>,
    pub used_in: Vec<OperationLink>,
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
        example_json: first_example_json(spec, &t.examples),
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
                .map(|p: &Property| PropertyView {
                    name: p.name.clone(),
                    r#type: render_typeref(spec, asset_prefix, &p.r#type),
                    required: p.required,
                    deprecated: p.deprecated,
                    description_html: markdown::render_opt(p.description.as_deref()),
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
                .map(|v| render_typeref(spec, asset_prefix, &v.r#type))
                .collect();
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

fn used_in_links(
    spec: &Ir,
    asset_prefix: &str,
    op_ids: BTreeSet<&String>,
) -> Vec<OperationLink> {
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
}

pub fn server_views(servers: &[forge_plugin_sdk::ir::Server]) -> Vec<ServerView> {
    servers
        .iter()
        .map(|s| ServerView {
            url: s.url.clone(),
            name: s.name.clone(),
            description_html: markdown::render_opt(s.description.as_deref()),
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
    let view = operation_view(spec, nav, &asset_prefix, op);
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
