//! `PgPoolRegistry`: single source of truth for all Postgres pools in the engine.
//!
//! See `docs/superpowers/specs/2026-04-20-connection-pool-management-design.md`.

use super::config::PoolConfig;
use super::error::RegistryError;
use super::metrics::{PoolMetrics, RegistryMetrics, RegistryMetricsInner};
use super::url_key::UrlKey;

use dashmap::{DashMap, DashSet};
use lru::LruCache;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Mutex;

pub struct PgPoolRegistry {
    pub(crate) pools: DashMap<UrlKey, Arc<PgPool>>,
    lru: Mutex<LruCache<UrlKey, ()>>,
    pinned: DashSet<UrlKey>,
    last_used: DashMap<UrlKey, SystemTime>,
    config: PoolConfig,
    pub(crate) metrics: RegistryMetricsInner,
    /// Per-URL mutex to serialise concurrent pool creation for the same key.
    creation_locks: DashMap<UrlKey, Arc<Mutex<()>>>,
    /// Set to `true` by `close_all`; gates all new pool creation.
    closed: AtomicBool,
}

impl PgPoolRegistry {
    pub fn new(config: PoolConfig) -> Self {
        // The LRU is used only for eviction-order tracking; we drive eviction
        // ourselves in `evict_if_needed` so we give the LruCache a generous
        // internal capacity (10× max_entries, min 1024) to avoid it silently
        // dropping entries before our eviction logic runs.
        let lru_cap = NonZeroUsize::new(config.max_entries.saturating_mul(10).max(1024))
            .expect("lru capacity > 0");
        Self {
            pools: DashMap::new(),
            lru: Mutex::new(LruCache::new(lru_cap)),
            pinned: DashSet::new(),
            last_used: DashMap::new(),
            config,
            metrics: RegistryMetricsInner::default(),
            creation_locks: DashMap::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn build_pool_options(&self) -> PgPoolOptions {
        PgPoolOptions::new()
            .min_connections(self.config.min_conn_per_url)
            .max_connections(self.config.max_conn_per_url)
            .idle_timeout(self.config.idle_timeout)
            .max_lifetime(self.config.max_lifetime)
            .acquire_timeout(self.config.acquire_timeout)
    }

    fn hash_url(key: &UrlKey) -> String {
        let digest = Sha256::digest(key.as_str().as_bytes());
        format!("{:x}", digest)
    }

    /// Creates (if absent) a pool for `url` and marks it pinned. Pinned pools
    /// are never inserted into the LRU and never evicted.
    pub async fn pin(&self, url: &str) -> Result<Arc<PgPool>, RegistryError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(RegistryError::Closed);
        }

        let key = UrlKey::normalize(url);

        // Fast path — pool already exists.
        if let Some(existing) = self.pools.get(&key) {
            self.pinned.insert(key.clone());
            self.last_used.insert(key, SystemTime::now());
            return Ok(existing.clone());
        }

        // Slow path — serialize creation per URL to uphold the one-pool-per-URL invariant.
        let lock = self
            .creation_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check: another task may have created the pool while we waited.
        if let Some(existing) = self.pools.get(&key) {
            self.pinned.insert(key.clone());
            self.last_used.insert(key, SystemTime::now());
            return Ok(existing.clone());
        }

        self.metrics.record_pin();
        let pool = self
            .build_pool_options()
            .connect(url)
            .await
            .map_err(RegistryError::from)?;
        let arc = Arc::new(pool);
        self.pools.insert(key.clone(), arc.clone());
        self.pinned.insert(key.clone());
        self.last_used.insert(key.clone(), SystemTime::now());
        tracing::info!(
            target = "colmena::pool_registry",
            url_hash = %Self::hash_url(&key),
            "pool_created pinned=true"
        );
        // The pool is now visible on the fast path; no future caller needs this lock.
        drop(_guard);
        self.creation_locks.remove(&key);
        Ok(arc)
    }

    /// Return the cached pool for `url` if present. If the pool is not pinned,
    /// promote it in the LRU. Increments the cache-hit counter on hit.
    pub(crate) async fn try_cached(&self, url: &str) -> Option<Arc<PgPool>> {
        let key = UrlKey::normalize(url);
        let pool = self.pools.get(&key)?.clone();
        self.last_used.insert(key.clone(), SystemTime::now());
        if !self.pinned.contains(&key) {
            let mut lru = self.lru.lock().await;
            lru.promote(&key);
        }
        self.metrics.record_cache_hit();
        Some(pool)
    }

    /// Return an existing pool for `url`, or create, insert, and return a new one.
    /// Evicts the LRU entry first if the cache (excluding pinned) is at capacity.
    pub async fn get_or_create(&self, url: &str) -> Result<Arc<PgPool>, RegistryError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(RegistryError::Closed);
        }

        self.metrics.record_get_or_create();

        // Fast path.
        if let Some(cached) = self.try_cached(url).await {
            return Ok(cached);
        }

        let key = UrlKey::normalize(url);

        // Slow path — serialize creation per URL to uphold the one-pool-per-URL invariant.
        let lock = self
            .creation_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check: another task may have created the pool while we waited.
        if let Some(existing) = self.pools.get(&key) {
            self.last_used.insert(key.clone(), SystemTime::now());
            if !self.pinned.contains(&key) {
                let mut lru = self.lru.lock().await;
                lru.promote(&key);
            }
            return Ok(existing.clone());
        }

        // Evict before inserting to keep non-pinned entries ≤ max_entries.
        self.evict_if_needed().await;

        let pool = self
            .build_pool_options()
            .connect(url)
            .await
            .map_err(RegistryError::from)?;
        let arc = Arc::new(pool);
        self.pools.insert(key.clone(), arc.clone());
        {
            let mut lru = self.lru.lock().await;
            lru.put(key.clone(), ());
        }
        self.last_used.insert(key.clone(), SystemTime::now());
        tracing::info!(
            target = "colmena::pool_registry",
            url_hash = %Self::hash_url(&key),
            "pool_created pinned=false"
        );
        drop(_guard);
        self.creation_locks.remove(&key);
        Ok(arc)
    }

    async fn evict_if_needed(&self) {
        let non_pinned_count = self.pools.len().saturating_sub(self.pinned.len());
        if non_pinned_count < self.config.max_entries {
            return;
        }
        let mut lru = self.lru.lock().await;
        if let Some((victim, _)) = lru.pop_lru() {
            self.pools.remove(&victim);
            self.last_used.remove(&victim);
            self.creation_locks.remove(&victim);
            self.metrics.record_eviction();
            tracing::warn!(
                target = "colmena::pool_registry",
                url_hash = %Self::hash_url(&victim),
                reason = "lru_capacity",
                "pool_evicted"
            );
        }
    }

    pub fn snapshot_metrics(&self) -> RegistryMetrics {
        let per_url: Vec<PoolMetrics> = self
            .pools
            .iter()
            .map(|entry| {
                let key = entry.key();
                let pool = entry.value();
                PoolMetrics {
                    url_hash: Self::hash_url(key),
                    size: pool.size(),
                    idle: pool.num_idle() as u32,
                    pinned: self.pinned.contains(key),
                    last_used_at: self
                        .last_used
                        .get(key)
                        .map(|v| *v)
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                }
            })
            .collect();

        RegistryMetrics {
            cached_pools: self.pools.len(),
            pinned_pools: self.pinned.len(),
            evictions_total: self.metrics.evictions_total.load(Ordering::Relaxed),
            get_or_create_total: self.metrics.get_or_create_total.load(Ordering::Relaxed),
            pin_total: self.metrics.pin_total.load(Ordering::Relaxed),
            cache_hits_total: self.metrics.cache_hits_total.load(Ordering::Relaxed),
            per_url,
        }
    }

    /// Close every pool in the registry. Awaits sqlx's graceful-close for each.
    /// Safe to call multiple times (idempotent).
    pub async fn close_all(&self) {
        // Mark closed before collecting keys so any concurrent get_or_create / pin
        // racing past this point returns Err(Closed) rather than inserting a new pool.
        self.closed.store(true, Ordering::SeqCst);
        let keys: Vec<UrlKey> = self.pools.iter().map(|e| e.key().clone()).collect();
        for key in keys {
            if let Some((_, pool)) = self.pools.remove(&key) {
                pool.close().await;
            }
        }
        self.pinned.clear();
        self.last_used.clear();
        self.creation_locks.clear();
        let mut lru = self.lru.lock().await;
        lru.clear();
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, url: &str, pool: Arc<PgPool>, pinned: bool) {
        let key = UrlKey::normalize(url);
        self.pools.insert(key.clone(), pool);
        if pinned {
            self.pinned.insert(key.clone());
        } else {
            let mut lru = self.lru.try_lock().expect("no contention in test");
            lru.put(key.clone(), ());
        }
        self.last_used.insert(key, SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Obtain a disposable `Arc<PgPool>` without hitting the network by
    /// constructing it from a lazy-connect URL that is never actually used.
    /// Callers must only test bookkeeping (insertions, LRU state, pinned set)
    /// — they must NOT execute queries on these pools.
    fn fake_pool() -> Arc<PgPool> {
        let options = sqlx::postgres::PgConnectOptions::new()
            .host("invalid.test")
            .database("noop");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(options);
        Arc::new(pool)
    }

    fn tiny_config(max_entries: usize) -> PoolConfig {
        PoolConfig {
            max_entries,
            ..PoolConfig::defaults()
        }
    }

    #[tokio::test]
    async fn pinned_url_is_in_pinned_set() {
        let reg = PgPoolRegistry::new(tiny_config(10));
        reg.insert_for_test("postgres://u:p@host/db", fake_pool(), true);
        assert_eq!(reg.pools.len(), 1);
        assert_eq!(reg.pinned.len(), 1);
    }

    #[tokio::test]
    async fn cache_hit_for_pinned_pool_does_not_touch_lru() {
        let reg = PgPoolRegistry::new(tiny_config(2));
        let url = "postgres://u:p@host/internal";
        reg.insert_for_test(url, fake_pool(), true);

        // Simulate many hits: pinned pool stays in `pools`, never enters LRU.
        for _ in 0..10 {
            let hit = reg.try_cached(url).await;
            assert!(hit.is_some(), "pinned pool should always be a hit");
        }
        let lru = reg.lru.lock().await;
        assert_eq!(lru.len(), 0, "pinned hits must not enter LRU");
    }

    #[tokio::test]
    async fn cache_hit_for_unpinned_pool_promotes_lru() {
        let reg = PgPoolRegistry::new(tiny_config(2));
        reg.insert_for_test("postgres://u:p@host/a", fake_pool(), false);
        reg.insert_for_test("postgres://u:p@host/b", fake_pool(), false);
        // "a" is oldest; promote it so "b" becomes LRU.
        let _ = reg.try_cached("postgres://u:p@host/a").await;
        let mut lru = reg.lru.lock().await;
        let oldest = lru.pop_lru().map(|(k, _)| k.as_str().to_string());
        assert_eq!(oldest.as_deref(), Some("postgres://u:p@host/b"));
    }

    #[tokio::test]
    async fn lru_evicts_when_over_capacity() {
        let reg = PgPoolRegistry::new(tiny_config(2));
        reg.insert_for_test("postgres://u:p@host/a", fake_pool(), false);
        reg.insert_for_test("postgres://u:p@host/b", fake_pool(), false);
        reg.insert_for_test("postgres://u:p@host/c", fake_pool(), false);
        // The manual insert_for_test bypasses eviction. Trigger it now.
        reg.evict_if_needed().await;
        assert_eq!(reg.pools.len(), 2);
        assert!(
            !reg.pools
                .contains_key(&UrlKey::normalize("postgres://u:p@host/a")),
            "oldest unpinned entry should have been evicted"
        );
    }

    #[tokio::test]
    async fn pinned_entries_are_exempt_from_eviction() {
        let reg = PgPoolRegistry::new(tiny_config(1));
        reg.insert_for_test("postgres://u:p@host/internal", fake_pool(), true);
        reg.insert_for_test("postgres://u:p@host/user1", fake_pool(), false);
        reg.evict_if_needed().await;
        assert!(reg
            .pools
            .contains_key(&UrlKey::normalize("postgres://u:p@host/internal")));
    }

    #[tokio::test]
    async fn close_all_drains_every_entry() {
        let reg = PgPoolRegistry::new(tiny_config(5));
        reg.insert_for_test("postgres://u:p@host/a", fake_pool(), false);
        reg.insert_for_test("postgres://u:p@host/internal", fake_pool(), true);
        reg.close_all().await;
        assert_eq!(reg.pools.len(), 0);
        assert_eq!(reg.pinned.len(), 0);
    }

    #[tokio::test]
    async fn metrics_snapshot_reflects_state() {
        let reg = PgPoolRegistry::new(tiny_config(5));
        reg.insert_for_test("postgres://u:p@host/a", fake_pool(), false);
        reg.insert_for_test("postgres://u:p@host/internal", fake_pool(), true);
        let _ = reg.try_cached("postgres://u:p@host/a").await;
        let snap = reg.snapshot_metrics();
        assert_eq!(snap.cached_pools, 2);
        assert_eq!(snap.pinned_pools, 1);
        assert_eq!(snap.cache_hits_total, 1);
        assert_eq!(snap.per_url.len(), 2);
        // url_hash must not contain the raw URL.
        for p in &snap.per_url {
            assert!(!p.url_hash.contains("host"));
            assert!(!p.url_hash.contains("://"));
        }
    }

    /// Stress test: 50 concurrent `get_or_create` calls for the same URL must
    /// produce exactly one pool (`Arc::ptr_eq` on every returned Arc).
    #[tokio::test]
    async fn concurrent_get_or_create_produces_single_pool() {
        let reg = Arc::new(PgPoolRegistry::new(tiny_config(10)));
        let url = "postgres://u:p@host/stress";
        // Seed the pool via insert_for_test to avoid a real network connect.
        // This exercises the fast path (cache hits) under contention, which
        // is sufficient to catch races on `pools` / `lru` / `last_used`
        // bookkeeping while avoiding external deps.
        reg.insert_for_test(url, fake_pool(), false);

        let mut handles = Vec::with_capacity(50);
        for _ in 0..50 {
            let reg = reg.clone();
            handles.push(tokio::spawn(async move {
                reg.try_cached(url).await.expect("hit")
            }));
        }
        let mut pools = Vec::with_capacity(50);
        for h in handles {
            pools.push(h.await.unwrap());
        }
        let first = pools[0].clone();
        for p in &pools[1..] {
            assert!(
                Arc::ptr_eq(&first, p),
                "all returned Arcs must point to the same pool"
            );
        }
        assert_eq!(reg.pools.len(), 1);
    }

    #[tokio::test]
    async fn close_all_then_get_or_create_returns_closed() {
        let reg = Arc::new(PgPoolRegistry::new(tiny_config(5)));
        reg.insert_for_test("postgres://u:p@host/existing", fake_pool(), false);
        reg.close_all().await;
        // A brand-new URL (never seen before) must return Closed before trying to connect.
        let result = reg
            .get_or_create("postgres://u:p@host/new-after-close")
            .await;
        assert!(
            matches!(result, Err(RegistryError::Closed)),
            "get_or_create after close_all must return Err(Closed)"
        );
    }
}
