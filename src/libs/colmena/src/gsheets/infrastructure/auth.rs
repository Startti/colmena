//! Token acquisition + caching for the Google Sheets REST client.
//!
//! Pattern mirrors `dag_engine::infrastructure::nodes::image_generation`
//! (see `get_vertex_token` around line 572) — same `yup-oauth2` ADC
//! flow, conservative 50-min cache (Google tokens last ~1h).

use crate::gsheets::domain::SheetsError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Holds a token cache + the configured scopes. Cheap to clone — the
/// inner state is an `Arc<Mutex<_>>`.
#[derive(Clone)]
pub struct TokenProvider {
    cache: Arc<Mutex<Option<CachedToken>>>,
    scopes: Vec<String>,
}

impl TokenProvider {
    pub fn new(scopes: Vec<String>) -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            scopes,
        }
    }

    /// Returns a fresh bearer token, hitting `yup-oauth2` only when the
    /// cache is empty or within 60s of expiry.
    pub async fn token(&self) -> Result<String, SheetsError> {
        use yup_oauth2::authenticator::ApplicationDefaultCredentialsTypes;
        use yup_oauth2::{
            ApplicationDefaultCredentialsAuthenticator, ApplicationDefaultCredentialsFlowOpts,
        };

        let mut cache = self.cache.lock().await;
        if let Some(c) = &*cache {
            if c.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(c.token.clone());
            }
        }

        let opts = ApplicationDefaultCredentialsFlowOpts::default();
        let auth = match ApplicationDefaultCredentialsAuthenticator::builder(opts).await {
            ApplicationDefaultCredentialsTypes::InstanceMetadata(builder) => builder
                .build()
                .await
                .map_err(|e| SheetsError::AuthFailed(format!("metadata server: {e}")))?,
            ApplicationDefaultCredentialsTypes::ServiceAccount(builder) => builder
                .build()
                .await
                .map_err(|e| SheetsError::AuthFailed(format!("service account: {e}")))?,
        };

        let scope_refs: Vec<&str> = self.scopes.iter().map(String::as_str).collect();
        let token = auth.token(&scope_refs).await.map_err(|e| {
            let msg = e.to_string();
            if msg.to_lowercase().contains("no credentials")
                || msg.contains("GOOGLE_APPLICATION_CREDENTIALS")
            {
                SheetsError::NotConfigured(
                    "no Google credentials found — set \
                     GOOGLE_APPLICATION_CREDENTIALS to a service-account JSON \
                     path, or run `gcloud auth application-default login` for \
                     local dev"
                        .to_string(),
                )
            } else {
                SheetsError::AuthFailed(msg)
            }
        })?;
        let access = token
            .token()
            .ok_or_else(|| SheetsError::AuthFailed("empty access token".to_string()))?
            .to_string();

        let expires_at = Instant::now() + Duration::from_secs(50 * 60);
        *cache = Some(CachedToken {
            token: access.clone(),
            expires_at,
        });
        Ok(access)
    }

    /// Force-invalidate the cache. Called by the HTTP client after a 401
    /// to trigger refresh on the retry.
    pub async fn invalidate(&self) {
        let mut cache = self.cache.lock().await;
        *cache = None;
    }

    /// Test-only: seed the cache with a known token so HTTP tests don't
    /// hit yup-oauth2. Available only under `#[cfg(test)]`.
    #[cfg(test)]
    pub async fn set_token_for_test(&self, token: impl Into<String>) {
        let mut cache = self.cache.lock().await;
        *cache = Some(CachedToken {
            token: token.into(),
            expires_at: Instant::now() + Duration::from_secs(60 * 60),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_is_cloneable_cheaply() {
        // Sanity: cloning shares the same cache (Arc).
        let p1 = TokenProvider::new(vec!["scope1".into()]);
        let p2 = p1.clone();
        assert!(Arc::ptr_eq(&p1.cache, &p2.cache));
    }
}
