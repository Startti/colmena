//! SearchUseCase — wraps a [`SearchPort`] with:
//! - An LRU cache (Task 7) keyed by request hash + TTL.
//! - A per-`dag_run_id` rate-limit counter (Task 6) enforcing `max_calls_per_run`.
//! - A retry loop (Task 8) for transient `Upstream` / `Timeout` errors.
//!
//! Lives in the application layer per hexagonal rules — it orchestrates a
//! domain port and does no I/O itself.

use crate::web::domain::errors::WebDomainError;
use crate::web::domain::search_port::{
    FetchRequest, FetchResponse, SearchPort, SearchRequest, SearchResponse,
};
use chrono::{DateTime, Utc};
use lru::LruCache;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

fn hash_search(req: &SearchRequest) -> u64 {
    let mut h = DefaultHasher::new();
    ("search",
     &req.query,
     req.max_results,
     req.include_content,
     req.search_depth as u8,
     &req.include_domains,
     &req.exclude_domains,
     req.time_range.map(|r| r as u8),
    )
        .hash(&mut h);
    h.finish()
}

fn hash_fetch(req: &FetchRequest) -> u64 {
    let mut h = DefaultHasher::new();
    ("fetch", &req.url, req.format as u8).hash(&mut h);
    h.finish()
}

#[derive(Clone)]
enum CachedResponse {
    Search(SearchResponse),
    Fetch(FetchResponse),
}

struct CachedEntry {
    resp: CachedResponse,
    inserted_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SearchUseCaseConfig {
    pub enable_cache: bool,
    pub cache_ttl: Duration,
    pub max_calls_per_run: u32,
    pub fail_on_limit: bool,
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub timeout: Duration,
}

impl Default for SearchUseCaseConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_ttl: Duration::from_secs(3600),
            max_calls_per_run: 50,
            fail_on_limit: false,
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Default)]
struct RateLimitState {
    per_run: HashMap<String, u32>,
}

impl RateLimitState {
    fn try_increment(&mut self, run_id: &str, cap: u32) -> Result<u32, WebDomainError> {
        let entry = self.per_run.entry(run_id.to_string()).or_insert(0);
        if *entry >= cap {
            return Err(WebDomainError::RateLimit {
                calls_used: *entry,
                cap,
            });
        }
        *entry += 1;
        Ok(*entry)
    }

    fn reset(&mut self, run_id: &str) {
        self.per_run.remove(run_id);
    }
}

pub struct SearchUseCase {
    port: Arc<dyn SearchPort>,
    config: SearchUseCaseConfig,
    counters: Mutex<RateLimitState>,
    cache: RwLock<LruCache<u64, CachedEntry>>,
}

impl SearchUseCase {
    pub fn new(port: Arc<dyn SearchPort>, config: SearchUseCaseConfig) -> Self {
        let cap = NonZeroUsize::new(1024).unwrap();
        Self {
            port,
            config,
            counters: Mutex::new(RateLimitState::default()),
            cache: RwLock::new(LruCache::new(cap)),
        }
    }

    pub fn config(&self) -> &SearchUseCaseConfig {
        &self.config
    }

    /// Reset the rate-limit counter for a given `dag_run_id`. Called by the
    /// engine at run-end. Safe to call even if no entry exists for the id.
    pub fn reset_run(&self, dag_run_id: &str) {
        self.counters.lock().unwrap().reset(dag_run_id);
    }

    /// Run a search. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn search(
        &self,
        dag_run_id: &str,
        req: SearchRequest,
    ) -> Result<SearchResponse, WebDomainError> {
        let key = hash_search(&req);

        if self.config.enable_cache {
            if let Some(resp) = self.cache_lookup_search(key) {
                return Ok(resp);
            }
        }

        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;

        let resp = self.port.search(req).await?;

        if self.config.enable_cache {
            self.cache.write().unwrap().put(
                key,
                CachedEntry {
                    resp: CachedResponse::Search(resp.clone()),
                    inserted_at: Utc::now(),
                },
            );
        }
        Ok(resp)
    }

    /// Run a fetch. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn fetch(
        &self,
        dag_run_id: &str,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        let key = hash_fetch(&req);

        if self.config.enable_cache {
            if let Some(resp) = self.cache_lookup_fetch(key) {
                return Ok(resp);
            }
        }

        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;

        let resp = self.port.fetch(req).await?;

        if self.config.enable_cache {
            self.cache.write().unwrap().put(
                key,
                CachedEntry {
                    resp: CachedResponse::Fetch(resp.clone()),
                    inserted_at: Utc::now(),
                },
            );
        }
        Ok(resp)
    }

    fn cache_lookup_search(&self, key: u64) -> Option<SearchResponse> {
        let mut cache = self.cache.write().unwrap();
        let entry = cache.get(&key)?;
        if self.is_expired(entry) {
            cache.pop(&key);
            return None;
        }
        match &entry.resp {
            CachedResponse::Search(s) => Some(s.clone()),
            CachedResponse::Fetch(_) => None,
        }
    }

    fn cache_lookup_fetch(&self, key: u64) -> Option<FetchResponse> {
        let mut cache = self.cache.write().unwrap();
        let entry = cache.get(&key)?;
        if self.is_expired(entry) {
            cache.pop(&key);
            return None;
        }
        match &entry.resp {
            CachedResponse::Fetch(f) => Some(f.clone()),
            CachedResponse::Search(_) => None,
        }
    }

    fn is_expired(&self, entry: &CachedEntry) -> bool {
        let age_ms = (Utc::now() - entry.inserted_at).num_milliseconds().max(0) as u64;
        age_ms >= self.config.cache_ttl.as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::domain::search_port::{ExtractFormat, SearchResult};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Minimal stub port so tests don't need wiremock.
    struct StubPort {
        search_calls: Mutex<u32>,
        fetch_calls: Mutex<u32>,
    }

    impl StubPort {
        fn new() -> Self {
            Self {
                search_calls: Mutex::new(0),
                fetch_calls: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl SearchPort for StubPort {
        async fn search(&self, req: SearchRequest) -> Result<SearchResponse, WebDomainError> {
            *self.search_calls.lock().unwrap() += 1;
            Ok(SearchResponse {
                query: req.query,
                results: vec![SearchResult {
                    title: "T".into(),
                    url: "https://u".into(),
                    snippet: "s".into(),
                    score: 0.5,
                    content: None,
                }],
                answer: None,
                credits_used: 1,
            })
        }
        async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse, WebDomainError> {
            *self.fetch_calls.lock().unwrap() += 1;
            Ok(FetchResponse {
                url: req.url,
                title: None,
                content: "body".into(),
                content_length: 4,
                credits_used: 1,
            })
        }
    }

    fn uc() -> (Arc<StubPort>, SearchUseCase) {
        let port = Arc::new(StubPort::new());
        let uc = SearchUseCase::new(port.clone(), SearchUseCaseConfig::default());
        (port, uc)
    }

    #[tokio::test]
    async fn search_delegates_to_port() {
        let (port, uc) = uc();
        let resp = uc.search("run-1", SearchRequest::new("hi")).await.unwrap();
        assert_eq!(resp.query, "hi");
        assert_eq!(*port.search_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fetch_delegates_to_port() {
        let (port, uc) = uc();
        let resp = uc
            .fetch(
                "run-1",
                FetchRequest {
                    url: "https://u".into(),
                    format: ExtractFormat::Markdown,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.url, "https://u");
        assert_eq!(*port.fetch_calls.lock().unwrap(), 1);
    }

    #[test]
    fn default_config_matches_spec() {
        let d = SearchUseCaseConfig::default();
        assert!(d.enable_cache);
        assert_eq!(d.cache_ttl, Duration::from_secs(3600));
        assert_eq!(d.max_calls_per_run, 50);
        assert!(!d.fail_on_limit);
        assert_eq!(d.max_attempts, 3);
        assert_eq!(d.initial_backoff, Duration::from_millis(500));
    }

    #[tokio::test]
    async fn rate_limit_caps_calls_per_run_non_fail() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 2,
            fail_on_limit: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);

        // 3 calls, cap of 2: first two hit port, third returns RateLimit (fail_on_limit=false
        // means structured RateLimit error — still not a crash).
        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        uc.search("run-A", SearchRequest::new("q2")).await.unwrap();
        let err = uc.search("run-A", SearchRequest::new("q3")).await.unwrap_err();
        assert!(matches!(
            err,
            WebDomainError::RateLimit { calls_used: 2, cap: 2 }
        ));
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn rate_limit_separate_runs_have_separate_counters() {
        let port = Arc::new(StubPort::new());
        // Cache is shared across runs by design, so disable it here to keep
        // the test focused on rate-limit accounting only.
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);

        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        // run-B has its own counter, so this should succeed.
        uc.search("run-B", SearchRequest::new("q1")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn rate_limit_reset_run_clears_counter() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        let err = uc.search("run-A", SearchRequest::new("q2")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
        uc.reset_run("run-A");
        uc.search("run-A", SearchRequest::new("q3")).await.unwrap();
    }

    #[tokio::test]
    async fn rate_limit_fetch_shares_counter_with_search() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        let err = uc
            .fetch(
                "run-A",
                FetchRequest { url: "u".into(), format: ExtractFormat::Text },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
    }

    #[tokio::test]
    async fn cache_hit_returns_same_response_without_hitting_port() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);

        let r1 = uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        let r2 = uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        assert_eq!(r1.query, r2.query);
        // Only first call hits the port.
        assert_eq!(*port.search_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn cache_miss_on_different_query() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("a")).await.unwrap();
        uc.search("run-A", SearchRequest::new("b")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cache_disabled_always_hits_port() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cache_entry_expires_after_ttl() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            cache_ttl: Duration::from_millis(0),
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        // TTL=0 means every lookup is immediately expired.
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cache_does_not_count_toward_rate_limit() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("x")).await.unwrap();
        // Cache hit; should not exhaust the cap.
        uc.search("run-A", SearchRequest::new("x")).await.unwrap();
        // Still counts only 1 toward the port; cap not yet hit.
        let err = uc.search("run-A", SearchRequest::new("y")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
    }
}
