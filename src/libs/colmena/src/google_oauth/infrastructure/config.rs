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
///
/// `Debug` is hand-written: `client_secret` and `refresh_token` are
/// redacted so a stray `{:?}` can never leak them.
#[derive(Clone)]
pub struct OAuthCredentials {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: RefreshTokenSecret,
}

impl std::fmt::Debug for OAuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCredentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
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

    /// Build credentials directly from config-supplied strings (used by the
    /// http_request node's native OAuth, where creds come from the graph
    /// config rather than env vars).
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        refresh_token: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            refresh_token: RefreshTokenSecret::new(refresh_token),
        }
    }

    /// Test-only direct constructor — bypasses env reads so wiremock
    /// suites can preseed deterministic credentials.
    #[cfg(test)]
    pub fn for_tests(client_id: &str, client_secret: &str, refresh_token: &str) -> Self {
        Self::new(client_id, client_secret, refresh_token)
    }
}

/// Fields of an `oauth2_refresh_token` block (shared by `http_request.auth`
/// and `llm_call.google_workspace_auth`). Debug redacts the secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthRefreshBlock {
    pub token_url: Option<String>,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for OAuthRefreshBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthRefreshBlock")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

/// Parse `{ type: "oauth2_refresh_token", token_url?, client_id,
/// client_secret, refresh_token }`.
///
/// `require_token_url`: `http_request` requires it (any provider); Google
/// Workspace defaults to Google's endpoint. Every missing field is listed in
/// one error; an empty or whitespace-only string counts as missing. Values
/// are returned raw (no `${ENV}` expansion, no trimming). Error messages name
/// the block `auth` (the `http_request` key).
pub fn parse_oauth_refresh_block(
    block: &serde_json::Value,
    require_token_url: bool,
) -> Result<OAuthRefreshBlock, String> {
    parse_oauth_refresh_block_named(block, require_token_url, "auth")
}

/// [`parse_oauth_refresh_block`] with the config key used in error messages
/// (`auth` for `http_request`, `google_workspace_auth` for `llm_call`).
pub(crate) fn parse_oauth_refresh_block_named(
    block: &serde_json::Value,
    require_token_url: bool,
    key: &str,
) -> Result<OAuthRefreshBlock, String> {
    let ty = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if ty != "oauth2_refresh_token" {
        return Err(format!(
            "unsupported {key}.type '{ty}'; v1 supports only 'oauth2_refresh_token'"
        ));
    }

    let get = |k: &str| {
        block
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
    };
    let token_url = get("token_url");
    let client_id = get("client_id");
    let client_secret = get("client_secret");
    let refresh_token = get("refresh_token");

    let mut missing = Vec::new();
    if require_token_url && token_url.is_none() {
        missing.push("token_url");
    }
    if client_id.is_none() {
        missing.push("client_id");
    }
    if client_secret.is_none() {
        missing.push("client_secret");
    }
    if refresh_token.is_none() {
        missing.push("refresh_token");
    }
    if !missing.is_empty() {
        return Err(format!(
            "{key} block missing required fields: {}",
            missing.join(", ")
        ));
    }

    Ok(OAuthRefreshBlock {
        token_url,
        client_id: client_id.unwrap_or_default(),
        client_secret: client_secret.unwrap_or_default(),
        refresh_token: refresh_token.unwrap_or_default(),
    })
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

    #[test]
    fn parse_oauth_refresh_block_reports_every_missing_field() {
        let v = serde_json::json!({ "type": "oauth2_refresh_token" });
        let err = parse_oauth_refresh_block(&v, true).unwrap_err();
        assert!(
            err.contains("token_url")
                && err.contains("client_id")
                && err.contains("client_secret")
                && err.contains("refresh_token"),
            "{err}"
        );
    }

    #[test]
    fn parse_oauth_refresh_block_rejects_other_types() {
        let v = serde_json::json!({ "type": "basic", "client_id": "a", "client_secret": "b", "refresh_token": "c" });
        assert!(parse_oauth_refresh_block(&v, false)
            .unwrap_err()
            .contains("oauth2_refresh_token"));
    }

    #[test]
    fn parse_oauth_refresh_block_token_url_optional_when_not_required() {
        let v = serde_json::json!({ "type": "oauth2_refresh_token", "client_id": "cid", "client_secret": "CS-SECRET", "refresh_token": "RT-SECRET" });
        let b = parse_oauth_refresh_block(&v, false).unwrap();
        assert_eq!(b.token_url, None);
        let dbg = format!("{b:?}");
        assert!(
            !dbg.contains("CS-SECRET") && !dbg.contains("RT-SECRET"),
            "{dbg}"
        );
    }

    #[test]
    fn parse_oauth_refresh_block_treats_empty_or_blank_values_as_missing() {
        let v = serde_json::json!({
            "type": "oauth2_refresh_token",
            "token_url": "",
            "client_id": "   ",
            "client_secret": "",
            "refresh_token": "\t\n"
        });
        let err = parse_oauth_refresh_block(&v, true).unwrap_err();
        assert!(
            err.contains("token_url")
                && err.contains("client_id")
                && err.contains("client_secret")
                && err.contains("refresh_token"),
            "{err}"
        );
    }

    #[test]
    fn parse_oauth_refresh_block_rejects_empty_refresh_token_only() {
        let v = serde_json::json!({
            "type": "oauth2_refresh_token",
            "client_id": "cid",
            "client_secret": "cs",
            "refresh_token": ""
        });
        let err = parse_oauth_refresh_block(&v, false).unwrap_err();
        assert!(
            err.ends_with("missing required fields: refresh_token"),
            "{err}"
        );
    }

    #[test]
    fn oauth_credentials_debug_redacts_secrets() {
        let creds = OAuthCredentials::new("cid-visible", "CS-SECRET", "RT-SECRET");
        let dbg = format!("{creds:?}");
        assert!(
            !dbg.contains("CS-SECRET") && !dbg.contains("RT-SECRET"),
            "{dbg}"
        );
        assert!(dbg.contains("cid-visible"), "{dbg}");
        assert!(dbg.contains("<redacted>"), "{dbg}");
    }

    #[test]
    fn new_builds_credentials_directly() {
        let creds = OAuthCredentials::new("cid", "csec", "1//rt");
        assert_eq!(creds.client_id, "cid");
        assert_eq!(creds.client_secret, "csec");
        assert_eq!(creds.refresh_token.expose(), "1//rt");
    }

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
