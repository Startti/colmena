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

use crate::dag_engine::infrastructure::mcp_registry::McpServerKey;
use crate::llm::domain::mcp::{McpClientPort, McpError, McpServerConfig};

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
    connector: Arc<dyn McpConnector>,
}

impl McpConnectionRegistry {
    pub fn new(connector: Arc<dyn McpConnector>) -> Self {
        Self {
            clients: DashMap::new(),
            creation_locks: DashMap::new(),
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

    struct StubClient(String);

    #[async_trait]
    impl McpClientPort for StubClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
            Ok(Vec::new())
        }
        async fn call_tool(&self, _n: &str, _a: Value) -> Result<McpToolResult, McpError> {
            Ok(McpToolResult {
                content: String::new(),
                is_error: false,
            })
        }
        fn server_label(&self) -> &str {
            &self.0
        }
    }

    /// Counts handshakes, and can be told to fail, so the pooling behaviour is
    /// observable without a socket.
    struct CountingConnector {
        calls: AtomicUsize,
        fail_first_n: usize,
        delay: Option<Duration>,
    }

    impl CountingConnector {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_first_n: 0,
                delay: None,
            }
        }
        fn failing_first(n: usize) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_first_n: n,
                delay: None,
            }
        }
        /// A slow handshake widens the race window, so a missing lock loses
        /// reliably instead of only under load.
        fn slow() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_first_n: 0,
                delay: Some(Duration::from_millis(50)),
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
            Ok(Arc::new(StubClient(server_label.to_string())))
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
