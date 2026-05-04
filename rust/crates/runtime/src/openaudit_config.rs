//! `~/.openaudit/config.toml` loader for `OpenAudit` role -> provider mapping.
//!
//! The `OpenAudit` MVP keeps provider/role wiring out of code by reading a
//! small TOML file from the user's home (or `OPENAUDIT_HOME`) directory. This
//! module is a pure parser plus a `resolve_role` lookup; it does not touch the
//! filesystem outside `load_default` / `load_from`.
//!
//! Example file shape (see `docs/OPENAUDIT.md`):
//!
//! ```toml
//! [providers.moonshot]
//! model = "kimi-2.6"
//! api_key_env = "MOONSHOT_API_KEY"
//! base_url = "https://api.moonshot.ai/v1"
//!
//! [providers.anthropic]
//! model = "claude-opus-4-7"
//! api_key_env = "ANTHROPIC_API_KEY"
//!
//! [roles]
//! auditor  = "moonshot"
//! reviewer = "anthropic"
//! planner  = "moonshot"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Top-level on-disk shape of `~/.openaudit/config.toml`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct OpenAuditConfig {
    /// `[providers.<name>]` blocks keyed by provider name.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
    /// `[roles]` block mapping role names to provider names.
    #[serde(default)]
    pub roles: RoleAssignments,
}

/// A single `[providers.<name>]` block.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderEntry {
    /// Canonical model identifier passed to the provider client (required).
    pub model: String,
    /// Optional override for the env var holding the provider API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Optional override for the provider base URL (e.g. proxy / staging).
    #[serde(default)]
    pub base_url: Option<String>,
}

/// `[roles]` block. Each field is the provider name assigned to that role,
/// or `None` when the role is unset in the file.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RoleAssignments {
    #[serde(default)]
    pub auditor: Option<String>,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub planner: Option<String>,
}

/// Errors returned by the loader. Kept narrow on purpose: the consumer
/// (Task 1.6 `--dry-run`) treats most failures as "fall back to defaults".
#[derive(Debug)]
pub enum ConfigError {
    /// `default_config_path` could not be derived (no `OPENAUDIT_HOME` and no
    /// `HOME`), or the resolved path does not exist on disk.
    NotFound(PathBuf),
    /// Filesystem error while reading the config file.
    Io(std::io::Error),
    /// TOML deserialization error.
    Parse(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(path) => {
                write!(f, "openaudit config not found at {}", path.display())
            }
            Self::Io(err) => write!(f, "openaudit config io error: {err}"),
            Self::Parse(err) => write!(f, "openaudit config parse error: {err}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound(_) => None,
            Self::Io(err) => Some(err),
            Self::Parse(err) => Some(err),
        }
    }
}

impl OpenAuditConfig {
    /// Load from the default path resolved by [`default_config_path`].
    ///
    /// Returns [`ConfigError::NotFound`] if neither `OPENAUDIT_HOME` nor
    /// `HOME` are set, or if the resolved file does not exist.
    pub fn load_default() -> Result<Self, ConfigError> {
        let path = default_config_path().ok_or_else(|| ConfigError::NotFound(PathBuf::new()))?;
        if !path.exists() {
            return Err(ConfigError::NotFound(path));
        }
        Self::load_from(&path)
    }

    /// Load and parse the TOML file at `path`.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let bytes = std::fs::read(path).map_err(ConfigError::Io)?;
        let text = String::from_utf8_lossy(&bytes);
        toml::from_str(&text).map_err(ConfigError::Parse)
    }

    /// Look up the provider entry assigned to `role`.
    ///
    /// Returns `None` if the role is unset, the role name is unknown, or the
    /// role points at a provider that has no `[providers.<name>]` block.
    #[must_use]
    pub fn resolve_role(&self, role: &str) -> Option<(&str, &ProviderEntry)> {
        let provider_name = match role {
            "auditor" => self.roles.auditor.as_deref(),
            "reviewer" => self.roles.reviewer.as_deref(),
            "planner" => self.roles.planner.as_deref(),
            _ => None,
        }?;
        let entry = self.providers.get(provider_name)?;
        Some((provider_name, entry))
    }
}

/// Resolve the on-disk path to `config.toml`.
///
/// Resolution order:
/// 1. `$OPENAUDIT_HOME/config.toml` if `OPENAUDIT_HOME` is set.
/// 2. `$HOME/.openaudit/config.toml` if `HOME` is set.
/// 3. `None` otherwise.
#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("OPENAUDIT_HOME") {
        return Some(PathBuf::from(custom).join("config.toml"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".openaudit").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> &'static str {
        r#"
[providers.moonshot]
model = "kimi-2.6"
api_key_env = "MOONSHOT_API_KEY"
base_url = "https://api.moonshot.ai/v1"

[providers.anthropic]
model = "claude-opus-4-7"
api_key_env = "ANTHROPIC_API_KEY"

[roles]
auditor  = "moonshot"
reviewer = "anthropic"
planner  = "moonshot"
"#
    }

    #[test]
    fn parses_openaudit_md_example() {
        let cfg: OpenAuditConfig = toml::from_str(sample_config()).expect("config parses");

        // All three roles populated.
        assert_eq!(cfg.roles.auditor.as_deref(), Some("moonshot"));
        assert_eq!(cfg.roles.reviewer.as_deref(), Some("anthropic"));
        assert_eq!(cfg.roles.planner.as_deref(), Some("moonshot"));

        // Provider entries deserialized correctly.
        let moonshot = cfg
            .providers
            .get("moonshot")
            .expect("moonshot provider present");
        assert_eq!(moonshot.model, "kimi-2.6");
        assert_eq!(moonshot.api_key_env.as_deref(), Some("MOONSHOT_API_KEY"));
        assert_eq!(
            moonshot.base_url.as_deref(),
            Some("https://api.moonshot.ai/v1"),
        );

        // All three roles resolve.
        assert!(cfg.resolve_role("auditor").is_some());
        assert!(cfg.resolve_role("reviewer").is_some());
        assert!(cfg.resolve_role("planner").is_some());
    }

    #[test]
    fn resolve_role_returns_moonshot_entry_for_auditor() {
        let cfg: OpenAuditConfig = toml::from_str(sample_config()).expect("config parses");
        let (provider_name, entry) = cfg
            .resolve_role("auditor")
            .expect("auditor resolves to a provider");
        assert_eq!(provider_name, "moonshot");
        assert_eq!(entry.model, "kimi-2.6");
        assert_eq!(entry.api_key_env.as_deref(), Some("MOONSHOT_API_KEY"));
    }

    #[test]
    fn resolve_role_returns_none_for_unknown_role() {
        let cfg: OpenAuditConfig = toml::from_str(sample_config()).expect("config parses");
        assert!(cfg.resolve_role("garbage").is_none());
        assert!(cfg.resolve_role("").is_none());
    }

    #[test]
    fn resolve_role_returns_none_when_provider_missing() {
        // auditor points at "missing", but no [providers.missing] block exists.
        let toml_text = r#"
[roles]
auditor = "missing"
"#;
        let cfg: OpenAuditConfig = toml::from_str(toml_text).expect("config parses");
        assert_eq!(cfg.roles.auditor.as_deref(), Some("missing"));
        assert!(cfg.providers.is_empty());
        // Must be None, not panic.
        assert!(cfg.resolve_role("auditor").is_none());
    }

    #[test]
    fn load_from_reads_a_file_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "openaudit-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).expect("tempdir created");
        let path = dir.join("config.toml");
        std::fs::write(&path, sample_config()).expect("config written");

        let cfg = OpenAuditConfig::load_from(&path).expect("load_from succeeds");
        assert_eq!(
            cfg.providers
                .get("moonshot")
                .expect("moonshot present")
                .model,
            "kimi-2.6"
        );
        assert!(cfg.resolve_role("reviewer").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_config_path_prefers_openaudit_home() {
        let _guard = crate::test_env_lock();
        let saved_oa = std::env::var_os("OPENAUDIT_HOME");
        let saved_home = std::env::var_os("HOME");

        // Edition 2021: set_var / remove_var are safe. The shared `test_env_lock`
        // serializes against other env-mutating tests in this crate.
        std::env::set_var("OPENAUDIT_HOME", "/tmp/openaudit-custom");
        std::env::set_var("HOME", "/tmp/home-fallback");
        let path = default_config_path().expect("path resolves");
        assert_eq!(path, PathBuf::from("/tmp/openaudit-custom/config.toml"));

        std::env::remove_var("OPENAUDIT_HOME");
        let path = default_config_path().expect("path resolves from HOME");
        assert_eq!(
            path,
            PathBuf::from("/tmp/home-fallback/.openaudit/config.toml"),
        );

        // Restore.
        match saved_oa {
            Some(v) => std::env::set_var("OPENAUDIT_HOME", v),
            None => std::env::remove_var("OPENAUDIT_HOME"),
        }
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
