//! Process-wide cache of `OAuthRefreshTokenProvider`s keyed by a hash of
//! the credentials. Guarantees that all http_request nodes/tool-calls
//! sharing one identity (same token_url + client_id + refresh_token) reuse
//! a single provider — hence a single access-token cache and a single mint.
//!
//! Injected into `HttpNode` at construction in `registry.rs`, same pattern
//! as `with_storage`.

use crate::google_oauth::infrastructure::{OAuthCredentials, OAuthRefreshTokenProvider};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Maps a credential fingerprint to a shared provider.
#[derive(Default)]
pub struct OAuthProviderCache {
    inner: Mutex<HashMap<String, Arc<OAuthRefreshTokenProvider>>>,
}

impl OAuthProviderCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// SHA-256 hex of the identity tuple. The refresh token is hashed, never
    /// embedded in clear — the key may appear in debug dumps of the map.
    pub fn fingerprint(
        token_url: &str,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> String {
        let mut h = Sha256::new();
        h.update(token_url.as_bytes());
        h.update([0u8]);
        h.update(client_id.as_bytes());
        h.update([0u8]);
        h.update(client_secret.as_bytes());
        h.update([0u8]);
        h.update(refresh_token.as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Return the shared provider for these creds, creating it on first use.
    pub fn get_or_create(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Arc<OAuthRefreshTokenProvider> {
        let fp = Self::fingerprint(token_url, client_id, client_secret, refresh_token);
        let mut guard = self
            .inner
            .lock()
            .expect("oauth provider cache mutex poisoned");
        if let Some(p) = guard.get(&fp) {
            return p.clone();
        }
        let creds = OAuthCredentials::new(client_id, client_secret, refresh_token);
        let provider = Arc::new(OAuthRefreshTokenProvider::with_endpoint(creds, token_url));
        guard.insert(fp, provider.clone());
        provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_creds_return_same_provider_arc() {
        let cache = OAuthProviderCache::new();
        let a = cache.get_or_create("https://t/token", "cid", "csec", "rt");
        let b = cache.get_or_create("https://t/token", "cid", "csec", "rt");
        assert!(Arc::ptr_eq(&a, &b), "same creds must share one provider");
    }

    #[test]
    fn different_creds_return_different_providers() {
        let cache = OAuthProviderCache::new();
        let a = cache.get_or_create("https://t/token", "cid", "csec", "rt1");
        let b = cache.get_or_create("https://t/token", "cid", "csec", "rt2");
        assert!(
            !Arc::ptr_eq(&a, &b),
            "different refresh tokens => different providers"
        );
    }

    #[test]
    fn fingerprint_does_not_contain_plaintext_refresh_token() {
        let fp = OAuthProviderCache::fingerprint("https://t/token", "cid", "csec", "1//SECRET");
        assert!(
            !fp.contains("1//SECRET"),
            "fingerprint must hash, not embed, the refresh token"
        );
    }
}
