use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default = "default_true")]
    pub include_schemas: bool,
    #[serde(default)]
    pub base_url: Option<String>,
    /// When `true` (default), each operation page renders an
    /// in-browser request builder that sends a real `fetch()` to the
    /// currently picked server. Disable for read-only static docs or
    /// when CORS makes the runtime call useless.
    #[serde(default = "default_true")]
    pub enable_try_it: bool,
    /// Per-scheme OAuth 2.0 client configuration (PKCE / Authorization
    /// Code, optionally with client_secret + RFC 8693 token exchange).
    /// The map is keyed by `securitySchemes.<id>`; only schemes declared
    /// in the spec are honoured.
    #[serde(default)]
    pub oauth: BTreeMap<String, OAuthClientConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OAuthClientConfig {
    /// OAuth client_id registered with the IdP.
    pub client_id: String,
    /// Optional client_secret. Embedded in the generated site — only
    /// use for clients the IdP treats as confidential AND that you're
    /// OK exposing publicly (typical when the docs site is internal
    /// or token-exchange-only). Required when the IdP demands client
    /// auth for token-exchange.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Default scopes to request at login. When empty, the security
    /// page's scope checkboxes default to the spec-declared scopes.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Pre-registered redirect URI. Defaults to
    /// `<page-origin>/<site-root>/auth/callback.html` computed in the
    /// browser. Set this when you're hosting the docs behind a known
    /// canonical origin and have registered just one redirect URI
    /// with the IdP.
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: None,
            theme: Theme::default(),
            include_schemas: true,
            base_url: None,
            enable_try_it: true,
            oauth: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    Auto,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::Auto => "auto",
        }
    }
}

fn default_true() -> bool {
    true
}
