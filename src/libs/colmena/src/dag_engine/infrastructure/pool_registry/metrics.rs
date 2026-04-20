//! Observability surface for the pool registry. Counters are atomic; snapshot
//! returns a plain struct suitable for JSON serialization in `/debug/pools`.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

#[derive(Default)]
pub(crate) struct RegistryMetricsInner {
    pub get_or_create_total: AtomicU64,
    pub pin_total: AtomicU64,
    pub cache_hits_total: AtomicU64,
    pub evictions_total: AtomicU64,
}

impl RegistryMetricsInner {
    pub fn record_get_or_create(&self) {
        self.get_or_create_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_pin(&self) {
        self.pin_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_cache_hit(&self) {
        self.cache_hits_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_eviction(&self) {
        self.evictions_total.fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot exported through `ColmenaEngine::registry_metrics()`.
#[derive(Debug, Clone, Serialize)]
pub struct RegistryMetrics {
    pub cached_pools: usize,
    pub pinned_pools: usize,
    pub evictions_total: u64,
    pub get_or_create_total: u64,
    pub pin_total: u64,
    pub cache_hits_total: u64,
    pub per_url: Vec<PoolMetrics>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoolMetrics {
    /// SHA-256 hex of the normalized URL — never expose raw credentials.
    pub url_hash: String,
    pub size: u32,
    pub idle: u32,
    pub pinned: bool,
    #[serde(skip)]
    pub last_used_at: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment() {
        let m = RegistryMetricsInner::default();
        m.record_get_or_create();
        m.record_get_or_create();
        m.record_pin();
        m.record_cache_hit();
        m.record_eviction();
        assert_eq!(m.get_or_create_total.load(Ordering::Relaxed), 2);
        assert_eq!(m.pin_total.load(Ordering::Relaxed), 1);
        assert_eq!(m.cache_hits_total.load(Ordering::Relaxed), 1);
        assert_eq!(m.evictions_total.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let m = RegistryMetrics {
            cached_pools: 1,
            pinned_pools: 1,
            evictions_total: 0,
            get_or_create_total: 5,
            pin_total: 2,
            cache_hits_total: 4,
            per_url: vec![],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"cached_pools\":1"));
        assert!(s.contains("\"pin_total\":2"));
    }
}
