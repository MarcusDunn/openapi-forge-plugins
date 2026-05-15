//! Generation entry point. Emits a buildable Rust CLI crate (clap
//! derive + reqwest) with tag-grouped subcommands. When the spec
//! plus plugin config opt in, the emitted CLI also supports OAuth
//! 2.0 PKCE login/logout and optional RFC-8693 token exchange driven
//! by a generic `x-token-exchange` extension on the chosen `oauth2`
//! security scheme.

use std::collections::BTreeSet;

use forge_plugin_sdk::ir::{
    Body, HttpMethod, Ir, OAuth2Flow, OAuth2FlowKind, Operation, Parameter, SecurityScheme,
    SecuritySchemeKind,
};
use forge_plugin_sdk::{values_ext, GenerationOutput, OutputFile};

use crate::config::{Config, OAuthConfig};
use crate::naming::{kebab_case, pascal_case, screaming_snake, snake_case};
use crate::schema;
use crate::tags::{self, TagGroup, TagTree};

pub fn all(ir: &Ir, cfg: &Config) -> GenerationOutput {
    let bin_name = bin_name(ir, cfg);
    let pkg_name = format!("{bin_name}-cli");
    let oauth = detect_oauth(ir, cfg);

    let mut files = vec![
        OutputFile::text(
            "Cargo.toml",
            emit_cargo_toml(&pkg_name, &bin_name, oauth.is_some()),
        ),
        OutputFile::text(
            "src/main.rs",
            emit_main_rs(ir, cfg, &bin_name, oauth.as_ref()),
        ),
        OutputFile::text("src/client.rs", emit_client_rs(ir)),
        OutputFile::text("src/runtime.rs", emit_runtime_rs()),
        OutputFile::text("README.md", emit_readme(ir, &bin_name, oauth.as_ref())),
    ];
    if let Some(oa) = &oauth {
        files.push(OutputFile::text(
            "src/auth.rs",
            emit_auth_rs(&bin_name, oa, &default_base_url(ir, cfg)),
        ));
    }

    GenerationOutput {
        files,
        diagnostics: vec![],
    }
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
// Cargo.toml
// ---------------------------------------------------------------------------

fn emit_cargo_toml(pkg_name: &str, bin_name: &str, oauth: bool) -> String {
    let oauth_block = if oauth {
        r#"sha2 = "0.10"
base64 = "0.22"
rand = "0.8"
webbrowser = "1"
directories = "6"
toml = "0.8"
dialoguer = "0.11"
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
reqwest = {{ version = "0.12", default-features = false, features = ["json", "rustls-tls"] }}
urlencoding = "2"
{oauth_block}"#
    )
}

// ---------------------------------------------------------------------------
// src/main.rs
// ---------------------------------------------------------------------------

fn emit_main_rs(ir: &Ir, cfg: &Config, bin_name: &str, oauth: Option<&OauthInfo>) -> String {
    let title = escape_rust_string(&ir.info.title);
    let version = escape_rust_string(&ir.info.version);
    let base_url = escape_rust_string(&default_base_url(ir, cfg));
    let prefix = env_prefix(bin_name);

    let tree = tags::build(ir);
    let oauth_active = oauth.is_some();
    let exchange = oauth.and_then(|o| o.exchange.as_ref());
    let placeholder_kebab = exchange.map(|e| kebab_case(&e.placeholder));
    let placeholder_snake = exchange.map(|e| snake_case(&e.placeholder));
    let placeholder_pascal = exchange.map(|e| pascal_case(&e.placeholder));

    let mut out = String::new();
    out.push_str("// Generated by openapi-forge / generator-rust-clap; do not edit by hand.\n#![allow(clippy::needless_late_init, clippy::redundant_field_names, clippy::too_many_arguments, clippy::collapsible_if)]\n\n");
    if oauth_active {
        out.push_str("mod auth;\n");
    }
    out.push_str("mod client;\nmod runtime;\n\nuse clap::{Args, CommandFactory, Parser, Subcommand};\nuse client::ApiClient;\nuse runtime::OutputMode;\n\n");

    out.push_str(&emit_schema_consts(ir));

    let base_url_decl = if oauth_active {
        format!(
            "    /// API base URL. When unset, falls back to the active profile, then the spec default.\n    #[arg(long, global = true, env = \"{prefix}_BASE_URL\")]\n    base_url: Option<String>,\n\n"
        )
    } else {
        format!(
            "    /// API base URL.\n    #[arg(long, global = true, env = \"{prefix}_BASE_URL\", default_value = \"{base_url}\")]\n    base_url: String,\n\n"
        )
    };
    let profile_decl = if oauth_active {
        format!(
            "    /// Profile to use; bundles base_url / auth_url / token_url / client_id / client_secret. Default: \"default\".\n    #[arg(long, global = true, env = \"{prefix}_PROFILE\", default_value = \"default\", add = clap_complete::ArgValueCandidates::new(__complete_profile_names))]\n    profile: String,\n\n"
        )
    } else {
        String::new()
    };
    let long_about = build_long_about(
        &ir.info.title,
        bin_name,
        &prefix,
        oauth_active,
        placeholder_kebab.as_deref(),
    );
    let long_about_lit = json_string(&long_about);
    out.push_str(&format!(
        "#[derive(Parser)]\n#[command(name = \"{bin_name}\", version = \"{version}\", about = \"{title}\", long_about = {long_about_lit})]\nstruct Cli {{\n{base_url_decl}    /// Bearer token. Overrides any stored OAuth or exchanged token.\n    #[arg(long, global = true, env = \"{prefix}_TOKEN\")]\n    token: Option<String>,\n\n    /// Output mode for response bodies.\n    #[arg(long, global = true, value_enum, default_value_t = OutputMode::Json)]\n    output: OutputMode,\n\n{profile_decl}"
    ));

    if let (Some(kebab), Some(snake)) = (&placeholder_kebab, &placeholder_snake) {
        let env_name = format!("{}_{}", prefix, screaming_snake(snake));
        out.push_str(&format!(
            "    /// Slug used to template the RFC 8693 exchange audience for tenant-scoped operations.\n    #[arg(long = \"{kebab}\", global = true, env = \"{env_name}\")]\n    {snake}: Option<String>,\n\n"
        ));
    }

    out.push_str("    #[command(subcommand)]\n    cmd: Cmd,\n}\n\n");

    if oauth_active {
        out.push_str("/// Dynamic-completion callback for the global `--profile` flag.\n/// Invoked at completion time by the shell; reads `config.toml` fresh.\nfn __complete_profile_names() -> Vec<clap_complete::CompletionCandidate> {\n    auth::list_profile_names()\n        .into_iter()\n        .map(clap_complete::CompletionCandidate::new)\n        .collect()\n}\n\n");
    }

    if ir.operations.is_empty() && !oauth_active {
        out.push_str("#[derive(Subcommand)]\nenum Cmd {\n    /// (No operations declared in the spec.)\n    #[command(hide = true)]\n    Noop,\n}\n\n");
    } else {
        emit_root_enum(
            &mut out,
            &tree,
            oauth_active,
            exchange,
            placeholder_pascal.as_deref(),
            placeholder_kebab.as_deref(),
        );
        for root in &tree.roots {
            emit_group_types(&mut out, root, "", exchange);
        }
    }

    // main()
    out.push_str(
        "#[tokio::main(flavor = \"multi_thread\")]\nasync fn main() -> anyhow::Result<()> {\n",
    );
    if oauth_active {
        out.push_str("    // Dynamic shell-completion dispatch. When `COMPLETE` env is set\n    // (e.g. by `eval \"$(COMPLETE=bash <bin>)\"` in shell init), this\n    // prints completions and exits before any normal CLI handling.\n    clap_complete::CompleteEnv::with_factory(Cli::command).complete();\n");
    }
    out.push_str("    let cli = Cli::parse();\n");

    // Shell-completion subcommand (always emitted, no spec opt-in).
    out.push_str(&format!(
        "    if let Cmd::Completion {{ shell }} = cli.cmd {{\n        let mut cmd = Cli::command();\n        clap_complete::generate(shell, &mut cmd, \"{bin_name}\", &mut std::io::stdout());\n        return Ok(());\n    }}\n"
    ));

    // Profile bootstrap + legacy migration (idempotent, run every invocation).
    if oauth_active {
        out.push_str(
            "    auth::migrate_legacy()?;\n    auth::bootstrap_default_profile_if_missing()?;\n",
        );
    }

    // Built-in handlers for login / logout / configure / profile / placeholder-config.
    if oauth_active {
        out.push_str("    if matches!(cli.cmd, Cmd::Login) {\n        auth::login(&cli.profile).await?;\n        eprintln!(\"logged in (profile: {})\", cli.profile);\n        return Ok(());\n    }\n    if matches!(cli.cmd, Cmd::Logout) {\n        let removed = auth::logout(&cli.profile).await?;\n        eprintln!(\"{}\", if removed { \"logged out\" } else { \"no stored token\" });\n        return Ok(());\n    }\n    if let Cmd::Configure { base_url, auth_url, token_url, client_id, client_secret, non_interactive } = &cli.cmd {\n        let any_field = base_url.is_some() || auth_url.is_some() || token_url.is_some() || client_id.is_some() || client_secret.is_some();\n        if *non_interactive || any_field {\n            auth::write_profile_fields(\n                &cli.profile,\n                base_url.clone(),\n                auth_url.clone(),\n                token_url.clone(),\n                client_id.clone(),\n                client_secret.clone(),\n            )?;\n        } else {\n            let should_login = auth::interactive_configure(&cli.profile)?;\n            if should_login {\n                auth::login(&cli.profile).await?;\n                eprintln!(\"logged in (profile: {})\", cli.profile);\n            }\n        }\n        return Ok(());\n    }\n    if let Cmd::Profile(args) = &cli.cmd {\n        match &args.cmd {\n            ProfileCmd::List => {\n                for name in auth::list_profile_names() {\n                    println!(\"{}\", name);\n                }\n                return Ok(());\n            }\n            ProfileCmd::Show { name } => {\n                let p = name.as_deref().unwrap_or(cli.profile.as_str());\n                auth::show_profile(p)?;\n                return Ok(());\n            }\n            ProfileCmd::Remove { name } => {\n                let removed = auth::remove_profile(name)?;\n                eprintln!(\"{}\", if removed { \"removed\" } else { \"not found\" });\n                return Ok(());\n            }\n        }\n    }\n");
    }
    if let Some(pascal) = &placeholder_pascal {
        let kebab = placeholder_kebab.as_ref().unwrap();
        out.push_str(&format!(
            "    if let Cmd::Set{pascal} {{ value }} = &cli.cmd {{\n        auth::write_persisted(&cli.profile, \"{kebab}\", value)?;\n        eprintln!(\"persisted {kebab} = {{}} (profile: {{}})\", value, cli.profile);\n        return Ok(());\n    }}\n    if matches!(cli.cmd, Cmd::Unset{pascal}) {{\n        let removed = auth::delete_persisted(&cli.profile, \"{kebab}\")?;\n        eprintln!(\"{{}}\", if removed {{ \"unset\" }} else {{ \"no persisted value\" }});\n        return Ok(());\n    }}\n    if matches!(cli.cmd, Cmd::Show{pascal}) {{\n        match auth::read_persisted(&cli.profile, \"{kebab}\")? {{\n            Some(v) => println!(\"{{}}\", v),\n            None => eprintln!(\"(none)\"),\n        }}\n        return Ok(());\n    }}\n"
        ));
    }

    // Resolve effective placeholder value (flag/env via clap → persisted default).
    if let Some(snake) = &placeholder_snake {
        let kebab = placeholder_kebab.as_ref().unwrap();
        out.push_str(&format!(
            "    let __resolved_{snake}: Option<String> = match cli.{snake}.clone() {{\n        Some(v) => Some(v),\n        None => auth::read_persisted(&cli.profile, \"{kebab}\")?,\n    }};\n"
        ));
    }

    if oauth_active {
        out.push_str("    let __base_url = auth::resolve_base_url(&cli.profile, cli.base_url.as_deref())?;\n    let client = ApiClient::new(__base_url)?;\n");
    } else {
        out.push_str("    let client = ApiClient::new(cli.base_url)?;\n");
    }
    out.push_str("    let result: serde_json::Value = match cli.cmd {\n");
    out.push_str("        Cmd::Completion { .. } => unreachable!(\"handled above\"),\n");
    if oauth_active {
        out.push_str("        Cmd::Login | Cmd::Logout | Cmd::Configure { .. } | Cmd::Profile(_) => unreachable!(\"handled above\"),\n");
    }
    if let Some(pascal) = &placeholder_pascal {
        out.push_str(&format!(
            "        Cmd::Set{pascal} {{ .. }} | Cmd::Unset{pascal} | Cmd::Show{pascal} => unreachable!(\"handled above\"),\n"
        ));
    }
    if ir.operations.is_empty() && !oauth_active {
        out.push_str("        Cmd::Noop => return Ok(()),\n");
    } else {
        for root in &tree.roots {
            emit_root_match_arms(&mut out, root, "", oauth, exchange);
        }
    }
    out.push_str("    };\n    runtime::print_output(&result, cli.output)\n}\n");

    out
}

fn emit_root_enum(
    out: &mut String,
    tree: &TagTree,
    oauth_active: bool,
    exchange: Option<&TokenExchangeInfo>,
    placeholder_pascal: Option<&str>,
    placeholder_kebab: Option<&str>,
) {
    out.push_str("#[derive(Subcommand)]\nenum Cmd {\n");
    out.push_str("    /// Print a shell completion script. e.g. `source <(<bin> completion bash)`.\n    Completion {\n        /// Target shell.\n        #[arg(value_enum)]\n        shell: clap_complete::Shell,\n    },\n");
    if oauth_active {
        out.push_str("    /// Run OAuth 2.0 authorization-code flow with PKCE; persists the access token.\n    Login,\n    /// Delete the stored OAuth token.\n    Logout,\n    /// Create or edit the active profile (use `--profile` to pick). With no flags, prompts interactively; with `--non-interactive` (or any field flag), writes without prompting.\n    Configure {\n        /// Set base_url non-interactively.\n        #[arg(long)]\n        base_url: Option<String>,\n        /// Set auth_url non-interactively.\n        #[arg(long)]\n        auth_url: Option<String>,\n        /// Set token_url non-interactively.\n        #[arg(long)]\n        token_url: Option<String>,\n        /// Set client_id non-interactively.\n        #[arg(long)]\n        client_id: Option<String>,\n        /// Set client_secret non-interactively. Stored in `config.toml` (mode 0600). Prefer setting via the env var when possible.\n        #[arg(long)]\n        client_secret: Option<String>,\n        /// Skip all prompts. Fields not given as flags keep their existing value.\n        #[arg(long)]\n        non_interactive: bool,\n    },\n    /// List, inspect, or delete profiles.\n    Profile(ProfileArgs),\n");
    }
    if exchange.is_some() {
        let pascal = placeholder_pascal.unwrap();
        let kebab = placeholder_kebab.unwrap();
        out.push_str(&format!(
            "    /// Persist a default `{kebab}` so subsequent calls can omit `--{kebab}`.\n    Set{pascal} {{ value: String }},\n    /// Clear the persisted default `{kebab}`.\n    Unset{pascal},\n    /// Print the persisted default `{kebab}`.\n    Show{pascal},\n"
        ));
    }
    for root in &tree.roots {
        if root.is_misc() {
            for op in &root.direct_ops {
                out.push_str(&render_op_variant(op, "    ", exchange));
            }
        } else {
            let variant = pascal_case(&root.name);
            let qualified = qualified_pascal("", &root.name);
            push_doc(out, group_doc(root), "    ");
            out.push_str(&format!("    {variant}({qualified}Args),\n"));
        }
    }
    out.push_str("}\n\n");

    if oauth_active {
        out.push_str("#[derive(Args)]\npub struct ProfileArgs {\n    #[command(subcommand)]\n    cmd: ProfileCmd,\n}\n\n#[derive(Subcommand)]\npub enum ProfileCmd {\n    /// List configured profile names from config.toml.\n    List,\n    /// Print the resolved settings for a profile (secret redacted).\n    Show {\n        /// Profile name; defaults to the global --profile value.\n        #[arg(add = clap_complete::ArgValueCandidates::new(__complete_profile_names))]\n        name: Option<String>,\n    },\n    /// Delete a profile from config.toml and remove its on-disk dir.\n    Remove {\n        /// Profile name (required to avoid accidental deletion).\n        #[arg(add = clap_complete::ArgValueCandidates::new(__complete_profile_names))]\n        name: String,\n    },\n}\n\n");
    }
}

fn emit_group_types(
    out: &mut String,
    group: &TagGroup,
    prefix: &str,
    exchange: Option<&TokenExchangeInfo>,
) {
    if group.is_misc() {
        return;
    }
    let q = qualified_pascal(prefix, &group.name);
    let about = group_doc(group).unwrap_or_default();
    out.push_str(&format!(
        "#[derive(Args)]\n#[command(about = {})]\npub struct {q}Args {{\n    #[command(subcommand)]\n    cmd: {q}Cmd,\n}}\n\n",
        json_string(&about),
    ));

    out.push_str(&format!("#[derive(Subcommand)]\npub enum {q}Cmd {{\n"));
    for child in &group.children {
        let child_q = qualified_pascal(&q, &child.name);
        let variant = pascal_case(&child.name);
        push_doc(out, group_doc(child), "    ");
        out.push_str(&format!("    {variant}({child_q}Args),\n"));
    }
    for op in &group.direct_ops {
        out.push_str(&render_op_variant(op, "    ", exchange));
    }
    out.push_str("}\n\n");

    for child in &group.children {
        emit_group_types(out, child, &q, exchange);
    }
}

fn emit_root_match_arms(
    out: &mut String,
    root: &TagGroup,
    prefix: &str,
    oauth: Option<&OauthInfo>,
    exchange: Option<&TokenExchangeInfo>,
) {
    if root.is_misc() {
        for op in &root.direct_ops {
            out.push_str(&render_op_match_arm(op, "Cmd", "        ", oauth, exchange));
        }
        return;
    }
    let variant = pascal_case(&root.name);
    let q = qualified_pascal(prefix, &root.name);
    out.push_str(&format!(
        "        Cmd::{variant}(__g) => match __g.cmd {{\n"
    ));
    emit_group_match_arms(out, root, &q, "            ", oauth, exchange);
    out.push_str("        },\n");
}

fn emit_group_match_arms(
    out: &mut String,
    group: &TagGroup,
    q: &str,
    indent: &str,
    oauth: Option<&OauthInfo>,
    exchange: Option<&TokenExchangeInfo>,
) {
    let cmd_ty = format!("{q}Cmd");
    for child in &group.children {
        let child_variant = pascal_case(&child.name);
        let child_q = qualified_pascal(q, &child.name);
        out.push_str(&format!(
            "{indent}{cmd_ty}::{child_variant}(__g) => match __g.cmd {{\n"
        ));
        emit_group_match_arms(
            out,
            child,
            &child_q,
            &format!("{indent}    "),
            oauth,
            exchange,
        );
        out.push_str(&format!("{indent}}},\n"));
    }
    for op in &group.direct_ops {
        out.push_str(&render_op_match_arm(op, &cmd_ty, indent, oauth, exchange));
    }
}

fn render_op_variant(op: &Operation, indent: &str, exchange: Option<&TokenExchangeInfo>) -> String {
    let variant = pascal_case(&op.id);
    let summary = first_line(op.documentation.as_deref()).unwrap_or_default();
    let mut s = String::new();
    if !summary.is_empty() {
        s.push_str(&format!("{indent}/// {}\n", escape_doc(&summary)));
    }

    let exclude = exchange
        .filter(|ex| op_uses_placeholder(op, &ex.placeholder))
        .map(|ex| ex.placeholder.as_str());
    let fields = collect_fields(op, exclude);
    if fields.is_empty() {
        s.push_str(&format!("{indent}{variant},\n"));
    } else {
        s.push_str(&format!("{indent}{variant} {{\n"));
        for f in &fields {
            if let Some(doc) = &f.doc {
                s.push_str(&format!("{indent}    /// {}\n", escape_doc(doc)));
            }
            for attr in &f.attrs {
                s.push_str(&format!("{indent}    {attr}\n"));
            }
            s.push_str(&format!("{indent}    {}: {},\n", f.ident, f.ty));
        }
        s.push_str(&format!("{indent}}},\n"));
    }
    s
}

fn render_op_match_arm(
    op: &Operation,
    cmd_ty: &str,
    indent: &str,
    oauth: Option<&OauthInfo>,
    exchange: Option<&TokenExchangeInfo>,
) -> String {
    let variant = pascal_case(&op.id);
    let method_ident = snake_case(&op.id);

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

    // Fields the variant destructures — excludes the path param that
    // duplicates the global `--<placeholder>` flag.
    let destruct_fields = collect_fields(op, exclude);
    let destruct_pat = if destruct_fields.is_empty() {
        String::new()
    } else {
        format!(
            " {{ {} }}",
            destruct_fields
                .iter()
                .map(|f| f.ident.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    // Client method arg expressions — in declaration order. Path
    // params matching the placeholder are sourced from `__slug`;
    // everything else from the destructured field of the same name.
    // When schema-flag relaxation is in effect, fields that were
    // formerly `String` are emitted as `Option<String>` (clap allows
    // them empty when --body-schema / --response-schema is set);
    // unwrap with `.expect(...)` here since clap guarantees `Some` on
    // the actual-call branch.
    let relax_active = !relax_unless(op).is_empty();
    let mut call_args: Vec<String> = vec!["__bearer.as_deref()".into()];
    for p in &op.path_params {
        let ident = snake_case(&p.name);
        if exclude.is_some_and(|ph| snake_case(ph) == ident) {
            call_args.push("__slug.clone()".into());
        } else if relax_active {
            let kebab = kebab_case(&p.name);
            call_args.push(format!("{ident}.expect(\"<{kebab}> required\")"));
        } else {
            call_args.push(ident);
        }
    }
    for p in &op.query_params {
        let ident = snake_case(&p.name);
        if p.required && relax_active {
            let kebab = kebab_case(&p.name);
            call_args.push(format!("{ident}.expect(\"--{kebab} required\")"));
        } else {
            call_args.push(ident);
        }
    }
    for p in &op.header_params {
        let ident = snake_case(&p.name);
        if p.required && relax_active {
            let kebab = kebab_case(&p.name);
            call_args.push(format!("{ident}.expect(\"--{kebab} required\")"));
        } else {
            call_args.push(ident);
        }
    }
    for p in &op.cookie_params {
        let ident = snake_case(&p.name);
        if p.required && relax_active {
            let kebab = kebab_case(&p.name);
            call_args.push(format!("{ident}.expect(\"--{kebab} required\")"));
        } else {
            call_args.push(ident);
        }
    }
    if let Some(b) = &op.request_body {
        if b.required {
            // clap's `required_unless_present_any` guarantees `body`
            // is `Some` whenever we reach the API call branch.
            call_args.push("body.expect(\"--body required\")".into());
        } else {
            call_args.push("body".into());
        }
    }
    let call = format!("client.{method_ident}({}).await?", call_args.join(", "));

    let has_body = op.request_body.is_some();
    let has_resp_schema = op_has_response_content(op);
    let any_guard = has_body || has_resp_schema;
    let inner_indent = if any_guard {
        format!("{indent}        ")
    } else {
        format!("{indent}    ")
    };

    let pre_block = if needs_exchange {
        let ex = exchange.unwrap();
        let ph_snake = snake_case(&ex.placeholder);
        let ph_kebab = kebab_case(&ex.placeholder);
        format!(
            "let __slug: String = __resolved_{ph_snake}.clone().ok_or_else(|| \
                anyhow::anyhow!(\"--{ph_kebab} is required for this operation (or run `set-{ph_kebab} <slug>`)\"))?;\n{inner_indent}"
        )
    } else {
        String::new()
    };

    let bearer_block = if needs_exchange {
        let ex = exchange.unwrap();
        let aud_fmt = ex
            .audience_template
            .replace(&format!("{{{}}}", ex.placeholder), "{}");
        let res_let = match &ex.resource_template {
            Some(rt) => {
                let rt_fmt = rt.replace(&format!("{{{}}}", ex.placeholder), "{}");
                format!("Some(format!(\"{}\", __slug))", escape_rust_string(&rt_fmt))
            }
            None => "None".into(),
        };
        let scope_let = if ex.extra_scope.is_empty() {
            "None".into()
        } else {
            format!(
                "Some(\"{}\")",
                escape_rust_string(&ex.extra_scope.join(" "))
            )
        };
        format!(
            "if let Some(t) = cli.token.clone() {{ Some(t) }} else {{ \
                let __aud = format!(\"{aud_fmt}\", __slug); \
                let __res: Option<String> = {res_let}; \
                let __scope: Option<&str> = {scope_let}; \
                auth::audience_access_token(&cli.profile, &__aud, __res.as_deref(), __scope).await? \
            }}"
        )
    } else if oauth.is_some() {
        "if let Some(t) = cli.token.clone() { Some(t) } else { auth::access_token(&cli.profile).await? }".into()
    } else {
        "cli.token.clone()".into()
    };

    let const_pascal = screaming_snake(&op.id);
    let api_path =
        format!("{pre_block}let __bearer: Option<String> = {bearer_block};\n{inner_indent}{call}");

    let mut guards = String::new();
    if has_body {
        guards.push_str(&format!(
            "if body_schema {{\n{indent}        println!(\"{{}}\", BODY_SCHEMA_{const_pascal});\n{indent}        serde_json::Value::Null\n{indent}    }} else "
        ));
    }
    if has_resp_schema {
        guards.push_str(&format!(
            "if response_schema {{\n{indent}        println!(\"{{}}\", RESPONSE_SCHEMA_{const_pascal});\n{indent}        serde_json::Value::Null\n{indent}    }} else "
        ));
    }

    let arm_body = if any_guard {
        format!("{guards}{{\n{indent}        {api_path}\n{indent}    }}")
    } else {
        api_path
    };

    format!(
        "{indent}{cmd_ty}::{variant}{destruct_pat} => {{\n{indent}    {arm_body}\n{indent}}},\n",
    )
}

struct Field {
    ident: String,
    ty: String,
    doc: Option<String>,
    attrs: Vec<String>,
}

fn collect_fields(op: &Operation, exclude_path_param: Option<&str>) -> Vec<Field> {
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
        out.push(field_for_param(p, FieldKind::Positional, &relax));
    }
    for p in &op.query_params {
        out.push(field_for_param(p, FieldKind::Flag, &relax));
    }
    for p in &op.header_params {
        out.push(field_for_param(p, FieldKind::Flag, &relax));
    }
    for p in &op.cookie_params {
        out.push(field_for_param(p, FieldKind::Flag, &relax));
    }
    if let Some(body) = &op.request_body {
        out.push(field_for_body(body, &relax));
        out.push(field_for_body_schema());
    }
    if op_has_response_content(op) {
        out.push(field_for_response_schema());
    }
    out
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

fn unless_clause(relax: &[&str]) -> String {
    relax
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Copy, Clone)]
enum FieldKind {
    Positional,
    Flag,
}

fn field_for_param(p: &Parameter, kind: FieldKind, relax: &[&str]) -> Field {
    let ident = snake_case(&p.name);
    let kebab = kebab_case(&p.name);
    let unless = unless_clause(relax);
    let relax_active = !relax.is_empty();

    let (ty, attrs) = match (kind, p.required) {
        (FieldKind::Positional, _) => {
            if relax_active {
                (
                    "Option<String>".to_string(),
                    vec![format!("#[arg(required_unless_present_any = [{unless}])]")],
                )
            } else {
                ("String".to_string(), vec![])
            }
        }
        (FieldKind::Flag, true) => {
            if relax_active {
                (
                    "Option<String>".to_string(),
                    vec![format!(
                        "#[arg(long = \"{kebab}\", required_unless_present_any = [{unless}])]"
                    )],
                )
            } else {
                (
                    "String".to_string(),
                    vec![format!("#[arg(long = \"{kebab}\")]")],
                )
            }
        }
        (FieldKind::Flag, false) => (
            "Option<String>".to_string(),
            vec![format!("#[arg(long = \"{kebab}\")]")],
        ),
    };
    let doc = first_line(p.documentation.as_deref());
    Field {
        ident,
        ty,
        doc,
        attrs,
    }
}

fn field_for_body(body: &Body, relax: &[&str]) -> Field {
    let attrs = if body.required {
        let unless = unless_clause(relax);
        vec![format!(
            "#[arg(long = \"body\", required_unless_present_any = [{unless}])]"
        )]
    } else {
        vec!["#[arg(long = \"body\")]".into()]
    };
    Field {
        ident: "body".into(),
        ty: "Option<String>".into(),
        doc: Some(
            "Request body. Inline JSON, @file.json (read from file), or - (read from stdin). \
             Run with --body-schema to print the JSON Schema."
                .into(),
        ),
        attrs,
    }
}

fn field_for_body_schema() -> Field {
    Field {
        ident: "body_schema".into(),
        ty: "bool".into(),
        doc: Some("Print the JSON Schema for the request body and exit.".into()),
        attrs: vec!["#[arg(long = \"body-schema\")]".into()],
    }
}

fn field_for_response_schema() -> Field {
    Field {
        ident: "response_schema".into(),
        ty: "bool".into(),
        doc: Some(
            "Print the JSON Schemas for the response bodies, keyed by status code, and exit."
                .into(),
        ),
        attrs: vec!["#[arg(long = \"response-schema\")]".into()],
    }
}

fn op_has_response_content(op: &Operation) -> bool {
    op.responses.iter().any(|r| !r.content.is_empty())
}

fn build_long_about(
    title: &str,
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

fn emit_schema_consts(ir: &Ir) -> String {
    let mut out = String::new();
    for op in &ir.operations {
        let pascal = screaming_snake(&op.id);
        if let Some(body) = &op.request_body {
            if let Some(s) = schema::render_body_schema(&ir.types, &ir.values, body) {
                out.push_str(&format!(
                    "const BODY_SCHEMA_{pascal}: &str = {};\n",
                    json_string(&s),
                ));
            }
        }
        if op_has_response_content(op) {
            if let Some(s) = schema::render_response_schemas(&ir.types, &ir.values, &op.responses) {
                out.push_str(&format!(
                    "const RESPONSE_SCHEMA_{pascal}: &str = {};\n",
                    json_string(&s),
                ));
            }
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn group_doc(group: &TagGroup) -> Option<String> {
    let tag = group.tag?;
    if let Some(s) = &tag.summary {
        if !s.is_empty() {
            return Some(s.clone());
        }
    }
    first_line(tag.description.as_deref())
}

fn push_doc(out: &mut String, doc: Option<String>, indent: &str) {
    if let Some(d) = doc.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("{indent}/// {}\n", escape_doc(d)));
    }
}

fn qualified_pascal(prefix: &str, name: &str) -> String {
    format!("{prefix}{}", pascal_case(name))
}

// ---------------------------------------------------------------------------
// src/client.rs
// ---------------------------------------------------------------------------

fn emit_client_rs(ir: &Ir) -> String {
    let mut out = String::new();
    out.push_str("// Generated by openapi-forge / generator-rust-clap; do not edit by hand.\n#![allow(clippy::too_many_arguments, clippy::needless_borrow)]\n\nuse anyhow::{anyhow, Context, Result};\nuse serde_json::Value;\n\nuse crate::runtime::parse_body_arg;\n\npub struct ApiClient {\n    http: reqwest::Client,\n    base_url: String,\n}\n\nimpl ApiClient {\n    pub fn new(base_url: String) -> Result<Self> {\n        Ok(Self {\n            http: reqwest::Client::builder().build()?,\n            base_url: base_url.trim_end_matches('/').to_string(),\n        })\n    }\n\n    fn req(&self, token: Option<&str>, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {\n        let mut b = self.http.request(method, format!(\"{}{}\", self.base_url, path));\n        if let Some(t) = token { b = b.bearer_auth(t); }\n        b\n    }\n\n");

    for op in &ir.operations {
        out.push_str(&render_client_method(op));
    }

    out.push_str("}\n");
    out
}

fn render_client_method(op: &Operation) -> String {
    let method_ident = snake_case(&op.id);
    let http_method = method_path(&op.method);
    let path_template = &op.path_template;

    let mut sig_args: Vec<String> = vec!["__bearer: Option<&str>".into()];
    for p in &op.path_params {
        sig_args.push(format!("{}: String", snake_case(&p.name)));
    }
    for p in &op.query_params {
        let ident = snake_case(&p.name);
        let ty = if p.required {
            "String"
        } else {
            "Option<String>"
        };
        sig_args.push(format!("{ident}: {ty}"));
    }
    for p in &op.header_params {
        let ident = snake_case(&p.name);
        let ty = if p.required {
            "String"
        } else {
            "Option<String>"
        };
        sig_args.push(format!("{ident}: {ty}"));
    }
    for p in &op.cookie_params {
        let ident = snake_case(&p.name);
        let ty = if p.required {
            "String"
        } else {
            "Option<String>"
        };
        sig_args.push(format!("{ident}: {ty}"));
    }
    if let Some(body) = &op.request_body {
        let ty = if body.required {
            "String"
        } else {
            "Option<String>"
        };
        sig_args.push(format!("body: {ty}"));
    }
    let sig = sig_args.join(", ");

    let mut body = String::new();

    let (path_fmt, path_args) = render_path_interpolation(path_template, &op.path_params);
    if path_args.is_empty() {
        body.push_str(&format!(
            "        let __path = String::from(\"{}\");\n",
            escape_rust_string(&path_fmt)
        ));
    } else {
        body.push_str(&format!(
            "        let __path = format!(\"{}\", {});\n",
            escape_rust_string(&path_fmt),
            path_args.join(", "),
        ));
    }
    body.push_str(&format!(
        "        let mut __r = self.req(__bearer, {http_method}, &__path);\n"
    ));

    for p in &op.query_params {
        let ident = snake_case(&p.name);
        let raw = &p.name;
        if p.required {
            body.push_str(&format!(
                "        __r = __r.query(&[(\"{raw}\", &{ident})]);\n"
            ));
        } else {
            body.push_str(&format!(
                "        if let Some(v) = &{ident} {{ __r = __r.query(&[(\"{raw}\", v)]); }}\n"
            ));
        }
    }
    for p in &op.header_params {
        let ident = snake_case(&p.name);
        let raw = &p.name;
        if p.required {
            body.push_str(&format!("        __r = __r.header(\"{raw}\", &{ident});\n"));
        } else {
            body.push_str(&format!(
                "        if let Some(v) = &{ident} {{ __r = __r.header(\"{raw}\", v); }}\n"
            ));
        }
    }
    if !op.cookie_params.is_empty() {
        body.push_str("        let mut __cookies: Vec<String> = Vec::new();\n");
        for p in &op.cookie_params {
            let ident = snake_case(&p.name);
            let raw = &p.name;
            if p.required {
                body.push_str(&format!(
                    "        __cookies.push(format!(\"{raw}={{}}\", urlencoding::encode(&{ident})));\n"
                ));
            } else {
                body.push_str(&format!(
                    "        if let Some(v) = &{ident} {{ __cookies.push(format!(\"{raw}={{}}\", urlencoding::encode(v))); }}\n"
                ));
            }
        }
        body.push_str("        if !__cookies.is_empty() { __r = __r.header(\"Cookie\", __cookies.join(\"; \")); }\n");
    }
    if let Some(b) = &op.request_body {
        if b.required {
            body.push_str(
                "        let __body_value = parse_body_arg(&body).context(\"--body\")?;\n",
            );
            body.push_str("        __r = __r.json(&__body_value);\n");
        } else {
            body.push_str("        if let Some(s) = &body {\n            let v = parse_body_arg(s).context(\"--body\")?;\n            __r = __r.json(&v);\n        }\n");
        }
    }

    body.push_str("        let __resp = __r.send().await.context(\"sending request\")?;\n");
    body.push_str("        let __status = __resp.status();\n");
    body.push_str("        let __text = __resp.text().await.context(\"reading response\")?;\n");
    body.push_str("        let __json: Value = if __text.is_empty() { Value::Null } else { serde_json::from_str(&__text).unwrap_or(Value::String(__text.clone())) };\n");
    body.push_str("        if !__status.is_success() {\n            return Err(anyhow!(\"HTTP {}: {}\", __status, __json));\n        }\n");
    body.push_str("        Ok(__json)\n");

    let mut s = String::new();
    if let Some(doc) = first_line(op.documentation.as_deref()) {
        s.push_str(&format!("    /// {}\n", escape_doc(&doc)));
    }
    s.push_str(&format!(
        "    pub async fn {method_ident}(&self, {sig}) -> Result<Value> {{\n"
    ));
    s.push_str(&body);
    s.push_str("    }\n\n");
    s
}

fn method_path(m: &HttpMethod) -> &'static str {
    match m {
        HttpMethod::Get => "reqwest::Method::GET",
        HttpMethod::Post => "reqwest::Method::POST",
        HttpMethod::Put => "reqwest::Method::PUT",
        HttpMethod::Delete => "reqwest::Method::DELETE",
        HttpMethod::Patch => "reqwest::Method::PATCH",
        HttpMethod::Options => "reqwest::Method::OPTIONS",
        HttpMethod::Head => "reqwest::Method::HEAD",
        HttpMethod::Trace => "reqwest::Method::TRACE",
        HttpMethod::Other(_) => "reqwest::Method::GET",
    }
}

fn render_path_interpolation(template: &str, path_params: &[Parameter]) -> (String, Vec<String>) {
    let mut out_template = String::with_capacity(template.len());
    let mut args = Vec::new();
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
            let ident = snake_case(&name);
            if path_params.iter().any(|p| snake_case(&p.name) == ident) {
                out_template.push_str("{}");
                args.push(format!("urlencoding::encode(&{ident})"));
            } else {
                out_template.push('{');
                out_template.push_str(&name);
                out_template.push('}');
            }
        } else if c == '}' {
            out_template.push('}');
        } else {
            out_template.push(c);
        }
    }
    (out_template, args)
}

// ---------------------------------------------------------------------------
// src/runtime.rs
// ---------------------------------------------------------------------------

fn emit_runtime_rs() -> String {
    r#"// Generated by openapi-forge / generator-rust-clap; do not edit by hand.
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;

pub fn parse_body_arg(s: &str) -> Result<Value> {
    if s == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).context("reading stdin")?;
        return serde_json::from_str(&buf).context("parsing stdin as JSON");
    }
    if let Some(path) = s.strip_prefix('@') {
        let buf = std::fs::read_to_string(path)
            .with_context(|| format!("reading file {path}"))?;
        return serde_json::from_str(&buf)
            .with_context(|| format!("parsing {path} as JSON"));
    }
    serde_json::from_str(s).context("parsing inline JSON")
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub enum OutputMode {
    Json,
    Compact,
}

pub fn print_output(v: &Value, mode: OutputMode) -> Result<()> {
    if v.is_null() {
        return Ok(());
    }
    let s = match mode {
        OutputMode::Json => serde_json::to_string_pretty(v)?,
        OutputMode::Compact => serde_json::to_string(v)?,
    };
    println!("{s}");
    Ok(())
}
"#
    .into()
}

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

const AUTH_RS_PROLOGUE: &str = r##"// Generated by openapi-forge / generator-rust-clap; do not edit by hand.
//! OAuth 2.0 PKCE authorization-code flow + token persistence + profiles.
//!
//! Profiles bundle deployment-specific settings (URLs, client id/secret)
//! under named blocks in `<config_dir>/config.toml`. The `default`
//! profile is auto-populated from spec values on first run.

#![allow(dead_code)]

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use rand::RngCore;
use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BIN_NAME: &str = "__BIN_NAME__";
const CLIENT_ID_DEFAULT: &str = "__CLIENT_ID__";
const AUTH_URL_DEFAULT: &str = "__AUTH_URL__";
const TOKEN_URL_DEFAULT: &str = "__TOKEN_URL__";
const BASE_URL_DEFAULT: &str = "__BASE_URL_DEFAULT__";
const BASE_URL_ENV: &str = "__PREFIX___BASE_URL";
const AUTH_URL_ENV: &str = "__PREFIX___AUTH_URL";
const TOKEN_URL_ENV: &str = "__PREFIX___TOKEN_URL";
const CLIENT_ID_ENV: &str = "__PREFIX___CLIENT_ID";
const REDIRECT_PORT: u16 = __REDIRECT_PORT__;
const SCOPES: &[&str] = &[__SCOPES__];
const CLIENT_SECRET_ENV: &str = "__CLIENT_SECRET_ENV__";
const REFRESH_SKEW_SECS: u64 = 30;
pub const DEFAULT_PROFILE: &str = "default";

// =====================================================================
// Profile config (TOML at <config_dir>/config.toml)
// =====================================================================

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

fn config_toml_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn read_config() -> Result<ConfigFile> {
    let p = config_toml_path()?;
    if !p.exists() { return Ok(ConfigFile::default()); }
    let s = std::fs::read_to_string(&p).context("reading config.toml")?;
    toml::from_str(&s).context("parsing config.toml")
}

pub fn write_config(cfg: &ConfigFile) -> Result<()> {
    let p = config_toml_path()?;
    let s = toml::to_string_pretty(cfg).context("serializing config.toml")?;
    std::fs::write(&p, s).context("writing config.toml")?;
    set_user_only_perms(&p)?;
    Ok(())
}

/// Writes a `[profiles.default]` block populated from spec values when
/// `config.toml` does not yet exist. Idempotent — returns immediately
/// if the file already exists.
pub fn bootstrap_default_profile_if_missing() -> Result<()> {
    let p = config_toml_path()?;
    if p.exists() { return Ok(()); }
    let mut cfg = ConfigFile::default();
    let mut prof = Profile::default();
    if !BASE_URL_DEFAULT.is_empty() { prof.base_url = Some(BASE_URL_DEFAULT.into()); }
    if !AUTH_URL_DEFAULT.is_empty() { prof.auth_url = Some(AUTH_URL_DEFAULT.into()); }
    if !TOKEN_URL_DEFAULT.is_empty() { prof.token_url = Some(TOKEN_URL_DEFAULT.into()); }
    if !CLIENT_ID_DEFAULT.is_empty() { prof.client_id = Some(CLIENT_ID_DEFAULT.into()); }
    cfg.profiles.insert(DEFAULT_PROFILE.into(), prof);
    write_config(&cfg)?;
    Ok(())
}

/// One-shot migration: move legacy `<config_dir>/auth.json` and
/// `<config_dir>/*.json` to `<config_dir>/profiles/default/*`.
/// Idempotent — safe to call on every invocation.
pub fn migrate_legacy() -> Result<()> {
    let dir = config_dir()?;
    let target_dir = profile_dir(DEFAULT_PROFILE)?;

    let legacy = dir.join("auth.json");
    if legacy.exists() {
        let target = target_dir.join("auth.json");
        let _ = std::fs::rename(&legacy, &target);
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file() && path.extension().is_some_and(|x| x == "json") {
                if let Some(name) = path.file_name() {
                    let dest = target_dir.join(name);
                    if !dest.exists() {
                        let _ = std::fs::rename(&path, &dest);
                    }
                }
            }
        }
    }
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

// =====================================================================
// Resolution chain (override → env → profile → spec default)
// =====================================================================

pub fn resolve_base_url(profile: &str, override_value: Option<&str>) -> Result<String> {
    if let Some(v) = override_value.filter(|s| !s.is_empty()) { return Ok(v.into()); }
    let cfg = read_config()?;
    if let Some(p) = cfg.profiles.get(profile) {
        if let Some(v) = &p.base_url { return Ok(v.clone()); }
    }
    Ok(BASE_URL_DEFAULT.into())
}

pub fn resolve_auth_url(profile: &str) -> Result<String> {
    if let Ok(v) = std::env::var(AUTH_URL_ENV) { if !v.is_empty() { return Ok(v); } }
    let cfg = read_config()?;
    if let Some(p) = cfg.profiles.get(profile) {
        if let Some(v) = &p.auth_url { return Ok(v.clone()); }
    }
    Ok(AUTH_URL_DEFAULT.into())
}

pub fn resolve_token_url(profile: &str) -> Result<String> {
    if let Ok(v) = std::env::var(TOKEN_URL_ENV) { if !v.is_empty() { return Ok(v); } }
    let cfg = read_config()?;
    if let Some(p) = cfg.profiles.get(profile) {
        if let Some(v) = &p.token_url { return Ok(v.clone()); }
    }
    Ok(TOKEN_URL_DEFAULT.into())
}

pub fn resolve_client_id(profile: &str) -> Result<String> {
    if let Ok(v) = std::env::var(CLIENT_ID_ENV) { if !v.is_empty() { return Ok(v); } }
    let cfg = read_config()?;
    if let Some(p) = cfg.profiles.get(profile) {
        if let Some(v) = &p.client_id { return Ok(v.clone()); }
    }
    Ok(CLIENT_ID_DEFAULT.into())
}

/// Resolves the client secret per profile. Chain:
///   profile literal → env var named by CLIENT_SECRET_ENV → None.
pub fn resolve_client_secret_value(profile: &str) -> Result<Option<String>> {
    let cfg = read_config()?;
    if let Some(p) = cfg.profiles.get(profile) {
        if let Some(v) = &p.client_secret {
            if !v.is_empty() { return Ok(Some(v.clone())); }
        }
    }
    if !CLIENT_SECRET_ENV.is_empty() {
        if let Ok(v) = std::env::var(CLIENT_SECRET_ENV) {
            if !v.is_empty() { return Ok(Some(v)); }
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub expires_at: Option<u64>,
    pub obtained_at: u64,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn random_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn challenge_from(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn config_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", BIN_NAME)
        .context("computing config dir")?;
    let dir = dirs.config_dir().to_path_buf();
    std::fs::create_dir_all(&dir).context("creating config dir")?;
    Ok(dir)
}

pub fn profile_dir(profile: &str) -> Result<PathBuf> {
    let dir = config_dir()?.join("profiles").join(sanitize(profile));
    std::fs::create_dir_all(&dir).context("creating profile dir")?;
    Ok(dir)
}

fn token_path(profile: &str) -> Result<PathBuf> {
    Ok(profile_dir(profile)?.join("auth.json"))
}

fn persisted_path(profile: &str, name: &str) -> Result<PathBuf> {
    Ok(profile_dir(profile)?.join(format!("{}.json", sanitize(name))))
}

pub fn read_persisted(profile: &str, name: &str) -> Result<Option<String>> {
    let p = persisted_path(profile, name)?;
    if !p.exists() { return Ok(None); }
    let s = std::fs::read_to_string(&p).context("reading persisted value")?;
    let v: serde_json::Value = serde_json::from_str(&s).context("parsing persisted value")?;
    Ok(v.get("value").and_then(|x| x.as_str()).map(|x| x.to_string()))
}

pub fn write_persisted(profile: &str, name: &str, value: &str) -> Result<()> {
    let p = persisted_path(profile, name)?;
    let json = serde_json::json!({ "value": value });
    let s = serde_json::to_string_pretty(&json)?;
    std::fs::write(&p, s).context("writing persisted value")?;
    set_user_only_perms(&p)?;
    Ok(())
}

pub fn delete_persisted(profile: &str, name: &str) -> Result<bool> {
    let p = persisted_path(profile, name)?;
    if p.exists() {
        std::fs::remove_file(&p).context("removing persisted value")?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn set_user_only_perms(_p: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(_p)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(_p, perm)?;
    }
    Ok(())
}

pub fn read_stored(profile: &str) -> Result<Option<StoredToken>> {
    let p = token_path(profile)?;
    if !p.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&p).context("reading auth.json")?;
    Ok(Some(serde_json::from_str(&s).context("parsing auth.json")?))
}

fn write_stored(profile: &str, t: &StoredToken) -> Result<()> {
    let p = token_path(profile)?;
    let s = serde_json::to_string_pretty(t).context("serializing token")?;
    std::fs::write(&p, &s).context("writing auth.json")?;
    set_user_only_perms(&p)?;
    Ok(())
}

pub async fn logout(profile: &str) -> Result<bool> {
    let p = token_path(profile)?;
    if p.exists() {
        std::fs::remove_file(&p).context("removing auth.json")?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Attaches `Authorization: Basic` to a token-endpoint request when a
/// client secret is resolvable for `profile`. No-op for public clients.
fn maybe_client_auth(b: RequestBuilder, profile: &str) -> RequestBuilder {
    let secret = match resolve_client_secret_value(profile) {
        Ok(Some(v)) => v,
        _ => return b,
    };
    let client_id = resolve_client_id(profile).unwrap_or_else(|_| CLIENT_ID_DEFAULT.into());
    let pair = format!("{}:{}", client_id, secret);
    let header = format!("Basic {}", B64_STANDARD.encode(pair));
    b.header(reqwest::header::AUTHORIZATION, header)
}

pub async fn login(profile: &str) -> Result<StoredToken> {
    let verifier = random_verifier();
    let challenge = challenge_from(&verifier);
    let state = random_state();
    let redirect_uri = format!("http://127.0.0.1:{}/callback", REDIRECT_PORT);
    let client_id = resolve_client_id(profile)?;

    let scope_joined = SCOPES.join(" ");
    let mut params: Vec<(&str, String)> = vec![
        ("client_id", client_id.clone()),
        ("response_type", "code".into()),
        ("redirect_uri", redirect_uri.clone()),
        ("code_challenge", challenge.clone()),
        ("code_challenge_method", "S256".into()),
        ("state", state.clone()),
    ];
    if !scope_joined.is_empty() {
        params.push(("scope", scope_joined.clone()));
    }
    let qs: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect();
    let auth_base = resolve_auth_url(profile)?;
    let join = if auth_base.contains('?') { "&" } else { "?" };
    let auth_url = format!("{}{}{}", auth_base, join, qs.join("&"));

    eprintln!("Opening browser to authorize: {}", auth_url);
    let _ = webbrowser::open(&auth_url);
    eprintln!("(if your browser doesn't open, paste the URL above)");

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", REDIRECT_PORT))
        .await
        .with_context(|| format!("binding 127.0.0.1:{} for OAuth callback", REDIRECT_PORT))?;
    let (mut socket, _) = listener.accept().await.context("accepting OAuth callback")?;
    let mut buf = vec![0u8; 8192];
    let n = socket.read(&mut buf).await.context("reading callback request")?;
    let req = std::str::from_utf8(&buf[..n]).context("UTF-8 in callback request")?;
    let first = req.lines().next().unwrap_or("");
    let path_qs = first.split_whitespace().nth(1).unwrap_or("");
    let qs_part = path_qs.split('?').nth(1).unwrap_or("");

    let mut got_code: Option<String> = None;
    let mut got_state: Option<String> = None;
    let mut got_error: Option<String> = None;
    for kv in qs_part.split('&') {
        let mut sp = kv.splitn(2, '=');
        let k = sp.next().unwrap_or("");
        let v_raw = sp.next().unwrap_or("");
        let v = urlencoding::decode(v_raw)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| v_raw.to_string());
        match k {
            "code" => got_code = Some(v),
            "state" => got_state = Some(v),
            "error" => got_error = Some(v),
            _ => {}
        }
    }

    let html = b"<!doctype html><html><body style=\"font-family:sans-serif\"><h2>Login complete</h2><p>You can close this window.</p></body></html>";
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        html.len(),
    );
    socket.write_all(head.as_bytes()).await.ok();
    socket.write_all(html).await.ok();
    socket.shutdown().await.ok();

    if let Some(err) = got_error {
        bail!("OAuth provider returned error: {err}");
    }
    let code = got_code.ok_or_else(|| anyhow!("authorization code missing from callback"))?;
    let st = got_state.ok_or_else(|| anyhow!("state missing from callback"))?;
    if st != state {
        bail!("state mismatch (CSRF check failed)");
    }

    let http = reqwest::Client::new();
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".into()),
        ("code", code.clone()),
        ("redirect_uri", redirect_uri.clone()),
        ("code_verifier", verifier.clone()),
    ];
    if resolve_client_secret_value(profile)?.is_none() {
        form.push(("client_id", client_id.clone()));
    }
    let req = http.post(resolve_token_url(profile)?).form(&form);
    let req = maybe_client_auth(req, profile);
    let resp = req.send().await.context("posting to token endpoint")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("token endpoint {status}: {body}");
    }
    let tr: TokenResponse = resp.json().await.context("parsing token response")?;

    let now = now_secs();
    let stored = StoredToken {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        token_type: tr.token_type.unwrap_or_else(|| "bearer".into()),
        expires_at: tr.expires_in.map(|s| now + s),
        obtained_at: now,
        scope: tr.scope,
    };
    write_stored(profile, &stored)?;
    Ok(stored)
}

pub async fn access_token(profile: &str) -> Result<Option<String>> {
    let Some(t) = read_stored(profile)? else {
        return Ok(None);
    };
    let now = now_secs();
    let needs_refresh = t
        .expires_at
        .map(|e| e.saturating_sub(REFRESH_SKEW_SECS) <= now)
        .unwrap_or(false);
    if !needs_refresh {
        return Ok(Some(t.access_token));
    }
    let Some(rt) = t.refresh_token.as_deref() else {
        return Ok(Some(t.access_token));
    };

    let http = reqwest::Client::new();
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".into()),
        ("refresh_token", rt.to_string()),
    ];
    let client_id = resolve_client_id(profile)?;
    if resolve_client_secret_value(profile)?.is_none() {
        form.push(("client_id", client_id));
    }
    let req = http.post(resolve_token_url(profile)?).form(&form);
    let req = maybe_client_auth(req, profile);
    let resp = req.send().await.context("refreshing OAuth token")?;
    if !resp.status().is_success() {
        let _ = std::fs::remove_file(token_path(profile)?);
        return Ok(None);
    }
    let tr: TokenResponse = resp.json().await.context("parsing refresh response")?;
    let now2 = now_secs();
    let stored = StoredToken {
        access_token: tr.access_token.clone(),
        refresh_token: tr.refresh_token.or(t.refresh_token),
        token_type: tr.token_type.unwrap_or(t.token_type),
        expires_at: tr.expires_in.map(|s| now2 + s),
        obtained_at: now2,
        scope: tr.scope.or(t.scope),
    };
    write_stored(profile, &stored)?;
    Ok(Some(stored.access_token))
}

pub fn list_profile_names() -> Vec<String> {
    read_config()
        .map(|cfg| cfg.profiles.keys().cloned().collect())
        .unwrap_or_default()
}

/// Interactive `<bin> configure` flow. Prompts for the profile's
/// fields with sensible defaults (current value or spec default),
/// writes back to config.toml, and returns whether the operator
/// asked to log in immediately.
pub fn interactive_configure(profile: &str) -> Result<bool> {
    use dialoguer::theme::ColorfulTheme;
    use dialoguer::{Confirm, Input, Password};

    let theme = ColorfulTheme::default();
    eprintln!("Configuring profile: {}", profile);

    let mut cfg = read_config()?;
    let current = cfg.profiles.get(profile).cloned().unwrap_or_default();

    let base_url: String = Input::with_theme(&theme)
        .with_prompt("Base URL")
        .default(current.base_url.clone().unwrap_or_else(|| BASE_URL_DEFAULT.into()))
        .interact_text()?;
    let auth_url: String = Input::with_theme(&theme)
        .with_prompt("Authorization URL")
        .default(current.auth_url.clone().unwrap_or_else(|| AUTH_URL_DEFAULT.into()))
        .interact_text()?;
    let token_url: String = Input::with_theme(&theme)
        .with_prompt("Token URL")
        .default(current.token_url.clone().unwrap_or_else(|| TOKEN_URL_DEFAULT.into()))
        .interact_text()?;
    let client_id: String = Input::with_theme(&theme)
        .with_prompt("Client ID")
        .default(current.client_id.clone().unwrap_or_else(|| CLIENT_ID_DEFAULT.into()))
        .interact_text()?;

    let store_secret = Confirm::with_theme(&theme)
        .with_prompt(format!(
            "Store the client secret in config.toml? (No → use the {} env var instead)",
            if CLIENT_SECRET_ENV.is_empty() { "PREFIX_CLIENT_SECRET" } else { CLIENT_SECRET_ENV }
        ))
        .default(current.client_secret.is_some())
        .interact()?;
    let client_secret = if store_secret {
        let s: String = Password::with_theme(&theme)
            .with_prompt("Client secret")
            .interact()?;
        if s.is_empty() { None } else { Some(s) }
    } else {
        if !CLIENT_SECRET_ENV.is_empty() {
            eprintln!("(remember to `export {}=...` in your shell)", CLIENT_SECRET_ENV);
        }
        None
    };

    cfg.profiles.insert(
        profile.to_string(),
        Profile {
            base_url: Some(base_url),
            auth_url: Some(auth_url),
            token_url: Some(token_url),
            client_id: Some(client_id),
            client_secret,
        },
    );
    write_config(&cfg)?;
    eprintln!("Wrote profile '{}' to config.toml.", profile);

    let should_login = Confirm::with_theme(&theme)
        .with_prompt("Run login now?")
        .default(true)
        .interact()?;
    Ok(should_login)
}

/// Non-interactive `<bin> configure --non-interactive` flow. Merges
/// the supplied fields into the named profile, leaving fields whose
/// argument is `None` untouched. Used by installer scripts and other
/// automation. Mirrors `interactive_configure` write semantics.
pub fn write_profile_fields(
    profile: &str,
    base_url: Option<String>,
    auth_url: Option<String>,
    token_url: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<()> {
    let mut cfg = read_config()?;
    let entry = cfg.profiles.entry(profile.to_string()).or_default();
    if base_url.is_some() { entry.base_url = base_url; }
    if auth_url.is_some() { entry.auth_url = auth_url; }
    if token_url.is_some() { entry.token_url = token_url; }
    if client_id.is_some() { entry.client_id = client_id; }
    if client_secret.is_some() { entry.client_secret = client_secret; }
    write_config(&cfg)?;
    eprintln!("Wrote profile '{}' to config.toml.", profile);
    Ok(())
}

pub fn show_profile(profile: &str) -> Result<()> {
    let cfg = read_config()?;
    let Some(p) = cfg.profiles.get(profile) else {
        eprintln!("profile '{}' not found", profile);
        std::process::exit(2);
    };
    let mut redacted = p.clone();
    if redacted.client_secret.as_deref().is_some_and(|s| !s.is_empty()) {
        redacted.client_secret = Some("***".into());
    }
    let s = toml::to_string_pretty(&redacted).unwrap_or_default();
    println!("[profiles.{}]", profile);
    print!("{}", s);
    Ok(())
}

pub fn remove_profile(profile: &str) -> Result<bool> {
    let mut cfg = read_config()?;
    let removed = cfg.profiles.remove(profile).is_some();
    if removed {
        write_config(&cfg)?;
        let dir = profile_dir(profile)?;
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    Ok(removed)
}
"##;

const AUTH_RS_EXCHANGE_TAIL: &str = r##"

// ---------------------------------------------------------------------------
// RFC 8693 standard token exchange — tenant / per-audience access tokens.
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use tokio::sync::Mutex;

static EXCHANGE_CACHE: tokio::sync::OnceCell<Mutex<HashMap<String, StoredToken>>> =
    tokio::sync::OnceCell::const_new();

async fn cache() -> &'static Mutex<HashMap<String, StoredToken>> {
    EXCHANGE_CACHE
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await
}

/// Returns a Bearer scoped to `audience` for the given `profile`.
/// Performs RFC 8693 standard token exchange against the token URL on
/// first use per (profile, audience) and caches the result in-process.
/// Refreshes lazily on a 30s skew.
pub async fn audience_access_token(
    profile: &str,
    audience: &str,
    resource: Option<&str>,
    extra_scope: Option<&str>,
) -> Result<Option<String>> {
    let cache_key = format!("{profile}\x00{audience}");
    {
        let map = cache().await.lock().await;
        if let Some(tok) = map.get(&cache_key) {
            let now = now_secs();
            let stale = tok.expires_at.map(|e| e.saturating_sub(REFRESH_SKEW_SECS) <= now).unwrap_or(false);
            if !stale {
                return Ok(Some(tok.access_token.clone()));
            }
        }
    }

    let Some(subject) = access_token(profile).await? else {
        return Ok(None);
    };

    let http = reqwest::Client::new();
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange".into()),
        ("subject_token", subject.clone()),
        ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token".into()),
        ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token".into()),
        ("audience", audience.to_string()),
    ];
    if let Some(r) = resource {
        form.push(("resource", r.to_string()));
    }
    if let Some(sc) = extra_scope.filter(|s| !s.is_empty()) {
        form.push(("scope", sc.to_string()));
    }
    if resolve_client_secret_value(profile)?.is_none() {
        form.push(("client_id", resolve_client_id(profile)?));
    }
    let req = http.post(resolve_token_url(profile)?).form(&form);
    let req = maybe_client_auth(req, profile);
    let resp = req.send().await.context("posting RFC 8693 token exchange")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "token exchange {status}: {body}\n\
             (audience={audience}; if the IdP requires client authentication, \
             set the `{}` env var to the client secret OR add `client_secret` \
             to the `[profiles.{}]` block of `<config_dir>/config.toml`)",
            CLIENT_SECRET_ENV,
            profile,
        );
    }
    let tr: TokenResponse = resp.json().await.context("parsing exchange response")?;
    let now = now_secs();
    let stored = StoredToken {
        access_token: tr.access_token.clone(),
        refresh_token: tr.refresh_token,
        token_type: tr.token_type.unwrap_or_else(|| "bearer".into()),
        expires_at: tr.expires_in.map(|s| now + s),
        obtained_at: now,
        scope: tr.scope,
    };
    let bearer = stored.access_token.clone();
    cache().await.lock().await.insert(cache_key, stored);
    Ok(Some(bearer))
}
"##;

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

fn first_line(s: Option<&str>) -> Option<String> {
    s.and_then(|s| s.lines().next())
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn escape_rust_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_doc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\r', "")
}

fn json_string(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
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

    #[test]
    fn configure_variant_exposes_non_interactive_flags() {
        // Empty tag tree is enough to drive emit_root_enum into the oauth branch.
        let tree = TagTree { roots: vec![] };
        let mut out = String::new();
        emit_root_enum(
            &mut out, &tree, /*oauth_active*/ true, None, None, None,
        );

        // Struct-form Configure with each scriptable field + the bypass flag.
        for needle in [
            "Configure {",
            "base_url: Option<String>",
            "auth_url: Option<String>",
            "token_url: Option<String>",
            "client_id: Option<String>",
            "client_secret: Option<String>",
            "non_interactive: bool",
        ] {
            assert!(
                out.contains(needle),
                "emitted Cmd enum missing `{needle}`:\n{out}"
            );
        }
    }
}
