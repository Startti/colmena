//! Process-global TTL cache of pre-flight provider credential-check results.
//!
//! Mirrors `gdocs::infrastructure::outline_cache::OutlineCache` (same
//! `Mutex<HashMap<Key, Entry>>` + `Instant`-based TTL shape). Used by
//! `preflight::validate_graph_providers` so a graph that re-enters the DAG
//! engine repeatedly within a single conversation (every ADP turn) doesn't
//! re-hit each provider's authenticated `/models` endpoint on every run —
//! only the first turn pays the live check; the rest hit the cache.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::llm::domain::ProviderKind;

#[derive(Debug, Clone)]
struct Entry {
    /// `Ok(())` on a valid key, `Err(reason)` on a rejected one. Both
    /// outcomes are cached under the same TTL (v1 — see module docs on the
    /// deferred "shorter TTL for errors" follow-up).
    result: Result<(), String>,
    checked_at: Instant,
}

/// In-process cache of `(provider, api_key) -> last credential-check result`.
///
/// `ProviderKind` has no `Hash` impl, so the key uses its `Display` string
/// (stable, cheap) paired with the raw api_key string.
pub struct PreflightCache {
    ttl: Duration,
    map: Mutex<HashMap<(String, String), Entry>>,
}

impl PreflightCache {
    /// Construct an empty cache with the given entry TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            map: Mutex::new(HashMap::new()),
        }
    }

    fn key(provider: &ProviderKind, api_key: &str) -> (String, String) {
        (provider.to_string(), api_key.to_string())
    }

    /// Returns the cached result if it is younger than the TTL. `None` means
    /// "no fresh entry — check live".
    pub fn get_fresh(&self, provider: &ProviderKind, api_key: &str) -> Option<Result<(), String>> {
        let guard = self.map.lock().expect("preflight cache mutex poisoned");
        guard.get(&Self::key(provider, api_key)).and_then(|entry| {
            if entry.checked_at.elapsed() < self.ttl {
                Some(entry.result.clone())
            } else {
                None
            }
        })
    }

    /// Insert or replace the cached result for `(provider, api_key)`, stamped
    /// with the current time.
    pub fn put(&self, provider: &ProviderKind, api_key: &str, result: Result<(), String>) {
        let mut guard = self.map.lock().expect("preflight cache mutex poisoned");
        guard.insert(
            Self::key(provider, api_key),
            Entry {
                result,
                checked_at: Instant::now(),
            },
        );
    }
}

// ── Process-wide singleton ──────────────────────────────────────────────

static CACHE: tokio::sync::OnceCell<Arc<PreflightCache>> = tokio::sync::OnceCell::const_new();

/// Default TTL (seconds) when `COLMENA_PREFLIGHT_HEALTH_TTL_SECS` is unset or unparsable.
const DEFAULT_TTL_SECS: u64 = 60;

/// Returns the process-wide `PreflightCache`, built once on first access with
/// the TTL from `COLMENA_PREFLIGHT_HEALTH_TTL_SECS` (default 60s).
pub async fn shared_preflight_cache() -> Arc<PreflightCache> {
    CACHE
        .get_or_init(|| async {
            let ttl_secs = std::env::var("COLMENA_PREFLIGHT_HEALTH_TTL_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(DEFAULT_TTL_SECS);
            Arc::new(PreflightCache::new(Duration::from_secs(ttl_secs)))
        })
        .await
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get_within_ttl_returns_cached_ok() {
        let cache = PreflightCache::new(Duration::from_secs(60));
        cache.put(&ProviderKind::OpenAi, "k1", Ok(()));

        let cached = cache.get_fresh(&ProviderKind::OpenAi, "k1");
        assert_eq!(cached, Some(Ok(())));
    }

    #[test]
    fn put_then_get_within_ttl_returns_cached_err() {
        let cache = PreflightCache::new(Duration::from_secs(60));
        cache.put(
            &ProviderKind::Anthropic,
            "k2",
            Err("invalid key".to_string()),
        );

        let cached = cache.get_fresh(&ProviderKind::Anthropic, "k2");
        assert_eq!(cached, Some(Err("invalid key".to_string())));
    }

    #[test]
    fn get_fresh_returns_none_when_never_cached() {
        let cache = PreflightCache::new(Duration::from_secs(60));
        assert_eq!(cache.get_fresh(&ProviderKind::Google, "never-seen"), None);
    }

    #[test]
    fn get_fresh_returns_none_after_ttl_expiry() {
        let cache = PreflightCache::new(Duration::from_millis(20));
        cache.put(&ProviderKind::Google, "k3", Ok(()));

        std::thread::sleep(Duration::from_millis(50));

        assert_eq!(cache.get_fresh(&ProviderKind::Google, "k3"), None);
    }

    #[test]
    fn different_keys_are_not_confused() {
        let cache = PreflightCache::new(Duration::from_secs(60));
        cache.put(&ProviderKind::OpenAi, "same-string", Ok(()));
        cache.put(
            &ProviderKind::Anthropic,
            "same-string",
            Err("rejected".to_string()),
        );

        assert_eq!(
            cache.get_fresh(&ProviderKind::OpenAi, "same-string"),
            Some(Ok(()))
        );
        assert_eq!(
            cache.get_fresh(&ProviderKind::Anthropic, "same-string"),
            Some(Err("rejected".to_string()))
        );
    }

    #[tokio::test]
    async fn shared_preflight_cache_returns_same_instance() {
        let a = shared_preflight_cache().await;
        let b = shared_preflight_cache().await;
        assert!(Arc::ptr_eq(&a, &b));
    }
}
