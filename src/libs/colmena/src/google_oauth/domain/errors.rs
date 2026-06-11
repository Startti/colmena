//! Domain errors surfaced by the OAuth subsystem.
//!
//! Each variant maps to an operator action documented in the
//! revocation runbook (`docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`
//! §8.4). The variants are deliberately coarse — clients (gsheets,
//! gdocs) translate them to subsystem-specific errors as needed.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OAuthError {
    /// Google rejected the refresh request with `invalid_grant`. The
    /// refresh token is no longer valid — typically because the user
    /// revoked consent at <https://myaccount.google.com/permissions>
    /// or Google rotated it without us catching up. The operator must
    /// re-run `colmena_oauth_setup` and upload the new token to
    /// Secret Manager.
    #[error(
        "OAuth refresh token revoked (Google returned invalid_grant). \
         Re-run colmena_oauth_setup to issue a new refresh token and \
         update Secret Manager."
    )]
    RefreshTokenRevoked,

    /// Google rejected the refresh request with `invalid_client`. The
    /// client_id or client_secret is wrong. Usually means a
    /// misconfigured env var or a stale Secret Manager value pointing
    /// at a deleted OAuth client.
    #[error("OAuth client credentials invalid: {0}")]
    ClientCredsInvalid(String),

    /// Transient failure (5xx, timeout, network error). Retried
    /// internally by the refresh client; this variant only surfaces
    /// after the retry budget is exhausted. Callers may try again
    /// on the next request — the access token cache is left empty so
    /// the next call re-attempts a fresh refresh.
    #[error("OAuth refresh transient failure (retries exhausted): {0}")]
    Transient(String),

    /// One or more required env vars were missing or empty at
    /// `OAuthCredentials::from_env()` time. The error carries the
    /// names of every missing variable so the operator sees the full
    /// migration list in a single boot attempt — no whack-a-mole.
    #[error(
        "OAuth credentials missing from environment. Missing: {0:?}. \
         Set them via deploy_gcp.sh / Secret Manager. \
         See docs/developer_guide/47_google_oauth.md."
    )]
    ConfigMissing(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoked_message_directs_to_runbook() {
        let err = OAuthError::RefreshTokenRevoked;
        let msg = format!("{err}");
        assert!(msg.contains("colmena_oauth_setup"));
        assert!(msg.contains("Secret Manager"));
    }

    #[test]
    fn config_missing_lists_every_var() {
        let err = OAuthError::ConfigMissing(vec![
            "COLMENA_GOOGLE_OAUTH_CLIENT_ID".to_string(),
            "COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN".to_string(),
        ]);
        let msg = format!("{err}");
        assert!(msg.contains("COLMENA_GOOGLE_OAUTH_CLIENT_ID"));
        assert!(msg.contains("COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN"));
    }

    #[test]
    fn errors_implement_partial_eq_for_test_assertions() {
        // Used heavily in `assert_eq!(err, OAuthError::RefreshTokenRevoked)`
        // patterns by callers.
        assert_eq!(
            OAuthError::RefreshTokenRevoked,
            OAuthError::RefreshTokenRevoked
        );
        assert_ne!(
            OAuthError::RefreshTokenRevoked,
            OAuthError::Transient("x".into())
        );
    }
}
