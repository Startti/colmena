//! `GSheetsConfig` — read from env vars at dispatcher construction.
//!
//! Post-OAuth migration (2026-06-10): credentials live in the shared
//! `google_oauth` subsystem (env vars `COLMENA_GOOGLE_OAUTH_*`). This
//! struct only carries gsheets-specific knobs (scopes for documentation
//! purposes, request timeout, retry budget) plus the `share_email`
//! that the LLM prelude uses to tell users which address to share
//! with.

use std::time::Duration;

/// Default scopes (sufficient for v1):
/// - `spreadsheets` — read/write all sheets the identity has access to.
/// - `drive.file` — create files; access files the app created/opened.
///   This scope does NOT grant blanket Drive access, only files this
///   app touches. Required for create_from_xlsx + export_xlsx.
///
/// Note: the OAuth flow consents to these scopes at the
/// `colmena_oauth_setup` stage; the runtime does NOT pass them in the
/// refresh request (Google's OAuth contract: scopes are fixed at
/// consent time, the access_token inherits them).
pub const DEFAULT_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/spreadsheets",
    "https://www.googleapis.com/auth/drive.file",
];

#[derive(Debug, Clone)]
pub struct GSheetsConfig {
    /// Scopes the consented refresh_token covers. Documentation only
    /// at runtime — included so `from_env` callers can pass it through
    /// for tracing / observability without re-deriving from the env.
    pub scopes: Vec<String>,
    /// Per-request HTTP timeout for calls to sheets.googleapis.com /
    /// drive.googleapis.com (separate from the OAuth refresh timeout
    /// which lives in the `google_oauth` subsystem).
    pub request_timeout: Duration,
    /// Max retries on 429 / 5xx before giving up.
    pub max_retries: u32,
    /// Workspace user email that the agent ACTS AS — the address users
    /// should share their spreadsheets with as Editor. Surfaces in
    /// `SheetsError::PermissionDenied` and in the auto-injected LLM
    /// prelude. Read from `COLMENA_GOOGLE_SHARE_EMAIL`. Falls back to
    /// empty string (degraded prelude) when unset, mirroring the
    /// resolution chain in
    /// `dag_engine/.../google_workspace_prelude.rs`.
    pub share_email: String,
}

impl GSheetsConfig {
    /// Read from environment. Always succeeds — the *consumer* of the
    /// config is the one that surfaces `SheetsError::NotConfigured`
    /// when the OAuth credentials env vars are missing.
    ///
    /// Env vars consumed:
    /// - `COLMENA_GOOGLE_SHARE_EMAIL` — the Workspace user email.
    /// - `COLMENA_GSHEETS_SCOPES` — comma-separated scope list (optional
    ///   override; default is `spreadsheets,drive.file`). Short names
    ///   are auto-prefixed with `https://www.googleapis.com/auth/`;
    ///   full URLs pass through.
    pub fn from_env() -> Self {
        let scopes = std::env::var("COLMENA_GSHEETS_SCOPES")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|p| {
                        let p = p.trim();
                        if p.starts_with("https://") {
                            p.to_string()
                        } else {
                            format!("https://www.googleapis.com/auth/{p}")
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect());

        let share_email = std::env::var("COLMENA_GOOGLE_SHARE_EMAIL")
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        Self {
            scopes,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            share_email,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scopes_cover_sheets_and_drive_file() {
        let cfg = GSheetsConfig {
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            share_email: String::new(),
        };
        assert!(cfg.scopes.iter().any(|s| s.ends_with("/spreadsheets")));
        assert!(cfg.scopes.iter().any(|s| s.ends_with("/drive.file")));
    }
}
