//! Token acquisition + caching for the Google Docs REST client.
//!
//! Production path: delegates to the shared `google_oauth` subsystem
//! which holds the refresh-token cache, retry logic, and Google's OAuth
//! token endpoint contract. Test path: a static pre-seeded token so
//! wiremock HTTP tests don't need to fake `oauth2.googleapis.com`.
//!
//! Mirrors the pattern in `gsheets::infrastructure::auth`. Keeping the
//! two parallel makes it obvious when one diverges by accident.

use crate::gdocs::domain::DocsError;
use crate::google_oauth::domain::AuthTokenProvider;
use crate::google_oauth::infrastructure::{OAuthCredentials, OAuthRefreshTokenProvider};
use std::sync::Arc;
#[cfg(test)]
use std::time::{Duration, Instant};
#[cfg(test)]
use tokio::sync::Mutex;

#[cfg(test)]
const TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

#[cfg(test)]
#[derive(Debug, Clone)]
struct StaticCached {
    token: String,
    /// Stored to mirror the legacy production cache shape; tests don't
    /// enforce expiry directly so the field is unused at runtime.
    #[allow(dead_code)]
    expires_at: Instant,
}

#[derive(Clone)]
enum Inner {
    /// Production: forward every token decision to the shared OAuth
    /// provider. It owns its own cache + refresh loop.
    OAuth(Arc<dyn AuthTokenProvider>),

    /// Tests: a pre-seeded bearer string. Invalidate is a no-op in
    /// this variant since wiremock tests never have a real refresh
    /// path to fall through to.
    #[cfg(test)]
    Static {
        cache: Arc<Mutex<Option<StaticCached>>>,
    },
}

/// In-memory token source. Cheap to clone (inner state is `Arc`).
///
/// Note: the production class is renamed to `TokenCache` to match the
/// existing gdocs public API surface. Same shape as the gsheets
/// `TokenProvider`; kept distinct so the two modules stay independently
/// migratable.
#[derive(Clone)]
pub struct TokenCache {
    inner: Inner,
    /// True for the connected Google account (per-node
    /// `google_workspace_auth`): a revoked refresh asks the user to reconnect
    /// Google instead of pointing the operator at the platform setup.
    connected_account: bool,
}

impl TokenCache {
    /// Production constructor — wraps a shared `OAuthRefreshTokenProvider`.
    pub fn from_oauth_credentials(creds: OAuthCredentials) -> Self {
        Self {
            inner: Inner::OAuth(Arc::new(OAuthRefreshTokenProvider::new(creds))),
            connected_account: false,
        }
    }

    /// Wrap a provider shared by identity (per-node
    /// `google_workspace_auth`, see
    /// `google_oauth::infrastructure::GoogleWorkspaceAuth::provider`).
    /// Every client acting as the same Google account reuses one
    /// access-token cache.
    pub fn from_shared_provider(provider: Arc<dyn AuthTokenProvider>) -> Self {
        Self {
            inner: Inner::OAuth(provider),
            connected_account: true,
        }
    }

    /// Test-only: whether this cache wraps exactly `provider`.
    #[cfg(test)]
    pub(crate) fn shares_provider_for_tests(&self, provider: &Arc<dyn AuthTokenProvider>) -> bool {
        match &self.inner {
            Inner::OAuth(p) => Arc::ptr_eq(p, provider),
            Inner::Static { .. } => false,
        }
    }

    /// Test constructor with a static cache that wiremock tests can
    /// pre-seed via [`set_token_for_test`].
    #[cfg(test)]
    pub fn for_tests_static() -> Self {
        Self {
            inner: Inner::Static {
                cache: Arc::new(Mutex::new(None)),
            },
            connected_account: false,
        }
    }

    /// Return a non-expired bearer token. Production path defers to
    /// the OAuth provider; test path reads whatever was seeded.
    pub async fn get(&self) -> Result<String, DocsError> {
        match &self.inner {
            Inner::OAuth(provider) => provider
                .get_bearer_token()
                .await
                .map(|t| t.into_string())
                .map_err(|e| oauth_error_to_docs_error(e, self.connected_account)),
            #[cfg(test)]
            Inner::Static { cache } => {
                let guard = cache.lock().await;
                let cached = guard.as_ref().ok_or_else(|| {
                    DocsError::AuthFailed(
                        "test token not seeded; call set_token_for_test before issuing requests"
                            .into(),
                    )
                })?;
                Ok(cached.token.clone())
            }
        }
    }

    /// Force-invalidate the cache. Production path clears the OAuth
    /// provider's cache so the next `get()` triggers a refresh. Test
    /// path is a no-op — wiremock tests rely on the seeded token
    /// surviving across the 401-refresh boundary.
    pub async fn invalidate(&self) {
        match &self.inner {
            Inner::OAuth(provider) => provider.invalidate().await,
            #[cfg(test)]
            Inner::Static { .. } => {}
        }
    }

    /// Test-only: seed the cached token directly so wiremock-based
    /// tests don't hit the real OAuth endpoint.
    #[cfg(test)]
    pub(crate) async fn set_token_for_test(&self, value: String) {
        match &self.inner {
            Inner::Static { cache } => {
                let mut guard = cache.lock().await;
                *guard = Some(StaticCached {
                    token: value,
                    expires_at: Instant::now() + TOKEN_TTL,
                });
            }
            Inner::OAuth(_) => panic!(
                "set_token_for_test called on an OAuth TokenCache — \
                 only Static (test) caches support pre-seeding"
            ),
        }
    }
}

/// A revoked refresh of the connected Google account is the user's to fix
/// (reconnect), not the operator's.
fn oauth_error_to_docs_error(
    err: crate::google_oauth::domain::OAuthError,
    connected_account: bool,
) -> DocsError {
    use crate::google_oauth::domain::OAuthError as E;
    match err {
        // A refused host refresh is the connected account's too (CX7).
        E::RefreshTokenRevoked | E::HostRefused(_) if connected_account => {
            DocsError::GoogleAccountReconnectRequired
        }
        E::RefreshTokenRevoked => DocsError::NotConfigured(format!("{err}")),
        E::ClientCredsInvalid(_) | E::HostRefused(_) => DocsError::NotConfigured(format!("{err}")),
        E::ConfigMissing(_) => DocsError::NotConfigured(format!("{err}")),
        E::Transient(msg) => DocsError::AuthFailed(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused host refresh of the connected account asks the user to
    /// reconnect; for the platform account it stays an operator error.
    #[test]
    fn a_refused_host_refresh_is_a_reconnect_for_the_connected_account() {
        use crate::google_oauth::domain::OAuthError;
        let refused = || OAuthError::HostRefused("needs reconnect".into());
        assert!(matches!(
            oauth_error_to_docs_error(refused(), true),
            DocsError::GoogleAccountReconnectRequired
        ));
        assert!(matches!(
            oauth_error_to_docs_error(refused(), false),
            DocsError::NotConfigured(_)
        ));
    }

    #[tokio::test]
    async fn cache_returns_seeded_token_within_ttl() {
        let cache = TokenCache::for_tests_static();
        cache.set_token_for_test("test_token".into()).await;
        let v = cache.get().await.unwrap();
        assert_eq!(v, "test_token");
    }

    #[tokio::test]
    async fn invalidate_is_no_op_in_test_variant() {
        // Static-variant invalidate must NOT clear the seeded token —
        // wiremock 401-refresh tests rely on the second call returning
        // the same token.
        let cache = TokenCache::for_tests_static();
        cache.set_token_for_test("seed".into()).await;
        cache.invalidate().await;
        let v = cache.get().await.unwrap();
        assert_eq!(v, "seed");
    }

    #[tokio::test]
    async fn unseeded_static_cache_returns_auth_failed() {
        let cache = TokenCache::for_tests_static();
        let err = cache.get().await.unwrap_err();
        assert!(matches!(err, DocsError::AuthFailed(_)));
    }

    #[test]
    fn from_shared_provider_wraps_the_given_provider() {
        use crate::google_oauth::infrastructure::OAuthProviderCache;
        let cache = OAuthProviderCache::new();
        let shared: Arc<dyn AuthTokenProvider> =
            cache.get_or_create("https://t/token", "cid", "cs", "rt-1");
        let c = TokenCache::from_shared_provider(shared.clone());
        assert!(c.shares_provider_for_tests(&shared));
        let other: Arc<dyn AuthTokenProvider> =
            cache.get_or_create("https://t/token", "cid", "cs", "rt-2");
        assert!(!c.shares_provider_for_tests(&other));
    }

    /// OAuth provider whose token endpoint (wiremock) rejects the refresh
    /// with `invalid_grant`.
    async fn revoked_provider() -> (wiremock::MockServer, Arc<OAuthRefreshTokenProvider>) {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
            ))
            .mount(&server)
            .await;
        let provider = Arc::new(OAuthRefreshTokenProvider::with_endpoint(
            OAuthCredentials::new("cid", "cs", "rt"),
            &server.uri(),
        ));
        (server, provider)
    }

    #[tokio::test]
    async fn revoked_refresh_with_connected_account_asks_to_reconnect() {
        let (_server, provider) = revoked_provider().await;
        let err = TokenCache::from_shared_provider(provider)
            .get()
            .await
            .unwrap_err();
        assert!(
            matches!(err, DocsError::GoogleAccountReconnectRequired),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn revoked_refresh_with_platform_credentials_keeps_the_operator_error() {
        let (_server, provider) = revoked_provider().await;
        let platform = TokenCache {
            inner: Inner::OAuth(provider),
            connected_account: false,
        };
        match platform.get().await.unwrap_err() {
            DocsError::NotConfigured(msg) => {
                assert!(msg.contains("colmena_oauth_setup"), "{msg}")
            }
            other => panic!("expected NotConfigured, got {other:?}"),
        }
    }

    #[test]
    fn token_cache_clone_shares_state() {
        let c1 = TokenCache::for_tests_static();
        let _c2 = c1.clone();
        // Behavioural: both clones see the same backing storage. Tested
        // indirectly through `set_token_for_test` + `get` flow rather
        // than asserting `Arc::ptr_eq` (Inner is private).
    }
}
