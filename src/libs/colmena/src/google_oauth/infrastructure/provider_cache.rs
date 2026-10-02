//! Process-wide cache of `OAuthRefreshTokenProvider`s keyed by a hash of
//! the credentials. Guarantees that all http_request nodes/tool-calls
//! sharing one identity (same token_url + client_id + client_secret +
//! refresh_token) reuse a single provider — hence a single access-token
//! cache and a single mint.
//!
//! Bounded: at [`MAX_CACHED_PROVIDERS`] entries, an insert first drops every
//! provider nobody else holds (only the cache's own `Arc`). Dropping one only
//! costs a fresh token mint on its next use; it also stops keeping a refresh
//! token of an identity no longer in use in memory. Providers still held by a
//! client are never dropped, so the map may exceed the bound while that many
//! identities are in flight at once. While it stays at or above the bound,
//! every insert sweeps the whole map (a linear pass under the lock); that is
//! only reached with 1024+ distinct identities held at the same time.
//!
//! Injected into `HttpNode` at construction in `registry.rs`, same pattern
//! as `with_storage`.

use crate::google_oauth::infrastructure::{OAuthCredentials, OAuthRefreshTokenProvider};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Entries kept before an insert sweeps out the providers nobody holds.
pub const MAX_CACHED_PROVIDERS: usize = 1024;

/// Maps a credential fingerprint to a shared provider.
pub struct OAuthProviderCache {
    inner: Mutex<HashMap<String, Arc<OAuthRefreshTokenProvider>>>,
    max_entries: usize,
}

impl Default for OAuthProviderCache {
    fn default() -> Self {
        Self::with_max_entries(MAX_CACHED_PROVIDERS)
    }
}

impl OAuthProviderCache {
    /// Create an empty cache bounded by [`MAX_CACHED_PROVIDERS`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty cache with a custom bound (tests).
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max_entries,
        }
    }

    /// Number of cached providers.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("oauth provider cache mutex poisoned")
            .len()
    }

    /// Whether the cache holds no provider.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        if guard.len() >= self.max_entries {
            guard.retain(|_, p| Arc::strong_count(p) > 1);
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
    fn at_the_bound_unheld_providers_are_dropped_and_held_ones_kept() {
        let cache = OAuthProviderCache::with_max_entries(2);
        let held = cache.get_or_create("https://t/token", "cid", "cs", "rt-held");
        drop(cache.get_or_create("https://t/token", "cid", "cs", "rt-idle"));
        assert_eq!(cache.len(), 2);
        let _third = cache.get_or_create("https://t/token", "cid", "cs", "rt-third");
        assert_eq!(cache.len(), 2, "the idle provider was swept");
        let again = cache.get_or_create("https://t/token", "cid", "cs", "rt-held");
        assert!(
            Arc::ptr_eq(&held, &again),
            "a held provider is never dropped"
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
