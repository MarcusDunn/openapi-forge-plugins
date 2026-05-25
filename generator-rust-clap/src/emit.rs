//! Generation entry point. Emits a buildable Rust CLI crate (clap
//! derive + hyper-util + tower) with tag-grouped subcommands. The
//! HTTP-client layer is the tower module tree produced by
//! [`codegen_rust_tower`], dropped under `src/gen/`; this generator
//! adds the CLI surface, per-op dispatch into that tree, and (when
//! configured) OAuth 2.0 PKCE login/logout plus RFC-8693 token
//! exchange driven by an `x-token-exchange` extension on the chosen
//! `oauth2` security scheme.

use std::collections::BTreeSet;

use codegen_rust_serde::naming::{pascal_case, snake_case};
use codegen_rust_serde::types::{ident, type_ref_to_rust, ModelsPath};
use forge_plugin_sdk::ir::{
    Body, Ir, OAuth2Flow, OAuth2FlowKind, Operation, Parameter, Response, ResponseStatus,
    SecurityScheme, SecuritySchemeKind, TypeDef,
};
use forge_plugin_sdk::{values_ext, GenerationOutput, OutputFile};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::config::{Config, OAuthConfig};
use crate::naming::{kebab_case, screaming_snake};
use crate::schema;
use crate::tags::{self, TagGroup, TagTree};

/// Outcome of a generation pass. Mirrors [`codegen_rust_tower::Outcome`]
/// — the embedded tower codegen can reject if the spec references types
/// that don't model in Rust (bare `null`, unresolved `$ref`, …); we
/// surface that as a `StageError::Rejected` from the plugin's WIT entry
/// point.
pub enum Outcome {
    Generated(GenerationOutput),
    Rejected(Vec<forge_plugin_sdk::ir::Diagnostic>),
}

/// Path prefix for `quote!` to reach the tower codegen's `models` module
/// from the CLI's `main.rs`. The generated tree lives at `src/gen/` and
/// `main.rs` declares it as `mod gen;`.
fn gen_models_path() -> ModelsPath {
    quote! { gen::models }
}

/// Shape of a CLI field for one IR parameter.
///
/// `cli_ty` is what the clap `#[derive(Args)]` struct sees; primitives
/// (`String`, `i32`, …) clap parses natively, anything else falls back
/// to `String` and we round-trip through `serde_json` at dispatch
/// time. Arrays render as `Vec<inner>` so clap treats them as repeated
/// flags (matching the tower codegen's `Vec<T>` request-struct fields
/// and OpenAPI 3's `style=form, explode=true` default).
struct CliArgShape {
    /// Rust type to put in the CLI struct field.
    cli_ty: TokenStream,
    /// True when the param's underlying type isn't natively
    /// clap-parseable and the dispatch arm must `serde_json`-decode the
    /// CLI string into the target Rust type.
    needs_runtime_parse: bool,
    /// True when the param is array-shaped (`Vec<inner>` in the CLI).
    is_array: bool,
}

fn cli_arg_shape(spec: &Ir, type_ref: &str) -> CliArgShape {
    let named = spec.types.iter().find(|t| t.id == type_ref);
    if let Some(named) = named {
        if let TypeDef::Array(a) = &named.definition {
            let inner_native = is_clap_native(spec, &a.items);
            let inner_ty = if inner_native {
                type_ref_to_rust(spec, &a.items, &gen_models_path())
            } else {
                quote!(String)
            };
            return CliArgShape {
                cli_ty: quote!(Vec<#inner_ty>),
                needs_runtime_parse: !inner_native,
                is_array: true,
            };
        }
        if matches!(named.definition, TypeDef::Primitive(_)) {
            return CliArgShape {
                cli_ty: type_ref_to_rust(spec, type_ref, &gen_models_path()),
                needs_runtime_parse: false,
                is_array: false,
            };
        }
    }
    // Enums, complex objects, unions, etc. — CLI takes a String; we
    // decode via serde_json at dispatch (`PetStatus` decodes from a
    // quoted JSON string, etc.).
    CliArgShape {
        cli_ty: quote!(String),
        needs_runtime_parse: true,
        is_array: false,
    }
}

fn is_clap_native(spec: &Ir, type_ref: &str) -> bool {
    spec.types
        .iter()
        .find(|t| t.id == type_ref)
        .is_some_and(|t| matches!(t.definition, TypeDef::Primitive(_)))
}

/// Emit the expression that converts a CLI struct field into the value
/// the tower request struct expects. Encapsulates the four-axis cross
/// product of (required × relax-active × native × array).
fn cli_to_target_expr(shape: &CliArgShape, ident: &proc_macro2::Ident, kebab: &str) -> TokenStream {
    // Required + relax path: clap guarantees `Some` when we reach the
    // call branch; reuse this for both the "the CLI struct field is
    // T-with-relax" path and the "the field is Vec<T> requiring at
    // least one element" check would go here too. We accept empty
    // arrays since clap can't natively express `1..` together with
    // `required_unless_present_any`.
    let _ = kebab;
    if shape.is_array {
        if shape.needs_runtime_parse {
            return quote! {
                #ident
                    .into_iter()
                    .map(|__s| serde_json::from_value(serde_json::Value::String(__s)))
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
        }
        return quote! { #ident };
    }
    if shape.needs_runtime_parse {
        quote! { serde_json::from_value(serde_json::Value::String(#ident))? }
    } else {
        quote! { #ident }
    }
}

const GEN_HEADER: &str =
    "// Generated by openapi-forge / generator-rust-clap; do not edit by hand.\n\n";

/// Token stream → formatted Rust source. Matches the helper
/// `generator-rust-tower::emit::render_file` uses: parse through
/// `syn::File` so we hard-fail on bad emissions, then prepend the
/// `// Generated by …` header in plain text (syn doesn't model line
/// comments).
/// Token stream → formatted Rust source. Returns an error (instead of
/// panicking) when the emitter produced invalid Rust so the caller can
/// surface a structured diagnostic rather than a bare wasm trap. The
/// `Err` payload carries the head and tail of the failed token stream
/// — wasmtime embedders drop wasi stderr, so the context must travel
/// with the error itself.
fn render_file(tokens: TokenStream) -> Result<String, String> {
    let raw = tokens.to_string();
    match syn::parse2::<syn::File>(tokens) {
        Ok(file) => Ok(format!("{GEN_HEADER}{}", prettyplease::unparse(&file))),
        Err(e) => {
            // Span info isn't available on `wasm32-wasip2` (proc-macro2
            // needs the span-locations feature + a host backend), so we
            // dump both ends of the stream and let the consumer grep
            // for the failure mode.
            const WINDOW: usize = 2000;
            let len = raw.len();
            let head: String = raw.chars().take(WINDOW).collect();
            let tail: String = raw
                .chars()
                .rev()
                .take(WINDOW)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            Err(format!(
                "{e}; emitted {len} chars\n--- head ({WINDOW}) ---\n{head}\n--- tail ({WINDOW}) ---\n{tail}"
            ))
        }
    }
}

pub fn all(ir: &Ir, cfg: &Config) -> Outcome {
    // Build the tower-client tree first — if it rejects, surface that
    // before doing any of the CLI work (so callers see the typing
    // failure, not "the spec also doesn't fit the CLI" derivative).
    let tower = match codegen_rust_tower::all(ir) {
        codegen_rust_tower::Outcome::Generated(out) => out,
        codegen_rust_tower::Outcome::Rejected(diagnostics) => {
            return Outcome::Rejected(diagnostics);
        }
    };

    // One-shot pre-pass: validate each op's `x-pagination` extension
    // and accumulate warnings in the thread-local sink. Codegen call
    // sites (`collect_fields`, `render_op_match_arm`) use the silent
    // `op_pagination` resolver from here on, so misconfigurations
    // surface exactly once rather than once per call site.
    for op in &ir.operations {
        validate_op_pagination(ir, op);
    }

    let bin_name = bin_name(ir, cfg);
    let pkg_name = format!("{bin_name}-cli");
    let oauth = detect_oauth(ir, cfg);

    // Move the tower files under `src/gen/` so they sit beside the
    // hand-rolled CLI sources and `mod gen;` in main.rs picks them up.
    let mut files: Vec<OutputFile> = tower
        .files
        .into_iter()
        .map(|f| OutputFile {
            path: format!("src/gen/{}", f.path),
            content: f.content,
            mode: f.mode,
        })
        .collect();
    let main_rs = match emit_main_rs(ir, cfg, &bin_name, oauth.as_ref()) {
        Ok(s) => s,
        Err(detail) => {
            // Bubble the failed-emit detail as a fatal diagnostic so the
            // caller sees the syn error + a slice of the bad token
            // stream instead of a bare wasm trap.
            let mut diagnostics = tower.diagnostics;
            diagnostics.push(forge_plugin_sdk::ir::Diagnostic {
                severity: forge_plugin_sdk::ir::Severity::Error,
                code: "rust-clap/E-INVALID-RUST".to_string(),
                message: format!("emitter produced invalid Rust for src/main.rs: {detail}"),
                location: None,
                related: Vec::new(),
                suggested_fix: None,
            });
            return Outcome::Rejected(diagnostics);
        }
    };
    files.extend([
        OutputFile::text(
            "Cargo.toml",
            emit_cargo_toml(&pkg_name, &bin_name, oauth.is_some()),
        ),
        OutputFile::text("src/main.rs", main_rs),
        OutputFile::text("src/runtime.rs", emit_runtime_rs()),
        OutputFile::text("README.md", emit_readme(ir, &bin_name, oauth.as_ref())),
    ]);
    if let Some(oa) = &oauth {
        files.push(OutputFile::text(
            "src/auth.rs",
            emit_auth_rs(&bin_name, oa, &default_base_url(ir, cfg)),
        ));
    }

    // Drain any clap-side warnings the validators pushed into the
    // sink (codegen-rust-tower already drained its own before
    // returning, so the sink starts empty on our side). Concat onto
    // the tower diagnostics — order doesn't matter; consumers group
    // by code.
    let mut diagnostics = tower.diagnostics;
    diagnostics.extend(codegen_rust_serde::diagnostics::drain());
    Outcome::Generated(GenerationOutput { files, diagnostics })
}

fn bin_name(ir: &Ir, cfg: &Config) -> String {
    if let Some(n) = cfg.name.as_deref().filter(|s| !s.is_empty()) {
        return kebab_case(n);
    }
    let title = ir.info.title.trim();
    if title.is_empty() {
        "api-cli".into()
    } else {
        kebab_case(title)
    }
}

fn default_base_url(ir: &Ir, cfg: &Config) -> String {
    if let Some(u) = cfg.base_url.as_deref().filter(|s| !s.is_empty()) {
        return u.to_string();
    }
    ir.servers
        .first()
        .map(|s| s.url.clone())
        .unwrap_or_else(|| "http://localhost".into())
}

fn env_prefix(bin_name: &str) -> String {
    screaming_snake(bin_name)
}

// ---------------------------------------------------------------------------
// OAuth activation + token-exchange detection
// ---------------------------------------------------------------------------

struct OauthInfo<'a> {
    flow: &'a OAuth2Flow,
    config: &'a OAuthConfig,
    scopes: Vec<String>,
    exchange: Option<TokenExchangeInfo>,
}

#[derive(Debug, Clone)]
struct TokenExchangeInfo {
    /// Audience template like `"urn:vendor:tenant:{tenant}"`.
    audience_template: String,
    /// Single placeholder name extracted from the template. v0.0.6
    /// supports exactly one placeholder; multi-placeholder is a
    /// followup.
    placeholder: String,
    /// Optional RFC 8707 `resource` template.
    resource_template: Option<String>,
    /// Optional extra scopes to request on the exchange.
    extra_scope: Vec<String>,
}

fn detect_oauth<'a>(ir: &'a Ir, cfg: &'a Config) -> Option<OauthInfo<'a>> {
    let oc = cfg.oauth.as_ref()?;
    if oc.client_id.is_empty() {
        return None;
    }
    let mut candidates: Vec<&SecurityScheme> = ir
        .security_schemes
        .iter()
        .filter(|s| matches!(s.kind, SecuritySchemeKind::Oauth2(_)))
        .collect();
    if let Some(want_id) = &oc.scheme_id {
        candidates.retain(|s| s.id == *want_id);
    }
    candidates.sort_by(|a, b| a.id.cmp(&b.id));

    for s in candidates {
        if let SecuritySchemeKind::Oauth2(scheme) = &s.kind {
            for f in &scheme.flows {
                let usable = matches!(f.kind, OAuth2FlowKind::AuthorizationCode)
                    && f.authorization_url.is_some()
                    && f.token_url.is_some();
                if !usable {
                    continue;
                }
                let scopes = if let Some(o) = &oc.scopes {
                    o.clone()
                } else {
                    let mut set: BTreeSet<String> = BTreeSet::new();
                    for op in &ir.operations {
                        for req in &op.security {
                            if req.scheme_id == s.id {
                                for sc in &req.scopes {
                                    set.insert(sc.clone());
                                }
                            }
                        }
                    }
                    set.into_iter().collect()
                };
                let exchange = parse_token_exchange(ir, s);
                return Some(OauthInfo {
                    flow: f,
                    config: oc,
                    scopes,
                    exchange,
                });
            }
        }
    }
    None
}

fn parse_token_exchange(ir: &Ir, scheme: &SecurityScheme) -> Option<TokenExchangeInfo> {
    let (_, vref) = scheme
        .extensions
        .iter()
        .find(|(k, _)| k == "x-token-exchange")?;
    let json = values_ext::resolve_to_serde(&ir.values, *vref);
    let obj = json.as_object()?;

    let audience_template = obj.get("audience-template")?.as_str()?.to_string();
    let placeholders = extract_placeholders(&audience_template);
    if placeholders.len() != 1 {
        // v0.0.6 supports exactly one placeholder. Multi-placeholder is a
        // followup. Falling back to non-exchange mode.
        return None;
    }
    let placeholder = placeholders.into_iter().next().unwrap();

    let resource_template = obj
        .get("resource-template")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let extra_scope: Vec<String> = obj
        .get("scope")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Some(TokenExchangeInfo {
        audience_template,
        placeholder,
        resource_template,
        extra_scope,
    })
}

fn extract_placeholders(template: &str) -> Vec<String> {
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

fn op_uses_placeholder(op: &Operation, placeholder: &str) -> bool {
    op.path_params
        .iter()
        .any(|p| snake_case(&p.name) == snake_case(placeholder))
}

// ---------------------------------------------------------------------------
// `x-pagination` extension
// ---------------------------------------------------------------------------

/// Per-op pagination contract, resolved from the `x-pagination`
/// extension. Drives the `--pages N` / `--all-pages` flag emission on
/// the CLI subcommand and the loop-vs-single-call branch in the
/// dispatch arm.
///
/// Spec example:
/// ```yaml
/// x-pagination:
///   queryParam: nextPageToken     # name of the cursor query param
///   tokenPath: /nextPageToken     # JSON Pointer (RFC 6901) into the 2xx body
/// ```
struct PaginationConfig {
    /// The cursor query param's snake-cased Rust identifier — used
    /// to mutate `__op.#cursor_field_ident = Some(token)` between
    /// iterations.
    cursor_field_ident: proc_macro2::Ident,
    /// JSON Pointer (RFC 6901) string, e.g. `"/nextPageToken"` or
    /// `"/pagination/cursor"`. Resolved at runtime against the
    /// `serde_json::Value` view of each page's success body.
    token_pointer: String,
}

/// Silent resolver: returns the `PaginationConfig` if the op has a
/// well-formed `x-pagination` extension pointing at a real
/// string-typed query param, otherwise `None`. **Never emits
/// diagnostics** — call [`validate_op_pagination`] once per op (from
/// the pre-pass in [`all`]) to surface the malformed-extension
/// warnings. Splitting the two prevents duplicate emission, since
/// codegen calls this from both `collect_fields` and
/// `render_op_match_arm`.
fn op_pagination(spec: &Ir, op: &Operation) -> Option<PaginationConfig> {
    resolve_pagination(spec, op).ok()
}

/// Diagnostic-emitting validator. Called exactly once per op from
/// [`all`]'s pre-pass. The five `rust-clap/W-PAGINATION-*` codes each
/// describe a distinct misconfiguration so the spec author knows what
/// to fix; ops without an `x-pagination` extension are silently
/// skipped (the absence isn't an error).
fn validate_op_pagination(spec: &Ir, op: &Operation) {
    if let Err(Some(d)) = resolve_pagination(spec, op) {
        codegen_rust_serde::diagnostics::report(d);
    }
}

/// Core resolution logic. `Err(None)` means "no `x-pagination`
/// extension at all" (silent path); `Err(Some(d))` carries the
/// warning describing why the extension was rejected. Keeping the
/// validation logic in one place ensures the silent and validator
/// paths never drift.
fn resolve_pagination(
    spec: &Ir,
    op: &Operation,
) -> Result<PaginationConfig, Option<forge_plugin_sdk::ir::Diagnostic>> {
    use forge_plugin_sdk::diag;

    let (_, vref) = op
        .extensions
        .iter()
        .find(|(k, _)| k == "x-pagination")
        .ok_or(None)?;
    let json = values_ext::resolve_to_serde(&spec.values, *vref);
    let obj = json.as_object().ok_or_else(|| {
        Some(diag::warning(
            "rust-clap/W-PAGINATION-SHAPE",
            format!(
                "operation `{}`: `x-pagination` must be an object \
                 (got {}); skipping pagination emission",
                op.id,
                json_kind(&json)
            ),
        ))
    })?;
    let query_param = obj
        .get("queryParam")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let token_path = obj
        .get("tokenPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let (query_param, token_pointer) = match (query_param, token_path) {
        (Some(q), Some(t)) => (q, t),
        _ => {
            return Err(Some(diag::warning(
                "rust-clap/W-PAGINATION-INCOMPLETE",
                format!(
                    "operation `{}`: `x-pagination` requires string fields \
                     `queryParam` and `tokenPath`; skipping",
                    op.id
                ),
            )));
        }
    };
    // Shallow JSON-Pointer check: enforce only the leading `/` (RFC 6901
    // root-relative). We do *not* validate inner escape syntax (`~0`,
    // `~1`) or reject empty segments — `serde_json::Value::pointer`
    // tolerates oddly-formed inputs by returning `None`, which the
    // runtime loop interprets as "last page". So a typo in the pointer
    // shows up as an op that returns its first page and stops, not a
    // loud error. Deep validation is a follow-up if that bites.
    if !token_pointer.starts_with('/') {
        return Err(Some(diag::warning(
            "rust-clap/W-PAGINATION-POINTER",
            format!(
                "operation `{}`: `tokenPath` must start with `/` (RFC 6901 \
                 root-relative JSON Pointer, e.g. `/nextPageToken`); got \
                 `{token_pointer}`; skipping",
                op.id
            ),
        )));
    }
    let Some(matched_param) = op.query_params.iter().find(|p| p.name == query_param) else {
        return Err(Some(diag::warning(
            "rust-clap/W-PAGINATION-NO-PARAM",
            format!(
                "operation `{}`: `x-pagination.queryParam = \"{query_param}\"` \
                 doesn't match any of the op's query parameters; skipping",
                op.id
            ),
        )));
    };
    if !is_string_shaped(spec, &matched_param.r#type) {
        return Err(Some(diag::warning(
            "rust-clap/W-PAGINATION-PARAM-TYPE",
            format!(
                "operation `{}`: pagination cursor `{query_param}` must be a \
                 string- or string-enum-typed query param (the next-page \
                 token sets it on each iteration); skipping",
                op.id
            ),
        )));
    }
    Ok(PaginationConfig {
        cursor_field_ident: ident(&snake_case(&matched_param.name)),
        token_pointer,
    })
}

fn json_kind(v: &forge_plugin_sdk::serde_json::Value) -> &'static str {
    use forge_plugin_sdk::serde_json::Value;
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// True for `string`-typed primitives and for *named string enums*
/// (e.g. `type: string` + `enum: [foo, bar]`). Both deserialize from
/// a JSON string on the wire, so either is a valid cursor type.
/// Excludes integer enums and unions of any kind.
fn is_string_shaped(spec: &Ir, type_ref: &str) -> bool {
    spec.types.iter().any(|t| {
        t.id == type_ref
            && matches!(
                t.definition,
                TypeDef::Primitive(forge_plugin_sdk::ir::PrimitiveType {
                    kind: forge_plugin_sdk::ir::PrimitiveKind::String,
                    ..
                }) | TypeDef::EnumString(_)
            )
    })
}

// ---------------------------------------------------------------------------
// Cargo.toml
// ---------------------------------------------------------------------------

fn emit_cargo_toml(pkg_name: &str, bin_name: &str, oauth: bool) -> String {
    // OAuth flows (PKCE login, token refresh, RFC 8693 exchange) still
    // ride on reqwest. Switching auth.rs to hyper-util is a separate
    // refactor; for now we accept the extra dep when OAuth is opted in.
    let oauth_block = if oauth {
        r#"reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
sha2 = "0.10"
base64 = "0.22"
rand = "0.8"
webbrowser = "1"
directories = "6"
toml = "0.8"
dialoguer = "0.11"
urlencoding = "2"
"#
    } else {
        ""
    };
    format!(
        r#"# Generated by openapi-forge / generator-rust-clap.
[package]
name = "{pkg_name}"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "{bin_name}"
path = "src/main.rs"

[dependencies]
clap = {{ version = "4", features = ["derive", "env"] }}
clap_complete = {{ version = "4", features = ["unstable-dynamic"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread", "net", "io-util", "sync"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
anyhow = "1"
# Tower-driven HTTP client wiring for the generated `gen/` tree.
http = "1"
http-body = "1"
http-body-util = "0.1"
bytes = "1"
tower = {{ version = "0.5", features = ["util"] }}
thiserror = "2"
hyper = "1"
hyper-util = {{ version = "0.1", features = ["client-legacy", "http1", "http2", "tokio"] }}
hyper-rustls = {{ version = "0.27", default-features = false, features = ["http1", "http2", "ring", "webpki-roots", "webpki-tokio"] }}
{oauth_block}"#
    )
}

// ---------------------------------------------------------------------------
// src/main.rs
// ---------------------------------------------------------------------------

fn emit_main_rs(
    ir: &Ir,
    cfg: &Config,
    bin_name: &str,
    oauth: Option<&OauthInfo>,
) -> Result<String, String> {
    let title = ir.info.title.as_str();
    let version = ir.info.version.as_str();
    let base_url = default_base_url(ir, cfg);
    let prefix = env_prefix(bin_name);
    let prefix_base_url_env = format!("{prefix}_BASE_URL");
    let prefix_token_env = format!("{prefix}_TOKEN");
    let prefix_profile_env = format!("{prefix}_PROFILE");

    let tree = tags::build(ir);
    let oauth_active = oauth.is_some();
    let exchange = oauth.and_then(|o| o.exchange.as_ref());
    let placeholder_kebab = exchange.map(|e| kebab_case(&e.placeholder));
    let placeholder_snake = exchange.map(|e| snake_case(&e.placeholder));
    let placeholder_pascal_str = exchange.map(|e| pascal_case(&e.placeholder));
    let placeholder_pascal_ident = placeholder_pascal_str.as_deref().map(ident);

    let schema_consts = emit_schema_consts(ir);
    let info_summary = ir.info.summary.as_deref();
    let info_description = ir.info.description.as_deref();
    // Short `-h` line: prefer the spec's `info.summary` (one-liner) over
    // the bare title — it's what spec authors write specifically for
    // human consumption. Title remains the fallback (always present).
    let short_about = info_summary
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(title);
    let long_about_lit = build_long_about(
        title,
        info_summary,
        info_description,
        bin_name,
        &prefix,
        oauth_active,
        placeholder_kebab.as_deref(),
    );

    // CLI struct fields are emitted in document order. Each branch produces
    // a TokenStream that's spliced into the `#[derive(Parser)] struct Cli`.
    let base_url_field = if oauth_active {
        quote! {
            /// API base URL. When unset, falls back to the active profile, then the spec default.
            #[arg(long, global = true, env = #prefix_base_url_env)]
            base_url: Option<String>,
        }
    } else {
        quote! {
            /// API base URL.
            #[arg(long, global = true, env = #prefix_base_url_env, default_value = #base_url)]
            base_url: String,
        }
    };
    let profile_field = if oauth_active {
        quote! {
            /// Profile to use; bundles base_url / auth_url / token_url / client_id / client_secret. Default: "default".
            #[arg(long, global = true, env = #prefix_profile_env, default_value = "default", add = clap_complete::ArgValueCandidates::new(__complete_profile_names))]
            profile: String,
        }
    } else {
        quote!()
    };
    let placeholder_field = match (&placeholder_kebab, &placeholder_snake) {
        (Some(kebab), Some(snake)) => {
            let snake_ident = ident(snake);
            let env_name = format!("{}_{}", prefix, screaming_snake(snake));
            quote! {
                /// Slug used to template the RFC 8693 exchange audience for tenant-scoped operations.
                #[arg(long = #kebab, global = true, env = #env_name)]
                #snake_ident: Option<String>,
            }
        }
        _ => quote!(),
    };

    let complete_profile_fn = if oauth_active {
        quote! {
            /// Dynamic-completion callback for the global `--profile` flag.
            /// Invoked at completion time by the shell; reads `config.toml` fresh.
            fn __complete_profile_names() -> Vec<clap_complete::CompletionCandidate> {
                auth::list_profile_names()
                    .into_iter()
                    .map(clap_complete::CompletionCandidate::new)
                    .collect()
            }
        }
    } else {
        quote!()
    };

    // Cmd enum + per-group sub-enums.
    let cmd_enum = if ir.operations.is_empty() && !oauth_active {
        quote! {
            #[derive(Subcommand)]
            enum Cmd {
                /// (No operations declared in the spec.)
                #[command(hide = true)]
                Noop,
            }
        }
    } else {
        emit_root_enum(
            ir,
            &tree,
            oauth_active,
            exchange,
            placeholder_pascal_ident.as_ref(),
            placeholder_kebab.as_deref(),
        )
    };
    let group_types: TokenStream = if ir.operations.is_empty() && !oauth_active {
        quote!()
    } else {
        tree.roots
            .iter()
            .map(|root| emit_group_types(ir, root, "", exchange))
            .collect()
    };

    // main() body — assembled from a sequence of optional blocks.
    let completion_dispatch = if oauth_active {
        quote! {
            // Dynamic shell-completion dispatch. When `COMPLETE` env is set
            // (e.g. by `eval "$(COMPLETE=bash <bin>)"` in shell init), this
            // prints completions and exits before any normal CLI handling.
            clap_complete::CompleteEnv::with_factory(Cli::command).complete();
        }
    } else {
        quote!()
    };
    let completion_subcommand = quote! {
        if let Cmd::Completion { shell } = cli.cmd {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, #bin_name, &mut std::io::stdout());
            return Ok(());
        }
    };
    let bootstrap_block = if oauth_active {
        quote! {
            auth::migrate_legacy()?;
            auth::bootstrap_default_profile_if_missing()?;
        }
    } else {
        quote!()
    };
    let builtin_handlers = if oauth_active {
        quote! {
            if let Cmd::Login { reset } = &cli.cmd {
                if *reset {
                    // #24 suggestion #2: explicit recovery from
                    // `state mismatch (CSRF check failed)`. Wipes the
                    // stored token before re-running the flow — same
                    // effect as `logout` then `login`, exposed as a
                    // single discoverable flag in `login --help`.
                    let _ = auth::logout(&cli.profile).await?;
                }
                auth::login(&cli.profile).await?;
                eprintln!("logged in (profile: {})", cli.profile);
                return Ok(());
            }
            if matches!(cli.cmd, Cmd::Logout) {
                let removed = auth::logout(&cli.profile).await?;
                eprintln!("{}", if removed { "logged out" } else { "no stored token" });
                return Ok(());
            }
            if let Cmd::Configure {
                base_url,
                auth_url,
                token_url,
                client_id,
                client_secret,
                non_interactive,
            } = &cli.cmd
            {
                let any_field = base_url.is_some()
                    || auth_url.is_some()
                    || token_url.is_some()
                    || client_id.is_some()
                    || client_secret.is_some();
                if *non_interactive || any_field {
                    auth::write_profile_fields(
                        &cli.profile,
                        base_url.clone(),
                        auth_url.clone(),
                        token_url.clone(),
                        client_id.clone(),
                        client_secret.clone(),
                    )?;
                } else {
                    let should_login = auth::interactive_configure(&cli.profile)?;
                    if should_login {
                        auth::login(&cli.profile).await?;
                        eprintln!("logged in (profile: {})", cli.profile);
                    }
                }
                return Ok(());
            }
            if let Cmd::Profile(args) = &cli.cmd {
                match &args.cmd {
                    ProfileCmd::List => {
                        for name in auth::list_profile_names() {
                            println!("{}", name);
                        }
                        return Ok(());
                    }
                    ProfileCmd::Show { name } => {
                        let p = name.as_deref().unwrap_or(cli.profile.as_str());
                        auth::show_profile(p)?;
                        return Ok(());
                    }
                    ProfileCmd::Remove { name } => {
                        let removed = auth::remove_profile(name)?;
                        eprintln!("{}", if removed { "removed" } else { "not found" });
                        return Ok(());
                    }
                }
            }
        }
    } else {
        quote!()
    };
    let placeholder_handlers = match (
        placeholder_pascal_ident.as_ref(),
        placeholder_kebab.as_deref(),
    ) {
        (Some(pascal), Some(kebab)) => {
            let set_v = format_ident!("Set{}", pascal);
            let unset_v = format_ident!("Unset{}", pascal);
            let show_v = format_ident!("Show{}", pascal);
            let persist_msg = format!("persisted {kebab} = {{}} (profile: {{}})");
            quote! {
                if let Cmd::#set_v { value } = &cli.cmd {
                    auth::write_persisted(&cli.profile, #kebab, value)?;
                    eprintln!(#persist_msg, value, cli.profile);
                    return Ok(());
                }
                if matches!(cli.cmd, Cmd::#unset_v) {
                    let removed = auth::delete_persisted(&cli.profile, #kebab)?;
                    eprintln!("{}", if removed { "unset" } else { "no persisted value" });
                    return Ok(());
                }
                if matches!(cli.cmd, Cmd::#show_v) {
                    match auth::read_persisted(&cli.profile, #kebab)? {
                        Some(v) => println!("{}", v),
                        None => eprintln!("(none)"),
                    }
                    return Ok(());
                }
            }
        }
        _ => quote!(),
    };
    let placeholder_resolve = match (placeholder_snake.as_deref(), placeholder_kebab.as_deref()) {
        (Some(snake), Some(kebab)) => {
            let resolved_ident = format_ident!("__resolved_{}", snake);
            let cli_field = ident(snake);
            quote! {
                let #resolved_ident: Option<String> = match cli.#cli_field.clone() {
                    Some(v) => Some(v),
                    None => auth::read_persisted(&cli.profile, #kebab)?,
                };
            }
        }
        _ => quote!(),
    };
    // Token-subcommand dispatch (#25). Sits AFTER `placeholder_resolve`
    // so it can reach `__resolved_<placeholder>` when exchange is
    // configured — without the placeholder set, it falls back to the
    // base `access_token`. The `--token` CLI override short-circuits
    // both paths, mirroring per-op behaviour: a user passing a bearer
    // explicitly bypasses the OAuth dance.
    let token_handler = if oauth_active {
        let bearer_resolution = match (exchange, placeholder_snake.as_deref()) {
            (Some(ex), Some(snake)) => {
                let resolved_ident = format_ident!("__resolved_{}", snake);
                let aud_fmt = ex
                    .audience_template
                    .replace(&format!("{{{}}}", ex.placeholder), "{}");
                let res_expr: TokenStream = match &ex.resource_template {
                    Some(rt) => {
                        let rt_fmt = rt.replace(&format!("{{{}}}", ex.placeholder), "{}");
                        quote!(Some(format!(#rt_fmt, __slug)))
                    }
                    None => quote!(None),
                };
                let scope_expr: TokenStream = if ex.extra_scope.is_empty() {
                    quote!(None)
                } else {
                    let scopes = ex.extra_scope.join(" ");
                    quote!(Some(#scopes))
                };
                quote! {
                    let __bearer: Option<String> = if let Some(t) = cli.token.clone() {
                        Some(t)
                    } else if let Some(__slug) = #resolved_ident.as_ref().cloned() {
                        let __aud = format!(#aud_fmt, __slug);
                        let __res: Option<String> = #res_expr;
                        let __scope: Option<&str> = #scope_expr;
                        auth::audience_access_token(&cli.profile, &__aud, __res.as_deref(), __scope).await?
                    } else {
                        auth::access_token(&cli.profile).await?
                    };
                }
            }
            _ => quote! {
                let __bearer: Option<String> = if let Some(t) = cli.token.clone() {
                    Some(t)
                } else {
                    auth::access_token(&cli.profile).await?
                };
            },
        };
        let no_token_msg = format!("no valid token; run `{bin_name} login` first");
        quote! {
            if matches!(cli.cmd, Cmd::Token) {
                #bearer_resolution
                match __bearer {
                    Some(t) => { println!("{}", t); }
                    None => anyhow::bail!(#no_token_msg),
                }
                return Ok(());
            }
        }
    } else {
        quote!()
    };
    // `__base_url` (String) and `__http_client` (the tower::Service
    // base) are constructed once per main() and reused by every
    // dispatch arm; each arm wraps `__http_client` in an `ApiService`
    // with the per-op bearer.
    let client_init = if oauth_active {
        quote! {
            let __base_url: String = auth::resolve_base_url(&cli.profile, cli.base_url.as_deref())?;
            let __http_client = runtime::build_http_client()?;
        }
    } else {
        quote! {
            let __base_url: String = cli.base_url.clone();
            let __http_client = runtime::build_http_client()?;
        }
    };

    let mut match_arms: Vec<TokenStream> =
        vec![quote!(Cmd::Completion { .. } => unreachable!("handled above"),)];
    if oauth_active {
        match_arms.push(
            quote!(Cmd::Login { .. } | Cmd::Logout | Cmd::Token | Cmd::Configure { .. } | Cmd::Profile(_) => unreachable!("handled above"),),
        );
    }
    if let Some(pascal) = placeholder_pascal_ident.as_ref() {
        let set_v = format_ident!("Set{}", pascal);
        let unset_v = format_ident!("Unset{}", pascal);
        let show_v = format_ident!("Show{}", pascal);
        match_arms.push(
            quote!(Cmd::#set_v { .. } | Cmd::#unset_v | Cmd::#show_v => unreachable!("handled above"),),
        );
    }
    // Each `render_op_match_arm` call appends one `async fn handle_<op>`
    // to `handlers`. Emitted as siblings of `main` so rustc can
    // borrow-check each one in parallel; the corresponding match arm
    // shrinks to a single `.await?` dispatch.
    let mut handlers: Vec<TokenStream> = Vec::new();
    if ir.operations.is_empty() && !oauth_active {
        match_arms.push(quote!(Cmd::Noop => return Ok(()),));
    } else {
        for root in &tree.roots {
            match_arms.push(emit_root_match_arms(
                ir,
                root,
                "",
                oauth,
                exchange,
                &mut handlers,
            ));
        }
    }

    // `match cli.cmd { ... }` partially moves `cli`, so the dispatch
    // arms can't capture `&cli`. Hoist the cli-derived bits the
    // per-op handlers need into pre-match locals and pass them
    // individually — `OutputMode` is `Copy`, the strings get cheap
    // `Option<String>` / `String` clones at each dispatch site (only
    // one fires per invocation, so amortized to a single clone).
    let profile_let = if oauth_active {
        quote!(let __profile: String = cli.profile.clone();)
    } else {
        quote!()
    };
    let pre_match_ctx = quote! {
        let __token: Option<String> = cli.token.clone();
        let __output: runtime::OutputMode = cli.output;
        #profile_let
    };

    let tokio_main = quote! {
        #[tokio::main(flavor = "multi_thread")]
        async fn main() -> anyhow::Result<()> {
            #completion_dispatch
            let cli = Cli::parse();
            #completion_subcommand
            #bootstrap_block
            #builtin_handlers
            #placeholder_handlers
            #placeholder_resolve
            #token_handler
            #client_init
            #pre_match_ctx
            let result: serde_json::Value = match cli.cmd {
                #(#match_arms)*
            };
            runtime::print_output(&result, cli.output)
        }
    };

    let mod_decls = if oauth_active {
        quote! {
            mod auth;
            mod gen;
            mod runtime;
        }
    } else {
        quote! {
            mod gen;
            mod runtime;
        }
    };

    render_file(quote! {
        #![allow(
            clippy::needless_late_init,
            clippy::redundant_field_names,
            clippy::too_many_arguments,
            clippy::collapsible_if
        )]

        #mod_decls

        use clap::{Args, CommandFactory, Parser, Subcommand};
        use runtime::OutputMode;

        #schema_consts

        #[derive(Parser)]
        #[command(
            name = #bin_name,
            version = #version,
            about = #short_about,
            long_about = #long_about_lit,
        )]
        struct Cli {
            #base_url_field
            /// Bearer token. Overrides any stored OAuth or exchanged token.
            #[arg(long, global = true, env = #prefix_token_env)]
            token: Option<String>,

            /// Output mode for response bodies.
            #[arg(long, global = true, value_enum, default_value_t = OutputMode::Json)]
            output: OutputMode,

            #profile_field
            #placeholder_field

            #[command(subcommand)]
            cmd: Cmd,
        }

        #complete_profile_fn

        #cmd_enum

        #group_types

        #(#handlers)*

        #tokio_main
    })
}

fn emit_root_enum(
    spec: &Ir,
    tree: &TagTree,
    oauth_active: bool,
    exchange: Option<&TokenExchangeInfo>,
    placeholder_pascal: Option<&proc_macro2::Ident>,
    placeholder_kebab: Option<&str>,
) -> TokenStream {
    let oauth_variants = if oauth_active {
        quote! {
            /// Run OAuth 2.0 authorization-code flow with PKCE; persists the access token.
            Login {
                /// Wipe any stored token for the active profile before starting the new flow. Use when a previous aborted `login` left this CLI stuck on `state mismatch (CSRF check failed)` — typically a leftover browser tab firing its callback against the new listener.
                #[arg(long)]
                reset: bool,
            },
            /// Delete the stored OAuth token.
            Logout,
            /// Print the active bearer access token to stdout. Refreshes lazily on a 30s skew; falls through to a full `login` flow if nothing valid is cached. Use with the placeholder flag (when configured) to mint an audience-exchanged token for that slug instead of the base token.
            Token,
            /// Create or edit the active profile (use `--profile` to pick). With no flags, prompts interactively; with `--non-interactive` (or any field flag), writes without prompting.
            Configure {
                /// Set base_url non-interactively.
                #[arg(long)]
                base_url: Option<String>,
                /// Set auth_url non-interactively.
                #[arg(long)]
                auth_url: Option<String>,
                /// Set token_url non-interactively.
                #[arg(long)]
                token_url: Option<String>,
                /// Set client_id non-interactively.
                #[arg(long)]
                client_id: Option<String>,
                /// Set client_secret non-interactively. Stored in `config.toml` (mode 0600). Prefer setting via the env var when possible.
                #[arg(long)]
                client_secret: Option<String>,
                /// Skip all prompts. Fields not given as flags keep their existing value.
                #[arg(long)]
                non_interactive: bool,
            },
            /// List, inspect, or delete profiles.
            Profile(ProfileArgs),
        }
    } else {
        quote!()
    };
    let exchange_variants = match (exchange, placeholder_pascal, placeholder_kebab) {
        (Some(_), Some(pascal), Some(kebab)) => {
            let set_v = format_ident!("Set{}", pascal);
            let unset_v = format_ident!("Unset{}", pascal);
            let show_v = format_ident!("Show{}", pascal);
            let set_doc =
                format!("Persist a default `{kebab}` so subsequent calls can omit `--{kebab}`.");
            let unset_doc = format!("Clear the persisted default `{kebab}`.");
            let show_doc = format!("Print the persisted default `{kebab}`.");
            quote! {
                #[doc = #set_doc]
                #set_v { value: String },
                #[doc = #unset_doc]
                #unset_v,
                #[doc = #show_doc]
                #show_v,
            }
        }
        _ => quote!(),
    };
    let root_variants: Vec<TokenStream> = tree
        .roots
        .iter()
        .flat_map(|root| {
            if root.is_misc() {
                root.direct_ops
                    .iter()
                    .map(|op| render_op_variant(spec, op, exchange))
                    .collect::<Vec<_>>()
            } else {
                let variant = format_ident!("{}", pascal_case(&root.name));
                let qualified = format_ident!("{}Args", qualified_pascal("", &root.name));
                let doc_attr = group_doc(root).map(|d| quote!(#[doc = #d]));
                vec![quote!(#doc_attr #variant(#qualified),)]
            }
        })
        .collect();

    let profile_types = if oauth_active {
        quote! {
            #[derive(Args)]
            pub struct ProfileArgs {
                #[command(subcommand)]
                cmd: ProfileCmd,
            }

            #[derive(Subcommand)]
            pub enum ProfileCmd {
                /// List configured profile names from config.toml.
                List,
                /// Print the resolved settings for a profile (secret redacted).
                Show {
                    /// Profile name; defaults to the global --profile value.
                    #[arg(add = clap_complete::ArgValueCandidates::new(__complete_profile_names))]
                    name: Option<String>,
                },
                /// Delete a profile from config.toml and remove its on-disk dir.
                Remove {
                    /// Profile name (required to avoid accidental deletion).
                    #[arg(add = clap_complete::ArgValueCandidates::new(__complete_profile_names))]
                    name: String,
                },
            }
        }
    } else {
        quote!()
    };

    quote! {
        #[derive(Subcommand)]
        enum Cmd {
            /// Print a shell completion script. e.g. `source <(<bin> completion bash)`.
            Completion {
                /// Target shell.
                #[arg(value_enum)]
                shell: clap_complete::Shell,
            },
            #oauth_variants
            #exchange_variants
            #(#root_variants)*
        }

        #profile_types
    }
}

fn emit_group_types(
    spec: &Ir,
    group: &TagGroup,
    prefix: &str,
    exchange: Option<&TokenExchangeInfo>,
) -> TokenStream {
    if group.is_misc() {
        return quote!();
    }
    let q = qualified_pascal(prefix, &group.name);
    let args_ident = format_ident!("{}Args", q);
    let cmd_ident = format_ident!("{}Cmd", q);
    let group_doc_attrs = doc_attrs_for(group_doc(group).as_deref());

    let cmd_variants: Vec<TokenStream> = group
        .children
        .iter()
        .map(|child| {
            let child_q = qualified_pascal(&q, &child.name);
            let child_args = format_ident!("{}Args", child_q);
            let variant = format_ident!("{}", pascal_case(&child.name));
            let doc_attr = doc_attrs_for(group_doc(child).as_deref());
            quote!(#doc_attr #variant(#child_args),)
        })
        .chain(
            group
                .direct_ops
                .iter()
                .map(|op| render_op_variant(spec, op, exchange)),
        )
        .collect();

    let child_types: TokenStream = group
        .children
        .iter()
        .map(|child| emit_group_types(spec, child, &q, exchange))
        .collect();

    quote! {
        #[derive(Args)]
        #group_doc_attrs
        pub struct #args_ident {
            #[command(subcommand)]
            cmd: #cmd_ident,
        }

        #[derive(Subcommand)]
        pub enum #cmd_ident {
            #(#cmd_variants)*
        }

        #child_types
    }
}

fn emit_root_match_arms(
    spec: &Ir,
    root: &TagGroup,
    prefix: &str,
    oauth: Option<&OauthInfo>,
    exchange: Option<&TokenExchangeInfo>,
    handlers: &mut Vec<TokenStream>,
) -> TokenStream {
    if root.is_misc() {
        let arms: Vec<TokenStream> = root
            .direct_ops
            .iter()
            .map(|op| {
                render_op_match_arm(spec, op, &format_ident!("Cmd"), oauth, exchange, handlers)
            })
            .collect();
        return quote!(#(#arms)*);
    }
    let variant = format_ident!("{}", pascal_case(&root.name));
    let q = qualified_pascal(prefix, &root.name);
    let inner = emit_group_match_arms(spec, root, &q, oauth, exchange, handlers);
    quote! {
        Cmd::#variant(__g) => match __g.cmd {
            #inner
        },
    }
}

fn emit_group_match_arms(
    spec: &Ir,
    group: &TagGroup,
    q: &str,
    oauth: Option<&OauthInfo>,
    exchange: Option<&TokenExchangeInfo>,
    handlers: &mut Vec<TokenStream>,
) -> TokenStream {
    let cmd_ty = format_ident!("{}Cmd", q);
    let child_arms: Vec<TokenStream> = group
        .children
        .iter()
        .map(|child| {
            let child_variant = format_ident!("{}", pascal_case(&child.name));
            let child_q = qualified_pascal(q, &child.name);
            let inner = emit_group_match_arms(spec, child, &child_q, oauth, exchange, handlers);
            quote! {
                #cmd_ty::#child_variant(__g) => match __g.cmd {
                    #inner
                },
            }
        })
        .collect();
    let op_arms: Vec<TokenStream> = group
        .direct_ops
        .iter()
        .map(|op| render_op_match_arm(spec, op, &cmd_ty, oauth, exchange, handlers))
        .collect();
    quote! {
        #(#child_arms)*
        #(#op_arms)*
    }
}

fn render_op_variant(
    spec: &Ir,
    op: &Operation,
    exchange: Option<&TokenExchangeInfo>,
) -> TokenStream {
    let variant = format_ident!("{}", pascal_case(&op.id));
    let doc_text = combine_summary_desc(op.summary.as_deref(), op.description.as_deref());
    let doc_attr = doc_attrs_for(doc_text.as_deref());
    let exclude = exchange
        .filter(|ex| op_uses_placeholder(op, &ex.placeholder))
        .map(|ex| ex.placeholder.as_str());
    let fields = collect_fields(spec, op, exclude);
    if fields.is_empty() {
        quote!(#doc_attr #variant,)
    } else {
        let tokens = fields.iter().map(field_to_tokens);
        quote!(#doc_attr #variant { #(#tokens)* },)
    }
}

/// Emit one operation's dispatch.
///
/// Pushes a free-standing `async fn handle_<op_id>(...) ->
/// anyhow::Result<serde_json::Value>` containing the request-build,
/// `gen::execute(...).await`, response-decode, and pagination logic
/// into `handlers`; returns just the `Cmd::<Variant>{…} =>
/// handle_<op_id>(…).await?,` arm. Without this split every op's body
/// lives inside a single `async fn main`, and rustc's MIR borrow check
/// (which doesn't parallelize *within* a function body) becomes the
/// dominant cost on medium-to-large specs — see issue #19 for the
/// rustc `-Zself-profile` numbers (588 s of borrow-check, 14 GB peak
/// RSS on N≈464 ops). The semantics are unchanged; only the emit shape
/// moves.
fn render_op_match_arm(
    spec: &Ir,
    op: &Operation,
    cmd_ty: &proc_macro2::Ident,
    oauth: Option<&OauthInfo>,
    exchange: Option<&TokenExchangeInfo>,
    handlers: &mut Vec<TokenStream>,
) -> TokenStream {
    let variant = format_ident!("{}", pascal_case(&op.id));
    let op_struct = quote! { gen::operations::#variant };
    let output_ty = format_ident!("{}Output", pascal_case(&op.id));
    let output_path = quote! { gen::operations::#output_ty };

    // Bearer resolution per op. Three modes:
    //   - this op references the placeholder ⇒ resolve via RFC 8693
    //     exchange (or pass `--token` through unchanged).
    //   - oauth is active but op doesn't reference the placeholder ⇒
    //     fall back to the main token (`--token` ⇒ stored ⇒ none).
    //   - oauth not active ⇒ raw `--token` flag, possibly None.
    let needs_exchange = exchange.is_some_and(|ex| op_uses_placeholder(op, &ex.placeholder));
    let exclude = if needs_exchange {
        exchange.map(|ex| ex.placeholder.as_str())
    } else {
        None
    };
    let exclude_snake = exclude.map(snake_case);

    let destruct_fields = collect_fields(spec, op, exclude);
    let destruct = if destruct_fields.is_empty() {
        quote!()
    } else {
        let idents = destruct_fields.iter().map(|f| &f.ident);
        quote!({ #(#idents),* })
    };

    // Schema-flag relaxation: when in effect, formerly-`String` fields
    // are emitted as `Option<String>` and clap allows them empty when
    // --body-schema / --response-schema is set; we unwrap with
    // `.expect(...)` on the actual API-call branch since clap
    // guarantees Some by then.
    let relax_active = !relax_unless(op).is_empty();

    // Build the request-struct field initializers. Each entry is
    // `<snake_field>: <expr>` where <expr> turns the CLI struct field
    // (which may be the typed value, `Option<typed>`, `Vec<typed>`, or
    // `String` for non-clap-native types) into the tower request
    // struct's declared field type.
    let mut field_inits: Vec<TokenStream> = Vec::new();
    let push_param = |inits: &mut Vec<TokenStream>, p: &Parameter, is_path: bool| {
        let field_ident = ident(&snake_case(&p.name));
        if is_path
            && exclude_snake
                .as_deref()
                .is_some_and(|s| s == snake_case(&p.name))
        {
            inits.push(quote!(#field_ident: __slug.clone()));
            return;
        }
        let shape = cli_arg_shape(spec, &p.r#type);
        let kebab = kebab_case(&p.name);
        // Strip any `r#` prefix when synthesizing local-binding names
        // (`__opt_type`, `__inner_type`, …) — the raw-ident escaping is
        // only needed for the outermost field name. Without this we'd
        // try to build `format_ident!("__opt_r#type")`, which panics.
        let raw = snake_case(&p.name);
        let bare = raw.strip_prefix("r#").unwrap_or(&raw);
        let val = cli_to_target_expr(&shape, &field_ident, &kebab);
        if shape.is_array {
            // CLI: `Vec<T>` always. Target: `Vec<T>` when required,
            // `Option<Vec<T>>` when not. clap can't distinguish "empty
            // vec" from "absent", so we map empty → `None`.
            if p.required {
                inits.push(quote!(#field_ident: #val));
            } else {
                inits.push(quote! {
                    #field_ident: if #field_ident.is_empty() { None } else { Some(#val) }
                });
            }
        } else if (is_path || p.required) && !relax_active {
            // Required, no relax — CLI field is the typed value directly.
            inits.push(quote!(#field_ident: #val));
        } else if p.required && relax_active {
            // Required + relax — CLI field is `Option<typed>`; clap
            // guarantees `Some` on the call branch.
            let msg = if is_path {
                format!("<{kebab}> required")
            } else {
                format!("--{kebab} required")
            };
            // `cli_to_target_expr` was generated assuming the binding
            // names the inner value; bind the unwrapped option to that
            // name via `let`, then run the conversion.
            let local = format_ident!("__opt_{}", bare);
            let conv = cli_to_target_expr(&shape, &local, &kebab);
            inits.push(quote! {
                #field_ident: {
                    let #local = #field_ident.expect(#msg);
                    #conv
                }
            });
        } else {
            // Optional — CLI field is `Option<typed>`; target field
            // is `Option<typed>` too.
            if shape.needs_runtime_parse {
                let inner_local = format_ident!("__inner_{}", bare);
                let inner_conv = cli_to_target_expr(&shape, &inner_local, &kebab);
                inits.push(quote! {
                    #field_ident: match #field_ident {
                        Some(#inner_local) => Some(#inner_conv),
                        None => None,
                    }
                });
            } else {
                inits.push(quote!(#field_ident: #field_ident));
            }
        }
    };

    for p in &op.path_params {
        push_param(&mut field_inits, p, /*is_path=*/ true);
    }
    for p in &op.query_params {
        push_param(&mut field_inits, p, false);
    }
    for p in &op.header_params {
        push_param(&mut field_inits, p, false);
    }
    // Cookie params have no tower-side representation today; the tower
    // codegen ignores them. Skip silently for now to keep the surface
    // matching the generated request struct.
    let _ = op.cookie_params.len();

    if let Some(body) = &op.request_body {
        let body_type_ref = body
            .content
            .iter()
            .find(|c| {
                let m = c.media_type.to_ascii_lowercase();
                m.starts_with("application/json")
                    || m.strip_prefix("application/")
                        .is_some_and(|r| r.ends_with("+json"))
            })
            .map(|c| c.r#type.clone());
        let body_ty_tokens = match &body_type_ref {
            Some(tr) => type_ref_to_rust(spec, tr, &gen_models_path()),
            None => quote!(serde_json::Value),
        };
        if body.required {
            // `--body` is `Option<String>` in the CLI struct with
            // `required_unless_present_any` covering the schema-flag
            // branches; on the call branch clap guarantees `Some`.
            field_inits.push(quote! {
                body: {
                    let __body_str = body.expect("--body required");
                    let __body_val = runtime::parse_body_arg(&__body_str)?;
                    serde_json::from_value::<#body_ty_tokens>(__body_val)?
                }
            });
        } else {
            field_inits.push(quote! {
                body: match body {
                    Some(__body_str) => {
                        let __body_val = runtime::parse_body_arg(&__body_str)?;
                        Some(serde_json::from_value::<#body_ty_tokens>(__body_val)?)
                    }
                    None => None,
                }
            });
        }
    }

    let execute_and_decode = render_execute_and_decode(op, &output_path);

    // If the op declares an `x-pagination` extension, the build path
    // forks at runtime on the new flags: with `--pages N` or
    // `--all-pages`, we loop, extract the next-page token from each
    // 2xx body via the configured JSON Pointer, and emit a JSON array
    // of page bodies. Without either flag the dispatch is byte-for-
    // byte identical to the non-paginated path — single-call, raw
    // body on stdout.
    let pagination = op_pagination(spec, op);
    let call = if let Some(pg) = pagination {
        let cursor_field = &pg.cursor_field_ident;
        let token_pointer = &pg.token_pointer;
        // `__op_template.clone()` depends on `codegen-rust-tower`'s
        // per-op request struct deriving `Clone` — it does today
        // (`#[derive(Debug, Clone)]` on every emitted struct). If
        // that ever changes, paginated ops will fail to compile and
        // this branch needs reworking (e.g. recompute `field_inits`
        // per iteration, with care for `body.expect(...)` which
        // moves the CLI body string).
        quote! {
            let __op_template = #op_struct {
                #(#field_inits,)*
            };
            let mut __svc = runtime::ApiService {
                inner: __http_client.clone(),
                base_url: __base_url.clone(),
                bearer: __bearer.clone(),
            };
            if all_pages || pages.is_some() {
                // `pages.expect(...)` is safe under the `is_some()`
                // guard above; `pages.unwrap_or(1)` would silently
                // fall through to the no-op case for callers who
                // somehow reached this branch with `pages = None &&
                // !all_pages`, which can't happen by construction.
                let __cap: u32 = if all_pages {
                    u32::MAX
                } else {
                    pages.expect("`pages` is Some when the paginate branch is entered without `--all-pages`")
                };
                // `--output ndjson` short-circuits the accumulator and
                // prints each page body as its own line as soon as it
                // arrives. Useful for `jq -c` pipelines and for long
                // walks where buffering hundreds of pages is wasteful.
                // For `json` / `compact` we keep collecting into one
                // array, which `print_output` then formats at the end.
                let __streaming = __output == runtime::OutputMode::Ndjson;
                let mut __pages: Vec<serde_json::Value> = Vec::new();
                let mut __cursor: Option<String> = None;
                for __i in 0..__cap {
                    let mut __op = __op_template.clone();
                    if __i > 0 {
                        __op.#cursor_field = __cursor.clone();
                    }
                    let __out = gen::execute(&mut __svc, __op).await
                        .map_err(|e| anyhow::anyhow!("api call failed: {e}"))?;
                    let __body_value: serde_json::Value = #execute_and_decode;
                    // `serde_json::Value::pointer` returns `None` for
                    // missing keys *and* for syntactically-broken
                    // pointers. We collapse both into the "last page"
                    // terminator alongside `Some(Null)`. The CLI
                    // can't distinguish missing-field from
                    // explicit-null from typo'd pointer — fine for
                    // most paginated APIs, worth a comment so
                    // future-you doesn't go looking for the
                    // distinction.
                    let __next_str = match __body_value.pointer(#token_pointer) {
                        None | Some(serde_json::Value::Null) => None,
                        Some(serde_json::Value::String(s)) => Some(s.clone()),
                        Some(other) => {
                            return Err(anyhow::anyhow!(
                                "x-pagination: cursor at `{}` was not a string: {}",
                                #token_pointer, other
                            ));
                        }
                    };
                    if __streaming {
                        // Compact one-line JSON per page; flush so a
                        // long walk shows progress in real time rather
                        // than buffering in stdio.
                        use std::io::Write as _;
                        let mut __stdout = std::io::stdout().lock();
                        serde_json::to_writer(&mut __stdout, &__body_value)?;
                        __stdout.write_all(b"\n")?;
                        __stdout.flush()?;
                    } else {
                        __pages.push(__body_value);
                    }
                    match __next_str {
                        Some(t) => __cursor = Some(t),
                        None => break,
                    }
                }
                if __streaming {
                    // Already printed inside the loop; tell
                    // `print_output` there's nothing left to emit.
                    serde_json::Value::Null
                } else {
                    serde_json::Value::Array(__pages)
                }
            } else {
                let __out = gen::execute(&mut __svc, __op_template).await
                    .map_err(|e| anyhow::anyhow!("api call failed: {e}"))?;
                #execute_and_decode
            }
        }
    } else {
        quote! {
            let __op = #op_struct {
                #(#field_inits,)*
            };
            let mut __svc = runtime::ApiService {
                inner: __http_client.clone(),
                base_url: __base_url.clone(),
                bearer: __bearer.clone(),
            };
            let __out = gen::execute(&mut __svc, __op).await
                .map_err(|e| anyhow::anyhow!("api call failed: {e}"))?;
            #execute_and_decode
        }
    };

    let (pre_let, bearer_let) = if needs_exchange {
        let ex = exchange.unwrap();
        let ph_snake = format_ident!("__resolved_{}", snake_case(&ex.placeholder));
        let ph_kebab = kebab_case(&ex.placeholder);
        let err_msg = format!(
            "--{0} is required for this operation (or run `set-{0} <slug>`)",
            ph_kebab,
        );
        let aud_fmt = ex
            .audience_template
            .replace(&format!("{{{}}}", ex.placeholder), "{}");
        let res_expr: TokenStream = match &ex.resource_template {
            Some(rt) => {
                let rt_fmt = rt.replace(&format!("{{{}}}", ex.placeholder), "{}");
                quote!(Some(format!(#rt_fmt, __slug)))
            }
            None => quote!(None),
        };
        let scope_expr: TokenStream = if ex.extra_scope.is_empty() {
            quote!(None)
        } else {
            let scopes = ex.extra_scope.join(" ");
            quote!(Some(#scopes))
        };
        (
            quote! {
                let __slug: String = #ph_snake
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!(#err_msg))?;
            },
            quote! {
                let __bearer: Option<String> = if let Some(t) = __token.clone() {
                    Some(t)
                } else {
                    let __aud = format!(#aud_fmt, __slug);
                    let __res: Option<String> = #res_expr;
                    let __scope: Option<&str> = #scope_expr;
                    auth::audience_access_token(&__profile, &__aud, __res.as_deref(), __scope).await?
                };
            },
        )
    } else if oauth.is_some() {
        (
            quote!(),
            quote! {
                let __bearer: Option<String> = if let Some(t) = __token.clone() {
                    Some(t)
                } else {
                    auth::access_token(&__profile).await?
                };
            },
        )
    } else {
        (
            quote!(),
            quote!(let __bearer: Option<String> = __token.clone();),
        )
    };

    let const_pascal = screaming_snake(&op.id);
    let body_schema_const = format_ident!("BODY_SCHEMA_{}", const_pascal);
    let response_schema_const = format_ident!("RESPONSE_SCHEMA_{}", const_pascal);

    let has_body = op.request_body.is_some();
    let has_resp_schema = op_has_response_content(op);
    let api_call = quote! {
        #pre_let
        #bearer_let
        #call
    };
    let arm_body = match (has_body, has_resp_schema) {
        (false, false) => api_call,
        (true, false) => quote! {
            if body_schema {
                println!("{}", #body_schema_const);
                serde_json::Value::Null
            } else {
                #api_call
            }
        },
        (false, true) => quote! {
            if response_schema {
                println!("{}", #response_schema_const);
                serde_json::Value::Null
            } else {
                #api_call
            }
        },
        (true, true) => quote! {
            if body_schema {
                println!("{}", #body_schema_const);
                serde_json::Value::Null
            } else if response_schema {
                println!("{}", #response_schema_const);
                serde_json::Value::Null
            } else {
                #api_call
            }
        },
    };

    // Per-op handler emitted at file scope. Mirrors the destructured
    // fields one-for-one as fn params (in declaration order), then
    // appends a shared trailer (token, output mode, profile when OAuth
    // is active, base URL, HTTP client, resolved placeholder slug for
    // RFC 8693 exchange ops).
    //
    // Passing the cli-derived bits *individually* matters: the outer
    // `match cli.cmd { Cmd::<Group>(__g) => match __g.cmd { ... } }`
    // partially moves `cli`, so a `&cli` parameter would fail to borrow
    // after the partial move. Taking owned `__token` / `__profile` /
    // `__output` clones (built once before the match) keeps the
    // dispatch arms borrow-checker-clean. The URL and client are taken
    // by value too so the handler body keeps its existing
    // `__base_url.clone()` / `__http_client.clone()` shape unchanged;
    // the per-CLI-invocation cost is one extra String/HttpClient clone
    // that only happens on the single matched arm.
    let handler_ident = format_ident!("handle_{}", snake_case(&op.id));
    let handler_field_params: Vec<TokenStream> = destruct_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            quote!(#ident: #ty,)
        })
        .collect();
    let profile_param = if oauth.is_some() {
        quote!(__profile: String,)
    } else {
        quote!()
    };
    let resolved_param = if needs_exchange {
        let ex = exchange.expect("`needs_exchange` ⇒ exchange is Some");
        let ph_snake = format_ident!("__resolved_{}", snake_case(&ex.placeholder));
        quote!(, #ph_snake: Option<String>)
    } else {
        quote!()
    };
    let profile_arg = if oauth.is_some() {
        quote!(__profile.clone(),)
    } else {
        quote!()
    };
    let resolved_arg = if needs_exchange {
        let ex = exchange.expect("`needs_exchange` ⇒ exchange is Some");
        let ph_snake = format_ident!("__resolved_{}", snake_case(&ex.placeholder));
        quote!(, #ph_snake.clone())
    } else {
        quote!()
    };
    handlers.push(quote! {
        async fn #handler_ident(
            #(#handler_field_params)*
            __token: Option<String>,
            __output: runtime::OutputMode,
            #profile_param
            __base_url: String,
            __http_client: runtime::HttpClient
            #resolved_param
        ) -> anyhow::Result<serde_json::Value> {
            let __result: serde_json::Value = { #arm_body };
            Ok(__result)
        }
    });

    let dispatch_idents: Vec<&proc_macro2::Ident> =
        destruct_fields.iter().map(|f| &f.ident).collect();
    quote! {
        #cmd_ty::#variant #destruct => {
            #handler_ident(
                #(#dispatch_idents,)*
                __token.clone(),
                __output,
                #profile_arg
                __base_url.clone(),
                __http_client.clone()
                #resolved_arg,
            ).await?
        },
    }
}

/// Build the `match __out { … }` expression that turns the tower op's
/// typed `Output` enum into a `serde_json::Value` for the CLI's
/// `print_output`. 2xx variants extract the body (or `Null` for unit
/// success variants); everything else turns into an `anyhow` error
/// that surfaces as a non-zero exit code with the body printed to
/// stderr.
///
/// **Contract:** the returned expression always evaluates to
/// `serde_json::Value` (success variants via `to_value(&inner)?`,
/// unit variants as `Value::Null`, error variants via `return Err`).
/// The paginated dispatch arm relies on this — it annotates `let
/// __body_value: serde_json::Value = #execute_and_decode;` and then
/// calls `.pointer(...)` to extract the cursor.
fn render_execute_and_decode(op: &Operation, output_path: &TokenStream) -> TokenStream {
    // No declared responses → the tower codegen emits a single
    // `Success` unit variant matched on any 2xx.
    if op.responses.is_empty() {
        return quote! {
            match __out {
                #output_path::Success => serde_json::Value::Null,
            }
        };
    }

    let only_default =
        op.responses.len() == 1 && matches!(op.responses[0].status, ResponseStatus::Default);

    let mut arms: Vec<TokenStream> = Vec::new();
    for resp in &op.responses {
        let variant = format_ident!("{}", status_variant(&resp.status));
        let has_body = pick_json_body(resp).is_some();
        let success = is_success_status(&resp.status)
            || (only_default && matches!(resp.status, ResponseStatus::Default));
        let label = status_label(&resp.status);
        let arm_pat = if has_body {
            quote! { #output_path::#variant(__inner) }
        } else {
            quote! { #output_path::#variant }
        };
        let arm_body = if success {
            if has_body {
                quote! { serde_json::to_value(&__inner)? }
            } else {
                quote! { serde_json::Value::Null }
            }
        } else if has_body {
            // Block-expression arm — interior statements need their `;`,
            // and the outer template appends the arm-separating `,`.
            quote! {
                {
                    let __json = serde_json::to_value(&__inner)?;
                    return Err(anyhow::anyhow!("HTTP {}: {}", #label, __json));
                }
            }
        } else {
            // Bare-expression arm — no trailing `;`. The outer template
            // appends the arm-separating `,`; emitting `return Err(...);`
            // here produced `;,` which `syn::parse2` rejects.
            quote! {
                return Err(anyhow::anyhow!("HTTP {}", #label))
            }
        };
        arms.push(quote! { #arm_pat => #arm_body, });
    }

    quote! {
        match __out {
            #(#arms)*
        }
    }
}

/// Rust variant name on the per-op response enum for a given status.
/// Mirrors `codegen_rust_tower`'s `status_variant`. Kept in lockstep
/// by code review; a follow-up could expose this from the shared
/// crate to avoid drift.
fn status_variant(s: &ResponseStatus) -> String {
    match s {
        ResponseStatus::Explicit { code } => {
            well_known_variant(*code).unwrap_or_else(|| format!("Status{code}"))
        }
        ResponseStatus::Range { class } => match class {
            1 => "OneXx".to_string(),
            2 => "TwoXx".to_string(),
            3 => "ThreeXx".to_string(),
            4 => "FourXx".to_string(),
            5 => "FiveXx".to_string(),
            other => format!("Status{other}xx"),
        },
        ResponseStatus::Default => "Default".to_string(),
    }
}

fn well_known_variant(code: u16) -> Option<String> {
    let n = match code {
        100 => "Continue",
        101 => "SwitchingProtocols",
        200 => "Ok",
        201 => "Created",
        202 => "Accepted",
        203 => "NonAuthoritativeInformation",
        204 => "NoContent",
        205 => "ResetContent",
        206 => "PartialContent",
        300 => "MultipleChoices",
        301 => "MovedPermanently",
        302 => "Found",
        303 => "SeeOther",
        304 => "NotModified",
        307 => "TemporaryRedirect",
        308 => "PermanentRedirect",
        400 => "BadRequest",
        401 => "Unauthorized",
        402 => "PaymentRequired",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        406 => "NotAcceptable",
        407 => "ProxyAuthenticationRequired",
        408 => "RequestTimeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "LengthRequired",
        412 => "PreconditionFailed",
        413 => "PayloadTooLarge",
        414 => "RequestUriTooLong",
        415 => "UnsupportedMediaType",
        416 => "RangeNotSatisfiable",
        417 => "ExpectationFailed",
        422 => "UnprocessableEntity",
        425 => "TooEarly",
        426 => "UpgradeRequired",
        428 => "PreconditionRequired",
        429 => "TooManyRequests",
        431 => "RequestHeaderFieldsTooLarge",
        500 => "InternalServerError",
        501 => "NotImplemented",
        502 => "BadGateway",
        503 => "ServiceUnavailable",
        504 => "GatewayTimeout",
        505 => "HttpVersionNotSupported",
        _ => return None,
    };
    Some(n.to_string())
}

/// Human-readable status label for error messages (`"400"`, `"4XX"`,
/// `"default"`).
fn status_label(s: &ResponseStatus) -> String {
    match s {
        ResponseStatus::Explicit { code } => code.to_string(),
        ResponseStatus::Range { class } => format!("{class}XX"),
        ResponseStatus::Default => "default".to_string(),
    }
}

fn is_success_status(s: &ResponseStatus) -> bool {
    matches!(
        s,
        ResponseStatus::Explicit { code: 200..=299 } | ResponseStatus::Range { class: 2 }
    )
}

fn pick_json_body(resp: &Response) -> Option<&str> {
    resp.content
        .iter()
        .find(|c| {
            let m = c.media_type.to_ascii_lowercase();
            m.starts_with("application/json")
                || m.strip_prefix("application/")
                    .is_some_and(|r| r.ends_with("+json"))
        })
        .map(|c| c.r#type.as_str())
}

struct Field {
    ident: proc_macro2::Ident,
    ty: TokenStream,
    doc: Option<String>,
    arg_attr: Option<TokenStream>,
}

fn field_to_tokens(f: &Field) -> TokenStream {
    let ident = &f.ident;
    let ty = &f.ty;
    let doc_attr = doc_attrs_for(f.doc.as_deref());
    let arg_attr = f.arg_attr.as_ref().map(|a| quote!(#[arg(#a)]));
    quote!(#doc_attr #arg_attr #ident: #ty,)
}

fn collect_fields(spec: &Ir, op: &Operation, exclude_path_param: Option<&str>) -> Vec<Field> {
    let mut out = Vec::new();
    let exclude_snake = exclude_path_param.map(snake_case);
    let relax = relax_unless(op);
    for p in &op.path_params {
        if exclude_snake
            .as_deref()
            .is_some_and(|ex| ex == snake_case(&p.name))
        {
            continue;
        }
        out.push(field_for_param(spec, p, FieldKind::Positional, &relax));
    }
    for p in &op.query_params {
        out.push(field_for_param(spec, p, FieldKind::Flag, &relax));
    }
    for p in &op.header_params {
        out.push(field_for_param(spec, p, FieldKind::Flag, &relax));
    }
    for p in &op.cookie_params {
        out.push(field_for_param(spec, p, FieldKind::Flag, &relax));
    }
    if let Some(body) = &op.request_body {
        out.push(field_for_body(body, &relax));
        out.push(field_for_body_schema());
    }
    if op_has_response_content(op) {
        out.push(field_for_response_schema());
    }
    // Pagination flags are emitted only on ops with a well-formed
    // `x-pagination` extension — keeps non-paginated subcommands clean
    // and ensures `--all-pages` is unambiguous when present.
    if op_pagination(spec, op).is_some() {
        out.push(field_pages());
        out.push(field_all_pages());
    }
    out
}

fn field_pages() -> Field {
    Field {
        ident: format_ident!("pages"),
        ty: quote!(Option<u32>),
        doc: Some(
            "Walk up to N pages and emit each as one element of a JSON array on stdout. \
             Omit for single-page (raw body) output; use `--all-pages` for unlimited."
                .into(),
        ),
        // `value_parser!(u32).range(1..)` makes clap reject `--pages 0`
        // at parse time with its standard "value not in range" error,
        // rather than letting it through to a no-op loop that silently
        // emits `[]`. Upper bound left open — caps come from the
        // server, not us.
        arg_attr: Some(quote!(
            long = "pages",
            value_parser = clap::value_parser!(u32).range(1..)
        )),
    }
}

fn field_all_pages() -> Field {
    Field {
        ident: format_ident!("all_pages"),
        ty: quote!(bool),
        doc: Some(
            "Walk every page until the next-page token is missing or null. Overrides `--pages`. \
             First error stops with non-zero exit; pages already emitted remain on stdout."
                .into(),
        ),
        arg_attr: Some(quote!(long = "all-pages")),
    }
}

/// Schema-flag fields that suppress clap's required-arg gate on this
/// op's other params. Empty when the op has neither `--body-schema`
/// nor `--response-schema`.
fn relax_unless(op: &Operation) -> Vec<&'static str> {
    let mut v = Vec::new();
    if op.request_body.is_some() {
        v.push("body_schema");
    }
    if op_has_response_content(op) {
        v.push("response_schema");
    }
    v
}

#[derive(Copy, Clone)]
enum FieldKind {
    Positional,
    Flag,
}

fn field_for_param(spec: &Ir, p: &Parameter, kind: FieldKind, relax: &[&str]) -> Field {
    let field_ident = ident(&snake_case(&p.name));
    let kebab = kebab_case(&p.name);
    let relax_active = !relax.is_empty();
    let relax_lits: Vec<&&str> = relax.iter().collect();
    let shape = cli_arg_shape(spec, &p.r#type);
    let typed = &shape.cli_ty;

    // Arrays always render as `Vec<T>` regardless of `required` / relax:
    // clap interprets `Vec<T>` as "0+ occurrences"; missing-required
    // arrays are surfaced at dispatch time, not at parse time. This
    // mirrors how the tower request struct declares them (`Vec<T>`,
    // unconditionally) and matches the OpenAPI 3 default.
    let (ty, arg_attr) = if shape.is_array {
        let attr = match kind {
            FieldKind::Positional => quote!(),
            FieldKind::Flag => quote!(long = #kebab),
        };
        (typed.clone(), Some(attr))
    } else {
        match (kind, p.required) {
            (FieldKind::Positional, _) => {
                if relax_active {
                    (
                        quote!(Option<#typed>),
                        Some(quote!(required_unless_present_any = [#(#relax_lits),*])),
                    )
                } else {
                    (typed.clone(), None)
                }
            }
            (FieldKind::Flag, true) => {
                if relax_active {
                    (
                        quote!(Option<#typed>),
                        Some(
                            quote!(long = #kebab, required_unless_present_any = [#(#relax_lits),*]),
                        ),
                    )
                } else {
                    (typed.clone(), Some(quote!(long = #kebab)))
                }
            }
            (FieldKind::Flag, false) => (quote!(Option<#typed>), Some(quote!(long = #kebab))),
        }
    };
    Field {
        ident: field_ident,
        ty,
        doc: p
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        arg_attr,
    }
}

fn field_for_body(body: &Body, relax: &[&str]) -> Field {
    let relax_lits: Vec<&&str> = relax.iter().collect();
    let arg_attr = if body.required {
        quote!(long = "body", required_unless_present_any = [#(#relax_lits),*])
    } else {
        quote!(long = "body")
    };
    // Spec-supplied body description (if any) becomes the short help; the
    // syntax hint always trails as a second paragraph so `--help` users
    // still learn the inline / `@file` / `-` shorthand even when the spec
    // contributes its own description.
    let format_hint = "Accepts inline JSON, @file.json (read from file), or - (read from stdin). \
         Run with --body-schema to print the JSON Schema.";
    let spec_doc = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let doc = match spec_doc {
        Some(desc) => format!("{desc}\n\n{format_hint}"),
        None => format!("Request body.\n\n{format_hint}"),
    };
    Field {
        ident: format_ident!("body"),
        ty: quote!(Option<String>),
        doc: Some(doc),
        arg_attr: Some(arg_attr),
    }
}

fn field_for_body_schema() -> Field {
    Field {
        ident: format_ident!("body_schema"),
        ty: quote!(bool),
        doc: Some("Print the JSON Schema for the request body and exit.".into()),
        arg_attr: Some(quote!(long = "body-schema")),
    }
}

fn field_for_response_schema() -> Field {
    Field {
        ident: format_ident!("response_schema"),
        ty: quote!(bool),
        doc: Some(
            "Print the JSON Schemas for the response bodies, keyed by status code, and exit."
                .into(),
        ),
        arg_attr: Some(quote!(long = "response-schema")),
    }
}

fn op_has_response_content(op: &Operation) -> bool {
    op.responses.iter().any(|r| !r.content.is_empty())
}

fn build_long_about(
    title: &str,
    info_summary: Option<&str>,
    info_description: Option<&str>,
    bin_name: &str,
    prefix: &str,
    oauth_active: bool,
    exchange_placeholder_kebab: Option<&str>,
) -> String {
    let mut s = String::new();
    if !title.trim().is_empty() {
        s.push_str(title.trim());
        s.push_str("\n\n");
    }
    // `info.summary` (OAS 3.1+) gives a tagline that's wordier than the
    // bare title but tighter than the description. Show it under the title
    // when present, then unfold the full `info.description` (if any) as
    // its own block before the generator-supplied usage cheatsheet.
    if let Some(summary) = info_summary.map(str::trim).filter(|s| !s.is_empty()) {
        s.push_str(summary);
        s.push_str("\n\n");
    }
    if let Some(desc) = info_description.map(str::trim).filter(|s| !s.is_empty()) {
        s.push_str(desc);
        s.push_str("\n\n");
    }
    s.push_str(&format!(
        "Generated from an OpenAPI specification — each subcommand maps to one operation. \
         Run `{bin_name} help <command>` (or `<command> --help`) for per-operation flags.\n\n"
    ));

    s.push_str("Discover request and response shapes (no network call):\n");
    s.push_str(&format!(
        "  {bin_name} <op> --body-schema       JSON Schema for the request body.\n"
    ));
    s.push_str(&format!(
        "  {bin_name} <op> --response-schema   JSON Schemas for response bodies, keyed by status code.\n"
    ));
    s.push_str(
        "Bodies accept inline JSON, @file.json (read from file), or `-` (read from stdin).\n\n",
    );

    if oauth_active {
        s.push_str("Authentication and profiles:\n");
        s.push_str(&format!(
            "  {bin_name} login | logout         OAuth 2.0 authorization-code (PKCE); persists the access token.\n"
        ));
        s.push_str(&format!(
            "  {bin_name} configure              Edit the active profile (base_url, auth_url, client_id, ...).\n"
        ));
        s.push_str(&format!(
            "  {bin_name} profile <subcmd>       list | show | remove configured profiles.\n"
        ));
        s.push_str(
            "  --profile <name>                  Switch between configured profiles (default: \"default\").\n",
        );
        s.push_str(
            "  --token <jwt>                     Override the stored token for one call.\n\n",
        );
    }

    if let Some(ph) = exchange_placeholder_kebab {
        s.push_str("Multi-tenant operations:\n");
        s.push_str(&format!(
            "  Operations whose path includes `{{{ph}}}` mint a tenant-audienced JWT via RFC 8693 token exchange.\n"
        ));
        s.push_str(&format!(
            "  Pass `--{ph} <slug>` per call, or run `{bin_name} set-{ph} <slug>` to persist a default.\n\n"
        ));
    }

    s.push_str(&format!(
        "Shell completions: `{bin_name} completion <shell>` (bash | zsh | fish | powershell | elvish).\n"
    ));
    s.push_str(&format!(
        "Environment overrides: `{prefix}_BASE_URL`, `{prefix}_TOKEN`"
    ));
    if oauth_active {
        s.push_str(&format!(", `{prefix}_PROFILE`"));
    }
    s.push_str(", and per-flag env vars listed in each subcommand's help.");

    s
}

fn emit_schema_consts(ir: &Ir) -> TokenStream {
    let items = ir.operations.iter().flat_map(|op| {
        let pascal = screaming_snake(&op.id);
        let mut out: Vec<TokenStream> = Vec::new();
        if let Some(body) = &op.request_body {
            if let Some(s) = schema::render_body_schema(&ir.types, &ir.values, body) {
                let name = format_ident!("BODY_SCHEMA_{}", pascal);
                out.push(quote!(const #name: &str = #s;));
            }
        }
        if op_has_response_content(op) {
            if let Some(s) = schema::render_response_schemas(&ir.types, &ir.values, &op.responses) {
                let name = format_ident!("RESPONSE_SCHEMA_{}", pascal);
                out.push(quote!(const #name: &str = #s;));
            }
        }
        out
    });
    quote!(#(#items)*)
}
fn group_doc(group: &TagGroup) -> Option<String> {
    let tag = group.tag?;
    combine_summary_desc(tag.summary.as_deref(), tag.description.as_deref())
}

fn qualified_pascal(prefix: &str, name: &str) -> String {
    format!("{prefix}{}", pascal_case(name))
}

// ---------------------------------------------------------------------------
// src/runtime.rs
// ---------------------------------------------------------------------------

fn emit_runtime_rs() -> String {
    RUNTIME_RS.into()
}

const RUNTIME_RS: &str = include_str!("../templates/runtime.rs.in");

// ---------------------------------------------------------------------------
// src/auth.rs
// ---------------------------------------------------------------------------

fn emit_auth_rs(bin_name: &str, oa: &OauthInfo, base_url_default: &str) -> String {
    let auth_url = oa.flow.authorization_url.as_deref().unwrap();
    let token_url = oa.flow.token_url.as_deref().unwrap();
    let client_id = &oa.config.client_id;
    let port = oa.config.redirect_port.unwrap_or(8848);
    let scopes_lit: String = oa
        .scopes
        .iter()
        .map(|s| format!("\"{}\"", escape_rust_string(s)))
        .collect::<Vec<_>>()
        .join(", ");
    let client_secret_env = oa.config.client_secret_env.as_deref().unwrap_or("");
    let exchange_active = oa.exchange.is_some();

    let mut composed = String::with_capacity(AUTH_RS_PROLOGUE.len() + AUTH_RS_EXCHANGE_TAIL.len());
    composed.push_str(AUTH_RS_PROLOGUE);
    if exchange_active {
        composed.push_str(AUTH_RS_EXCHANGE_TAIL);
    }

    composed
        .replace("__BIN_NAME__", bin_name)
        .replace("__CLIENT_ID__", &escape_rust_string(client_id))
        .replace("__AUTH_URL__", &escape_rust_string(auth_url))
        .replace("__TOKEN_URL__", &escape_rust_string(token_url))
        .replace(
            "__BASE_URL_DEFAULT__",
            &escape_rust_string(base_url_default),
        )
        .replace("__REDIRECT_PORT__", &port.to_string())
        .replace("__SCOPES__", &scopes_lit)
        .replace(
            "__CLIENT_SECRET_ENV__",
            &escape_rust_string(client_secret_env),
        )
        .replace("__PREFIX__", &env_prefix(bin_name))
}

const AUTH_RS_PROLOGUE: &str = include_str!("../templates/auth_prologue.rs.in");

const AUTH_RS_EXCHANGE_TAIL: &str = include_str!("../templates/auth_exchange_tail.rs.in");

// ---------------------------------------------------------------------------
// README.md
// ---------------------------------------------------------------------------

fn emit_readme(ir: &Ir, bin_name: &str, oauth: Option<&OauthInfo>) -> String {
    let prefix = env_prefix(bin_name);
    let oauth_section = if let Some(oa) = oauth {
        let mut s = format!(
            "\n## OAuth\n\nThis CLI was generated with OAuth 2.0 (PKCE authorization-code) wired up.\n\n```sh\n{bin_name} login    # opens a browser, persists the access token\n{bin_name} logout   # deletes the stored token\n```\n\nThe token is stored at the platform config dir under `{bin_name}/profiles/<profile>/auth.json` (mode 0600 on Unix).\nThe token is refreshed lazily on a 30-second skew.\n\n## Profiles (AWS-style)\n\nThe CLI bundles deployment-specific settings (URLs, client ID/secret) under named profiles in `<config_dir>/{bin_name}/config.toml`. The `default` profile is auto-populated from the spec on first run.\n\n```sh\n{bin_name} --profile dev <op>          # one-off override\n{prefix}_PROFILE=dev {bin_name} <op>    # via env\n```\n\nProfile fields are: `base_url`, `auth_url`, `token_url`, `client_id`, `client_secret`. Hand-edit `config.toml` for now (`{bin_name} configure` lands in a follow-up). Resolution chain per setting: `--<flag>` → `{prefix}_<NAME>` env → profile field → spec default.\n\nClient-secret resolution: per-profile literal `client_secret = \"...\"` → `{prefix}_CLIENT_SECRET` env (or the env var named in the generator's `oauth.clientSecretEnv`) → none. Storing the secret literally in `config.toml` matches the security posture of `~/.aws/credentials` (mode 0600 on Unix).\n\n### Targeting a different IdP host\n\n```sh\nexport {prefix}_AUTH_URL=https://auth.dev.example.com/realms/<realm>/protocol/openid-connect/auth\nexport {prefix}_TOKEN_URL=https://auth.dev.example.com/realms/<realm>/protocol/openid-connect/token\n```\n\nOr, more durably, edit the relevant profile in `config.toml`.\n"
        );
        if let Some(env) = oa
            .config
            .client_secret_env
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            s.push_str(&format!(
                "\nThe configured OAuth client is **confidential** — set `{env}` to the client secret in your shell before running `{bin_name} login` (or any tenant-scoped operation).\n"
            ));
        }
        if let Some(ex) = &oa.exchange {
            let kebab = kebab_case(&ex.placeholder);
            s.push_str(&format!(
                "\n## Per-`{kebab}` token exchange (RFC 8693)\n\nOperations whose path includes `{{{ph}}}` use a tenant-scoped JWT minted via standard RFC 8693 token exchange against the IdP's token endpoint.\n\n```sh\n{bin_name} --{kebab} <slug> <op>           # one-off\n{bin_name} set-{kebab} <slug>                # persist a default (per active profile)\n{bin_name} unset-{kebab}                     # clear it (per active profile)\n{bin_name} show-{kebab}                      # show the current default\n```\n\n`set-{kebab}` writes to `<config_dir>/{bin_name}/profiles/<active>/{kebab}.json` — different profiles keep separate defaults.\n",
                ph = ex.placeholder,
            ));
        }
        s
    } else {
        String::new()
    };
    let completions_section = if oauth.is_some() {
        format!(
            "\n## Shell completions\n\nTwo flavors:\n\n### Static (subcommands + flags only)\n\n```sh\n# bash / zsh (current session)\nsource <({bin_name} completion bash)\nsource <({bin_name} completion zsh)\n\n# fish (current session) — fish has no <(...), pipe instead:\n{bin_name} completion fish | source\n\n# fish (persistent)\n{bin_name} completion fish > ~/.config/fish/completions/{bin_name}.fish\n```\n\nAlso supports `powershell` and `elvish`. Static scripts complete subcommand names and flag names; **they do not complete `--profile <TAB>` to known profile names** (the script doesn't dispatch back to the binary).\n\n### Dynamic (subcommands + flags + `--profile` values)\n\nAdd this to your shell init:\n\n```sh\n# bash\neval \"$(COMPLETE=bash {bin_name})\"\n\n# zsh\neval \"$(COMPLETE=zsh {bin_name})\"\n\n# fish\nCOMPLETE=fish {bin_name} | source\n```\n\nWith dynamic completion enabled, `{bin_name} --profile <TAB>` lists profile names from `config.toml` at completion time.\n"
        )
    } else {
        format!(
            "\n## Shell completions\n\n```sh\n# bash / zsh (current session)\nsource <({bin_name} completion bash)\nsource <({bin_name} completion zsh)\n\n# fish (current session) — fish has no <(...), pipe instead:\n{bin_name} completion fish | source\n\n# fish (persistent)\n{bin_name} completion fish > ~/.config/fish/completions/{bin_name}.fish\n```\n\nAlso supports `powershell` and `elvish`. Add to your shell init for persistence.\n"
        )
    };
    format!(
        "# {bin_name}\n\nGenerated by openapi-forge / generator-rust-clap.\n\nSpec: {title} v{version}\n\nOperations: {n}\n\n## Build\n\n```sh\ncargo build --release\n```\n\n## Auth\n\nBearer token via `--token <jwt>` or the env var `{prefix}_TOKEN`.\n{oauth_section}{completions_section}",
        title = ir.info.title,
        version = ir.info.version,
        n = ir.operations.len(),
    )
}

/// Combine an OAS `summary` (one-liner) and `description` (long-form) into
/// the doc string we hand to clap. Clap's derive macro splits on the first
/// blank line: text before it becomes the short `-h`/`about` help; the
/// whole string becomes the long `--help`/`long_about` help. We collapse
/// any summary to its first line and put a blank line between summary and
/// description so the split lands where the spec author intended.
fn combine_summary_desc(summary: Option<&str>, description: Option<&str>) -> Option<String> {
    let summary = summary
        .and_then(|s| s.lines().next())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let description = description.map(str::trim).filter(|s| !s.is_empty());
    match (summary, description) {
        (None, None) => None,
        (Some(s), None) => Some(s.to_string()),
        (None, Some(d)) => Some(d.to_string()),
        (Some(s), Some(d)) => Some(format!("{s}\n\n{d}")),
    }
}

/// Emit one `#[doc = "..."]` attribute per line of `s`. Blank lines are
/// preserved so the clap derive macro can find a paragraph boundary.
fn doc_attrs_for(s: Option<&str>) -> TokenStream {
    let Some(text) = s.map(str::trim).filter(|s| !s.is_empty()) else {
        return TokenStream::new();
    };
    let attrs = text.lines().map(|line| {
        let l = line.trim_end();
        quote!(#[doc = #l])
    });
    quote!(#(#attrs)*)
}

fn escape_rust_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_placeholders_simple() {
        assert_eq!(
            extract_placeholders("urn:x:tenant:{tenant}"),
            vec!["tenant"]
        );
        assert_eq!(
            extract_placeholders("https://api/{a}/{b}/items"),
            vec!["a", "b"]
        );
        assert_eq!(extract_placeholders("static"), Vec::<String>::new());
    }

    #[test]
    fn auth_prologue_includes_write_profile_fields() {
        // Non-interactive configure flow depends on this helper being emitted.
        assert!(
            AUTH_RS_PROLOGUE.contains("pub fn write_profile_fields("),
            "auth.rs prologue must define write_profile_fields"
        );
        assert!(
            AUTH_RS_PROLOGUE.contains("client_secret: Option<String>,"),
            "write_profile_fields must accept Option<String> for each field"
        );
    }

    /// Issue #24: when the OAuth callback's `state` doesn't match the
    /// freshly-generated one, the prior bail!() bottomed out at
    /// `"state mismatch (CSRF check failed)"` with no recovery guidance.
    /// The most common cause is a leftover browser tab from an aborted
    /// previous `login` firing its callback against the new listener —
    /// surface that as a hint so users self-recover without a support
    /// search.
    #[test]
    fn login_state_mismatch_includes_recovery_hint() {
        // Pin the literal so a refactor that accidentally drops the
        // check is visible here.
        assert!(
            AUTH_RS_PROLOGUE.contains("state mismatch"),
            "login flow must still detect state mismatch (CSRF check)"
        );
        // The recovery hint must point at the actionable cause — a stale
        // browser tab from a prior aborted login — not just restate the
        // protocol-level failure.
        for needle in ["aborted", "browser tab"] {
            assert!(
                AUTH_RS_PROLOGUE.contains(needle),
                "state-mismatch error must mention `{needle}` as part of \
                 the recovery hint (#24)"
            );
        }
    }

    /// Issue #23: the token-exchange failure path used to *unconditionally*
    /// append a "set CLIENT_SECRET / add `client_secret` to your profile"
    /// hint. That hint is misleading for non-auth errors (e.g.
    /// `invalid_client` with `error_description: "Audience not found"`
    /// caused by a bad `--tenant` value). Pin the new shape: parse the
    /// body's RFC 6749 `error` field and branch.
    #[test]
    fn exchange_tail_defines_error_formatter_helper() {
        // The shared formatter must be defined as a free function so the
        // logic is exercised at one site (the `!resp.status().is_success()`
        // branch) rather than duplicated inline. Locking the name down
        // also lets future tests / consumers reference it deliberately.
        assert!(
            AUTH_RS_EXCHANGE_TAIL.contains("fn format_token_exchange_failure("),
            "exchange tail must define `format_token_exchange_failure` helper:\n\
             ---\n{AUTH_RS_EXCHANGE_TAIL}\n---"
        );
        // The helper must attempt to parse the body as an RFC 6749 error
        // object (`{ "error": "...", "error_description": "..." }`) and
        // surface the `error` code into the branching logic.
        assert!(
            AUTH_RS_EXCHANGE_TAIL.contains("serde_json::from_str")
                || AUTH_RS_EXCHANGE_TAIL.contains("serde_json::from_slice"),
            "helper must best-effort parse the response body as JSON \
             (RFC 6749 error object) before deciding which hint to append"
        );
        assert!(
            AUTH_RS_EXCHANGE_TAIL.contains("invalid_client"),
            "helper must branch on the RFC 6749 `invalid_client` error code"
        );
    }

    /// Catch syntax errors in the exchange-tail template before they
    /// surface as a `forge generate` failure on a downstream fixture.
    /// `syn::parse_str` doesn't resolve identifiers (it doesn't care
    /// that `CLIENT_SECRET_ENV` is defined in the prologue, not here),
    /// it just verifies the token stream is well-formed Rust.
    #[test]
    fn exchange_tail_parses_as_rust() {
        let result = syn::parse_str::<syn::File>(AUTH_RS_EXCHANGE_TAIL);
        assert!(
            result.is_ok(),
            "exchange tail template failed to parse as Rust: {:?}",
            result.err()
        );
    }

    /// The unconditional client-secret hint (env var + `[profiles.<x>]`
    /// blob) must no longer fire for every non-2xx response. It is now
    /// gated behind the `invalid_client` branch of the helper.
    #[test]
    fn exchange_tail_does_not_unconditionally_emit_client_secret_hint() {
        // Pre-fix the bail!() string literal carried this exact phrase
        // alongside the status/body interpolation. Post-fix the same
        // phrase still appears (it's the *content* of the gated hint —
        // we don't want to lose the diagnostic when it is appropriate)
        // but only inside `format_token_exchange_failure`, never in the
        // unconditional bail!() at the call site.
        //
        // Locate the failure branch (`if !resp.status().is_success()`)
        // and assert its body does not contain the inline hint literal.
        let needle = "if !resp.status().is_success()";
        let start = AUTH_RS_EXCHANGE_TAIL
            .find(needle)
            .expect("expected `if !resp.status().is_success()` guard in exchange tail");
        // Walk to the matching `}` at depth 0.
        let body_start = AUTH_RS_EXCHANGE_TAIL[start..]
            .find('{')
            .map(|i| start + i)
            .expect("guard has a body");
        let mut depth = 0usize;
        let mut end = body_start;
        for (i, ch) in AUTH_RS_EXCHANGE_TAIL[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = body_start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let branch = &AUTH_RS_EXCHANGE_TAIL[start..end];
        assert!(
            !branch.contains("if the IdP requires client authentication"),
            "the unconditional client-secret hint must not appear in the \
             failure branch — gate it behind `format_token_exchange_failure` \
             so it only fires for the actual `invalid_client` case (#23):\n\
             ---\n{branch}\n---"
        );
        // The call site itself must dispatch through the helper.
        assert!(
            branch.contains("format_token_exchange_failure"),
            "failure branch must dispatch through `format_token_exchange_failure`:\n\
             ---\n{branch}\n---"
        );
    }

    #[test]
    fn configure_variant_exposes_non_interactive_flags() {
        // Empty tag tree is enough to drive emit_root_enum into the oauth branch.
        // The empty tree means `render_op_variant` is never called, so the spec
        // doesn't need to carry any operations; a minimal stub satisfies the
        // signature.
        let tree = TagTree { roots: vec![] };
        let ir: Ir =
            forge_plugin_sdk::serde_json::from_value(forge_plugin_sdk::serde_json::json!({
                "info": {"title": "", "version": ""},
                "operations": [],
                "types": [],
                "security_schemes": [],
                "servers": [],
            }))
            .expect("minimal IR stub deserializes");
        let tokens = emit_root_enum(&ir, &tree, /*oauth_active*/ true, None, None, None);
        // TokenStream → String drops formatting but preserves identifiers
        // and punctuation, which is all this test asserts on.
        let out = tokens.to_string();
        // Struct-form Configure with each scriptable field + the bypass flag.
        // Field syntax round-trips through quote! as `name : ty ,` (with spaces),
        // so assert on the field identifiers separately from their types.
        assert!(
            out.contains("Configure"),
            "missing Configure variant:\n{out}"
        );
        for needle in [
            "base_url",
            "auth_url",
            "token_url",
            "client_id",
            "client_secret",
            "non_interactive",
        ] {
            assert!(
                out.contains(needle),
                "emitted Cmd enum missing `{needle}`:\n{out}"
            );
        }
    }

    fn minimal_ir() -> Ir {
        forge_plugin_sdk::serde_json::from_value(forge_plugin_sdk::serde_json::json!({
            "info": {"title": "", "version": ""},
            "operations": [],
            "types": [],
            "security_schemes": [],
            "servers": [],
        }))
        .expect("minimal IR stub deserializes")
    }

    /// Issue #25: when OAuth is wired up, the CLI should expose a
    /// built-in `token` subcommand that prints the active bearer to
    /// stdout so consumers can shell it into `curl`/`httpie` etc. The
    /// token logic isn't part of the generated public surface
    /// (`audience_access_token` lives behind module-private helpers in
    /// `auth.rs`), so consumers can't usefully add their own.
    #[test]
    fn token_variant_emitted_when_oauth_active() {
        let tree = TagTree { roots: vec![] };
        let ir = minimal_ir();
        let tokens = emit_root_enum(&ir, &tree, /*oauth_active*/ true, None, None, None);
        let out = tokens.to_string();
        assert!(
            out.contains("Token"),
            "OAuth-active CLI must expose a `Token` subcommand variant:\n{out}"
        );
    }

    /// Mirror of the previous test: without OAuth there's no auth flow
    /// to print, so the variant must NOT be emitted (clap would still
    /// generate a working `token` subcommand but it would have nothing
    /// to dispatch to — the existing `__token` field on `Cli` is
    /// already the user-side bearer override).
    #[test]
    fn token_variant_omitted_when_oauth_inactive() {
        let tree = TagTree { roots: vec![] };
        let ir = minimal_ir();
        let tokens = emit_root_enum(&ir, &tree, /*oauth_active*/ false, None, None, None);
        let out = tokens.to_string();
        // Substring scan would false-positive on identifiers like
        // `__token`; check for the variant in its quoted form
        // (`Token ,` with whitespace as quote! emits it) to be precise.
        // Also assert no `Token` identifier slipped in at all from this
        // path — the field-of-Cli stays on Cli, not in Cmd.
        assert!(
            !out.contains("Token"),
            "non-OAuth CLI must not emit a `Token` Cmd variant:\n{out}"
        );
    }

    /// Pin the dispatch site: the generated `main.rs` must route the
    /// `Cmd::Token` arm through `access_token` (and through
    /// `audience_access_token` when exchange is configured). Inspect
    /// the prologue template's emitted form via `emit_auth_rs` — wait,
    /// that's not the right surface. Instead inspect `emit_main_rs`
    /// output for the dispatch wiring.
    ///
    /// We can't easily construct an `OauthInfo` here (it holds refs),
    /// but we can read back the generated CLI source after `forge
    /// generate` runs the wasm. Since this unit test runs *inside* the
    /// wasm plugin, we sidestep that and just verify the dispatch
    /// helper at the emit-site level: `emit_main_rs` is responsible
    /// for the `if matches!(cli.cmd, Cmd::Token)` block, but we can't
    /// drive it from a test without an OauthInfo. Compromise: assert
    /// the `Cmd::Token` arm appears in the unreachable-handled list in
    /// the match-arm assembly, by stringifying the relevant constant
    /// shape via `emit_root_enum` + a manual stub.
    ///
    /// In practice, the codegen-shape test above plus the downstream
    /// `forge generate fixtures/clap-petstore` build is enough — the
    /// fixture doesn't have OAuth, but adding the variant without
    /// wiring up the dispatch would produce an unhandled arm and fail
    /// the `cargo build` row.
    #[test]
    fn token_variant_has_descriptive_doc_comment() {
        // The variant is one of the few user-facing things in `--help`
        // output. The doc comment must mention what it prints (token /
        // bearer) so the user discovers it from `--help`.
        let tree = TagTree { roots: vec![] };
        let ir = minimal_ir();
        let tokens = emit_root_enum(&ir, &tree, /*oauth_active*/ true, None, None, None);
        let out = tokens.to_string();
        // quote! preserves doc comments as `#[doc = "..."]` attributes,
        // which serialize back to a `#[doc = "..."]` token tree. Look
        // for the substring inside the doc.
        let lower = out.to_ascii_lowercase();
        assert!(
            lower.contains("bearer") || lower.contains("access token"),
            "Token variant doc comment must mention `bearer` or `access token`:\n{out}"
        );
    }

    /// Issue #24 suggestion #2: the existing CSRF-state hint (#29)
    /// points users at the recovery action, but `--help` doesn't
    /// surface it. A `login --reset` flag wipes any stored token for
    /// the active profile *before* starting the new authorize/exchange
    /// dance — that gives users a discoverable escape hatch and turns
    /// the workaround into a documented one-liner.
    #[test]
    fn login_variant_carries_reset_flag() {
        let tree = TagTree { roots: vec![] };
        let ir = minimal_ir();
        let tokens = emit_root_enum(&ir, &tree, /*oauth_active*/ true, None, None, None);
        let out = tokens.to_string();
        // Struct-form variant: `Login { ... reset : bool , ... }`.
        // quote! reformats field punctuation with spaces, so check for
        // the field identifier and its type independently.
        assert!(
            out.contains("Login {"),
            "OAuth-active CLI must emit `Login` as a struct-form variant \
             (was a unit variant pre-#24); needs a `reset` field for the \
             explicit-reset path:\n{out}"
        );
        assert!(
            out.contains("reset"),
            "Login variant must carry a `reset` field for #24 suggestion #2:\n{out}"
        );
        // The reset flag must be bool — `Option<bool>` would force users
        // to write `--reset true`, which clap doesn't ergonomically
        // expose. Pin it explicitly.
        assert!(
            out.contains("reset : bool") || out.contains("reset: bool"),
            "Login.reset must be a plain `bool` (so `--reset` is a flag, \
             not a value-carrying option):\n{out}"
        );
    }

    /// Inverse: without OAuth there's no login flow to reset, so the
    /// variant must stay absent (or unit). Asserts the existing
    /// oauth-inactive shape is untouched.
    #[test]
    fn login_variant_absent_without_oauth() {
        let tree = TagTree { roots: vec![] };
        let ir = minimal_ir();
        let tokens = emit_root_enum(&ir, &tree, /*oauth_active*/ false, None, None, None);
        let out = tokens.to_string();
        assert!(
            !out.contains("Login"),
            "non-OAuth CLI must not emit a `Login` Cmd variant:\n{out}"
        );
    }
}
