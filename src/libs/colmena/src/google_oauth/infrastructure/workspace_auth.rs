//! Per-node Google Workspace credentials (`llm_call.config.google_workspace_auth`).
//!
//! Absent → the `gsheets`/`gdocs` toolkits keep using the platform env
//! credentials (`COLMENA_GOOGLE_OAUTH_*`). Present but invalid → hard error:
//! a typo must never silently authenticate as the platform account.
//!
//! Providers are cached process-wide by identity (same `OAuthProviderCache`
//! the `http_request` node uses), so every tool call acting as one Google
//! account shares one access-token cache, and distinct accounts never share
//! a provider.

use super::config::parse_oauth_refresh_block_named;
use super::OAuthProviderCache;
use crate::google_oauth::domain::AuthTokenProvider;
use std::sync::{Arc, OnceLock};

/// Google's OAuth 2.0 token endpoint — the default when the block omits
/// `token_url`.
pub const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Process-wide provider cache for Google Workspace identities.
static PROVIDERS: OnceLock<OAuthProviderCache> = OnceLock::new();

/// Resolved `google_workspace_auth` block. `Debug` redacts the secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct GoogleWorkspaceAuth {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for GoogleWorkspaceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleWorkspaceAuth")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

impl GoogleWorkspaceAuth {
    /// Read `config.google_workspace_auth`.
    /// - `Ok(None)` when the key is absent (env credentials apply).
    /// - `Ok(Some(_))` when it is a valid `oauth2_refresh_token` block;
    ///   `token_url` defaults to [`GOOGLE_TOKEN_ENDPOINT`].
    /// - `Err(msg)` when present but invalid — never an env fallback.
    pub fn from_node_config(config: &serde_json::Value) -> Result<Option<Self>, String> {
        let Some(block) = config.get("google_workspace_auth") else {
            return Ok(None);
        };
        // Messages name `google_workspace_auth` (never the secret values).
        let b = parse_oauth_refresh_block_named(block, false, "google_workspace_auth")?;
        Ok(Some(Self {
            token_url: b
                .token_url
                .unwrap_or_else(|| GOOGLE_TOKEN_ENDPOINT.to_string()),
            client_id: b.client_id,
            client_secret: b.client_secret,
            refresh_token: b.refresh_token,
        }))
    }

    /// Shared provider for this identity (process-wide `OAuthProviderCache`),
    /// behind the port so callers never depend on how the token is refreshed.
    pub fn provider(&self) -> Arc<dyn AuthTokenProvider> {
        PROVIDERS
            .get_or_init(OAuthProviderCache::new)
            .get_or_create(
                &self.token_url,
                &self.client_id,
                &self.client_secret,
                &self.refresh_token,
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_block_is_none() {
        assert!(
            GoogleWorkspaceAuth::from_node_config(&serde_json::json!({}))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn defaults_token_url_to_google_and_redacts_debug() {
        let cfg = serde_json::json!({ "google_workspace_auth": { "type": "oauth2_refresh_token", "client_id": "cid", "client_secret": "CS-SECRET", "refresh_token": "RT-SECRET" } });
        let a = GoogleWorkspaceAuth::from_node_config(&cfg)
            .unwrap()
            .unwrap();
        assert_eq!(a.token_url, GOOGLE_TOKEN_ENDPOINT);
        let dbg = format!("{a:?}");
        assert!(
            !dbg.contains("CS-SECRET") && !dbg.contains("RT-SECRET"),
            "{dbg}"
        );
    }

    #[test]
    fn explicit_token_url_is_kept() {
        let cfg = serde_json::json!({ "google_workspace_auth": { "type": "oauth2_refresh_token", "token_url": "https://t/token", "client_id": "cid", "client_secret": "cs", "refresh_token": "rt" } });
        let a = GoogleWorkspaceAuth::from_node_config(&cfg)
            .unwrap()
            .unwrap();
        assert_eq!(a.token_url, "https://t/token");
    }

    #[test]
    fn same_identity_shares_one_provider_distinct_identities_do_not() {
        let base = GoogleWorkspaceAuth {
            token_url: GOOGLE_TOKEN_ENDPOINT.into(),
            client_id: "cid".into(),
            client_secret: "cs".into(),
            refresh_token: "rt-1".into(),
        };
        assert!(Arc::ptr_eq(&base.provider(), &base.clone().provider()));

        // Each field of the identity tuple, varied alone, yields its own provider.
        let variants = [
            GoogleWorkspaceAuth {
                token_url: "https://other.example/token".into(),
                ..base.clone()
            },
            GoogleWorkspaceAuth {
                client_id: "cid-2".into(),
                ..base.clone()
            },
            GoogleWorkspaceAuth {
                client_secret: "cs-2".into(),
                ..base.clone()
            },
            GoogleWorkspaceAuth {
                refresh_token: "rt-2".into(),
                ..base.clone()
            },
        ];
        for v in &variants {
            assert!(
                !Arc::ptr_eq(&base.provider(), &v.provider()),
                "a different identity must not share the provider: {v:?}"
            );
        }
    }

    #[test]
    fn invalid_block_errors_name_google_workspace_auth() {
        let missing = serde_json::json!({ "google_workspace_auth": { "type": "oauth2_refresh_token", "client_id": "cid" } });
        let err = GoogleWorkspaceAuth::from_node_config(&missing).unwrap_err();
        assert_eq!(
            err,
            "google_workspace_auth block missing required fields: client_secret, refresh_token"
        );

        let wrong_type = serde_json::json!({ "google_workspace_auth": { "type": "basic" } });
        let err = GoogleWorkspaceAuth::from_node_config(&wrong_type).unwrap_err();
        assert!(
            err.starts_with("unsupported google_workspace_auth.type 'basic'"),
            "{err}"
        );
        assert!(!err.contains(" auth.type"), "{err}");
    }

    #[test]
    fn empty_refresh_token_is_error_never_env_fallback() {
        let cfg = serde_json::json!({ "google_workspace_auth": { "type": "oauth2_refresh_token", "client_id": "cid", "client_secret": "cs", "refresh_token": "" } });
        let err = GoogleWorkspaceAuth::from_node_config(&cfg).unwrap_err();
        assert!(err.contains("refresh_token"), "{err}");
    }

    /// An explicit `token_url` must be the endpoint the shared provider
    /// actually refreshes against — with the block's own credentials.
    #[tokio::test]
    async fn explicit_token_url_reaches_the_provider() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/custom/token"))
            .and(body_string_contains("refresh_token=rt-explicit-url"))
            .and(body_string_contains("client_id=cid-explicit-url"))
            .and(body_string_contains("client_secret=cs-explicit-url"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.explicit","expires_in":3600,"token_type":"Bearer"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let cfg = serde_json::json!({ "google_workspace_auth": {
            "type": "oauth2_refresh_token",
            "token_url": format!("{}/custom/token", server.uri()),
            "client_id": "cid-explicit-url",
            "client_secret": "cs-explicit-url",
            "refresh_token": "rt-explicit-url"
        } });
        let auth = GoogleWorkspaceAuth::from_node_config(&cfg)
            .unwrap()
            .unwrap();
        let token = auth
            .provider()
            .get_bearer_token()
            .await
            .expect("token from the explicit endpoint");
        assert_eq!(token.as_str(), "ya29.explicit");
    }
}
