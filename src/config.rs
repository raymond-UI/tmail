//! Config loading and precedence merge (DESIGN.md §9).
//!
//! Precedence is **flags > env > file** and is applied per-setting at the call
//! site via the resolver helpers below; this module owns the file model and its
//! location.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, ErrorCode, Result};

/// Default `wait`/`otp` timeout when nothing overrides it.
pub const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 120;
/// Default polling interval for the `wait`/`otp` loop.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 3;

/// The on-disk `config.toml` model. Every section is optional.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// SMTP send transport settings.
    #[serde(default)]
    pub smtp: Option<SmtpConfig>,
    /// Reserved for future multi-provider support (accepted but not yet read).
    #[serde(default)]
    #[allow(dead_code)]
    pub provider: Option<ProviderConfig>,
    /// Tunable defaults for the blocking verbs.
    #[serde(default)]
    pub defaults: Option<Defaults>,
}

/// `[smtp]` section.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpConfig {
    /// `smtps://user:pass@host:465` or `smtp://user:pass@host:587`.
    pub url: Option<String>,
    /// Default `From` when `--from` is omitted.
    pub from: Option<String>,
}

/// `[provider]` section (reserved).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Default receive provider; only `mail.tm` exists today.
    #[allow(dead_code)]
    pub default: Option<String>,
}

/// `[defaults]` section.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    /// Default `wait`/`otp` timeout in seconds.
    pub wait_timeout_secs: Option<u64>,
    /// Default poll interval in seconds.
    pub poll_interval_secs: Option<u64>,
}

impl Config {
    /// Load config from an explicit path, or the default location when `None`.
    /// A missing default-location file yields an empty config (not an error);
    /// an explicitly-requested missing file *is* a `CONFIG` error.
    pub fn load(explicit: Option<&Path>) -> Result<Config> {
        match explicit {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| {
                    AppError::config(format!("cannot read config {}: {e}", path.display()))
                })?;
                Self::parse(&text)
            }
            None => match default_config_path() {
                Some(path) if path.exists() => {
                    let text = std::fs::read_to_string(&path).map_err(|e| {
                        AppError::config(format!("cannot read config {}: {e}", path.display()))
                    })?;
                    Self::parse(&text)
                }
                _ => Ok(Config::default()),
            },
        }
    }

    /// Parse TOML into a [`Config`], mapping syntax errors to `CONFIG`.
    pub fn parse(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| AppError::new(ErrorCode::Config, format!("config: {e}")))
    }

    /// Resolve the effective SMTP URL: flag > env > file.
    pub fn resolve_smtp_url(&self, flag: Option<&str>) -> Option<String> {
        flag.map(str::to_owned)
            .or_else(|| std::env::var("TMAIL_SMTP_URL").ok())
            .or_else(|| self.smtp.as_ref().and_then(|s| s.url.clone()))
    }

    /// Resolve the default `From`: flag > file `[smtp].from`.
    pub fn resolve_from(&self, flag: Option<&str>) -> Option<String> {
        flag.map(str::to_owned)
            .or_else(|| self.smtp.as_ref().and_then(|s| s.from.clone()))
    }

    /// Effective wait timeout: flag > file > built-in default.
    pub fn wait_timeout_secs(&self, flag: Option<u64>) -> u64 {
        flag.or_else(|| self.defaults.as_ref().and_then(|d| d.wait_timeout_secs))
            .unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS)
    }

    /// Effective poll interval: file > built-in default.
    pub fn poll_interval_secs(&self) -> u64 {
        self.defaults
            .as_ref()
            .and_then(|d| d.poll_interval_secs)
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECS)
    }
}

/// `~/.config/tmail/config.toml`, honoring `XDG_CONFIG_HOME` via `directories`.
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "tmail").map(|d| d.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml: &str) -> Config {
        Config::parse(toml).expect("valid config")
    }

    #[test]
    fn smtp_precedence_flag_over_env_over_file() {
        let c = cfg("[smtp]\nurl = \"smtp://file\"\n");
        // flag wins
        assert_eq!(
            c.resolve_smtp_url(Some("smtp://flag")).as_deref(),
            Some("smtp://flag")
        );
        // file used when no flag/env
        std::env::remove_var("TMAIL_SMTP_URL");
        assert_eq!(c.resolve_smtp_url(None).as_deref(), Some("smtp://file"));
    }

    #[test]
    fn wait_timeout_precedence() {
        let c = cfg("[defaults]\nwait_timeout_secs = 90\n");
        assert_eq!(c.wait_timeout_secs(Some(5)), 5); // flag
        assert_eq!(c.wait_timeout_secs(None), 90); // file
        assert_eq!(
            Config::default().wait_timeout_secs(None),
            DEFAULT_WAIT_TIMEOUT_SECS
        );
    }

    #[test]
    fn malformed_config_is_config_error() {
        let err = Config::parse("this is = = not toml").unwrap_err();
        assert_eq!(err.code, ErrorCode::Config);
    }

    #[test]
    fn unknown_keys_rejected() {
        let err = Config::parse("[smtp]\nbogus = 1\n").unwrap_err();
        assert_eq!(err.code, ErrorCode::Config);
    }
}
