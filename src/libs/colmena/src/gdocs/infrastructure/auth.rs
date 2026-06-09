//! ADC + SA JSON token acquisition via `yup-oauth2`. Token cached
//! in-memory with 50-min TTL (Google tokens last ~60 min).
//!
//! Mirrors `crate::gsheets::infrastructure::auth` — same `yup-oauth2`
//! ADC flow as `dag_engine::infrastructure::nodes::image_generation`
//! (`get_vertex_token`, around line 572).

use crate::gdocs::domain::DocsError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const TOKEN_TTL: Duration = Duration::from_secs(50 * 60);
/// Refresh proactively when the cached token is within this many seconds
/// of expiry, to avoid racing the 1-hour Google deadline.
const REFRESH_LEEWAY: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// In-memory token cache + configured scopes. Cheap to clone — the
/// inner state lives in an `Arc<Mutex<_>>`.
#[derive(Clone)]
pub struct TokenCache {
    cache: Arc<Mutex<Option<CachedToken>>>,
    scopes: Vec<String>,
}

impl TokenCache {
    pub fn new(scopes: Vec<String>) -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            scopes,
        }
    }

    /// Return a non-expired bearer token. Hits `yup-oauth2` only when
    /// the cache is empty or within [`REFRESH_LEEWAY`] of expiry.
    pub async fn get(&self) -> Result<String, DocsError> {
        let mut cache = self.cache.lock().await;
        if let Some(c) = &*cache {
            if c.expires_at > Instant::now() + REFRESH_LEEWAY {
                return Ok(c.token.clone());
            }
        }

        let scope_refs: Vec<&str> = self.scopes.iter().map(String::as_str).collect();
        let access = acquire_token(&scope_refs).await?;
        *cache = Some(CachedToken {
            token: access.clone(),
            expires_at: Instant::now() + TOKEN_TTL,
        });
        Ok(access)
    }

    /// Force-invalidate the cache. Called by the HTTP client after a
    /// 401 to trigger refresh on the retry.
    pub async fn invalidate(&self) {
        let mut cache = self.cache.lock().await;
        *cache = None;
    }

    /// Test-only: seed the cached token directly so wiremock-based
    /// tests don't hit the real ADC endpoint.
    #[cfg(test)]
    pub(crate) async fn set_token_for_test(&self, value: String) {
        let mut guard = self.cache.lock().await;
        *guard = Some(CachedToken {
            token: value,
            expires_at: Instant::now() + TOKEN_TTL,
        });
    }
}

async fn acquire_token(scopes: &[&str]) -> Result<String, DocsError> {
    use yup_oauth2::authenticator::ApplicationDefaultCredentialsTypes;
    use yup_oauth2::{
        ApplicationDefaultCredentialsAuthenticator, ApplicationDefaultCredentialsFlowOpts,
    };

    let opts = ApplicationDefaultCredentialsFlowOpts::default();
    let auth = match ApplicationDefaultCredentialsAuthenticator::builder(opts).await {
        ApplicationDefaultCredentialsTypes::InstanceMetadata(builder) => builder
            .build()
            .await
            .map_err(|e| DocsError::AuthFailed(format!("metadata server: {e}")))?,
        ApplicationDefaultCredentialsTypes::ServiceAccount(builder) => builder
            .build()
            .await
            .map_err(|e| DocsError::AuthFailed(format!("service account: {e}")))?,
    };

    let token = auth.token(scopes).await.map_err(|e| {
        let msg = e.to_string();
        if msg.to_lowercase().contains("no credentials")
            || msg.contains("GOOGLE_APPLICATION_CREDENTIALS")
        {
            DocsError::NotConfigured(
                "no Google credentials found — set \
                 GOOGLE_APPLICATION_CREDENTIALS to a service-account JSON \
                 path, or run `gcloud auth application-default login` for \
                 local dev"
                    .to_string(),
            )
        } else {
            DocsError::AuthFailed(msg)
        }
    })?;
    Ok(token
        .token()
        .ok_or_else(|| DocsError::AuthFailed("empty access token".to_string()))?
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_returns_seeded_token_within_ttl() {
        let cache = TokenCache::new(vec!["https://www.googleapis.com/auth/documents".to_string()]);
        // Seed the cache directly so we don't hit Google.
        {
            let mut guard = cache.cache.lock().await;
            *guard = Some(CachedToken {
                token: "test_token".into(),
                expires_at: Instant::now() + TOKEN_TTL,
            });
        }
        let v = cache.get().await.unwrap();
        assert_eq!(v, "test_token");
    }

    #[tokio::test]
    async fn invalidate_clears_cached_token() {
        let cache = TokenCache::new(vec!["scope".into()]);
        {
            let mut guard = cache.cache.lock().await;
            *guard = Some(CachedToken {
                token: "stale".into(),
                expires_at: Instant::now() + TOKEN_TTL,
            });
        }
        cache.invalidate().await;
        let guard = cache.cache.lock().await;
        assert!(guard.is_none());
    }

    #[test]
    fn token_cache_clone_shares_state() {
        let c1 = TokenCache::new(vec!["scope".into()]);
        let c2 = c1.clone();
        assert!(Arc::ptr_eq(&c1.cache, &c2.cache));
    }
}
