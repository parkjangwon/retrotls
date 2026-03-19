use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use http::Uri;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub listeners: Vec<ListenerConfig>,
    #[serde(default)]
    pub tls: TlsConfig,
    #[serde(default)]
    pub timeouts: TimeoutConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerConfig {
    pub bind: SocketAddr,
    pub upstream: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    #[serde(default = "default_min_version")]
    pub min_version: String,
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutConfig {
    #[serde(default = "default_connect_ms")]
    pub connect_ms: u64,
    #[serde(default = "default_request_ms")]
    pub request_ms: u64,
    #[serde(default = "default_idle_ms")]
    pub idle_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_access_log")]
    pub access_log: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            min_version: default_min_version(),
            insecure_skip_verify: false,
        }
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect_ms: default_connect_ms(),
            request_ms: default_request_ms(),
            idle_ms: default_idle_ms(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            access_log: default_access_log(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let raw =
            fs::read_to_string(path).map_err(|err| AppError::config_load(path.display(), err))?;

        let config: Config =
            serde_yaml::from_str(&raw).map_err(|err| AppError::config_load(path.display(), err))?;

        validate_listeners(&config.listeners)?;

        if config.tls.insecure_skip_verify {
            eprintln!(
                "warning: tls.insecure_skip_verify=true disables upstream certificate verification"
            );
        }

        Ok(config)
    }
}

pub fn join_paths(base: &str, path: &str) -> String {
    if base.is_empty() {
        return path.to_string();
    }
    if path.is_empty() {
        return base.to_string();
    }

    match (base.ends_with('/'), path.starts_with('/')) {
        (true, true) => format!("{}{}", base.trim_end_matches('/'), path),
        (false, false) => format!("{base}/{path}"),
        _ => format!("{base}{path}"),
    }
}

fn validate_listeners(listeners: &[ListenerConfig]) -> Result<(), AppError> {
    let mut binds = HashSet::with_capacity(listeners.len());

    for listener in listeners {
        if !binds.insert(listener.bind) {
            return Err(AppError::config_validation(format!(
                "duplicate listener bind address: {}",
                listener.bind
            )));
        }

        if !listener.upstream.starts_with("https://") {
            return Err(AppError::config_validation(format!(
                "listener upstream must start with https://: {}",
                listener.upstream
            )));
        }

        let uri = listener.upstream.parse::<Uri>().map_err(|err| {
            AppError::config_validation(format!(
                "invalid upstream URL '{}': {err}",
                listener.upstream
            ))
        })?;

        if uri.authority().is_none() {
            return Err(AppError::config_validation(format!(
                "invalid upstream URL '{}': missing authority",
                listener.upstream
            )));
        }

        if uri.scheme_str() != Some("https") {
            return Err(AppError::config_validation(format!(
                "listener upstream must use https scheme: {}",
                listener.upstream
            )));
        }
    }

    Ok(())
}

fn default_min_version() -> String {
    "1.2".to_string()
}

fn default_connect_ms() -> u64 {
    5_000
}

fn default_request_ms() -> u64 {
    30_000
}

fn default_idle_ms() -> u64 {
    60_000
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_access_log() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{join_paths, Config};

    #[test]
    fn join_paths_trims_duplicate_slashes() {
        let joined = join_paths("https://api.com/base", "/v1/test");
        assert_eq!(joined, "https://api.com/base/v1/test");
    }

    #[test]
    fn join_paths_adds_missing_separator() {
        let joined = join_paths("https://api.com/base", "v1/test");
        assert_eq!(joined, "https://api.com/base/v1/test");
    }

    #[test]
    fn join_paths_keeps_existing_separator() {
        let joined = join_paths("https://api.com/base/", "/v1/test");
        assert_eq!(joined, "https://api.com/base/v1/test");
    }

    #[test]
    fn load_applies_defaults_and_accepts_valid_config() {
        let path = write_temp_config(
            r#"
listeners:
  - bind: "127.0.0.1:8080"
    upstream: "https://api.example.com/base"
"#,
        );

        let config = Config::load(&path).expect("valid config should load");
        assert_eq!(config.listeners.len(), 1);
        assert_eq!(config.tls.min_version, "1.2");
        assert!(!config.tls.insecure_skip_verify);
        assert_eq!(config.timeouts.connect_ms, 5_000);
        assert_eq!(config.timeouts.request_ms, 30_000);
        assert_eq!(config.timeouts.idle_ms, 60_000);
        assert_eq!(config.logging.level, "info");
        assert!(config.logging.access_log);
    }

    #[test]
    fn load_rejects_duplicate_listener_bind_addresses() {
        let path = write_temp_config(
            r#"
listeners:
  - bind: "127.0.0.1:8080"
    upstream: "https://api.example.com"
  - bind: "127.0.0.1:8080"
    upstream: "https://api2.example.com"
"#,
        );

        let result = Config::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_rejects_non_https_upstream() {
        let path = write_temp_config(
            r#"
listeners:
  - bind: "127.0.0.1:8080"
    upstream: "http://api.example.com"
"#,
        );

        let result = Config::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_rejects_invalid_upstream_url() {
        let path = write_temp_config(
            r#"
listeners:
  - bind: "127.0.0.1:8080"
    upstream: "https://not a url"
"#,
        );

        let result = Config::load(&path);
        assert!(result.is_err());
    }

    fn write_temp_config(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX_EPOCH")
            .as_nanos();
        path.push(format!("retrotls-config-{unique}.yaml"));

        write_file(&path, contents);
        path
    }

    fn write_file(path: &Path, contents: &str) {
        fs::write(path, contents).expect("failed to write temp config file");
    }
}
