use serde::Deserialize;

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: None,
            theme: Theme::default(),
            include_schemas: true,
            base_url: None,
            enable_try_it: true,
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
