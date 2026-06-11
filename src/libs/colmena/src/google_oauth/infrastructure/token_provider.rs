//! Production implementation of [`AuthTokenProvider`].
//!
//! Owns:
//!   1. The credentials (read once at construction from env or
//!      provided directly in tests).
//!   2. The HTTP refresh client.
//!   3. A `tokio::sync::Mutex<Option<CachedToken>>` shared across
//!      requests.
//!
//! The mutex serves two roles:
//!   - It synchronises the read-then-write check on the cache so a
//!     stale or expired token doesn't get observed by one task while
//!     another is mid-refresh.
//!   - It coalesces a thundering herd: when N concurrent tasks all
//!     find the cache empty/expired, only one of them performs the
//!     refresh; the others wait, observe the freshly-written cache,
//!     and return the same token. Cheap because the critical section
//!     is tiny — a couple of Instant comparisons plus an HTTP round
//!     trip during the actual refresh.
//!
//! When Google rotates the refresh token (response contains a new
//! `refresh_token` field), we emit a `WARN` event with
//! `event = "oauth.refresh_token_rotated"`. We do **NOT** persist the
//! new value — by contract the library has no Secret Manager write
//! permission. The OLD refresh token remains valid for some grace
//! period; eventual failures map to [`OAuthError::RefreshTokenRevoked`]
//! and the operator runs the consent flow again.

use crate::google_oauth::domain::{AccessToken, AuthTokenProvider, CachedToken, OAuthError};
use crate::google_oauth::infrastructure::config::OAuthCredentials;
use crate::google_oauth::infrastructure::refresh_client::{RefreshClient, RefreshResponse};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Margin subtracted from the cached token's expiry when deciding
/// whether to reuse it. Any token with less than this remaining is
/// treated as already-expired so an HTTP request in flight cannot race
/// the actual expiry server-side.
const EXPIRY_MARGIN_SECONDS: i64 = 60;

#[derive(Clone)]
pub struct OAuthRefreshTokenProvider {
    creds: OAuthCredentials,
    refresh_client: RefreshClient,
    cache: Arc<Mutex<Option<CachedToken>>>,
}

impl OAuthRefreshTokenProvider {
    /// Build a provider from already-loaded credentials. Use
    /// [`OAuthCredentials::from_env`] in production callers.
    pub fn new(creds: OAuthCredentials) -> Self {
        Self {
            creds,
            refresh_client: RefreshClient::new(),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Test constructor — accepts a custom `RefreshClient` (typically
    /// pointed at wiremock).
    #[cfg(test)]
    pub fn with_refresh_client(creds: OAuthCredentials, refresh_client: RefreshClient) -> Self {
        Self {
            creds,
            refresh_client,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Drop any cached access token so the next `get_bearer_token`
    /// call refreshes from scratch. Used by HTTP-client retry loops
    /// to recover from a 401 (the cached token may have been
    /// revoked server-side; force a fresh exchange of the
    /// refresh_token).
    pub async fn invalidate_cache(&self) {
        let mut guard = self.cache.lock().await;
        *guard = None;
    }
}

#[async_trait]
impl AuthTokenProvider for OAuthRefreshTokenProvider {
    async fn get_bearer_token(&self) -> Result<AccessToken, OAuthError> {
        let mut guard = self.cache.lock().await;

        // Fast path — cache hit with comfortable margin.
        if let Some(cached) = &*guard {
            let margin = ChronoDuration::seconds(EXPIRY_MARGIN_SECONDS);
            if Utc::now() < cached.expires_at - margin {
                return Ok(cached.access_token.clone());
            }
        }

        // Cache miss or near-expiry. Refresh under the lock so
        // concurrent callers wait here and then observe the
        // freshly-cached token.
        let resp = self.refresh_client.refresh(&self.creds).await?;
        if let Some(rotated) = &resp.rotated_refresh_token {
            log_token_rotation(rotated);
        }

        let new_token = build_cached_token(&resp);
        let access = new_token.access_token.clone();
        *guard = Some(new_token);
        Ok(access)
    }
}

/// Construct a `CachedToken` from a refresh response. Always anchors
/// `expires_at` to "now + expires_in" — Google's server-side notion
/// of "now" might differ slightly but the 60-second margin in
/// `EXPIRY_MARGIN_SECONDS` absorbs the skew.
fn build_cached_token(resp: &RefreshResponse) -> CachedToken {
    let expires_at = Utc::now() + ChronoDuration::seconds(resp.expires_in as i64);
    CachedToken {
        access_token: AccessToken(resp.access_token.clone()),
        expires_at,
    }
}

/// Log the rotation event so operators can monitor for it. Structured
/// fields (not log-message string interpolation) so log aggregators
/// can index on the event name. The rotated value itself is NOT
/// logged — only the bare fact that it occurred — because the value
/// is high-sensitivity.
fn log_token_rotation(_rotated_refresh_token: &str) {
    tracing::warn!(
        event = "oauth.refresh_token_rotated",
        "Google rotated the refresh token. Library does not persist the new \
         value (Secret Manager write is out of scope). Old token remains \
         in use until invalidated; monitor for subsequent RefreshTokenRevoked errors."
    );
}

#[cfg(test)]
mod tests {
    //! Tests use wiremock to control the OAuth server side and assert
    //! on observable behaviour (returned tokens, request counts).

    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn creds() -> OAuthCredentials {
        OAuthCredentials::for_tests("CID", "CSEC", "RT")
    }

    /// First call has an empty cache → triggers a refresh and stores
    /// the resulting token.
    #[tokio::test]
    async fn first_call_triggers_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.FIRST","expires_in":3599,"token_type":"Bearer"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OAuthRefreshTokenProvider::with_refresh_client(
            creds(),
            RefreshClient::for_tests(&server.uri()),
        );
        let token = provider.get_bearer_token().await.unwrap();
        assert_eq!(token.as_str(), "ya29.FIRST");
    }

    /// Second call within the cache's TTL must NOT re-hit the
    /// network. wiremock's `expect(1)` enforces this — a second call
    /// would fail loudly when the mock is dropped.
    #[tokio::test]
    async fn second_call_within_ttl_returns_cached() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.CACHED","expires_in":3599,"token_type":"Bearer"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OAuthRefreshTokenProvider::with_refresh_client(
            creds(),
            RefreshClient::for_tests(&server.uri()),
        );
        let t1 = provider.get_bearer_token().await.unwrap();
        let t2 = provider.get_bearer_token().await.unwrap();
        assert_eq!(t1, t2);
        assert_eq!(t1.as_str(), "ya29.CACHED");
    }

    /// A near-expiry cached token (< 60 s remaining) must NOT be
    /// served — the provider refreshes instead. Simulated by seeding
    /// the cache with a token expiring in 10 seconds.
    #[tokio::test]
    async fn near_expiry_triggers_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.REFRESHED","expires_in":3599,"token_type":"Bearer"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OAuthRefreshTokenProvider::with_refresh_client(
            creds(),
            RefreshClient::for_tests(&server.uri()),
        );
        // Seed the cache with a soon-to-expire token.
        {
            let mut guard = provider.cache.lock().await;
            *guard = Some(CachedToken {
                access_token: AccessToken("ya29.STALE".into()),
                expires_at: Utc::now() + ChronoDuration::seconds(10),
            });
        }
        let token = provider.get_bearer_token().await.unwrap();
        // The stale token should NOT have been returned; the new one
        // came from the refresh.
        assert_eq!(token.as_str(), "ya29.REFRESHED");
    }

    /// Ten concurrent callers must coalesce into a single refresh —
    /// the mutex ensures one task does the network round-trip and
    /// the others wait and observe the cached result. wiremock
    /// `expect(1)` is the load-bearing assertion.
    #[tokio::test]
    async fn concurrent_calls_coalesce_into_single_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.SHARED","expires_in":3599,"token_type":"Bearer"}"#,
            ))
            .expect(1) // EXACTLY one refresh request even with 10 callers.
            .mount(&server)
            .await;

        let provider = Arc::new(OAuthRefreshTokenProvider::with_refresh_client(
            creds(),
            RefreshClient::for_tests(&server.uri()),
        ));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let p = provider.clone();
            handles.push(tokio::spawn(async move { p.get_bearer_token().await }));
        }
        for h in handles {
            let token = h.await.unwrap().unwrap();
            assert_eq!(token.as_str(), "ya29.SHARED");
        }
    }

    /// When Google rotates the refresh token, the provider proceeds
    /// without persisting the rotated value and DOES NOT crash. The
    /// rotation is logged (verified manually via tracing); the
    /// behavioural assertion here is that the call succeeds and
    /// returns the new access_token.
    #[tokio::test]
    async fn rotated_refresh_token_is_handled_without_persistence() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.ROTATED","expires_in":3599,
                    "refresh_token":"1//NEW-ROTATED-VALUE","token_type":"Bearer"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OAuthRefreshTokenProvider::with_refresh_client(
            creds(),
            RefreshClient::for_tests(&server.uri()),
        );
        let token = provider.get_bearer_token().await.unwrap();
        assert_eq!(token.as_str(), "ya29.ROTATED");
        // The credentials in the provider must STILL hold the
        // original refresh_token, not the rotated one — we don't
        // mutate state outside the cache.
        assert_eq!(provider.creds.refresh_token.expose(), "RT");
    }

    /// When the refresh fails non-transiently, the cache must be
    /// left empty so subsequent calls retry from scratch rather than
    /// serving a stale token.
    #[tokio::test]
    async fn failed_refresh_leaves_cache_empty() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string(r#"{"error":"invalid_grant","error_description":"revoked"}"#),
            )
            .mount(&server)
            .await;

        let provider = OAuthRefreshTokenProvider::with_refresh_client(
            creds(),
            RefreshClient::for_tests(&server.uri()),
        );
        let err = provider.get_bearer_token().await.unwrap_err();
        assert_eq!(err, OAuthError::RefreshTokenRevoked);

        // Cache should be None — nothing to serve to the next caller.
        let guard = provider.cache.lock().await;
        assert!(
            guard.is_none(),
            "cache must remain empty after failed refresh"
        );
    }
}
