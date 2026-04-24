//! `ApiSpecUseCase` — orchestrates fetch / cache / search / build for a
//! single conversation.

use crate::web::domain::{
    ApiSpecPort, ParsedSpec, SessionKey, SessionRegistry, SpecFetchResult, WebDomainError,
};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Tunables for the use case. Defaults match the Spec C "minimal config" values.
#[derive(Debug, Clone)]
pub struct ApiSpecUseCaseConfig {
    pub enable_cache: bool,
    pub cache_ttl: Duration,
    pub max_cached_specs: usize,
    pub fuzzy_match_threshold: f32,
    pub default_base_url_override: Option<String>,
}

impl Default for ApiSpecUseCaseConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_ttl: Duration::from_secs(86_400),
            max_cached_specs: 100,
            fuzzy_match_threshold: 0.6,
            default_base_url_override: None,
        }
    }
}

/// Per-conversation cache of parsed specs.
pub struct SpecCache {
    specs: Mutex<LruCache<String, CachedSpec>>,
}

impl SpecCache {
    pub fn new(max: usize) -> Self {
        Self {
            specs: Mutex::new(LruCache::new(
                NonZeroUsize::new(max.max(1)).unwrap(),
            )),
        }
    }
}

#[derive(Clone)]
pub struct CachedSpec {
    pub parsed: Arc<ParsedSpec>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cached_at: Instant,
}

pub struct ApiSpecUseCase {
    port: Arc<dyn ApiSpecPort>,
    registry: Arc<SessionRegistry<Arc<SpecCache>>>,
    config: ApiSpecUseCaseConfig,
}

impl ApiSpecUseCase {
    pub fn new(
        port: Arc<dyn ApiSpecPort>,
        registry: Arc<SessionRegistry<Arc<SpecCache>>>,
        config: ApiSpecUseCaseConfig,
    ) -> Self {
        Self { port, registry, config }
    }

    /// Fetch-or-reuse a spec for a given conversation.
    ///
    /// * If `force_reload` is true or the cached entry is older than the
    ///   TTL, hit the port. A 304 response refreshes `cached_at` in place.
    /// * Always returns a `CachedSpec` (either the freshly-parsed one or
    ///   the reused one).
    pub async fn fetch_spec(
        &self,
        conversation_id: &str,
        input_url: &str,
        force_reload: bool,
    ) -> Result<CachedSpec, WebDomainError> {
        let key = SessionKey::new(conversation_id, "api_explorer");

        // Get-or-create the per-conversation cache.
        let cache = self
            .registry
            .with_entry(&key, |c| c.clone())
            .await
            .unwrap_or_else(|| Arc::new(SpecCache::new(self.config.max_cached_specs)));
        // Insert if absent so future lookups share the Arc.
        self.registry.insert(key.clone(), cache.clone()).await;

        if self.config.enable_cache && !force_reload {
            if let Some(hit) = cache
                .specs
                .lock()
                .await
                .get(input_url)
                .cloned()
            {
                if hit.cached_at.elapsed() < self.config.cache_ttl {
                    // Fresh enough — skip revalidation to avoid network.
                    return Ok(hit);
                }
            }
        }

        // Either no cache entry, stale, or forced reload. Re-fetch.
        let previous = cache.specs.lock().await.get(input_url).cloned();
        let etag = previous.as_ref().and_then(|c| c.etag.clone());
        let last_modified = previous.as_ref().and_then(|c| c.last_modified.clone());

        match self
            .port
            .fetch_and_parse(input_url, etag.as_deref(), last_modified.as_deref())
            .await?
        {
            SpecFetchResult::Fresh {
                spec,
                etag,
                last_modified,
            } => {
                let entry = CachedSpec {
                    parsed: Arc::new(spec),
                    etag,
                    last_modified,
                    cached_at: Instant::now(),
                };
                cache.specs.lock().await.put(input_url.to_string(), entry.clone());
                Ok(entry)
            }
            SpecFetchResult::NotModified => {
                let mut prev = previous.ok_or_else(|| WebDomainError::AdapterInit(
                    "got 304 Not Modified without a cached spec".into(),
                ))?;
                prev.cached_at = Instant::now();
                cache
                    .specs
                    .lock()
                    .await
                    .put(input_url.to_string(), prev.clone());
                Ok(prev)
            }
        }
    }

    /// Public accessor for tests + later tasks that need the registry's
    /// Arc (e.g. lifecycle subscription).
    pub fn registry(&self) -> Arc<SessionRegistry<Arc<SpecCache>>> {
        self.registry.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::domain::{
        Endpoint, HttpMethod, ParsedSpec, SpecFormat, TtlConfig,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingPort {
        calls: AtomicU32,
        respond_with: Mutex<Option<SpecFetchResult>>,
    }

    #[async_trait]
    impl ApiSpecPort for CountingPort {
        async fn fetch_and_parse(
            &self,
            _url: &str,
            _etag: Option<&str>,
            _lm: Option<&str>,
        ) -> Result<SpecFetchResult, WebDomainError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Return whatever was prepared; default to a fresh tiny spec.
            let prepared = self.respond_with.lock().await.take();
            if let Some(r) = prepared {
                return Ok(r);
            }
            Ok(SpecFetchResult::Fresh {
                spec: tiny_spec(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            })
        }
    }

    fn tiny_spec() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "https://ex/s.yaml".into(),
            input_url: "https://ex/s.yaml".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "getThing".into(),
                method: HttpMethod::Get,
                path: "/thing".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    fn use_case_with(port: Arc<CountingPort>) -> ApiSpecUseCase {
        let registry: Arc<SessionRegistry<Arc<SpecCache>>> =
            SessionRegistry::new(TtlConfig::default());
        ApiSpecUseCase::new(port, registry, ApiSpecUseCaseConfig::default())
    }

    #[tokio::test]
    async fn first_fetch_hits_the_port() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_fetch_within_ttl_does_not_hit_the_port() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn force_reload_bypasses_cache() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-1", "https://ex/s.yaml", true).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn different_conversations_have_separate_caches() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-a", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-b", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn not_modified_refreshes_existing_cached_entry() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        *port.respond_with.lock().await = Some(SpecFetchResult::NotModified);
        let res = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", true)
            .await
            .unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
        assert_eq!(res.etag.as_deref(), Some("\"v1\""));
    }
}
