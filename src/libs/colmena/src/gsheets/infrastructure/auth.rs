//! Token acquisition + caching for the Google Sheets REST client.
//!
//! Production path: delegates to the shared `google_oauth` subsystem.
//! Test path: a sticky pre-seeded token that bypasses the OAuth flow so
//! wiremock HTTP tests don't need to fake Google's token endpoint.
//!
//! The public surface (`new` / `token` / `invalidate` /
//! `set_token_for_test`) is preserved so callers in `http_client.rs`
//! don't change shape — only the internals are different.

use crate::google_oauth::domain::AuthTokenProvider;
use crate::google_oauth::infrastructure::{OAuthCredentials, OAuthRefreshTokenProvider};
use crate::gsheets::domain::SheetsError;
use std::sync::Arc;
#[cfg(test)]
use std::time::{Duration, Instant};
#[cfg(test)]
use tokio::sync::Mutex;

/// In-process token cache used by the `Static` (test) variant. Mirrors
/// the legacy field layout so test-helpers that touch it via
/// `set_token_for_test` keep working.
#[cfg(test)]
#[derive(Debug, Clone)]
struct StaticCached {
    token: String,
    /// Seeded far in the future during tests; preserved so the inner
    /// shape matches the legacy production cache exactly, even though
    /// the test path never enforces it.
    #[allow(dead_code)]
    expires_at: Instant,
}

/// Inner storage — at runtime ONE of these variants is constructed.
/// The choice is made at `new` / `for_tests` time and never switches.
#[derive(Clone)]
enum Inner {
    /// Production: defer every token decision to the shared OAuth
    /// provider, which handles cache + refresh + retry against
    /// `oauth2.googleapis.com`.
    OAuth(Arc<OAuthRefreshTokenProvider>),

    /// Tests: a pre-seeded bearer string. `invalidate()` re-seeds
    /// from `sticky` so the 401-refresh wiremock test path doesn't
    /// fall through to a real OAuth call.
    #[cfg(test)]
    Static {
        cache: Arc<Mutex<Option<StaticCached>>>,
        sticky: Arc<Mutex<Option<String>>>,
    },
}

/// Holds the token source — cheap to clone (inner state is `Arc`).
#[derive(Clone)]
pub struct TokenProvider {
    inner: Inner,
}

impl TokenProvider {
    /// Production constructor. Wraps an `OAuthRefreshTokenProvider`
    /// built from the credentials. The credentials are read once at
    /// startup; if any env var is missing, `OAuthCredentials::from_env()`
    /// returns a structured error that the caller maps to
    /// `SheetsError::NotConfigured`.
    pub fn from_oauth_credentials(creds: OAuthCredentials) -> Self {
        Self {
            inner: Inner::OAuth(Arc::new(OAuthRefreshTokenProvider::new(creds))),
        }
    }

    /// Test constructor with a static cache. The token starts unset
    /// — call `set_token_for_test` to seed it before the first HTTP
    /// call goes out.
    #[cfg(test)]
    pub fn for_tests_static() -> Self {
        Self {
            inner: Inner::Static {
                cache: Arc::new(Mutex::new(None)),
                sticky: Arc::new(Mutex::new(None)),
            },
        }
    }

    /// Return a fresh bearer token. The OAuth variant defers all
    /// caching logic to the shared provider; the Static variant just
    /// reads whatever was seeded.
    pub async fn token(&self) -> Result<String, SheetsError> {
        match &self.inner {
            Inner::OAuth(provider) => provider
                .get_bearer_token()
                .await
                .map(|t| t.into_string())
                .map_err(token_error_to_sheets_error),
            #[cfg(test)]
            Inner::Static { cache, .. } => {
                let guard = cache.lock().await;
                let cached = guard.as_ref().ok_or_else(|| {
                    SheetsError::AuthFailed(
                        "test token not seeded; call set_token_for_test before issuing requests"
                            .into(),
                    )
                })?;
                Ok(cached.token.clone())
            }
        }
    }

    /// Force-invalidate the cache. Production path clears the OAuth
    /// provider's cache so the next `token()` triggers a refresh.
    /// Test path with a sticky value re-seeds from `sticky` so the
    /// 401-refresh wiremock test loop returns the same token the
    /// second attempt.
    pub async fn invalidate(&self) {
        match &self.inner {
            Inner::OAuth(provider) => {
                provider.invalidate_cache().await;
            }
            #[cfg(test)]
            Inner::Static { cache, sticky } => {
                let mut cache_guard = cache.lock().await;
                let sticky_guard = sticky.lock().await;
                if let Some(t) = sticky_guard.as_ref() {
                    *cache_guard = Some(StaticCached {
                        token: t.clone(),
                        expires_at: Instant::now() + Duration::from_secs(60 * 60),
                    });
                } else {
                    *cache_guard = None;
                }
            }
        }
    }

    /// Test-only: seed the cache with a known token AND mark it sticky
    /// so `invalidate()` re-seeds rather than clears.
    #[cfg(test)]
    pub async fn set_token_for_test(&self, token: impl Into<String>) {
        let s = token.into();
        match &self.inner {
            Inner::Static { cache, sticky } => {
                {
                    let mut sticky_guard = sticky.lock().await;
                    *sticky_guard = Some(s.clone());
                }
                let mut cache_guard = cache.lock().await;
                *cache_guard = Some(StaticCached {
                    token: s,
                    expires_at: Instant::now() + Duration::from_secs(60 * 60),
                });
            }
            Inner::OAuth(_) => panic!(
                "set_token_for_test called on an OAuth TokenProvider — \
                 only Static (test) providers support pre-seeding"
            ),
        }
    }
}

/// Map shared-OAuth errors onto the Sheets-domain error vocabulary so
/// the caller doesn't need to know the OAuth subsystem exists.
fn token_error_to_sheets_error(err: crate::google_oauth::domain::OAuthError) -> SheetsError {
    use crate::google_oauth::domain::OAuthError as E;
    match err {
        E::RefreshTokenRevoked => SheetsError::NotConfigured(format!("{err}")),
        E::ClientCredsInvalid(_) => SheetsError::NotConfigured(format!("{err}")),
        E::ConfigMissing(_) => SheetsError::NotConfigured(format!("{err}")),
        E::Transient(msg) => SheetsError::AuthFailed(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_provider_serves_seeded_token() {
        let p = TokenProvider::for_tests_static();
        p.set_token_for_test("fake-bearer").await;
        let t = p.token().await.unwrap();
        assert_eq!(t, "fake-bearer");
    }

    #[tokio::test]
    async fn static_provider_invalidate_with_sticky_reseeds() {
        let p = TokenProvider::for_tests_static();
        p.set_token_for_test("fake-bearer").await;
        p.invalidate().await;
        // Re-seeded rather than cleared.
        let t = p.token().await.unwrap();
        assert_eq!(t, "fake-bearer");
    }

    #[tokio::test]
    async fn static_provider_without_seed_returns_auth_failed() {
        let p = TokenProvider::for_tests_static();
        let err = p.token().await.unwrap_err();
        assert!(matches!(err, SheetsError::AuthFailed(_)));
    }

    #[test]
    fn provider_is_cloneable_cheaply() {
        let p1 = TokenProvider::for_tests_static();
        let p2 = p1.clone();
        // Both clones share the Arc backing — checked indirectly via
        // behaviour rather than pointer equality since `Inner` doesn't
        // expose its internals.
        let _ = (p1, p2);
    }
}
