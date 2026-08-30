//! Process-level pool of live MCP server connections.
//!
//! An MCP connection is expensive: a TCP+TLS handshake plus a JSON-RPC
//! `initialize` round-trip before a single tool can be listed. A
//! `DagToolExecutor` is built fresh for every `llm_call` execution, so the
//! connection cannot live there — every turn of every agent loop would
//! re-handshake. This registry outlives executions and hands the same client
//! back for the same [`McpServerKey`].

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::dag_engine::infrastructure::mcp_registry::McpServerKey;
use crate::llm::domain::mcp::{McpClientPort, McpError, McpServerConfig, McpToolDescriptor};

/// How a registry obtains a client for a configuration.
///
/// A port rather than a direct call to `RmcpHttpClient::connect`, so the
/// pooling logic can be tested for what it actually is — a concurrency and
/// caching problem — without a socket. Wiremock proves we speak the protocol;
/// it cannot prove that two callers racing on a cold key produce one
/// handshake.
#[async_trait]
pub trait McpConnector: Send + Sync {
    async fn connect(
        &self,
        server_label: &str,
        config: &McpServerConfig,
    ) -> Result<Arc<dyn McpClientPort>, McpError>;
}

/// Pool of connections keyed by [`McpServerKey`].
pub struct McpConnectionRegistry {
    clients: DashMap<McpServerKey, Arc<dyn McpClientPort>>,
    /// One lock per key, so two callers racing on a cold key produce one
    /// handshake instead of two.
    ///
    /// Deliberate deviation from `pool_registry`, which removes the entry
    /// after creating: entries are kept here. That registry keys on arbitrary
    /// database URLs and must bound growth; these keys are operator-declared
    /// MCP servers, so the map holds a handful of entries for the process
    /// lifetime. Removing would open a window where a late waiter and a fresh
    /// caller hold two different mutexes for one key — harmless, because the
    /// re-check below catches it, but it makes the lock's guarantee harder to
    /// reason about for no benefit at this cardinality.
    creation_locks: DashMap<McpServerKey, Arc<Mutex<()>>>,
    /// Last `tools/list` result per server, with the moment it was fetched.
    tool_cache: DashMap<McpServerKey, CachedTools>,
    /// Single-flight for cache fills, separate from `creation_locks` so a
    /// catalog refresh never serialises behind an unrelated handshake.
    fetch_locks: DashMap<McpServerKey, Arc<Mutex<()>>>,
    connector: Arc<dyn McpConnector>,
}

/// A cached catalog and the instant it was fetched.
///
/// `tokio::time::Instant` rather than `SystemTime`: it is monotonic, so a
/// wall-clock jump (NTP correction, a suspended container waking up) cannot
/// make an entry look older or newer than it is — and it is virtualisable,
/// so expiry is tested by advancing a paused clock instead of sleeping.
#[derive(Clone)]
struct CachedTools {
    tools: Arc<Vec<McpToolDescriptor>>,
    fetched_at: Instant,
}

impl McpConnectionRegistry {
    pub fn new(connector: Arc<dyn McpConnector>) -> Self {
        Self {
            clients: DashMap::new(),
            creation_locks: DashMap::new(),
            tool_cache: DashMap::new(),
            fetch_locks: DashMap::new(),
            connector,
        }
    }

    /// The pooled client for `key`, connecting once if this is its first use.
    ///
    /// A failed connect is NOT cached: a server that was down when the agent
    /// first reached for it must be reachable on the next turn, not poisoned
    /// for the life of the process.
    pub async fn client(
        &self,
        key: &McpServerKey,
        server_label: &str,
        config: &McpServerConfig,
    ) -> Result<Arc<dyn McpClientPort>, McpError> {
        if let Some(existing) = self.clients.get(key) {
            return Ok(existing.clone());
        }

        let lock = self
            .creation_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check under the lock: another task may have connected while we
        // waited. Without this the lock would serialise the handshakes but
        // still perform every one of them.
        if let Some(existing) = self.clients.get(key) {
            return Ok(existing.clone());
        }

        let client = self.connector.connect(server_label, config).await?;
        self.clients.insert(key.clone(), client.clone());
        Ok(client)
    }

    /// The server's tool catalog, served from cache while it is inside
    /// `config.cache_ttl` (R3.5).
    ///
    /// Under lazy loading the exposure stage runs on EVERY agent-loop
    /// iteration, so without this every turn pays a `tools/list` round-trip
    /// for a catalog that almost never changes.
    ///
    /// Fills are single-flighted per key: on a cold or just-expired entry,
    /// concurrent turns would otherwise all fire their own `tools/list` — a
    /// thundering herd against the server at exactly the moment the cache
    /// turns over. A failed fetch is never cached, for the same reason a
    /// failed connect is not: one bad moment would blank the catalog until
    /// the TTL elapsed.
    pub async fn tools(
        &self,
        key: &McpServerKey,
        server_label: &str,
        config: &McpServerConfig,
    ) -> Result<Arc<Vec<McpToolDescriptor>>, McpError> {
        if let Some(fresh) = self.cached_if_fresh(key, config) {
            return Ok(fresh);
        }

        let lock = self
            .fetch_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check under the lock: a racing task may have filled the entry
        // while we waited. Without this the lock would serialise the fetches
        // and still perform every one of them.
        if let Some(fresh) = self.cached_if_fresh(key, config) {
            return Ok(fresh);
        }

        let client = self.client(key, server_label, config).await?;
        let tools = Arc::new(client.list_tools().await?);
        self.tool_cache.insert(
            key.clone(),
            CachedTools {
                tools: tools.clone(),
                fetched_at: Instant::now(),
            },
        );
        Ok(tools)
    }

    /// The cached catalog if it is still inside its TTL.
    ///
    /// `elapsed() >= ttl` expires, so `cache_ttl: 0` means "never cache"
    /// rather than "cache forever" — a zero TTL is a legitimate operator
    /// choice and must not read as an accidental permanent hit.
    fn cached_if_fresh(
        &self,
        key: &McpServerKey,
        config: &McpServerConfig,
    ) -> Option<Arc<Vec<McpToolDescriptor>>> {
        let entry = self.tool_cache.get(key)?;
        (entry.fetched_at.elapsed() < config.cache_ttl).then(|| entry.tools.clone())
    }

    /// How many connections are currently pooled. For tests and metrics.
    pub fn len(&self) -> usize {
        self.clients.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::llm::domain::mcp::{McpToolDescriptor, McpToolResult, McpTransport};

    /// Counts `tools/list` round-trips, so cache hits are observable, and can
    /// be told to fail so the not-cached-on-failure path is testable.
    struct StubClient {
        label: String,
        list_calls: Arc<AtomicUsize>,
        fail_first_n_lists: usize,
        list_delay: Option<Duration>,
    }

    #[async_trait]
    impl McpClientPort for StubClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
            let n = self.list_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(d) = self.list_delay {
                tokio::time::sleep(d).await;
            }
            if n < self.fail_first_n_lists {
                return Err(McpError::Transport {
                    server: self.label.clone(),
                    reason: "synthetic list failure".to_string(),
                });
            }
            Ok(vec![McpToolDescriptor {
                name: format!("{}_tool", self.label),
                title: None,
                description: String::new(),
                input_schema: Value::Null,
            }])
        }
        async fn call_tool(&self, _n: &str, _a: Value) -> Result<McpToolResult, McpError> {
            Ok(McpToolResult {
                content: String::new(),
                is_error: false,
            })
        }
        fn server_label(&self) -> &str {
            &self.label
        }
    }

    /// Counts handshakes, and can be told to fail, so the pooling behaviour is
    /// observable without a socket.
    struct CountingConnector {
        calls: AtomicUsize,
        fail_first_n: usize,
        delay: Option<Duration>,
        /// One shared counter across every client this connector hands out, so
        /// `tools/list` round-trips are counted per connector rather than per
        /// client instance.
        list_calls: Arc<AtomicUsize>,
        fail_first_n_lists: usize,
        list_delay: Option<Duration>,
    }

    impl CountingConnector {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_first_n: 0,
                delay: None,
                list_calls: Arc::new(AtomicUsize::new(0)),
                fail_first_n_lists: 0,
                list_delay: None,
            }
        }
        fn failing_first(n: usize) -> Self {
            Self {
                fail_first_n: n,
                ..Self::new()
            }
        }
        /// A connector whose first `n` `tools/list` calls fail.
        fn listing_fails_first(n: usize) -> Self {
            Self {
                fail_first_n_lists: n,
                ..Self::new()
            }
        }
        /// A slow `tools/list`, to widen the single-flight race window.
        fn slow_listing() -> Self {
            Self {
                list_delay: Some(Duration::from_millis(50)),
                ..Self::new()
            }
        }
        fn list_count(&self) -> usize {
            self.list_calls.load(Ordering::SeqCst)
        }
        /// A slow handshake widens the race window, so a missing lock loses
        /// reliably instead of only under load.
        fn slow() -> Self {
            Self {
                delay: Some(Duration::from_millis(50)),
                ..Self::new()
            }
        }
        fn count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl McpConnector for CountingConnector {
        async fn connect(
            &self,
            server_label: &str,
            _config: &McpServerConfig,
        ) -> Result<Arc<dyn McpClientPort>, McpError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            if n < self.fail_first_n {
                return Err(McpError::Transport {
                    server: server_label.to_string(),
                    reason: "synthetic".to_string(),
                });
            }
            Ok(Arc::new(StubClient {
                label: server_label.to_string(),
                list_calls: self.list_calls.clone(),
                fail_first_n_lists: self.fail_first_n_lists,
                list_delay: self.list_delay,
            }))
        }
    }

    fn config(url: &str) -> McpServerConfig {
        McpServerConfig {
            url: url.to_string(),
            transport: McpTransport::StreamableHttp,
            header_refs: BTreeMap::new(),
            timeout: Duration::from_secs(30),
            cache_ttl: Duration::from_secs(300),
        }
    }

    /// R3.4 — the reason the registry exists. Two executions of the same agent
    /// must not re-handshake; a `DagToolExecutor` is rebuilt every turn, so
    /// without pooling every turn pays a TLS + `initialize` round-trip.
    #[tokio::test]
    async fn two_executions_with_the_same_config_share_one_connection() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new(connector.clone());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        let a = registry.client(&key, "docs", &cfg).await.unwrap();
        let b = registry.client(&key, "docs", &cfg).await.unwrap();

        assert_eq!(
            connector.count(),
            1,
            "the second call must not re-handshake"
        );
        assert!(Arc::ptr_eq(&a, &b), "both callers must get the SAME client");
        assert_eq!(registry.len(), 1);
    }

    /// Different servers must never share a connection — sharing one would
    /// send the second server's calls over the first server's session.
    #[tokio::test]
    async fn different_keys_get_different_connections() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new(connector.clone());
        let a_cfg = config("https://a.example.com/mcp");
        let b_cfg = config("https://b.example.com/mcp");

        registry
            .client(&McpServerKey::from_config(&a_cfg), "a", &a_cfg)
            .await
            .unwrap();
        registry
            .client(&McpServerKey::from_config(&b_cfg), "b", &b_cfg)
            .await
            .unwrap();

        assert_eq!(connector.count(), 2);
        assert_eq!(registry.len(), 2);
    }

    /// The whole point of `creation_locks`. Without them the fast path misses
    /// for every racing caller and each performs its own handshake — N
    /// connections for one key, N-1 of them leaked. Verified load-bearing:
    /// removing the lock makes this fail.
    #[tokio::test]
    async fn concurrent_first_use_produces_exactly_one_handshake() {
        let connector = Arc::new(CountingConnector::slow());
        let registry = Arc::new(McpConnectionRegistry::new(connector.clone()));
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let (r, k, c) = (registry.clone(), key.clone(), cfg.clone());
            tasks.push(tokio::spawn(async move {
                r.client(&k, "docs", &c).await.map(|_| ())
            }));
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }

        assert_eq!(
            connector.count(),
            1,
            "16 racing callers must produce ONE handshake, not 16"
        );
        assert_eq!(registry.len(), 1);
    }

    /// A server that was down on the first reach must be reachable on the
    /// next turn. Caching the failure would poison the key for the life of
    /// the process, turning a transient outage into a permanent one.
    #[tokio::test]
    async fn a_failed_connect_is_not_cached() {
        let connector = Arc::new(CountingConnector::failing_first(1));
        let registry = McpConnectionRegistry::new(connector.clone());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        assert!(registry.client(&key, "docs", &cfg).await.is_err());
        assert!(registry.is_empty(), "a failure must not occupy the pool");

        assert!(
            registry.client(&key, "docs", &cfg).await.is_ok(),
            "the next attempt must retry, not replay the cached failure"
        );
        assert_eq!(connector.count(), 2);
    }

    // --- tools/list TTL cache (R3.5) ---

    /// R3.5 — the reason the cache exists. Under lazy loading the exposure
    /// stage runs on EVERY agent-loop iteration; without a cache each one
    /// pays a `tools/list` round-trip for a catalog that almost never changes.
    #[tokio::test]
    async fn a_cache_hit_skips_the_tools_list_roundtrip() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new(connector.clone());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        let first = registry.tools(&key, "docs", &cfg).await.unwrap();
        let second = registry.tools(&key, "docs", &cfg).await.unwrap();

        assert_eq!(
            connector.list_count(),
            1,
            "the second call must be served from cache"
        );
        assert!(
            Arc::ptr_eq(&first, &second),
            "both callers get the same list"
        );
    }

    /// An entry past its TTL must be refetched — a cache that never expires
    /// would pin a stale catalog for the life of the process, so a tool added
    /// server-side would stay invisible forever.
    #[tokio::test(start_paused = true)]
    async fn an_expired_entry_is_refetched() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new(connector.clone());
        let cfg = config("https://mcp.example.com/mcp"); // cache_ttl = 300s
        let key = McpServerKey::from_config(&cfg);

        registry.tools(&key, "docs", &cfg).await.unwrap();
        tokio::time::advance(Duration::from_secs(299)).await;
        registry.tools(&key, "docs", &cfg).await.unwrap();
        assert_eq!(connector.list_count(), 1, "still inside the TTL");

        tokio::time::advance(Duration::from_secs(2)).await;
        registry.tools(&key, "docs", &cfg).await.unwrap();
        assert_eq!(connector.list_count(), 2, "past the TTL, it must refetch");
    }

    /// Two servers must never share a catalog — exposing server A's tools for
    /// server B would dispatch calls to the wrong endpoint.
    #[tokio::test]
    async fn the_cache_is_keyed_per_server() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new(connector.clone());
        let a = config("https://a.example.com/mcp");
        let b = config("https://b.example.com/mcp");

        let a_tools = registry
            .tools(&McpServerKey::from_config(&a), "a", &a)
            .await
            .unwrap();
        let b_tools = registry
            .tools(&McpServerKey::from_config(&b), "b", &b)
            .await
            .unwrap();

        assert_eq!(connector.list_count(), 2);
        assert_ne!(a_tools[0].name, b_tools[0].name);
    }

    /// Same rule as a failed connect: a `tools/list` that failed must not
    /// occupy the cache, or one bad moment would blank the server's catalog
    /// until the TTL elapsed.
    #[tokio::test]
    async fn a_failed_tools_list_is_not_cached() {
        let connector = Arc::new(CountingConnector::listing_fails_first(1));
        let registry = McpConnectionRegistry::new(connector.clone());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        assert!(registry.tools(&key, "docs", &cfg).await.is_err());
        assert!(
            registry.tools(&key, "docs", &cfg).await.is_ok(),
            "the next turn must retry, not serve a cached failure"
        );
        assert_eq!(connector.list_count(), 2);
    }

    /// Single-flight. Without it, every agent turn racing on a cold or
    /// just-expired entry fires its own `tools/list` — a thundering herd
    /// against the server at exactly the moment the cache turns over.
    #[tokio::test]
    async fn concurrent_cold_reads_produce_exactly_one_tools_list() {
        let connector = Arc::new(CountingConnector::slow_listing());
        let registry = Arc::new(McpConnectionRegistry::new(connector.clone()));
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let (r, k, c) = (registry.clone(), key.clone(), cfg.clone());
            tasks.push(tokio::spawn(async move {
                r.tools(&k, "docs", &c).await.map(|_| ())
            }));
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }

        assert_eq!(
            connector.list_count(),
            1,
            "16 racing readers must produce ONE tools/list, not 16"
        );
    }

    /// A zero TTL is a legitimate operator choice meaning "never cache", not
    /// an accidental always-hit.
    #[tokio::test(start_paused = true)]
    async fn a_zero_ttl_refetches_every_time() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new(connector.clone());
        let mut cfg = config("https://mcp.example.com/mcp");
        cfg.cache_ttl = Duration::ZERO;
        let key = McpServerKey::from_config(&cfg);

        registry.tools(&key, "docs", &cfg).await.unwrap();
        registry.tools(&key, "docs", &cfg).await.unwrap();
        assert_eq!(connector.list_count(), 2, "ttl 0 must disable the cache");
    }

    /// The registry is directly constructible, so no test ever reaches for a
    /// process-wide singleton and tests stay order-independent.
    #[tokio::test]
    async fn registries_are_independent() {
        let c1 = Arc::new(CountingConnector::new());
        let c2 = Arc::new(CountingConnector::new());
        let (r1, r2) = (
            McpConnectionRegistry::new(c1.clone()),
            McpConnectionRegistry::new(c2.clone()),
        );
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_config(&cfg);

        r1.client(&key, "docs", &cfg).await.unwrap();
        r2.client(&key, "docs", &cfg).await.unwrap();

        assert_eq!(c1.count(), 1);
        assert_eq!(
            c2.count(),
            1,
            "a second registry must not see the first's pool"
        );
    }
}
