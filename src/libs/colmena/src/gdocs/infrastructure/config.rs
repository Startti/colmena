//! Operator-facing config. Constructed from env vars; can be overridden
//! at runtime by callers (per-tool fixed_config). Mirrors the
//! `GSheetsConfig` pattern post-OAuth migration (2026-06-10).

use std::time::Duration;

/// Operator-controlled runtime configuration for the gdocs subsystem.
///
/// Auth credentials live in the shared `google_oauth` subsystem (env
/// vars `COLMENA_GOOGLE_OAUTH_*`); this struct carries only
/// gdocs-specific knobs plus the `share_email` (the Workspace user the
/// agent acts as).
#[derive(Debug, Clone)]
pub struct GDocsConfig {
    /// OAuth scopes consented at `colmena_oauth_setup` time.
    /// Documentation-only at runtime — the Google token endpoint
    /// returns access tokens inheriting whatever scopes the
    /// refresh_token was issued with.
    pub scopes: Vec<String>,
    /// Drive folder ID that `create_*` calls drop new docs into. The
    /// folder must be shared as Editor with the `share_email`.
    pub default_parent_folder: Option<String>,
    /// Per-request HTTP timeout.
    pub request_timeout: Duration,
    /// Total retries on 429 / 5xx before surfacing the error.
    pub max_retries: u32,
    /// TTL for the in-memory snapshot cache used by the co-edit guard.
    pub revision_cache_ttl: Duration,
    /// Workspace user the agent acts as — the address users should
    /// share their docs with as Editor. Read from
    /// `COLMENA_GOOGLE_SHARE_EMAIL`. Empty string in degraded
    /// deployments where the var is not set; the LLM prelude
    /// surfaces a fallback message in that case.
    pub share_email: String,
}

impl GDocsConfig {
    /// Build from env vars with sane defaults.
    ///
    /// Recognised env vars:
    /// - `COLMENA_GOOGLE_SHARE_EMAIL`             — Workspace user email
    /// - `COLMENA_GDOCS_SCOPES`                   — comma-sep overrides
    /// - `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID` — Drive folder
    /// - `COLMENA_GDOCS_REVISION_CACHE_SECS`      — cache TTL (default 5)
    pub fn from_env() -> Self {
        let scopes = std::env::var("COLMENA_GDOCS_SCOPES")
            .ok()
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_else(|| {
                // `drive.file` (per-file) is privacy-friendly in theory but
                // only grants access to files the app CREATED or received
                // via Drive Picker — NOT files a user shared with the SA
                // via the "Share" button (those return
                // `appNotAuthorizedToFile`). For colmena's realistic v1
                // flow (user shares an existing doc with `share_email`),
                // the agent must use `drive` to read/export shared
                // files. Operators can downgrade to `drive.readonly`
                // for read-only profiles or override entirely via
                // COLMENA_GDOCS_SCOPES.
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
        let share_email = std::env::var("COLMENA_GOOGLE_SHARE_EMAIL")
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Self {
            scopes,
            default_parent_folder,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            revision_cache_ttl: Duration::from_secs(cache_secs),
            share_email,
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
            scopes: vec![
                "https://www.googleapis.com/auth/documents".to_string(),
                "https://www.googleapis.com/auth/drive.file".to_string(),
            ],
            default_parent_folder: None,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            revision_cache_ttl: Duration::from_secs(5),
            share_email: String::new(),
        };
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.revision_cache_ttl, Duration::from_secs(5));
        assert_eq!(cfg.scopes.len(), 2);
    }
}
