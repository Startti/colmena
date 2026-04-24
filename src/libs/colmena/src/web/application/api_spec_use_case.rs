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

use crate::web::domain::{Endpoint, HttpMethod};

#[derive(Debug, Clone)]
pub struct EndpointListPage {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub endpoints: Vec<EndpointSummary>,
}

#[derive(Debug, Clone)]
pub struct EndpointSummary {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
}

impl From<&Endpoint> for EndpointSummary {
    fn from(e: &Endpoint) -> Self {
        Self {
            operation_id: e.operation_id.clone(),
            method: e.method.as_str().to_string(),
            path: e.path.clone(),
            summary: e.summary.clone(),
            tags: e.tags.clone(),
        }
    }
}

pub fn list_endpoints(
    spec: &ParsedSpec,
    tag: Option<&str>,
    limit: usize,
    offset: usize,
) -> EndpointListPage {
    let filtered: Vec<&Endpoint> = spec
        .endpoints
        .iter()
        .filter(|e| match tag {
            None => true,
            Some(t) => e.tags.iter().any(|candidate| candidate == t),
        })
        .collect();
    let total = filtered.len();
    let slice: Vec<EndpointSummary> = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(EndpointSummary::from)
        .collect();
    let returned = slice.len();
    EndpointListPage {
        total,
        returned,
        offset,
        endpoints: slice,
    }
}

#[derive(Debug, Clone)]
pub struct EndpointSearchHit {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub score: f32,
    pub match_reason: String,
}

/// Fuzzy-search endpoints by a free-text query. The input is tokenized
/// and each token is scored independently against a concatenated
/// searchable string; the final score is the normalized sum.
pub fn search_endpoint(
    spec: &ParsedSpec,
    query: &str,
    method_filter: Option<&str>,
    max_results: usize,
    threshold: f32,
) -> Vec<EndpointSearchHit> {
    use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
    use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32Str};

    let method_filter = method_filter.and_then(HttpMethod::parse);

    let mut matcher = Matcher::new(NucleoConfig::DEFAULT);

    let candidates: Vec<(&Endpoint, String)> = spec
        .endpoints
        .iter()
        .filter(|e| match method_filter {
            None => true,
            Some(m) => e.method == m,
        })
        .map(|e| {
            let mut haystack = String::new();
            haystack.push_str(&e.path);
            haystack.push(' ');
            haystack.push_str(&e.operation_id);
            haystack.push(' ');
            if let Some(s) = &e.summary {
                haystack.push_str(s);
                haystack.push(' ');
            }
            if let Some(d) = &e.description {
                haystack.push_str(d);
                haystack.push(' ');
            }
            for t in &e.tags {
                haystack.push_str(t);
                haystack.push(' ');
            }
            (e, haystack)
        })
        .collect();

    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    // Score each haystack with nucleo.
    let mut scored: Vec<(f32, &Endpoint, String)> = Vec::new();
    let mut hay_buf = Vec::new();
    for (ep, hay) in &candidates {
        hay_buf.clear();
        let hay_u32 = Utf32Str::new(hay, &mut hay_buf);
        if let Some(raw) = pattern.score(hay_u32, &mut matcher) {
            let normalized = (raw as f32) / (hay.chars().count().max(1) as f32 * 16.0);
            if normalized >= threshold {
                let reason = explain_match(query, ep);
                scored.push((normalized.min(1.0), ep, reason));
            }
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(max_results)
        .map(|(score, ep, reason)| EndpointSearchHit {
            operation_id: ep.operation_id.clone(),
            method: ep.method.as_str().to_string(),
            path: ep.path.clone(),
            summary: ep.summary.clone(),
            score,
            match_reason: reason,
        })
        .collect()
}

fn explain_match(query: &str, ep: &Endpoint) -> String {
    let q = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    if ep.path.to_ascii_lowercase().contains(&q) {
        hits.push(format!("path matches '{}'", q));
    }
    if let Some(s) = &ep.summary {
        if s.to_ascii_lowercase().contains(&q) {
            hits.push(format!("summary matches '{}'", q));
        }
    }
    if ep.operation_id.to_ascii_lowercase().contains(&q) {
        hits.push(format!("operation_id matches '{}'", q));
    }
    if hits.is_empty() {
        "fuzzy match across path/summary/description/tags".into()
    } else {
        hits.join("; ")
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

#[cfg(test)]
mod tests_list_and_search {
    use crate::web::domain::{Endpoint, HttpMethod, ParsedSpec, SpecFormat};
    use std::collections::HashMap;

    fn spec_with(endpoints: Vec<Endpoint>) -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints,
            security_schemes: HashMap::new(),
            tags: vec!["pet".into(), "store".into()],
        }
    }

    fn ep(
        op: &str,
        method: HttpMethod,
        path: &str,
        summary: &str,
        tag: &str,
    ) -> Endpoint {
        Endpoint {
            operation_id: op.into(),
            method,
            path: path.into(),
            summary: Some(summary.into()),
            description: None,
            tags: vec![tag.into()],
            path_params: Vec::new(),
            query_params: Vec::new(),
            header_params: Vec::new(),
            request_body: None,
            responses: HashMap::new(),
            security: Vec::new(),
        }
    }

    fn sample() -> ParsedSpec {
        spec_with(vec![
            ep("listPets", HttpMethod::Get, "/pet", "List pets", "pet"),
            ep("addPet", HttpMethod::Post, "/pet", "Add a new pet", "pet"),
            ep("getPet", HttpMethod::Get, "/pet/{id}", "Get pet by ID", "pet"),
            ep("listStores", HttpMethod::Get, "/store", "List stores", "store"),
            ep("createSubscription", HttpMethod::Post, "/subscription", "Create a subscription", "billing"),
        ])
    }

    #[test]
    fn list_all_returns_all_endpoints() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, None, 50, 0);
        assert_eq!(page.total, 5);
        assert_eq!(page.returned, 5);
        assert_eq!(page.endpoints.len(), 5);
    }

    #[test]
    fn list_paginates() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, None, 2, 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.returned, 2);
        assert_eq!(page.endpoints[0].operation_id, "getPet");
    }

    #[test]
    fn list_filters_by_tag() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, Some("billing"), 50, 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.endpoints[0].operation_id, "createSubscription");
    }

    #[test]
    fn search_finds_by_summary() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "create subscription",
            None,
            10,
            0.1,
        );
        assert!(!results.is_empty());
        assert_eq!(results[0].operation_id, "createSubscription");
    }

    #[test]
    fn search_filters_by_method() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "pet",
            Some("GET"),
            10,
            0.1,
        );
        for r in &results {
            assert_eq!(r.method, "GET");
        }
    }

    #[test]
    fn search_respects_threshold() {
        let spec = sample();
        let tight = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "completely unrelated nonsense xyz",
            None,
            10,
            0.99,
        );
        assert!(tight.is_empty());
    }

    #[test]
    fn search_returns_top_n() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "pet",
            None,
            2,
            0.1,
        );
        assert!(results.len() <= 2);
    }
}
