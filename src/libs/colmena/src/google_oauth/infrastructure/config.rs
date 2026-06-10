//! Env-var-based credential loader for the OAuth subsystem.
//!
//! The four-tuple `(client_id, client_secret, refresh_token,
//! share_email)` is what the runtime needs to mint access tokens. Three
//! of those four live in Secret Manager (mounted as env vars by
//! Cloud Run); `share_email` is set directly in `deploy_gcp.sh`
//! because it's not secret — it's the address users SHARE WITH and
//! must be visible to the agent prelude.

use crate::google_oauth::domain::{OAuthError, RefreshTokenSecret};

/// All credentials the runtime needs to refresh access tokens.
///
/// Clone is cheap (only `String` + a thin wrapper). The struct is
/// passed into `RefreshClient::refresh` per call so the client is
/// stateless and can be shared across many providers if needed in
/// the future.
#[derive(Debug, Clone)]
pub struct OAuthCredentials {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: RefreshTokenSecret,
}

impl OAuthCredentials {
    /// Read credentials from `COLMENA_GOOGLE_OAUTH_CLIENT_ID`,
    /// `_CLIENT_SECRET`, `_REFRESH_TOKEN`.
    ///
    /// Returns `OAuthError::ConfigMissing` listing **every** missing
    /// or empty variable. The "list every missing" contract matters:
    /// during a migration the operator should see the complete set of
    /// env vars to set in a single boot, not get a different error on
    /// each redeploy.
    ///
    /// "Missing or empty" means the same thing — Cloud Run secret
    /// mounts that point to a nonexistent secret silently produce an
    /// empty env var, so treating empty as missing catches that
    /// failure mode in the same code path.
    pub fn from_env() -> Result<Self, OAuthError> {
        let client_id = read_env("COLMENA_GOOGLE_OAUTH_CLIENT_ID");
        let client_secret = read_env("COLMENA_GOOGLE_OAUTH_CLIENT_SECRET");
        let refresh_token = read_env("COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN");

        let mut missing: Vec<String> = Vec::new();
        if client_id.is_none() {
            missing.push("COLMENA_GOOGLE_OAUTH_CLIENT_ID".to_string());
        }
        if client_secret.is_none() {
            missing.push("COLMENA_GOOGLE_OAUTH_CLIENT_SECRET".to_string());
        }
        if refresh_token.is_none() {
            missing.push("COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN".to_string());
        }
        if !missing.is_empty() {
            return Err(OAuthError::ConfigMissing(missing));
        }

        // expect() lifts the value out — the all-missing check above
        // guarantees each is Some here.
        Ok(Self {
            client_id: client_id.expect("client_id missing check above"),
            client_secret: client_secret.expect("client_secret missing check above"),
            refresh_token: RefreshTokenSecret::new(
                refresh_token.expect("refresh_token missing check above"),
            ),
        })
    }

    /// Test-only direct constructor — bypasses env reads so wiremock
    /// suites can preseed deterministic credentials.
    #[cfg(test)]
    pub fn for_tests(client_id: &str, client_secret: &str, refresh_token: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            refresh_token: RefreshTokenSecret::new(refresh_token),
        }
    }
}

/// Read an env var, returning `Some(trimmed)` when set AND non-empty
/// after trim. Treats whitespace-only as missing (Cloud Run secret
/// mounts that resolve to empty strings are a common misconfig).
fn read_env(name: &str) -> Option<String> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    //! Tests mutate process env vars and therefore use
    //! `serial_test::serial` to prevent concurrent races. The
    //! crate is already a dev-dep.

    use super::*;
    use serial_test::serial;

    const CID: &str = "COLMENA_GOOGLE_OAUTH_CLIENT_ID";
    const CSEC: &str = "COLMENA_GOOGLE_OAUTH_CLIENT_SECRET";
    const RT: &str = "COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN";

    fn clear_all() {
        std::env::remove_var(CID);
        std::env::remove_var(CSEC);
        std::env::remove_var(RT);
    }

    #[test]
    #[serial]
    fn from_env_returns_credentials_when_all_set() {
        clear_all();
        std::env::set_var(CID, "client-123");
        std::env::set_var(CSEC, "secret-456");
        std::env::set_var(RT, "1//abc");
        let creds = OAuthCredentials::from_env().expect("all vars present");
        clear_all();
        assert_eq!(creds.client_id, "client-123");
        assert_eq!(creds.client_secret, "secret-456");
        assert_eq!(creds.refresh_token.expose(), "1//abc");
    }

    #[test]
    #[serial]
    fn from_env_lists_all_missing_in_one_error() {
        clear_all();
        // Note: deliberately set NONE of the vars. The error must list
        // ALL three, not stop at the first.
        let err = OAuthCredentials::from_env().expect_err("nothing set");
        match err {
            OAuthError::ConfigMissing(names) => {
                assert_eq!(names.len(), 3);
                assert!(names.contains(&CID.to_string()));
                assert!(names.contains(&CSEC.to_string()));
                assert!(names.contains(&RT.to_string()));
            }
            other => panic!("expected ConfigMissing, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn from_env_treats_empty_string_as_missing() {
        clear_all();
        std::env::set_var(CID, "");
        std::env::set_var(CSEC, "secret");
        std::env::set_var(RT, "token");
        let err = OAuthCredentials::from_env().expect_err("empty var");
        clear_all();
        match err {
            OAuthError::ConfigMissing(names) => {
                assert_eq!(names, vec![CID.to_string()]);
            }
            other => panic!("expected ConfigMissing, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn from_env_treats_whitespace_only_as_missing() {
        // Cloud Run mounts that point at the wrong secret version
        // sometimes resolve to whitespace. Catch that here so it
        // fails fast instead of getting a confusing OAuth error.
        clear_all();
        std::env::set_var(CID, "   ");
        std::env::set_var(CSEC, "secret");
        std::env::set_var(RT, "token");
        let err = OAuthCredentials::from_env().expect_err("whitespace var");
        clear_all();
        assert!(matches!(err, OAuthError::ConfigMissing(_)));
    }

    #[test]
    #[serial]
    fn from_env_trims_leading_trailing_whitespace() {
        // Some Secret Manager UIs strip trailing newlines, some don't.
        // Be tolerant.
        clear_all();
        std::env::set_var(CID, "  client-123\n");
        std::env::set_var(CSEC, " secret\t");
        std::env::set_var(RT, "\ntoken-value\n");
        let creds = OAuthCredentials::from_env().expect("all set");
        clear_all();
        assert_eq!(creds.client_id, "client-123");
        assert_eq!(creds.client_secret, "secret");
        assert_eq!(creds.refresh_token.expose(), "token-value");
    }
}
