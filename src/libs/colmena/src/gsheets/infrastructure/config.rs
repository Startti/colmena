//! `GSheetsConfig` — read from env vars at dispatcher construction.

use std::path::PathBuf;
use std::time::Duration;

/// Default scopes (sufficient for v1):
/// - `spreadsheets` — read/write all sheets the identity has access to.
/// - `drive.file` — create files; access files the app created/opened.
///   This scope does NOT grant blanket Drive access, only files this
///   app touches. Required for create_from_xlsx + export_xlsx.
pub const DEFAULT_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/spreadsheets",
    "https://www.googleapis.com/auth/drive.file",
];

#[derive(Debug, Clone)]
pub struct GSheetsConfig {
    /// Path to a service-account JSON. When `None`, `yup-oauth2` falls
    /// through to ADC (`GOOGLE_APPLICATION_CREDENTIALS` env var or
    /// gcloud-saved user creds or GCE metadata server, in that order).
    pub credentials_path: Option<PathBuf>,
    /// Scopes the token must cover.
    pub scopes: Vec<String>,
    /// Per-request HTTP timeout.
    pub request_timeout: Duration,
    /// Max retries on 429 / 5xx before giving up.
    pub max_retries: u32,
}

impl GSheetsConfig {
    /// Read from environment. Always succeeds — the *consumer* of the
    /// config is the one that surfaces `SheetsError::NotConfigured` when
    /// the chosen auth method actually fails to produce a token.
    ///
    /// Env vars consumed:
    /// - `GOOGLE_APPLICATION_CREDENTIALS` — path to service-account JSON.
    /// - `COLMENA_GSHEETS_SCOPES` — comma-separated scope list. Short
    ///   names (e.g. `spreadsheets`) are auto-prefixed with
    ///   `https://www.googleapis.com/auth/`; full URLs pass through.
    pub fn from_env() -> Self {
        let credentials_path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .map(PathBuf::from);
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
        Self {
            credentials_path,
            scopes,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scopes_cover_sheets_and_drive_file() {
        let cfg = GSheetsConfig {
            credentials_path: None,
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
        };
        assert!(cfg
            .scopes
            .iter()
            .any(|s| s.ends_with("/spreadsheets")));
        assert!(cfg
            .scopes
            .iter()
            .any(|s| s.ends_with("/drive.file")));
    }
}
