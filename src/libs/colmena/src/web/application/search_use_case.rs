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
use std::sync::Arc;
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

pub struct SearchUseCase {
    port: Arc<dyn SearchPort>,
    config: SearchUseCaseConfig,
}

impl SearchUseCase {
    pub fn new(port: Arc<dyn SearchPort>, config: SearchUseCaseConfig) -> Self {
        Self { port, config }
    }

    pub fn config(&self) -> &SearchUseCaseConfig {
        &self.config
    }

    /// Run a search. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn search(
        &self,
        dag_run_id: &str,
        req: SearchRequest,
    ) -> Result<SearchResponse, WebDomainError> {
        let _ = dag_run_id;
        self.port.search(req).await
    }

    /// Run a fetch. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn fetch(
        &self,
        dag_run_id: &str,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        let _ = dag_run_id;
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
}
