use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    pub output_root: Option<String>,
    pub missing_extension_policy: Option<MissingExtensionPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissingExtensionPolicy {
    /// Fail the run with a single diagnostic listing every un-annotated
    /// user-facing type. Strictest mode; matches the original design where
    /// the spec is expected to be fully annotated.
    Error,
    /// Emit a warning per un-annotated type and skip generating it.
    /// Designed for incremental migration of a large spec.
    Warn,
}

impl Default for MissingExtensionPolicy {
    fn default() -> Self {
        MissingExtensionPolicy::Error
    }
}

impl Config {
    pub fn output_root(&self) -> &str {
        self.output_root.as_deref().unwrap_or(".")
    }
    pub fn missing_extension_policy(&self) -> MissingExtensionPolicy {
        self.missing_extension_policy.unwrap_or_default()
    }
}
