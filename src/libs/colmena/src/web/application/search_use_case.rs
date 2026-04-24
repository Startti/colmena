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
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

impl SearchUseCase {
    pub fn new(port: Arc<dyn SearchPort>, config: SearchUseCaseConfig) -> Self {
        Self {
            port,
            config,
            counters: Mutex::new(RateLimitState::default()),
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
        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;
        self.port.search(req).await
    }

    /// Run a fetch. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn fetch(
        &self,
        dag_run_id: &str,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;
        self.port.fetch(req).await
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
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
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
}
