//! Process-level pool of live MCP server connections.
//!
//! An MCP connection is expensive: a TCP+TLS handshake plus a JSON-RPC
//! `initialize` round-trip before a single tool can be listed. A
//! `DagToolExecutor` is built fresh for every `llm_call` execution, so the
//! connection cannot live there — every turn of every agent loop would
//! re-handshake. This registry outlives executions and hands the same client
//! back for the same [`McpServerKey`].

use std::num::NonZeroUsize;
use std::sync::Arc;

use dashmap::DashMap;
use lru::LruCache;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::dag_engine::infrastructure::mcp_registry::McpServerKey;
use std::future::Future;

use crate::llm::domain::mcp::{McpClientPort, McpError, McpServerConfig, McpToolDescriptor};

/// Pool of connections keyed by [`McpServerKey`].
pub struct McpConnectionRegistry {
    clients: DashMap<McpServerKey, Arc<dyn McpClientPort>>,
    /// One lock per key, so two callers racing on a cold key produce one
    /// handshake instead of two.
    ///
    /// Entries are dropped by eviction, but ONLY while the registry holds the
    /// sole reference. Removing a lock a waiter still holds would let a fresh
    /// caller mint a second, independent mutex for the same key: neither sees
    /// the other's insert, both pass the `clients` re-check while it is empty,
    /// and both connect — two live connections for one key.
    ///
    /// Two removal MECHANISMS, reached from three call sites. The targeted
    /// one is `evict_if_needed`'s `remove_if`, which drops only the key it
    /// just evicted. The other is `sweep_orphan_locks`, a `retain` over the
    /// whole map that keeps any entry still pooled or still held; it runs
    /// both at the tail of `evict_if_needed` and from the failure paths of
    /// `client` and `tools`. Those failure paths do NOT remove their own
    /// entry directly — they trigger the whole-map sweep and let it decide.
    /// A lock can therefore be collected without any eviction happening.
    creation_locks: DashMap<McpServerKey, Arc<Mutex<()>>>,
    /// When each key's connect last failed.
    ///
    /// NOT a cached failure — the value is a timestamp, never an error, and it is
    /// cleared as soon as one attempt succeeds. It exists because a connect costs
    /// up to the server's full `timeout` and runs BEFORE the model is invoked, so
    /// a server that is simply down would otherwise add that to every turn of a
    /// conversation for the life of the process.
    recent_failures: DashMap<McpServerKey, std::time::Instant>,
    /// Last `tools/list` result per server, with the moment it was fetched.
    tool_cache: DashMap<McpServerKey, CachedTools>,
    /// Single-flight for cache fills, separate from `creation_locks` so a
    /// catalog refresh never serialises behind an unrelated handshake.
    fetch_locks: DashMap<McpServerKey, Arc<Mutex<()>>>,
    /// Eviction order. Used only to pick a victim; the cap is enforced by
    /// `evict_if_needed`, so the cache itself is given generous headroom
    /// rather than being allowed to drop entries behind our back.
    lru: Mutex<LruCache<McpServerKey, ()>>,
    max_entries: usize,
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

/// How many servers stay pooled before the least recently used is dropped.
///
/// Sized for the credential-scoped key space: one entry per (server, distinct
/// resolved credential) actually in flight, not per declared server. Two
/// callers holding the same secret share one entry; a rotation mints another.
/// Generous enough that a normal deployment never evicts, small enough to
/// bound a pathological one.
pub const DEFAULT_MAX_POOLED_SERVERS: usize = 128;

impl Default for McpConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpConnectionRegistry {
    pub fn new() -> Self {
        Self::with_max_entries(DEFAULT_MAX_POOLED_SERVERS)
    }

    pub fn with_max_entries(max_entries: usize) -> Self {
        // Headroom, as in `pool_registry`: eviction is meant to be driven by
        // `evict_if_needed`, not by the cache dropping a record on its own.
        // Headroom makes that rare — it does NOT make it impossible, which is
        // why `touch` handles the displacement instead of assuming it away.
        let lru_cap = max_entries.max(1).saturating_mul(10).max(1024);
        Self::with_capacities(max_entries, lru_cap)
    }

    /// `max_entries` with an explicit LRU capacity, so the displacement path
    /// in `touch` can be exercised without minting 1024 keys.
    fn with_capacities(max_entries: usize, lru_cap: usize) -> Self {
        let max_entries = max_entries.max(1);
        let lru_cap = NonZeroUsize::new(lru_cap.max(1)).expect("clamped to at least 1");
        Self {
            clients: DashMap::new(),
            creation_locks: DashMap::new(),
            recent_failures: DashMap::new(),
            tool_cache: DashMap::new(),
            fetch_locks: DashMap::new(),
            lru: Mutex::new(LruCache::new(lru_cap)),
            max_entries,
        }
    }

    /// Mark a key as most recently used.
    ///
    /// `push`, deliberately, NOT `put`. `put` returns `Option<V>` — it throws
    /// away the key half and so cannot report that the cache dropped a record
    /// of its own to stay within capacity. `push` returns the displaced
    /// `(key, value)`.
    ///
    /// That distinction is load-bearing. `evict_if_needed` can only choose a
    /// victim the LRU still knows, so a key whose record the cache dropped
    /// silently would stay in `clients` forever, unevictable, and the cap
    /// would quietly stop holding. Headroom makes the displacement rare;
    /// handling it here is what closes it.
    ///
    /// SCOPE, since an earlier version of this comment overstated it: this
    /// closes displacement by the LRU's own capacity, and nothing else. This
    /// method never ADDS to `clients` — it can remove from it, via
    /// `collect_displaced` — so it cannot close the other route, a key
    /// reaching `clients` with no LRU record. That one is closed in
    /// `register_and_pool`, which does the LRU record and the pool insert
    /// under a single lock; see its comment.
    ///
    /// REACHABILITY, so nobody reads the displacement branch here as a normal
    /// path. `touch` runs only on a hit, and a key in `clients` is in the LRU,
    /// so its `push` is a re-rank and displaces nothing. The branch fires only
    /// through a narrow window — the disclosed `tools` cold-fill orphan, or
    /// the gap inside `evict_if_needed` between popping a victim and removing
    /// it. It is defensive, and NO test pins it: driving it deterministically
    /// means staging one of those races.
    async fn touch(&self, key: &McpServerKey) {
        let displaced = { self.lru.lock().await.push(key.clone(), ()) };
        self.collect_displaced(key, displaced);
    }

    /// Register `key` in the LRU and pool `client` under the SAME LRU lock.
    ///
    /// Ordering the two is not enough. Every other writer of the LRU takes
    /// this mutex, so releasing it between the `push` and the `insert` leaves
    /// a window — on a multi-threaded runtime a genuinely concurrent one, not
    /// merely an await point — in which another task can displace this very
    /// key and run `drop_footprint` on it BEFORE it is in `clients`. The
    /// removal would find nothing, our insert would land afterwards, and the
    /// key would sit in `clients` with no LRU record: unevictable, exactly
    /// the failure the `push` handling exists to prevent.
    ///
    /// Holding the lock across both closes it. `drop_footprint` never takes
    /// the LRU mutex, so collecting the displaced key afterwards cannot
    /// deadlock against it.
    async fn register_and_pool(&self, key: &McpServerKey, client: Arc<dyn McpClientPort>) {
        let displaced = {
            let mut lru = self.lru.lock().await;
            let displaced = lru.push(key.clone(), ());
            self.clients.insert(key.clone(), client);
            displaced
        };
        self.collect_displaced(key, displaced);
    }

    /// Drop the footprint of a key the LRU displaced to stay within capacity.
    ///
    /// `push` also returns the old entry when the key was already present.
    /// That is a re-rank, not a displacement, and must not drop anything.
    fn collect_displaced(&self, key: &McpServerKey, displaced: Option<(McpServerKey, ())>) {
        if let Some((displaced, _)) = displaced {
            if displaced != *key {
                self.drop_footprint(&displaced, "lru_capacity_displaced");
            }
        }
    }

    /// Remove every trace of `key`: client, cached catalog and both locks.
    ///
    /// Dropping only the client would leave the other three growing without
    /// bound, which is the problem this exists to solve.
    ///
    /// A lock is removed ONLY when the registry holds its sole reference.
    /// Removing one a waiter still holds would leave that waiter serialised
    /// against a mutex nobody else can reach: a fresh caller would mint a
    /// second, independent mutex for the same key, both would pass the
    /// `clients` re-check while it is empty, and BOTH would connect — two
    /// live connections for one key, the invariant this registry exists to
    /// hold. `remove_if` evaluates the predicate under the shard lock, so no
    /// one can clone the `Arc` between the check and the removal. A contended
    /// entry survives this pass and is collected by `sweep_orphan_locks` once
    /// its waiter is done.
    fn drop_footprint(&self, key: &McpServerKey, reason: &'static str) {
        self.clients.remove(key);
        self.tool_cache.remove(key);
        self.creation_locks
            .remove_if(key, |_, lock| Arc::strong_count(lock) == 1);
        self.fetch_locks
            .remove_if(key, |_, lock| Arc::strong_count(lock) == 1);
        tracing::debug!(
            target: "colmena::mcp",
            event = "mcp.pool_evicted",
            key = %key.as_str(),
            reason,
            "dropped a pooled MCP connection"
        );
    }

    /// Drop least-recently-used keys until the pool is within its cap.
    ///
    /// Each victim's whole footprint goes via `drop_footprint`; see there for
    /// why a held lock survives the pass.
    ///
    /// LIMIT, stated rather than implied: this bounds the MAP, not live
    /// sockets. Evicting drops the registry's `Arc`, so the connection closes
    /// only once every other holder drops theirs. A caller mid-request keeps
    /// its connection alive until it is done, by design — eviction means "no
    /// longer handed out", never "torn out from under a caller".
    async fn evict_if_needed(&self) {
        while self.clients.len() > self.max_entries {
            let victim = { self.lru.lock().await.pop_lru() };
            let Some((victim, _)) = victim else {
                // The LRU is empty while the map is over cap: nothing left to
                // choose, so stop rather than spin.
                break;
            };
            self.drop_footprint(&victim, "lru_capacity");
        }
        self.sweep_orphan_locks();
    }

    /// Drop lock entries whose key is no longer pooled and whose mutex nobody
    /// holds.
    ///
    /// Without this, a lock that was contended at the moment its key was
    /// evicted would linger forever: eviction never revisits a key it already
    /// popped from the LRU.
    ///
    /// Runs on every `evict_if_needed` call, NOT only when something was
    /// actually evicted. An earlier version gated it on a real eviction so
    /// that a comment claiming "only on eviction" would be true — which
    /// introduced a leak, because a FAILED connect creates a lock entry and
    /// returns before `evict_if_needed` is ever reached, and in a deployment
    /// that never hits the cap no eviction would ever come to collect it. The
    /// cadence was load-bearing; the comment was the thing that was wrong.
    /// Cost is a walk of two maps. The POOLED population is capped by
    /// `max_entries`, but entries for connects that are in flight or have
    /// failed are not in `clients` and so are bounded by concurrency instead —
    /// a burst of simultaneous failures against distinct servers makes each
    /// failure walk a transiently larger map. Bounded and self-draining, not
    /// capped.
    fn sweep_orphan_locks(&self) {
        let pooled = |k: &McpServerKey| self.clients.contains_key(k);
        self.creation_locks
            .retain(|k, lock| pooled(k) || Arc::strong_count(lock) > 1);
        self.fetch_locks
            .retain(|k, lock| pooled(k) || Arc::strong_count(lock) > 1);
    }

    /// The pooled client for `key`, connecting once if this is its first use.
    ///
    /// A failed connect is NOT cached: a server that was down when the agent
    /// first reached for it must be reachable on the next turn, not poisoned
    /// for the life of the process.
    ///
    /// Re-ATTEMPTS are spaced all the same, because not caching a failure is not
    /// the same as retrying it without limit. An attempt costs up to `timeout`
    /// and happens before the model is invoked, so a dead server would otherwise
    /// tax every turn by that much, forever. Within one `timeout` of a failure
    /// the attempt is skipped and the caller degrades immediately.
    ///
    /// The spacing is the server's OWN `timeout` rather than a constant, because
    /// that value already says what an attempt costs: waiting it out caps the
    /// time spent re-dialling a dead server at roughly half, whether its timeout
    /// is one second or thirty. A server that recovers is picked up on the first
    /// attempt after that window.
    pub async fn client<F, Fut>(
        &self,
        key: &McpServerKey,
        config: &McpServerConfig,
        connect: F,
    ) -> Result<Arc<dyn McpClientPort>, McpError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Arc<dyn McpClientPort>, McpError>> + Send,
    {
        if let Some(existing) = self.clients.get(key) {
            let client = existing.clone();
            drop(existing);
            self.touch(key).await;
            tracing::debug!(
                target: "colmena::mcp",
                event = "mcp.connection_reused",
                key = %key.as_str(),
                raced = false,
                "reused a pooled MCP connection"
            );
            return Ok(client);
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
            let client = existing.clone();
            drop(existing);
            self.touch(key).await;
            tracing::debug!(
                target: "colmena::mcp",
                event = "mcp.connection_reused",
                key = %key.as_str(),
                raced = true,
                "reused a pooled MCP connection"
            );
            return Ok(client);
        }

        if let Some(failed_at) = self.recent_failures.get(key) {
            let since = failed_at.elapsed();
            drop(failed_at);
            if since < config.timeout {
                // The attempt is SKIPPED, and the caller degrades. Without this
                // line a server inside its cooldown drops out of every turn with
                // nothing in the log to say why.
                tracing::debug!(
                    target: "colmena::mcp",
                    event = "mcp.connection_cooldown",
                    key = %key.as_str(),
                    since_ms = since.as_millis() as u64,
                    window_ms = config.timeout.as_millis() as u64,
                    "skipped dialling an MCP server that failed recently"
                );
                return Err(McpError::Transport {
                    // The key is a salted hash carrying no alias and no URL, so
                    // it would tell an operator nothing. The caller adds the
                    // alias it already knows.
                    server: "(recently failed)".to_string(),
                    reason: format!(
                        "not retried: the last attempt failed {}s ago and another \
                         would cost up to {}s before the model runs. It will be \
                         dialled again once that window passes.",
                        since.as_secs(),
                        config.timeout.as_secs()
                    ),
                });
            }
            self.recent_failures.remove(key);
        }

        let connected = connect().await;
        let client = match connected {
            Ok(client) => client,
            Err(e) => {
                self.recent_failures
                    .insert(key.clone(), std::time::Instant::now());
                // This key never made it into `clients`, so nothing will ever
                // evict it and nothing will sweep on its behalf. Release our
                // hold and collect it here, or a server that fails once for a
                // session that never retries leaves its lock behind for the
                // life of the process.
                drop(_guard);
                drop(lock);
                self.sweep_orphan_locks();
                return Err(e);
            }
        };
        // One lock covers both the LRU record and the pool entry; see
        // `register_and_pool`.
        // A caller cancelled on that lock leaves neither, and no concurrent
        // displacement can slip between them.
        self.register_and_pool(key, client.clone()).await;
        self.evict_if_needed().await;
        tracing::debug!(
            target: "colmena::mcp",
            event = "mcp.connection_opened",
            key = %key.as_str(),
            pooled = self.clients.len(),
            "opened and pooled a new MCP connection"
        );
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
    pub async fn tools<F, Fut>(
        &self,
        key: &McpServerKey,
        config: &McpServerConfig,
        connect: F,
    ) -> Result<Arc<Vec<McpToolDescriptor>>, McpError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Arc<dyn McpClientPort>, McpError>> + Send,
    {
        if let Some(fresh) = self.cached_if_fresh(key, config) {
            // A catalog hit is a USE. Under lazy loading the exposure stage
            // calls `tools` on every agent-loop iteration and may never call
            // `client` again, so without this a server in constant use would
            // sink to the bottom of the LRU and be evicted ahead of an idle
            // one — the eviction order would be exactly backwards for the
            // access pattern the cache exists to serve.
            self.touch(key).await;
            tracing::debug!(
                target: "colmena::mcp",
                event = "mcp.catalog_hit",
                key = %key.as_str(),
                tools = fresh.len(),
                raced = false,
                "served an MCP tool catalog from cache"
            );
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
            // A catalog hit is a USE. Under lazy loading the exposure stage
            // calls `tools` on every agent-loop iteration and may never call
            // `client` again, so without this a server in constant use would
            // sink to the bottom of the LRU and be evicted ahead of an idle
            // one — the eviction order would be exactly backwards for the
            // access pattern the cache exists to serve.
            self.touch(key).await;
            tracing::debug!(
                target: "colmena::mcp",
                event = "mcp.catalog_hit",
                key = %key.as_str(),
                tools = fresh.len(),
                raced = true,
                "served an MCP tool catalog from cache"
            );
            return Ok(fresh);
        }

        // Same cleanup obligation as `client`'s failure path, for the OTHER
        // lock map. `client` cannot collect this entry on our behalf: we still
        // hold a clone of it while it runs, so its sweep sees `strong_count >
        // 1` and correctly leaves it alone. If we returned without sweeping,
        // a server reached only through `tools` that is permanently
        // unreachable would strand one `fetch_locks` entry for good.
        let filled = async {
            let client = self.client(key, config, connect).await?;
            Ok::<_, McpError>(Arc::new(client.list_tools().await?))
        }
        .await;
        let tools = match filled {
            Ok(tools) => tools,
            Err(e) => {
                drop(_guard);
                drop(lock);
                self.sweep_orphan_locks();
                return Err(e);
            }
        };
        self.tool_cache.insert(
            key.clone(),
            CachedTools {
                tools: tools.clone(),
                fetched_at: Instant::now(),
            },
        );
        // Re-register in the LRU, because between `client` returning and this
        // insert there is a real await — the `list_tools` round-trip — and a
        // racing task's `evict_if_needed` may have popped this key in it. The
        // insert would then land on a key the LRU no longer names, and since
        // eviction can only choose keys it still names, nothing would ever
        // collect this catalog entry. The lock maps survive that race through
        // their `strong_count` guard; `tool_cache` has no equivalent, so it
        // needs the key put back.
        //
        // Cheap on the ordinary path: the key is normally still present, and
        // `push` on a present key is a re-rank that displaces nothing.
        //
        // Pinned by `a_catalog_cached_for_a_key_evicted_mid_fill_stays_collectable`,
        // which stages the racing eviction from inside `list_tools`.
        self.touch(key).await;
        // Logged after the fetch rather than before it, so the count is the real
        // one and a failed fetch is not reported as a miss that filled the cache
        // — that path already leaves through `mcp.server_unavailable`.
        tracing::debug!(
            target: "colmena::mcp",
            event = "mcp.catalog_miss",
            key = %key.as_str(),
            tools = tools.len(),
            "fetched an MCP tool catalog and cached it"
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
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::dag_engine::infrastructure::mcp_registry::key::CredentialFingerprint;
    use crate::llm::domain::mcp::{McpToolDescriptor, McpToolResult, McpTransport};

    /// Counts `tools/list` round-trips, so cache hits are observable, and can
    /// be told to fail so the not-cached-on-failure path is testable.
    /// Runs inside `list_tools`, i.e. exactly in the window `tools()` leaves
    /// open between pooling the client and caching the catalog.
    type DuringList = Arc<dyn Fn() + Send + Sync>;

    struct StubClient {
        label: String,
        list_calls: Arc<AtomicUsize>,
        fail_first_n_lists: usize,
        list_delay: Option<Duration>,
        during_list: Option<DuringList>,
    }

    #[async_trait]
    impl McpClientPort for StubClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
            let n = self.list_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(hook) = &self.during_list {
                hook();
            }
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
        during_list: Option<DuringList>,
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
                during_list: None,
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

    impl CountingConnector {
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
                during_list: self.during_list.clone(),
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

    // --- Eviction ---

    /// The cap must actually hold. Without it the pool grows one entry per
    /// (server, agent session) for the life of the process.
    #[tokio::test]
    async fn the_pool_stays_within_its_cap() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(2);

        for host in ["a", "b", "c"] {
            let cfg = config(&format!("https://{host}.example.com/mcp"));
            let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());
            registry
                .client(&key, &cfg, || connector.connect(host, &cfg))
                .await
                .unwrap();
        }

        assert_eq!(registry.len(), 2, "three servers, a cap of two");
    }

    /// LRU, not arbitrary. A and B are identical in every way except WHEN they
    /// were last used, so only the eviction ORDER can decide which survives —
    /// picking a victim any other way flips this.
    #[tokio::test]
    async fn the_least_recently_used_server_is_the_one_evicted() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(2);

        let mk = |h: &str| {
            let cfg = config(&format!("https://{h}.example.com/mcp"));
            let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());
            (key, cfg)
        };
        let (a_key, a_cfg) = mk("a");
        let (b_key, b_cfg) = mk("b");
        let (c_key, c_cfg) = mk("c");

        registry
            .client(&a_key, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        registry
            .client(&b_key, &b_cfg, || connector.connect("b", &b_cfg))
            .await
            .unwrap();
        // Re-touch A so B becomes the oldest.
        registry
            .client(&a_key, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        registry
            .client(&c_key, &c_cfg, || connector.connect("c", &c_cfg))
            .await
            .unwrap();

        assert_eq!(registry.len(), 2);
        // A must still be pooled: reusing it performs NO new handshake.
        let before = connector.count();
        registry
            .client(&a_key, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        assert_eq!(
            connector.count(),
            before,
            "A was used most recently and must have survived"
        );
        // B must be gone: reusing it DOES handshake again.
        registry
            .client(&b_key, &b_cfg, || connector.connect("b", &b_cfg))
            .await
            .unwrap();
        assert_eq!(
            connector.count(),
            before + 1,
            "B was the least recently used and must have been evicted"
        );
    }

    /// Eviction must drop a key's WHOLE footprint. Dropping only the client
    /// would leave the cached catalog and both lock maps growing without
    /// bound — the very thing the cap exists to stop. Observable through the
    /// catalog: a re-admitted server must re-fetch `tools/list`.
    #[tokio::test]
    async fn eviction_drops_the_cached_catalog_with_the_client() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(1);

        let a_cfg = config("https://a.example.com/mcp");
        let a_key = McpServerKey::from_resolved(&a_cfg, &CredentialFingerprint::none());
        let b_cfg = config("https://b.example.com/mcp");
        let b_key = McpServerKey::from_resolved(&b_cfg, &CredentialFingerprint::none());

        registry
            .tools(&a_key, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        assert_eq!(connector.list_count(), 1);

        // Admitting B evicts A.
        registry
            .tools(&b_key, &b_cfg, || connector.connect("b", &b_cfg))
            .await
            .unwrap();

        // A comes back: its catalog must be gone, not served stale from a map
        // the eviction forgot to clear.
        registry
            .tools(&a_key, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        assert_eq!(
            connector.list_count(),
            3,
            "an evicted server must re-fetch its catalog, not hit a surviving cache entry"
        );
    }

    /// A cap of zero would evict every connection the instant it was created,
    /// turning the pool into a no-op that re-handshakes every call. Clamped to
    /// one instead.
    #[tokio::test]
    async fn a_cap_below_one_is_clamped_rather_than_disabling_the_pool() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(0);

        let cfg = config("https://a.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());
        registry
            .client(&key, &cfg, || connector.connect("a", &cfg))
            .await
            .unwrap();
        registry
            .client(&key, &cfg, || connector.connect("a", &cfg))
            .await
            .unwrap();

        assert_eq!(registry.len(), 1, "the pool must still hold one entry");
        assert_eq!(
            connector.count(),
            1,
            "the second call must still be served from the pool"
        );
    }

    /// A connect that FAILS must not strand its lock entry.
    ///
    /// The entry is created before the handshake is attempted, and the failure
    /// path returns before `evict_if_needed` is ever reached — so nothing
    /// would ever evict this key and nothing would sweep on its behalf. In a
    /// deployment that never reaches the cap, that entry would live as long as
    /// the process. Only whether the failure path cleans up can decide this:
    /// the cap is never approached here.
    #[tokio::test]
    async fn a_failed_connect_leaves_no_lock_behind() {
        let connector = Arc::new(CountingConnector::failing_first(1));
        let registry = McpConnectionRegistry::with_max_entries(128);

        let cfg = config("https://down.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        assert!(registry
            .client(&key, &cfg, || connector.connect("down", &cfg))
            .await
            .is_err());

        assert!(registry.is_empty(), "a failure must not occupy the pool");
        assert!(
            !registry.creation_locks.contains_key(&key),
            "nor leave its creation lock behind, with no eviction ever coming to collect it"
        );
    }

    /// A catalog cached for a key evicted mid-fill must stay collectable.
    ///
    /// `tools()` pools the client, then awaits `list_tools`, then caches the
    /// catalog. A racing eviction inside that await pops the key, so the cache
    /// write lands on a key the LRU no longer names — and eviction can only
    /// choose keys it names, so nothing would ever collect that entry. Before
    /// rotation existed this self-healed on the next access; a rotated-away
    /// credential has no next access.
    ///
    /// The race is staged synchronously: the stub's `list_tools` runs a hook in
    /// exactly that window and evicts the key itself. No sleeps, no threads.
    /// Drop the `touch` after the insert and this fails.
    #[tokio::test]
    async fn a_catalog_cached_for_a_key_evicted_mid_fill_stays_collectable() {
        let registry = Arc::new(McpConnectionRegistry::new());
        let cfg = config("https://midfill.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        // The eviction that races the fill, run from inside `list_tools`.
        let evictor = {
            let registry = registry.clone();
            let key = key.clone();
            Arc::new(move || {
                let popped = registry
                    .lru
                    .try_lock()
                    .expect("uncontended in test")
                    .pop_lru();
                assert_eq!(
                    popped.map(|(k, _)| k),
                    Some(key.clone()),
                    "the hook must evict the key being filled"
                );
                registry.drop_footprint(&key, "test_race");
            }) as DuringList
        };

        let connector = CountingConnector {
            during_list: Some(evictor),
            ..CountingConnector::new()
        };
        registry
            .tools(&key, &cfg, || connector.connect("midfill", &cfg))
            .await
            .unwrap();

        // The catalog is cached, so eviction MUST be able to name the key again.
        let named = registry.lru.lock().await.peek(&key).is_some();
        assert!(
            named,
            "the key is back out of the LRU while its catalog is cached: nothing \
             would ever collect that entry"
        );
    }

    /// A caller cancelled mid-connect must not strand an unevictable key.
    ///
    /// `touch` suspends on the `lru` mutex. Holding that mutex parks the
    /// connect path at exactly that await, and dropping the future there IS
    /// the cancellation — no timing, no sleeps. With `clients.insert` ordered
    /// before `touch`, the key would survive in `clients` with no LRU record
    /// and `pop_lru` could never name it again. Swap the two lines back and
    /// this fails.
    #[tokio::test]
    async fn a_cancelled_connect_does_not_strand_an_unevictable_key() {
        use std::future::Future;
        use std::task::{Context, Poll};

        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(128);

        let cfg = config("https://cancelled.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        // Park the connect path on the LRU lock.
        let held = registry.lru.lock().await;

        let mut connecting =
            Box::pin(registry.client(&key, &cfg, || connector.connect("cancelled", &cfg)));
        let mut cx = Context::from_waker(std::task::Waker::noop());
        for _ in 0..8 {
            assert!(
                matches!(connecting.as_mut().poll(&mut cx), Poll::Pending),
                "the connect path must park on the held LRU lock, not complete"
            );
        }

        drop(connecting); // the cancellation
        drop(held);

        assert!(
            registry.is_empty(),
            "a cancelled connect must leave nothing in `clients`: a key there \
             with no LRU record is unevictable, since eviction can only choose \
             keys the LRU still names"
        );
    }

    /// A record the LRU drops on its OWN capacity must not leave the key
    /// stranded in `clients`.
    ///
    /// `evict_if_needed` can only pick a victim the LRU still knows, so a key
    /// whose record vanished silently would never be evictable again and the
    /// cap would stop holding. `max_entries` is deliberately generous here so
    /// that `evict_if_needed` NEVER fires: the only thing that can remove `a`
    /// is the displacement handling itself. Revert `push` to `put` and this
    /// fails, because `put` cannot report which key it dropped.
    ///
    /// The method under test is `register_and_pool`, NOT `touch`. All three
    /// keys are first-time keys, so every call takes the cold-connect path;
    /// `touch` is never reached. The two share `collect_displaced`, so the
    /// shared logic is covered — but `touch`'s own displacement branch is
    /// not; see its doc for why that branch is nearly unreachable.
    #[tokio::test]
    async fn a_key_the_lru_displaces_on_its_own_is_not_left_stranded() {
        let connector = Arc::new(CountingConnector::new());
        // Room for three clients, but an LRU that only remembers two.
        let registry = McpConnectionRegistry::with_capacities(3, 2);

        let mk = |h: &str| {
            let cfg = config(&format!("https://{h}.example.com/mcp"));
            (
                McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none()),
                cfg,
            )
        };
        let (a, a_cfg) = mk("a");

        for host in ["a", "b", "c"] {
            let (key, cfg) = mk(host);
            registry
                .client(&key, &cfg, || connector.connect(host, &cfg))
                .await
                .unwrap();
        }

        assert!(
            !registry.clients.contains_key(&a),
            "`a` was displaced from the LRU by `c`; leaving it pooled makes it \
             unevictable, since eviction can only choose keys the LRU knows"
        );
        assert_eq!(
            registry.len(),
            2,
            "the pool tracks only what the LRU can still name"
        );

        // And it is genuinely gone, not merely unreachable: asking again is a
        // fresh handshake.
        let before = connector.calls.load(Ordering::SeqCst);
        registry
            .client(&a, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        assert_eq!(
            connector.calls.load(Ordering::SeqCst),
            before + 1,
            "a displaced key reconnects rather than serving a stale entry"
        );
    }

    /// The `tools` twin of `a_failed_connect_leaves_no_lock_behind`.
    ///
    /// The exclusivity claim applies to the FETCH lock only: `client` cannot
    /// collect that entry for us, because we hold a clone of it while `client`
    /// runs, so its sweep correctly leaves it alone. Only `tools` cleaning up
    /// after itself can satisfy that assertion — the cap is never approached
    /// here, so no eviction will ever come.
    ///
    /// The second assertion, on the creation lock, is NOT exclusive; see the
    /// comment on it.
    #[tokio::test]
    async fn a_failed_catalog_fetch_leaves_no_lock_behind() {
        let connector = Arc::new(CountingConnector::failing_first(1));
        let registry = McpConnectionRegistry::with_max_entries(128);

        let cfg = config("https://down.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        assert!(registry
            .tools(&key, &cfg, || connector.connect("down", &cfg))
            .await
            .is_err());

        assert!(
            !registry.fetch_locks.contains_key(&key),
            "tools must sweep its own fetch lock; client cannot, because we still hold it"
        );
        // NOT a proof that `client`'s own failure path ran: `tools`' sweep
        // walks BOTH lock maps, so this would still pass with `client`'s
        // cleanup deleted. `a_failed_connect_leaves_no_lock_behind` is the
        // test that isolates that path; this is only a check that the fetch
        // failure leaves neither map dirty.
        assert!(
            !registry.creation_locks.contains_key(&key),
            "no creation lock survives either, whichever sweep collected it"
        );
    }

    /// The race that removing lock entries reopened. A waiter holding a clone
    /// of a key's mutex must NOT have that mutex dropped from the map: a fresh
    /// caller would then mint a second, independent mutex for the same key,
    /// both would pass the `clients` re-check while it is empty, and both
    /// would connect — two live connections for one key.
    ///
    /// The victim is evicted either way; the ONLY thing that differs is
    /// whether someone still holds its lock, so nothing but the held-clone
    /// check can decide whether that lock survives.
    #[tokio::test]
    async fn a_lock_someone_still_holds_is_not_evicted() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(2);

        let victim_cfg = config("https://victim.example.com/mcp");
        let victim = McpServerKey::from_resolved(&victim_cfg, &CredentialFingerprint::none());
        let other_cfg = config("https://other.example.com/mcp");
        let other = McpServerKey::from_resolved(&other_cfg, &CredentialFingerprint::none());

        registry
            .client(&victim, &victim_cfg, || {
                connector.connect("victim", &victim_cfg)
            })
            .await
            .unwrap();
        registry
            .client(&other, &other_cfg, || {
                connector.connect("other", &other_cfg)
            })
            .await
            .unwrap();

        // Stand in for a waiter that grabbed the mutex before eviction ran.
        let waiter = registry
            .creation_locks
            .get(&victim)
            .map(|e| e.value().clone())
            .expect("the lock exists while the key is pooled");

        // `victim` is now least-recently-used, so admitting a third evicts it.
        let third_cfg = config("https://third.example.com/mcp");
        let third = McpServerKey::from_resolved(&third_cfg, &CredentialFingerprint::none());
        registry
            .client(&third, &third_cfg, || {
                connector.connect("third", &third_cfg)
            })
            .await
            .unwrap();

        assert!(
            !registry.clients.contains_key(&victim),
            "the victim's CLIENT is evicted as usual"
        );
        assert!(
            registry.creation_locks.contains_key(&victim),
            "but its lock must survive while a waiter still holds it, or a fresh \
             caller would mint a second mutex and both would connect"
        );
        drop(waiter);
    }

    /// Once the waiter is gone the orphan must be collected, or a lock that
    /// happened to be contended at eviction time would linger for the life of
    /// the process — eviction never revisits a key it already popped.
    #[tokio::test]
    async fn an_orphan_lock_is_swept_once_its_holder_releases() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(1);

        let a_cfg = config("https://a.example.com/mcp");
        let a = McpServerKey::from_resolved(&a_cfg, &CredentialFingerprint::none());
        registry
            .client(&a, &a_cfg, || connector.connect("a", &a_cfg))
            .await
            .unwrap();
        let waiter = registry
            .creation_locks
            .get(&a)
            .map(|e| e.value().clone())
            .expect("lock exists");

        let b_cfg = config("https://b.example.com/mcp");
        let b = McpServerKey::from_resolved(&b_cfg, &CredentialFingerprint::none());
        registry
            .client(&b, &b_cfg, || connector.connect("b", &b_cfg))
            .await
            .unwrap();
        assert!(registry.creation_locks.contains_key(&a), "still held");

        drop(waiter);

        // Any later eviction sweeps it.
        let c_cfg = config("https://c.example.com/mcp");
        let c = McpServerKey::from_resolved(&c_cfg, &CredentialFingerprint::none());
        registry
            .client(&c, &c_cfg, || connector.connect("c", &c_cfg))
            .await
            .unwrap();

        assert!(
            !registry.creation_locks.contains_key(&a),
            "an orphan must be collected once nobody holds it"
        );
    }

    /// A server kept warm entirely through `tools` — the lazy-loading access
    /// pattern — must keep its LRU rank. Both keys are connected once and
    /// never touched through `client` again, so only whether a CATALOG hit
    /// counts as a use can decide which one survives.
    #[tokio::test]
    async fn a_catalog_hit_refreshes_lru_rank() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::with_max_entries(2);

        let busy_cfg = config("https://busy.example.com/mcp");
        let busy = McpServerKey::from_resolved(&busy_cfg, &CredentialFingerprint::none());
        let idle_cfg = config("https://idle.example.com/mcp");
        let idle = McpServerKey::from_resolved(&idle_cfg, &CredentialFingerprint::none());

        registry
            .tools(&busy, &busy_cfg, || connector.connect("busy", &busy_cfg))
            .await
            .unwrap();
        registry
            .tools(&idle, &idle_cfg, || connector.connect("idle", &idle_cfg))
            .await
            .unwrap();
        // Busy keeps working, served purely from its cached catalog.
        registry
            .tools(&busy, &busy_cfg, || connector.connect("busy", &busy_cfg))
            .await
            .unwrap();

        let third_cfg = config("https://third.example.com/mcp");
        let third = McpServerKey::from_resolved(&third_cfg, &CredentialFingerprint::none());
        registry
            .client(&third, &third_cfg, || {
                connector.connect("third", &third_cfg)
            })
            .await
            .unwrap();

        assert!(
            registry.clients.contains_key(&busy),
            "a server in constant use through tools() must not be evicted first"
        );
        assert!(
            !registry.clients.contains_key(&idle),
            "the genuinely idle server is the one that should go"
        );
    }

    /// R3.4 — the reason the registry exists. Two executions of the same agent
    /// must not re-handshake; a `DagToolExecutor` is rebuilt every turn, so
    /// without pooling every turn pays a TLS + `initialize` round-trip.
    #[tokio::test]
    async fn two_executions_with_the_same_config_share_one_connection() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new();
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        let a = registry
            .client(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        let b = registry
            .client(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();

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
        let registry = McpConnectionRegistry::new();
        let a_cfg = config("https://a.example.com/mcp");
        let b_cfg = config("https://b.example.com/mcp");

        registry
            .client(
                &McpServerKey::from_resolved(&a_cfg, &CredentialFingerprint::none()),
                &a_cfg,
                || connector.connect("a", &a_cfg),
            )
            .await
            .unwrap();
        registry
            .client(
                &McpServerKey::from_resolved(&b_cfg, &CredentialFingerprint::none()),
                &b_cfg,
                || connector.connect("b", &b_cfg),
            )
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
        let registry = Arc::new(McpConnectionRegistry::new());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let (r, k, c, conn) = (
                registry.clone(),
                key.clone(),
                cfg.clone(),
                connector.clone(),
            );
            tasks.push(tokio::spawn(async move {
                r.client(&k, &c, || conn.connect("docs", &c))
                    .await
                    .map(|_| ())
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

    /// A server that was down must be reachable again, not poisoned for the life
    /// of the process. The window is what changed: the retry is SPACED by the
    /// server's own `timeout`, so a dead server cannot charge that much to every
    /// turn. A near-zero timeout here keeps the test fast while exercising the
    /// real boundary.
    #[tokio::test]
    async fn a_failed_connect_is_retried_once_its_window_passes() {
        let connector = Arc::new(CountingConnector::failing_first(1));
        let registry = McpConnectionRegistry::new();
        let mut cfg = config("https://mcp.example.com/mcp");
        cfg.timeout = Duration::from_millis(20);
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        assert!(registry
            .client(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .is_err());
        assert!(registry.is_empty(), "a failure must not occupy the pool");

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            registry
                .client(&key, &cfg, || connector.connect("docs", &cfg))
                .await
                .is_ok(),
            "past its window the server must be dialled again"
        );
        assert_eq!(connector.count(), 2, "the second attempt really happened");
        // The mark must be GONE, not merely ignored. Behaviour is identical
        // either way — the window is recomputed on the next call — so nothing
        // else notices, and a mutation dropping the removal passed until this
        // assertion existed. What it costs is memory: `recent_failures` is
        // process-global, so every server that ever failed and recovered would
        // leave an entry behind for the life of the process.
        assert!(
            registry.recent_failures.is_empty(),
            "a recovered server left its failure mark behind"
        );
    }

    /// The point of the window. An attempt costs up to `timeout` and runs before
    /// the model is invoked, so re-dialling a dead server on every turn would tax
    /// the whole conversation. Inside the window the caller is refused without
    /// the connector ever being touched.
    #[tokio::test]
    async fn a_failed_connect_is_not_retried_inside_its_window() {
        let connector = Arc::new(CountingConnector::failing_first(1));
        let registry = McpConnectionRegistry::new();
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        assert!(registry
            .client(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .is_err());

        let second = registry
            .client(&key, &cfg, || connector.connect("docs", &cfg))
            .await;

        assert!(second.is_err(), "the second attempt must still fail");
        assert_eq!(
            connector.count(),
            1,
            "the connector was dialled again inside the window, so a dead server \
             would cost its full timeout on every turn"
        );
    }

    // There is deliberately no "a success clears the mark" test, and no such
    // line in `client`: the window check above removes the mark before any
    // connect is attempted, so by the time one succeeds there is nothing left to
    // clear. A first draft had both, and the mutation that deleted the clearing
    // line passed the suite — not because a test was missing but because the line
    // was unreachable. The test that "covered" it was passing on the expiry
    // removal instead.

    // --- tools/list TTL cache (R3.5) ---

    /// R3.5 — the reason the cache exists. Under lazy loading the exposure
    /// stage runs on EVERY agent-loop iteration; without a cache each one
    /// pays a `tools/list` round-trip for a catalog that almost never changes.
    #[tokio::test]
    async fn a_cache_hit_skips_the_tools_list_roundtrip() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new();
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        let first = registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        let second = registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();

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
        let registry = McpConnectionRegistry::new();
        let cfg = config("https://mcp.example.com/mcp"); // cache_ttl = 300s
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_secs(299)).await;
        registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        assert_eq!(connector.list_count(), 1, "still inside the TTL");

        tokio::time::advance(Duration::from_secs(2)).await;
        registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        assert_eq!(connector.list_count(), 2, "past the TTL, it must refetch");
    }

    /// Two servers must never share a catalog — exposing server A's tools for
    /// server B would dispatch calls to the wrong endpoint.
    #[tokio::test]
    async fn the_cache_is_keyed_per_server() {
        let connector = Arc::new(CountingConnector::new());
        let registry = McpConnectionRegistry::new();
        let a = config("https://a.example.com/mcp");
        let b = config("https://b.example.com/mcp");

        let a_tools = registry
            .tools(
                &McpServerKey::from_resolved(&a, &CredentialFingerprint::none()),
                &a,
                || connector.connect("a", &a),
            )
            .await
            .unwrap();
        let b_tools = registry
            .tools(
                &McpServerKey::from_resolved(&b, &CredentialFingerprint::none()),
                &b,
                || connector.connect("b", &b),
            )
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
        let registry = McpConnectionRegistry::new();
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        assert!(registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .is_err());
        assert!(
            registry
                .tools(&key, &cfg, || connector.connect("docs", &cfg))
                .await
                .is_ok(),
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
        let registry = Arc::new(McpConnectionRegistry::new());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let (r, k, c, conn) = (
                registry.clone(),
                key.clone(),
                cfg.clone(),
                connector.clone(),
            );
            tasks.push(tokio::spawn(async move {
                r.tools(&k, &c, || conn.connect("docs", &c))
                    .await
                    .map(|_| ())
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
        let registry = McpConnectionRegistry::new();
        let mut cfg = config("https://mcp.example.com/mcp");
        cfg.cache_ttl = Duration::ZERO;
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        registry
            .tools(&key, &cfg, || connector.connect("docs", &cfg))
            .await
            .unwrap();
        assert_eq!(connector.list_count(), 2, "ttl 0 must disable the cache");
    }

    /// The registry is directly constructible, so no test ever reaches for a
    /// process-wide singleton and tests stay order-independent.
    #[tokio::test]
    async fn registries_are_independent() {
        let c1 = Arc::new(CountingConnector::new());
        let c2 = Arc::new(CountingConnector::new());
        let (r1, r2) = (McpConnectionRegistry::new(), McpConnectionRegistry::new());
        let cfg = config("https://mcp.example.com/mcp");
        let key = McpServerKey::from_resolved(&cfg, &CredentialFingerprint::none());

        r1.client(&key, &cfg, || c1.connect("docs", &cfg))
            .await
            .unwrap();
        r2.client(&key, &cfg, || c2.connect("docs", &cfg))
            .await
            .unwrap();

        assert_eq!(c1.count(), 1);
        assert_eq!(
            c2.count(),
            1,
            "a second registry must not see the first's pool"
        );
    }
}
