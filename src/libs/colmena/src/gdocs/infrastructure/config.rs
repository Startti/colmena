//! Operator-facing config. Constructed from env vars; can be overridden
//! at runtime by callers (per-tool fixed_config). Mirrors the
//! `GSheetsConfig` pattern.

use std::path::PathBuf;
use std::time::Duration;

/// Operator-controlled runtime configuration for the gdocs subsystem.
///
/// Built from env vars via [`GDocsConfig::from_env`]; can be modified
/// in tests or by dispatchers that override per-tool fixed_config.
#[derive(Debug, Clone)]
pub struct GDocsConfig {
    /// Path to a service-account JSON file (from `GOOGLE_APPLICATION_CREDENTIALS`).
    /// `None` triggers ADC fallback.
    pub credentials_path: Option<PathBuf>,
    /// OAuth scopes requested at token acquisition. Defaults to the
    /// minimum for read/write Docs + per-file Drive access.
    pub scopes: Vec<String>,
    /// Drive folder ID that `create_*` calls drop new docs into. The
    /// folder must be shared with the SA / authenticated user.
    pub default_parent_folder: Option<String>,
    /// Per-request HTTP timeout.
    pub request_timeout: Duration,
    /// Total retries on 429 / 5xx before surfacing the error.
    pub max_retries: u32,
    /// TTL for the in-memory snapshot cache used by the co-edit guard.
    pub revision_cache_ttl: Duration,
}

impl GDocsConfig {
    /// Build from env vars with sane defaults.
    ///
    /// Recognised env vars:
    /// - `GOOGLE_APPLICATION_CREDENTIALS`         — SA JSON path
    /// - `COLMENA_GDOCS_SCOPES`                   — comma-sep overrides
    /// - `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID` — Drive folder
    /// - `COLMENA_GDOCS_REVISION_CACHE_SECS`      — cache TTL (default 5)
    pub fn from_env() -> Self {
        let credentials_path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .map(PathBuf::from);
        let scopes = std::env::var("COLMENA_GDOCS_SCOPES")
            .ok()
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_else(|| {
                // `drive.file` (per-file) is privacy-friendly in theory but
                // only grants access to files the app CREATED or received
                // via Drive Picker — NOT files a user shared with the SA
                // via the "Share" button (those return
                // `appNotAuthorizedToFile`). For colmena's realistic v1
                // flow (user shares an existing doc with the SA), the SA
                // must use `drive` to read/export shared files. Operators
                // can downgrade to `drive.readonly` for read-only profiles
                // or override entirely via COLMENA_GDOCS_SCOPES.
                vec![
                    "https://www.googleapis.com/auth/documents".to_string(),
                    "https://www.googleapis.com/auth/drive".to_string(),
                ]
            });
        let default_parent_folder = std::env::var("COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID").ok();
        let cache_secs = std::env::var("COLMENA_GDOCS_REVISION_CACHE_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5);
        Self {
            credentials_path,
            scopes,
            default_parent_folder,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            revision_cache_ttl: Duration::from_secs(cache_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_struct_values() {
        // Document the default shape (without env manipulation, which
        // would race other tests in the same process).
        let cfg = GDocsConfig {
            credentials_path: None,
            scopes: vec![
                "https://www.googleapis.com/auth/documents".to_string(),
                "https://www.googleapis.com/auth/drive.file".to_string(),
            ],
            default_parent_folder: None,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            revision_cache_ttl: Duration::from_secs(5),
        };
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.revision_cache_ttl, Duration::from_secs(5));
        assert_eq!(cfg.scopes.len(), 2);
    }
}
