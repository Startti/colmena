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
use super::{HostRefreshTokenProvider, OAuthProviderCache, DEFAULT_TOKEN_ENDPOINT};
use crate::dag_engine::application::ports::HostTokenPort;
use crate::google_oauth::domain::AuthTokenProvider;
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};

/// Process-wide provider cache for Google Workspace identities.
static PROVIDERS: OnceLock<OAuthProviderCache> = OnceLock::new();

/// Resolved `google_workspace_auth` block. `Debug` redacts the secrets.
#[derive(Clone, PartialEq, Eq)]
pub enum GoogleWorkspaceAuth {
    /// `type: "oauth2_refresh_token"`: the engine refreshes with the
    /// connection's own client credentials.
    RefreshToken {
        token_url: String,
        client_id: String,
        client_secret: String,
        refresh_token: String,
    },
    /// `type: "host_refresh_bearer"` (CX7): the embedder seeds an access token
    /// and renews it through the engine's `HostTokenPort` with `handle`.
    HostRefresh {
        access_token: String,
        /// Unix seconds.
        expires_at: i64,
        handle: String,
        host: HostTokenContext,
    },
}

/// What a `HostRefresh` block needs from the run: the engine's port and the
/// run's `agent_session_id`. Bound by the node that read the block
/// ([`GoogleWorkspaceAuth::with_host_context`]); its provider is built once and
/// shared by every clone, so all tool calls of that node see one renewal.
#[derive(Clone, Default)]
pub struct HostTokenContext {
    pub port: Option<Arc<dyn HostTokenPort>>,
    pub agent_session_id: Option<String>,
    provider: Arc<OnceLock<Arc<dyn AuthTokenProvider>>>,
}

/// Same credential = equal: the bound context is not part of the identity.
impl PartialEq for HostTokenContext {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for HostTokenContext {}

impl std::fmt::Debug for GoogleWorkspaceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RefreshToken {
                token_url,
                client_id,
                ..
            } => f
                .debug_struct("GoogleWorkspaceAuth::RefreshToken")
                .field("token_url", token_url)
                .field("client_id", client_id)
                .field("client_secret", &"<redacted>")
                .field("refresh_token", &"<redacted>")
                .finish(),
            Self::HostRefresh { expires_at, .. } => f
                .debug_struct("GoogleWorkspaceAuth::HostRefresh")
                .field("access_token", &"<redacted>")
                .field("expires_at", expires_at)
                .field("handle", &"<redacted>")
                .finish(),
        }
    }
}

const NO_HOST_PORT: &str = "google_workspace_auth: this engine has no host token port";

impl GoogleWorkspaceAuth {
    /// Read `config.google_workspace_auth`.
    /// - `Ok(None)` when the key is absent (env credentials apply).
    /// - `Ok(Some(_))` for a valid `oauth2_refresh_token` block (`token_url`
    ///   defaults to [`DEFAULT_TOKEN_ENDPOINT`]) or `host_refresh_bearer` block.
    /// - `Err(msg)` when present but invalid — never an env fallback.
    pub fn from_node_config(config: &serde_json::Value) -> Result<Option<Self>, String> {
        let Some(block) = config.get("google_workspace_auth") else {
            return Ok(None);
        };
        match block.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "host_refresh_bearer" => return Self::host_refresh(block).map(Some),
            "oauth2_refresh_token" => {}
            other => {
                return Err(format!(
                    "unsupported google_workspace_auth.type '{other}'; supported: \
                     'oauth2_refresh_token', 'host_refresh_bearer'"
                ))
            }
        }
        // Messages name `google_workspace_auth` (never the secret values).
        let b = parse_oauth_refresh_block_named(block, false, "google_workspace_auth")?;
        Ok(Some(Self::RefreshToken {
            token_url: b
                .token_url
                .unwrap_or_else(|| DEFAULT_TOKEN_ENDPOINT.to_string()),
            client_id: b.client_id,
            client_secret: b.client_secret,
            refresh_token: b.refresh_token,
        }))
    }

    /// The `google_workspace_auth` config block for these credentials, the
    /// inverse of [`Self::from_node_config`]. The tool executor uses it to hand
    /// an `llm_call`'s credentials to a node it dispatches (`for_each`). Holds
    /// the secrets in clear: never log or persist it. The bound host context
    /// is not part of the block: the receiving node binds its own.
    pub fn to_config_block(&self) -> serde_json::Value {
        match self {
            Self::RefreshToken {
                token_url,
                client_id,
                client_secret,
                refresh_token,
            } => serde_json::json!({
                "type": "oauth2_refresh_token",
                "token_url": token_url,
                "client_id": client_id,
                "client_secret": client_secret,
                "refresh_token": refresh_token,
            }),
            Self::HostRefresh {
                access_token,
                expires_at,
                handle,
                ..
            } => serde_json::json!({
                "type": "host_refresh_bearer",
                "access_token": access_token,
                "expires_at": expires_at,
                "handle": handle,
            }),
        }
    }

    /// `{access_token, expires_at, handle}`; every missing field is listed.
    fn host_refresh(block: &serde_json::Value) -> Result<Self, String> {
        let text = |k: &str| {
            let v = block.get(k).and_then(|v| v.as_str());
            v.filter(|s| !s.trim().is_empty()).map(str::to_string)
        };
        let (access_token, handle) = (text("access_token"), text("handle"));
        let expires_at = block.get("expires_at").and_then(|v| v.as_i64());
        let fields = [
            ("access_token", access_token.is_none()),
            ("expires_at", expires_at.is_none()),
            ("handle", handle.is_none()),
        ];
        let missing: Vec<&str> = fields.iter().filter(|f| f.1).map(|f| f.0).collect();
        match (access_token, expires_at, handle) {
            (Some(access_token), Some(expires_at), Some(handle)) => Ok(Self::HostRefresh {
                access_token,
                expires_at,
                handle,
                host: HostTokenContext::default(),
            }),
            _ => Err(format!(
                "google_workspace_auth block missing required fields: {}",
                missing.join(", ")
            )),
        }
    }

    /// Bind the run's port and session to a `HostRefresh` block (no-op for a
    /// `RefreshToken` one). The session must be the run's
    /// `__colmena_agent_session_id`, never a value from the block.
    pub fn with_host_context(
        mut self,
        port: Option<Arc<dyn HostTokenPort>>,
        agent_session_id: Option<String>,
    ) -> Self {
        if let Self::HostRefresh { host, .. } = &mut self {
            let provider = Arc::default();
            (host.port, host.agent_session_id, host.provider) = (port, agent_session_id, provider);
        }
        self
    }

    /// The identity a cache keyed by account may use: the provider fingerprint
    /// for a refresh token, `sha256(handle)` for a host-refreshed bearer.
    /// Never a secret in clear.
    pub fn identity_key(&self) -> String {
        match self {
            Self::RefreshToken {
                token_url,
                client_id,
                client_secret,
                refresh_token,
            } => {
                OAuthProviderCache::fingerprint(token_url, client_id, client_secret, refresh_token)
            }
            Self::HostRefresh { handle, .. } => format!("{:x}", Sha256::digest(handle.as_bytes())),
        }
    }

    /// The token provider for this identity, behind the port so callers never
    /// depend on how the token is refreshed.
    ///
    /// `RefreshToken`: the process-wide `OAuthProviderCache` (bounded). Values
    /// are used literally: `${VAR}` is never expanded here. ADP sends the
    /// resolved credentials of the user's connection; expanding them would let
    /// a graph author name an engine env var (for instance the platform's own
    /// `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN`) and act with it.
    ///
    /// `HostRefresh`: a `HostRefreshTokenProvider` over the bound port, built
    /// once per binding. Without a port it is an error: never the platform
    /// account.
    pub fn provider(&self) -> Result<Arc<dyn AuthTokenProvider>, String> {
        match self {
            Self::RefreshToken {
                token_url,
                client_id,
                client_secret,
                refresh_token,
            } => {
                let cache = PROVIDERS.get_or_init(OAuthProviderCache::new);
                Ok(cache.get_or_create(token_url, client_id, client_secret, refresh_token))
            }
            Self::HostRefresh {
                access_token,
                expires_at,
                handle,
                host,
            } => {
                let port = host.port.clone().ok_or_else(|| NO_HOST_PORT.to_string())?;
                let (seed, sid) = (access_token.clone(), host.agent_session_id.clone());
                let new =
                    || HostRefreshTokenProvider::new(port, handle.clone(), seed, *expires_at, sid);
                Ok(host.provider.get_or_init(|| Arc::new(new())).clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `${VAR}` in a block is sent as written, never replaced by the engine's
    /// env (the platform refresh token must stay out of reach of a graph).
    #[tokio::test]
    #[serial_test::serial]
    async fn env_placeholders_are_never_expanded() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"tok-cx7-lit","expires_in":3600,"token_type":"Bearer"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        /// Removes the env var even when an assertion below panics.
        struct EnvGuard(&'static str);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                std::env::remove_var(self.0);
            }
        }
        std::env::set_var("CX7_PLATFORM_RT", "tok-cx7-platform-secret");
        let _env = EnvGuard("CX7_PLATFORM_RT");
        let auth = GoogleWorkspaceAuth::from_node_config(&serde_json::json!({
            "google_workspace_auth": {
                "type": "oauth2_refresh_token",
                "token_url": format!("{}/token", server.uri()),
                "client_id": "cid",
                "client_secret": "cs",
                "refresh_token": "${CX7_PLATFORM_RT}",
            }
        }))
        .unwrap()
        .unwrap();
        let GoogleWorkspaceAuth::RefreshToken { refresh_token, .. } = &auth else {
            panic!("refresh-token block");
        };
        assert_eq!(refresh_token, "${CX7_PLATFORM_RT}");
        auth.provider().unwrap().get_bearer_token().await.unwrap();
        let body = String::from_utf8_lossy(&server.received_requests().await.unwrap()[0].body)
            .into_owned();
        assert!(!body.contains("tok-cx7-platform-secret"), "{body}");
        assert!(body.contains("CX7_PLATFORM_RT"), "{body}");
    }

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
        assert_eq!(token_url_of(&a), DEFAULT_TOKEN_ENDPOINT);
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
        assert_eq!(token_url_of(&a), "https://t/token");
    }

    fn token_url_of(a: &GoogleWorkspaceAuth) -> &str {
        match a {
            GoogleWorkspaceAuth::RefreshToken { token_url, .. } => token_url,
            other => panic!("not a refresh-token block: {other:?}"),
        }
    }

    fn refresh(url: &str, id: &str, secret: &str, rt: &str) -> GoogleWorkspaceAuth {
        GoogleWorkspaceAuth::RefreshToken {
            token_url: url.into(),
            client_id: id.into(),
            client_secret: secret.into(),
            refresh_token: rt.into(),
        }
    }

    #[test]
    fn same_identity_shares_one_provider_distinct_identities_do_not() {
        let base = refresh(DEFAULT_TOKEN_ENDPOINT, "cid", "cs", "rt-1");
        let p = |a: &GoogleWorkspaceAuth| a.provider().unwrap();
        assert!(Arc::ptr_eq(&p(&base), &p(&base.clone())));

        // Each field of the identity tuple, varied alone, yields its own provider.
        let variants = [
            refresh("https://other.example/token", "cid", "cs", "rt-1"),
            refresh(DEFAULT_TOKEN_ENDPOINT, "cid-2", "cs", "rt-1"),
            refresh(DEFAULT_TOKEN_ENDPOINT, "cid", "cs-2", "rt-1"),
            refresh(DEFAULT_TOKEN_ENDPOINT, "cid", "cs", "rt-2"),
        ];
        for v in &variants {
            assert!(
                !Arc::ptr_eq(&p(&base), &p(v)),
                "a different identity must not share the provider: {v:?}"
            );
        }
    }

    #[test]
    fn to_config_block_round_trips_through_from_node_config() {
        let auth = GoogleWorkspaceAuth::RefreshToken {
            token_url: "https://t/token".into(),
            client_id: "cid".into(),
            client_secret: "cs".into(),
            refresh_token: "rt".into(),
        };
        let host = GoogleWorkspaceAuth::HostRefresh {
            access_token: "tok-cx7-seed".into(),
            expires_at: 1790000000,
            handle: "cth1-cx7-h".into(),
            host: HostTokenContext::default(),
        };
        for auth in [auth, host] {
            let config = serde_json::json!({ "google_workspace_auth": auth.to_config_block() });
            assert_eq!(
                GoogleWorkspaceAuth::from_node_config(&config).unwrap(),
                Some(auth)
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
        assert!(
            err.contains("'oauth2_refresh_token', 'host_refresh_bearer'"),
            "{err}"
        );
    }

    fn host_block(fields: serde_json::Value) -> serde_json::Value {
        let mut b = serde_json::json!({ "type": "host_refresh_bearer" });
        b.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        serde_json::json!({ "google_workspace_auth": b })
    }

    /// A well-formed `host_refresh_bearer` block parses; `Debug` shows neither
    /// the token nor the handle; its identity is `sha256(handle)`, so a renewed
    /// token keeps it.
    #[test]
    fn host_refresh_bearer_parses_redacts_and_keys_by_handle() {
        let parse = |token: &str, handle: &str| {
            let cfg = host_block(serde_json::json!({
                "access_token": token, "expires_at": 1790000000, "handle": handle }));
            GoogleWorkspaceAuth::from_node_config(&cfg)
                .unwrap()
                .unwrap()
        };
        let a = parse("tok-cx7-seed", "cth1-cx7-h");
        assert!(matches!(
            &a,
            GoogleWorkspaceAuth::HostRefresh {
                expires_at: 1790000000,
                ..
            }
        ));
        let dbg = format!("{a:?}");
        assert!(
            !dbg.contains("tok-cx7-") && !dbg.contains("cth1-cx7-"),
            "{dbg}"
        );
        let sha = format!("{:x}", Sha256::digest(b"cth1-cx7-h"));
        assert_eq!(a.identity_key(), sha);
        assert_eq!(
            a.identity_key(),
            parse("tok-cx7-new", "cth1-cx7-h").identity_key()
        );
        assert_ne!(
            a.identity_key(),
            parse("tok-cx7-seed", "cth1-cx7-k").identity_key()
        );
    }

    #[test]
    fn host_refresh_bearer_lists_every_missing_field() {
        let cfg = host_block(serde_json::json!({ "access_token": "tok-cx7-seed", "handle": "" }));
        let err = GoogleWorkspaceAuth::from_node_config(&cfg).unwrap_err();
        assert_eq!(
            err,
            "google_workspace_auth block missing required fields: expires_at, handle"
        );
    }

    /// No port bound: an error that says so, never the platform account.
    #[test]
    fn host_refresh_bearer_without_a_port_is_an_error() {
        let cfg = host_block(serde_json::json!({
            "access_token": "tok-cx7-seed", "expires_at": 1, "handle": "cth1-cx7-h" }));
        let a = GoogleWorkspaceAuth::from_node_config(&cfg)
            .unwrap()
            .unwrap();
        let err = a.provider().err().expect("no provider without a port");
        assert_eq!(
            err,
            "google_workspace_auth: this engine has no host token port"
        );
        let unbound = a.with_host_context(None, Some("sess_1".into()));
        assert!(unbound.provider().is_err());
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
            .unwrap()
            .get_bearer_token()
            .await
            .expect("token from the explicit endpoint");
        assert_eq!(token.as_str(), "ya29.explicit");
    }
}
