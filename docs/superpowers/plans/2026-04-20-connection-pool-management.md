# Connection Pool Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate per-request Postgres pool creation by introducing a shared `ColmenaEngine` that owns a single `PgPoolRegistry`; all internal and user DB pools flow through it.

**Architecture:** New `pool_registry` module exposes `PgPoolRegistry` (DashMap + LRU + pinned set). New `engine` module exposes `ColmenaEngine` built once per process, owning the registry, state/secure-value repositories backed by the pinned internal pool, and the existing `DagRunUseCase`. Existing factories (`ConversationRepositoryFactory`, new `SqlPortFactory`) delegate pool acquisition to the registry. `api.rs`, CLI `main.rs`, and the `adp/.../platform/worker/` are refactored to build the engine once and reuse it across requests.

**Tech Stack:** Rust 1.x, sqlx 0.8 (postgres, sqlite), axum 0.7, tokio, DashMap 6, lru 0.12, tracing 0.1. Source tree: `src/libs/colmena/src/` (crate `colmena_dag_engine`) and `adp/apps/service/ia/platform/worker/`.

**Spec reference:** [docs/superpowers/specs/2026-04-20-connection-pool-management-design.md](../specs/2026-04-20-connection-pool-management-design.md).

---

## File Structure

### Files created

| File | Responsibility |
|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/mod.rs` | Re-exports for the registry module. |
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/url_key.rs` | `UrlKey` newtype + normalization. |
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/config.rs` | `PoolConfig` + `ConfigError` + `from_env()`. |
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/metrics.rs` | `RegistryMetrics`, `PoolMetrics`, atomic counters. |
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs` | `PgPoolRegistry`: `get_or_create`, `pin`, `close_all`. |
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/error.rs` | `RegistryError` enum. |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs` | `SqlPortFactory::get_port(url, ...) → Arc<PgPoolAdapter>`. |
| `src/libs/colmena/src/dag_engine/engine.rs` | `ColmenaEngine`, `EngineConfig`, `EngineError`. |

### Files modified

| File | Change |
|---|---|
| `src/libs/colmena/Cargo.toml` | Add `dashmap = "6"` and `lru = "0.12"`. |
| `src/libs/colmena/src/dag_engine/infrastructure/mod.rs` | `pub mod pool_registry;` + `pub mod sql_port_factory;`. |
| `src/libs/colmena/src/dag_engine/mod.rs` | `pub mod engine;`. |
| `src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs` | Constructor takes `Arc<PgPoolRegistry>`; internal Postgres branch calls registry instead of `PgPoolOptions::new()`. |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs` | `new(pool: Arc<PgPool>, statement_timeout_ms, work_mem_mb)`; drop the `Option<PgPool>` + `connect()` plumbing. |
| `src/libs/colmena/src/dag_engine/domain/sql_ports.rs` | Remove `connect()` from `SqlConnectionPort` trait. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` | Take `Arc<SqlPortFactory>` in `new`; use factory during `initialize()`. |
| `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` | `new_with_secure_values` takes an extra `Arc<SqlPortFactory>`; pass it into `SqlNode::new`. |
| `src/libs/colmena/src/dag_engine/api.rs` | `run_dag` / `serve_dag` build `ColmenaEngine` once, reuse it, call `shutdown().await` on exit. |
| `src/libs/colmena/src/dag_engine/main.rs` | Same: one `ColmenaEngine` per CLI invocation. |
| `adp/apps/service/ia/platform/worker/src/main.rs` | Build engine in `main()`, inject into axum `AppState`; `process_job` uses `&ColmenaEngine`; add SIGTERM graceful shutdown. |
| `adp/apps/service/ia/platform/worker/Cargo.toml` | No dep change; just bump the `colmena_dag_engine` git rev once the lib PR lands. |

### Test files created

| Test file | What it covers |
|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/url_key.rs` (inline `#[cfg(test)]`) | URL normalization cases. |
| `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs` (inline `#[cfg(test)]`) | Pin/unpin, LRU eviction, concurrent `get_or_create`, `close_all`. Uses a private `new_mock()` helper that skips real Postgres connects. |
| `src/libs/colmena/tests/engine_pool_sharing.rs` (integration) | Engine boot, factory sharing, shutdown semantics. Gated behind `#[ignore]` + env var so CI without Postgres skips it. |

---

## Commit 1 — `pool_registry` module

### Task 1.1: Add dependencies

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`

- [ ] **Step 1: Add dashmap and lru dependencies**

Append to the `[dependencies]` section, right after the `regex = "1.10"` line:

```toml
# Shared connection-pool registry (see docs/superpowers/specs/2026-04-20-connection-pool-management-design.md)
dashmap = "6"
lru = "0.12"
```

- [ ] **Step 2: Verify the crate still compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: PASS (no warnings about new deps being unused — they'll be used in Task 1.2).

- [ ] **Step 3: Commit nothing yet**

Dependencies alone are not a meaningful commit — they go with the module in a single commit. Proceed directly to Task 1.2.

---

### Task 1.2: Scaffold the `pool_registry` module skeleton

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/mod.rs`
- Create: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/error.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`

- [ ] **Step 1: Create `pool_registry/mod.rs` with re-exports**

```rust
//! Shared Postgres connection-pool registry.
//!
//! Single source of truth for all `PgPool` instances in the engine. One pool per
//! unique (normalized) `connection_url`, reused across jobs and consumers.
//! See `docs/superpowers/specs/2026-04-20-connection-pool-management-design.md`.

mod config;
mod error;
mod metrics;
mod registry;
mod url_key;

pub use config::{ConfigError, PoolConfig};
pub use error::RegistryError;
pub use metrics::{PoolMetrics, RegistryMetrics};
pub use registry::PgPoolRegistry;
pub use url_key::UrlKey;
```

- [ ] **Step 2: Create `pool_registry/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to create Postgres pool for URL: {message}")]
    PoolCreation { message: String },
    #[error("registry is closed")]
    Closed,
}

impl From<sqlx::Error> for RegistryError {
    fn from(e: sqlx::Error) -> Self {
        RegistryError::PoolCreation {
            message: e.to_string(),
        }
    }
}
```

- [ ] **Step 3: Register the module**

Edit `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`, add `pub mod pool_registry;` right after `pub mod persistence;`:

```rust
pub mod dag_tool_executor;
pub mod nodes;
pub mod persistence;
pub mod pool_registry;
pub mod registry;
pub mod sql_function_registry;
pub mod sql_llm_critic;
pub mod sql_pool_adapter;
pub mod sql_static_validator;
```

- [ ] **Step 4: Verify the skeleton compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: FAIL — missing `config`, `metrics`, `registry`, `url_key` submodules. That's expected; the next tasks add them.

---

### Task 1.3: `UrlKey` with normalization (TDD)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/url_key.rs`

- [ ] **Step 1: Write the failing tests**

Create the file with the tests first, then the implementation placeholder:

```rust
//! Normalized representation of a Postgres connection URL used as a registry key.
//!
//! Conservative normalization: lowercase the scheme and host, strip a single
//! trailing slash on the path, preserve query parameters and credentials.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UrlKey(String);

impl UrlKey {
    pub fn normalize(raw: &str) -> Self {
        todo!("implement in next step")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UrlKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_scheme_and_host() {
        let a = UrlKey::normalize("POSTGRES://User:Pass@HOST.example.COM:5432/db");
        let b = UrlKey::normalize("postgres://User:Pass@host.example.com:5432/db");
        assert_eq!(a, b);
    }

    #[test]
    fn preserves_credentials_case_sensitive() {
        let a = UrlKey::normalize("postgres://User:Pass@host/db");
        let b = UrlKey::normalize("postgres://user:pass@host/db");
        assert_ne!(a, b, "credentials must not be normalized");
    }

    #[test]
    fn strips_single_trailing_slash() {
        let a = UrlKey::normalize("postgres://host/db/");
        let b = UrlKey::normalize("postgres://host/db");
        assert_eq!(a, b);
    }

    #[test]
    fn preserves_query_parameters() {
        let a = UrlKey::normalize("postgres://host/db?sslmode=require");
        let b = UrlKey::normalize("postgres://host/db");
        assert_ne!(
            a, b,
            "query parameters can change connection behavior and must be preserved"
        );
    }

    #[test]
    fn distinct_users_are_distinct_keys() {
        let a = UrlKey::normalize("postgres://alice:pw@host/db");
        let b = UrlKey::normalize("postgres://bob:pw@host/db");
        assert_ne!(a, b);
    }

    #[test]
    fn handles_url_without_path() {
        let a = UrlKey::normalize("postgres://user:pass@host:5432");
        assert_eq!(a.as_str(), "postgres://user:pass@host:5432");
    }
}
```

- [ ] **Step 2: Run the tests — they should fail**

Run: `cargo test -p colmena_dag_engine --lib url_key`
Expected: All tests panic with `todo!` message.

- [ ] **Step 3: Implement `normalize`**

Replace the `todo!` body in `UrlKey::normalize`:

```rust
    pub fn normalize(raw: &str) -> Self {
        // Split into "scheme://rest" and "rest" without parsing with `url` crate
        // (avoids a new dep and keeps behavior predictable for custom query params).
        let (scheme, rest) = match raw.split_once("://") {
            Some((s, r)) => (s.to_ascii_lowercase(), r),
            None => return UrlKey(raw.to_string()),
        };

        // Split rest into "authority" and "path+query" around the first '/'
        // (after the "://"). Authority may contain "user:pass@host:port".
        let (authority, path_q) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        };

        // Split authority into "credentials@host" — lowercase only the host part.
        let (creds, hostport) = match authority.rfind('@') {
            Some(idx) => (Some(&authority[..idx]), &authority[idx + 1..]),
            None => (None, authority),
        };
        let hostport_lower = hostport.to_ascii_lowercase();

        let mut out = String::with_capacity(raw.len());
        out.push_str(&scheme);
        out.push_str("://");
        if let Some(c) = creds {
            out.push_str(c);
            out.push('@');
        }
        out.push_str(&hostport_lower);

        // Strip one trailing slash from the path, but only if there's no query string.
        if let Some(q_idx) = path_q.find('?') {
            let (path, query) = path_q.split_at(q_idx);
            let path = path.strip_suffix('/').unwrap_or(path);
            out.push_str(path);
            out.push_str(query);
        } else {
            let path = path_q.strip_suffix('/').unwrap_or(path_q);
            out.push_str(path);
        }

        UrlKey(out)
    }
```

- [ ] **Step 4: Run the tests — they should pass**

Run: `cargo test -p colmena_dag_engine --lib url_key`
Expected: 6 passed, 0 failed.

- [ ] **Step 5: No commit yet**

Commit at the end of the module. Proceed to Task 1.4.

---

### Task 1.4: `PoolConfig` with env loading (TDD)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/config.rs`

- [ ] **Step 1: Write the failing tests**

```rust
//! Validated configuration for the `PgPoolRegistry`.

use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid {var}: {reason}")]
    Invalid { var: &'static str, reason: String },
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_entries: usize,
    pub max_conn_per_url: u32,
    pub min_conn_per_url: u32,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub acquire_timeout: Duration,
}

impl PoolConfig {
    pub fn defaults() -> Self {
        Self {
            max_entries: 100,
            max_conn_per_url: 2,
            min_conn_per_url: 0,
            idle_timeout: Duration::from_secs(30),
            max_lifetime: Duration::from_secs(600),
            acquire_timeout: Duration::from_secs(10),
        }
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        todo!("implement in next step")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = PoolConfig::defaults();
        assert_eq!(c.max_entries, 100);
        assert_eq!(c.max_conn_per_url, 2);
        assert_eq!(c.min_conn_per_url, 0);
        assert_eq!(c.idle_timeout, Duration::from_secs(30));
        assert_eq!(c.max_lifetime, Duration::from_secs(600));
        assert_eq!(c.acquire_timeout, Duration::from_secs(10));
    }

    #[test]
    fn from_env_uses_defaults_when_unset() {
        // Safety: these tests run serially via `#[serial]`-ish discipline — we
        // clear every known var so accidental leaks from other tests don't leak in.
        for var in [
            "COLMENA_POOL_MAX_ENTRIES",
            "COLMENA_POOL_MAX_CONN_PER_URL",
            "COLMENA_POOL_MIN_CONN_PER_URL",
            "COLMENA_POOL_IDLE_TIMEOUT_SEC",
            "COLMENA_POOL_MAX_LIFETIME_SEC",
            "COLMENA_POOL_ACQUIRE_TIMEOUT_SEC",
        ] {
            std::env::remove_var(var);
        }
        let c = PoolConfig::from_env().unwrap();
        assert_eq!(c.max_entries, 100);
    }

    #[test]
    fn from_env_rejects_out_of_range() {
        std::env::set_var("COLMENA_POOL_MAX_CONN_PER_URL", "999");
        let err = PoolConfig::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { var: "COLMENA_POOL_MAX_CONN_PER_URL", .. }));
        std::env::remove_var("COLMENA_POOL_MAX_CONN_PER_URL");
    }

    #[test]
    fn from_env_rejects_min_greater_than_max() {
        std::env::set_var("COLMENA_POOL_MIN_CONN_PER_URL", "5");
        std::env::set_var("COLMENA_POOL_MAX_CONN_PER_URL", "3");
        let err = PoolConfig::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }));
        std::env::remove_var("COLMENA_POOL_MIN_CONN_PER_URL");
        std::env::remove_var("COLMENA_POOL_MAX_CONN_PER_URL");
    }
}
```

- [ ] **Step 2: Run the tests — they should fail**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::config`
Expected: All `from_env` tests panic with `todo!`.

- [ ] **Step 3: Implement `from_env`**

Replace the `todo!` body:

```rust
    pub fn from_env() -> Result<Self, ConfigError> {
        fn parse<T: std::str::FromStr>(
            var: &'static str,
            default: T,
        ) -> Result<T, ConfigError> {
            match std::env::var(var) {
                Ok(s) => s.trim().parse::<T>().map_err(|_| ConfigError::Invalid {
                    var,
                    reason: format!("could not parse value '{}'", s),
                }),
                Err(_) => Ok(default),
            }
        }

        let max_entries: usize = parse("COLMENA_POOL_MAX_ENTRIES", 100)?;
        let max_conn_per_url: u32 = parse("COLMENA_POOL_MAX_CONN_PER_URL", 2)?;
        let min_conn_per_url: u32 = parse("COLMENA_POOL_MIN_CONN_PER_URL", 0)?;
        let idle_timeout_sec: u64 = parse("COLMENA_POOL_IDLE_TIMEOUT_SEC", 30)?;
        let max_lifetime_sec: u64 = parse("COLMENA_POOL_MAX_LIFETIME_SEC", 600)?;
        let acquire_timeout_sec: u64 = parse("COLMENA_POOL_ACQUIRE_TIMEOUT_SEC", 10)?;

        if !(1..=10_000).contains(&max_entries) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_ENTRIES",
                reason: format!("{} out of range 1..=10000", max_entries),
            });
        }
        if !(1..=50).contains(&max_conn_per_url) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_CONN_PER_URL",
                reason: format!("{} out of range 1..=50", max_conn_per_url),
            });
        }
        if min_conn_per_url > max_conn_per_url {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MIN_CONN_PER_URL",
                reason: format!(
                    "{} cannot exceed max_conn_per_url={}",
                    min_conn_per_url, max_conn_per_url
                ),
            });
        }
        if !(10..=3600).contains(&idle_timeout_sec) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_IDLE_TIMEOUT_SEC",
                reason: format!("{} out of range 10..=3600", idle_timeout_sec),
            });
        }
        if !(60..=86_400).contains(&max_lifetime_sec) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_LIFETIME_SEC",
                reason: format!("{} out of range 60..=86400", max_lifetime_sec),
            });
        }
        if !(1..=60).contains(&acquire_timeout_sec) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_ACQUIRE_TIMEOUT_SEC",
                reason: format!("{} out of range 1..=60", acquire_timeout_sec),
            });
        }

        Ok(Self {
            max_entries,
            max_conn_per_url,
            min_conn_per_url,
            idle_timeout: Duration::from_secs(idle_timeout_sec),
            max_lifetime: Duration::from_secs(max_lifetime_sec),
            acquire_timeout: Duration::from_secs(acquire_timeout_sec),
        })
    }
```

- [ ] **Step 4: Run the tests — they should pass**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::config`
Expected: 4 passed.

> **Note on env-test isolation:** these four tests each set/clear env vars. `cargo test` runs them on separate threads, so they may race. If you see flakiness, serialize them with `#[serial]` (add `serial_test = "3"` to `[dev-dependencies]`). Don't add the dep preemptively — only if CI is flaky.

---

### Task 1.5: `RegistryMetrics` structs (no logic yet)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/metrics.rs`

- [ ] **Step 1: Create the metrics module**

```rust
//! Observability surface for the pool registry. Counters are atomic; snapshot
//! returns a plain struct suitable for JSON serialization in `/debug/pools`.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

#[derive(Default)]
pub(crate) struct RegistryMetricsInner {
    pub get_or_create_total: AtomicU64,
    pub cache_hits_total: AtomicU64,
    pub evictions_total: AtomicU64,
}

impl RegistryMetricsInner {
    pub fn record_get_or_create(&self) {
        self.get_or_create_total.fetch_add(1, Ordering::Relaxed);
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
        m.record_cache_hit();
        m.record_eviction();
        assert_eq!(m.get_or_create_total.load(Ordering::Relaxed), 2);
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
            cache_hits_total: 4,
            per_url: vec![],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"cached_pools\":1"));
    }
}
```

- [ ] **Step 2: Run the metrics tests**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::metrics`
Expected: 2 passed.

> **sha256 helper:** we use a small custom hashing helper inline in `registry.rs` via the `sha2` crate. `sha2` is already pulled in transitively through `sqlx`/`tokio-tungstenite`? Check with `cargo tree | grep sha2` — if absent, add `sha2 = "0.10"` to `[dependencies]` at the start of Task 1.6 rather than here.

---

### Task 1.6: `PgPoolRegistry` core — structure (TDD with mockable pool)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs`

This is the biggest file in the module. We decompose into: struct + constructor, `pin`, `get_or_create`, LRU eviction, `close_all`, `snapshot_metrics`. Tests that require a real Postgres are marked `#[ignore]`; pin/LRU bookkeeping tests use a `#[cfg(test)]`-only `insert_for_test` helper that bypasses network connects.

- [ ] **Step 1: Add `sha2` dep if missing**

Check: `cargo tree -p colmena_dag_engine 2>/dev/null | grep -i sha2 | head -3`

If no output, append to `[dependencies]` in `src/libs/colmena/Cargo.toml`:

```toml
sha2 = "0.10"
```

- [ ] **Step 2: Write the registry skeleton**

```rust
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
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Mutex;

pub struct PgPoolRegistry {
    pools: DashMap<UrlKey, Arc<PgPool>>,
    lru: Mutex<LruCache<UrlKey, ()>>,
    pinned: DashSet<UrlKey>,
    last_used: DashMap<UrlKey, SystemTime>,
    config: PoolConfig,
    pub(crate) metrics: RegistryMetricsInner,
}

impl PgPoolRegistry {
    pub fn new(config: PoolConfig) -> Self {
        let capacity = NonZeroUsize::new(config.max_entries.max(1)).expect("max_entries > 0");
        Self {
            pools: DashMap::new(),
            lru: Mutex::new(LruCache::new(capacity)),
            pinned: DashSet::new(),
            last_used: DashMap::new(),
            config,
            metrics: RegistryMetricsInner::default(),
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
}
```

Save the file but do not run tests yet — the rest of the methods come in 1.7–1.10.

---

### Task 1.7: `pin` method with in-memory tests

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs`

- [ ] **Step 1: Add `pin` and a test-only `insert_for_test` helper**

Append inside `impl PgPoolRegistry { ... }`:

```rust
    /// Creates (if absent) a pool for `url` and marks it pinned. Pinned pools
    /// are never inserted into the LRU and never evicted.
    pub async fn pin(&self, url: &str) -> Result<Arc<PgPool>, RegistryError> {
        let key = UrlKey::normalize(url);
        if let Some(existing) = self.pools.get(&key) {
            self.pinned.insert(key.clone());
            self.last_used.insert(key, SystemTime::now());
            return Ok(existing.clone());
        }
        self.metrics.record_get_or_create();
        let pool = self
            .build_pool_options()
            .connect(url)
            .await
            .map_err(RegistryError::from)?;
        let arc = Arc::new(pool);
        self.pools.insert(key.clone(), arc.clone());
        self.pinned.insert(key.clone());
        self.last_used.insert(key, SystemTime::now());
        tracing::info!(target = "colmena::pool_registry", "pool_created pinned=true");
        Ok(arc)
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
```

- [ ] **Step 2: Write the pin tests**

Append `#[cfg(test)] mod tests { ... }` at the bottom of the file:

```rust
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
}
```

- [ ] **Step 3: Run the pin test**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::registry::tests::pinned_url_is_in_pinned_set`
Expected: 1 passed.

---

### Task 1.8: `get_or_create` with LRU promotion

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs`

- [ ] **Step 1: Write the failing LRU-bookkeeping test first**

Add inside the existing `mod tests`:

```rust
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
```

- [ ] **Step 2: Run — they should fail to compile (no `try_cached` yet)**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::registry`
Expected: FAIL with "no method named `try_cached`".

- [ ] **Step 3: Implement `try_cached` and `get_or_create`**

Append inside `impl PgPoolRegistry { ... }` (before the `#[cfg(test)]` helper):

```rust
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
        self.metrics.record_get_or_create();
        if let Some(cached) = self.try_cached(url).await {
            return Ok(cached);
        }

        // Evict before inserting to keep non-pinned entries ≤ max_entries.
        self.evict_if_needed().await;

        let key = UrlKey::normalize(url);
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
            self.metrics.record_eviction();
            tracing::warn!(
                target = "colmena::pool_registry",
                url_hash = %Self::hash_url(&victim),
                reason = "lru_capacity",
                "pool_evicted"
            );
        }
    }
```

- [ ] **Step 4: Re-run the LRU tests**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::registry`
Expected: 3 passed (the 1 from 1.7 plus the 2 new ones).

---

### Task 1.9: LRU eviction test + `close_all`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
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
            !reg.pools.contains_key(&UrlKey::normalize("postgres://u:p@host/a")),
            "oldest unpinned entry should have been evicted"
        );
    }

    #[tokio::test]
    async fn pinned_entries_are_exempt_from_eviction() {
        let reg = PgPoolRegistry::new(tiny_config(1));
        reg.insert_for_test("postgres://u:p@host/internal", fake_pool(), true);
        reg.insert_for_test("postgres://u:p@host/user1", fake_pool(), false);
        reg.evict_if_needed().await;
        assert!(reg.pools.contains_key(&UrlKey::normalize(
            "postgres://u:p@host/internal"
        )));
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
```

- [ ] **Step 2: Run — `close_all` doesn't exist yet**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::registry`
Expected: FAIL with "no method named `close_all`". The two eviction tests should already pass.

- [ ] **Step 3: Implement `close_all`**

Append inside `impl PgPoolRegistry { ... }`:

```rust
    /// Close every pool in the registry. Awaits sqlx's graceful-close for each.
    /// Safe to call multiple times (idempotent).
    pub async fn close_all(&self) {
        let keys: Vec<UrlKey> = self.pools.iter().map(|e| e.key().clone()).collect();
        for key in keys {
            if let Some((_, pool)) = self.pools.remove(&key) {
                pool.close().await;
            }
        }
        self.pinned.clear();
        self.last_used.clear();
        let mut lru = self.lru.lock().await;
        lru.clear();
    }
```

- [ ] **Step 4: Re-run the tests**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::registry`
Expected: 6 passed (3 from earlier + 3 new).

---

### Task 1.10: `snapshot_metrics`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs`

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
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
```

- [ ] **Step 2: Run — missing `snapshot_metrics`**

Run: `cargo test -p colmena_dag_engine --lib pool_registry::registry::tests::metrics_snapshot_reflects_state`
Expected: FAIL.

- [ ] **Step 3: Implement `snapshot_metrics`**

Append inside `impl PgPoolRegistry { ... }`:

```rust
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
            cache_hits_total: self.metrics.cache_hits_total.load(Ordering::Relaxed),
            per_url,
        }
    }
```

- [ ] **Step 4: Run the tests — all should pass**

Run: `cargo test -p colmena_dag_engine --lib pool_registry`
Expected: all tests in the module pass (config: 4, url_key: 6, metrics: 2, registry: 7 = 19 tests).

---

### Task 1.11: Clippy + commit

**Files:**
- Modify: all files in `pool_registry/`, `Cargo.toml`

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -p colmena_dag_engine --lib -- -D warnings`
Expected: PASS. Fix any lints inline before committing.

- [ ] **Step 2: Run fmt**

Run: `cargo fmt -p colmena_dag_engine`

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/Cargo.toml \
        src/libs/colmena/src/dag_engine/infrastructure/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/pool_registry/
git commit -m "feat(pool_registry): add PgPoolRegistry with LRU + pinned pools

Introduces the shared connection-pool registry that will replace per-request
pool creation. One pool per normalized URL, with pinned entries exempt from
LRU eviction. No consumers wired yet — that happens in the follow-up commits.

See docs/superpowers/specs/2026-04-20-connection-pool-management-design.md."
```

---

## Commit 2 — `ColmenaEngine` + factory refactors

This commit refactors existing types (`ConversationRepositoryFactory`, `PgPoolAdapter`) to consume pools from the registry, and adds `ColmenaEngine` as the single entry point. After this commit, the engine compiles but no caller uses it yet — that's commit 4.

### Task 2.1: Refactor `ConversationRepositoryFactory`

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs`

- [ ] **Step 1: Replace constructor and `get_repository`**

Overwrite the file with:

```rust
use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{ConversationRepository, LlmError};
use crate::llm::infrastructure::persistence::{
    PostgresConversationRepository, SqliteConversationRepository,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Factory that returns `ConversationRepository` instances keyed by connection URL.
///
/// For Postgres URLs the pool is obtained from the shared `PgPoolRegistry` — so
/// all LLM memory operations share pools with state persistence, secure values,
/// and SQL nodes pointing at the same URL. SQLite repositories are still owned
/// per-URL by this factory (SQLite has no central pool concept).
#[derive(Clone)]
pub struct ConversationRepositoryFactory {
    registry: Arc<PgPoolRegistry>,
    repositories: Arc<Mutex<HashMap<String, Arc<dyn ConversationRepository>>>>,
}

impl ConversationRepositoryFactory {
    pub fn new(registry: Arc<PgPoolRegistry>) -> Self {
        Self {
            registry,
            repositories: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_repository(
        &self,
        connection_url: &str,
    ) -> Result<Arc<dyn ConversationRepository>, LlmError> {
        let mut repos = self.repositories.lock().await;
        if let Some(repo) = repos.get(connection_url) {
            return Ok(repo.clone());
        }

        let repo: Arc<dyn ConversationRepository> = if connection_url.starts_with("postgres://")
            || connection_url.starts_with("postgresql://")
        {
            let pool_arc =
                self.registry
                    .get_or_create(connection_url)
                    .await
                    .map_err(|e| LlmError::RequestFailed {
                        message: format!("Failed to get Postgres pool: {}", e),
                    })?;

            // Run migrations (ignore missing: the DB may have old migrations
            // that no longer exist on disk from previous schema consolidations).
            let mut migrator = sqlx::migrate!("migrations/postgres");
            migrator.set_ignore_missing(true);
            migrator
                .run(&*pool_arc)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Migration failed: {}", e),
                })?;

            Arc::new(PostgresConversationRepository::new((*pool_arc).clone()))
        } else if connection_url.starts_with("sqlite://") {
            let options = SqliteConnectOptions::from_str(connection_url)
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Invalid SQLite URL: {}", e),
                })?
                .create_if_missing(true);

            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Failed to connect to SQLite: {}", e),
                })?;

            let mut migrator = sqlx::migrate!("migrations/sqlite");
            migrator.set_ignore_missing(true);
            migrator
                .run(&pool)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Migration failed: {}", e),
                })?;

            Arc::new(SqliteConversationRepository::new(pool))
        } else {
            return Err(LlmError::RequestFailed {
                message: format!("Unsupported database protocol in URL: {}", connection_url),
            });
        };

        repos.insert(connection_url.to_string(), repo.clone());
        Ok(repo)
    }
}
```

> **Note:** `PostgresConversationRepository::new` currently takes `PgPool` by value. We dereference the `Arc<PgPool>` to clone one — sqlx's `PgPool` is itself cheaply cloneable (`Arc` inside), so this does not open a second pool. If clippy warns about cloning the deref, add `#[allow(clippy::clone_on_ref_ptr)]` locally with a comment referencing this behavior.

- [ ] **Step 2: Verify**

Run: `cargo check -p colmena_dag_engine`
Expected: FAIL — callers still use `ConversationRepositoryFactory::new()` with no args. That's fine; we'll fix those call-sites in later tasks. The file itself should be type-correct.

To verify the file alone, run: `cargo check -p colmena_dag_engine 2>&1 | head -40` and confirm the errors are only in `api.rs`, `main.rs`, `registry.rs`, and the worker — not in `repository_factory.rs`.

---

### Task 2.2: Refactor `PgPoolAdapter` to take an injected pool

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`

- [ ] **Step 1: Remove `connect` from the `SqlConnectionPort` trait**

In `sql_ports.rs`, delete the `connect` method from the trait:

```rust
/// Port for managing the PostgreSQL connection pool and executing queries.
#[async_trait::async_trait]
pub trait SqlConnectionPort: Send + Sync {
    /// Execute a SQL query and return results as JSON.
    /// If `tenant_user_id` is Some, runs `SET LOCAL app.current_user_id` in the same transaction.
    async fn execute_query(
        &self,
        query: &str,
        max_rows: u64,
        tenant_user_id: Option<&str>,
    ) -> Result<QueryResult, SqlNodeError>;

    /// Load table metadata (names + comments) for the given schemas.
    async fn load_table_metadata(&self, schemas: &[String])
        -> Result<Vec<TableInfo>, SqlNodeError>;

    /// Check if the pool is connected and ready.
    fn is_connected(&self) -> bool;
}
```

- [ ] **Step 2: Rewrite `PgPoolAdapter` to own its `Arc<PgPool>` directly**

In `sql_pool_adapter.rs`, replace the struct and its `new` / `get_pool` / `Default` / `connect` impls with:

```rust
/// Adapter that wraps a PostgreSQL connection pool with per-query runtime limits.
///
/// Does NOT own pool creation. The caller (normally `SqlPortFactory`) must pass
/// an `Arc<PgPool>` obtained from the shared `PgPoolRegistry`. Per-query
/// `statement_timeout` and `work_mem` are applied via `SET LOCAL` inside every
/// transaction, so multiple adapters can safely share a pool.
pub struct PgPoolAdapter {
    pool: Arc<PgPool>,
    statement_timeout_ms: u64,
    work_mem_mb: u64,
}

impl PgPoolAdapter {
    pub fn new(pool: Arc<PgPool>, statement_timeout_ms: u64, work_mem_mb: u64) -> Self {
        Self {
            pool,
            statement_timeout_ms,
            work_mem_mb,
        }
    }

    /// Shared reference to the underlying pool — used by `PgRegistryAdapter`
    /// (sandbox function registry) to reuse the same connections.
    pub fn pool(&self) -> Arc<PgPool> {
        self.pool.clone()
    }

    /// Quote a SQL identifier to prevent injection (equivalent to PostgreSQL's quote_ident).
    fn quote_ident(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}
```

Remove the `Arc<RwLock<Option<PgPool>>>` field, the `pool_ref()` method that returned the RwLock, the `get_pool()` async helper, and the `Default` impl.

- [ ] **Step 3: Update every `self.get_pool().await?` to `&self.pool`**

Scan for `self.get_pool().await` in the file — there are ~8 call-sites (one per RLS helper + `execute_query` + `load_table_metadata`). Replace each binding:

```rust
// before
let pool = self.get_pool().await?;
...
.fetch_optional(&pool).await

// after
.fetch_optional(&*self.pool).await
```

For the loop in `execute_query` replace the top `let pool = self.get_pool().await?;` with `let pool = &*self.pool;`. Remove the `timeout_ms = *self.statement_timeout_ms.read().await;` / `work_mem = *self.work_mem_mb.read().await;` lines — read the fields directly:

```rust
let timeout_ms = self.statement_timeout_ms;
let work_mem = self.work_mem_mb;
let mut tx = pool.begin().await.map_err(...)?;
```

- [ ] **Step 4: Remove the `SqlConnectionPort::connect` impl**

Delete the entire `async fn connect(...)` inside `impl SqlConnectionPort for PgPoolAdapter`. Keep `execute_query`, `load_table_metadata`, `is_connected`. Change `is_connected` to return `true` unconditionally (the pool is always present now):

```rust
    fn is_connected(&self) -> bool {
        true
    }
```

- [ ] **Step 5: Delete stale imports**

Remove:
- `use sqlx::postgres::PgPoolOptions;` — no longer used.
- `use tokio::sync::RwLock;` — no longer used.

- [ ] **Step 6: Verify the file compiles in isolation**

Run: `cargo check -p colmena_dag_engine 2>&1 | head -40`
Expected: errors only in `nodes/sql.rs`, `sql_function_registry.rs`, and the bootstrap files (which expect the old API). The adapter and ports are type-correct.

---

### Task 2.3: Fix `PgRegistryAdapter` (sandbox registry) for the new `pool()` signature

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs`

- [ ] **Step 1: Inspect how it reads the pool**

Run: `grep -n 'pool_ref\|Arc<RwLock' src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs | head -10`

The file receives the old `Arc<RwLock<Option<PgPool>>>` from `pool_adapter.pool_ref()`. We need to change it to `Arc<PgPool>`.

- [ ] **Step 2: Update the struct and constructor**

In `sql_function_registry.rs`, change the field and constructor:

```rust
// before
pub struct PgRegistryAdapter {
    pool: Arc<RwLock<Option<PgPool>>>,
    sandbox_schema: String,
}
impl PgRegistryAdapter {
    pub fn new(pool: Arc<RwLock<Option<PgPool>>>, sandbox_schema: String) -> Self { ... }
}

// after
pub struct PgRegistryAdapter {
    pool: Arc<PgPool>,
    sandbox_schema: String,
}
impl PgRegistryAdapter {
    pub fn new(pool: Arc<PgPool>, sandbox_schema: String) -> Self {
        Self { pool, sandbox_schema }
    }
}
```

- [ ] **Step 3: Replace `pool.read().await.clone().ok_or(...)` with `&*self.pool`**

Every method that used `let pool = self.pool.read().await.clone().ok_or(...)?;` changes to `let pool = &*self.pool;`. The `sqlx::query(...).execute(pool)` calls work unchanged.

- [ ] **Step 4: Remove unused imports**

Drop `use tokio::sync::RwLock;` if no longer needed.

- [ ] **Step 5: Verify**

Run: `cargo check -p colmena_dag_engine 2>&1 | grep -E 'error|warning' | head -20`
Expected: errors only in `nodes/sql.rs` (`pool_ref` gone), `api.rs`, `main.rs`, `worker`. Registry file itself is clean.

---

### Task 2.4: Create `SqlPortFactory`

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`

- [ ] **Step 1: Write the factory**

```rust
//! Factory that builds `PgPoolAdapter` instances on top of shared registry pools.
//!
//! Each adapter keeps its own `statement_timeout_ms` / `work_mem_mb` (applied
//! per-query via `SET LOCAL`), so multiple nodes hitting the same URL with
//! different runtime limits do not interfere with each other.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::dag_engine::infrastructure::sql_pool_adapter::PgPoolAdapter;
use std::sync::Arc;

pub struct SqlPortFactory {
    registry: Arc<PgPoolRegistry>,
}

impl SqlPortFactory {
    pub fn new(registry: Arc<PgPoolRegistry>) -> Self {
        Self { registry }
    }

    /// Obtain a `PgPoolAdapter` wrapping the shared registry pool for `url`.
    pub async fn get_adapter(
        &self,
        url: &str,
        statement_timeout_ms: u64,
        work_mem_mb: u64,
    ) -> Result<Arc<PgPoolAdapter>, SqlNodeError> {
        let pool = self.registry.get_or_create(url).await.map_err(|e| {
            SqlNodeError::ConnectionError(format!("pool registry: {}", e))
        })?;
        Ok(Arc::new(PgPoolAdapter::new(
            pool,
            statement_timeout_ms,
            work_mem_mb,
        )))
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`, add `pub mod sql_port_factory;` alphabetically:

```rust
pub mod sql_function_registry;
pub mod sql_llm_critic;
pub mod sql_pool_adapter;
pub mod sql_port_factory;
pub mod sql_static_validator;
```

- [ ] **Step 3: Verify**

Run: `cargo check -p colmena_dag_engine 2>&1 | grep -E 'error\[' | head -20`
Expected: errors in `nodes/sql.rs`, `api.rs`, `main.rs`, `registry.rs`, worker — not in the factory.

---

### Task 2.5: Scaffold `ColmenaEngine` struct

**Files:**
- Create: `src/libs/colmena/src/dag_engine/engine.rs`
- Modify: `src/libs/colmena/src/dag_engine/mod.rs`

- [ ] **Step 1: Create the module**

```rust
//! `ColmenaEngine`: process-wide entry point for DAG execution.
//!
//! Owns the shared `PgPoolRegistry`, the pinned internal-DB pool, state + secure
//! value repositories, the node registry, and the `DagRunUseCase`. Consumers
//! (CLI, HTTP worker, `run_dag`/`serve_dag`) build one per process.

use crate::dag_engine::application::run_use_case::DagRunUseCase;
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::events::DagExecutionEvent;
use crate::dag_engine::domain::graph::Graph;
use crate::dag_engine::domain::state::DagTaskMemoryRepository;
use crate::dag_engine::infrastructure::persistence::postgres_dag_state_repository::PostgresDagStateRepository;
use crate::dag_engine::infrastructure::persistence::PostgresSecureValueRepository;
use crate::dag_engine::infrastructure::pool_registry::{ConfigError, PoolConfig, PgPoolRegistry, RegistryError, RegistryMetrics};
use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
use crate::llm::infrastructure::persistence::repository_factory::ConversationRepositoryFactory;

use futures::Stream;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
    #[error("migration failed: {0}")]
    Migration(String),
    #[error("{0}")]
    Other(String),
}

pub struct EngineConfig {
    pub internal_database_url: String,
    pub pool_config: PoolConfig,
}

impl EngineConfig {
    pub fn from_env() -> Result<Self, EngineError> {
        let internal_database_url = std::env::var("DATABASE_URL").map_err(|_| {
            EngineError::Other("DATABASE_URL must be set to build ColmenaEngine".to_string())
        })?;
        let pool_config = PoolConfig::from_env()?;
        Ok(Self {
            internal_database_url,
            pool_config,
        })
    }
}

pub struct ColmenaEngine {
    registry: Arc<PgPoolRegistry>,
    use_case: Arc<DagRunUseCase>,
    closed: AtomicBool,
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/dag_engine/mod.rs`:

```rust
pub mod api;
pub mod application;
pub mod domain;
pub mod engine;
pub mod infrastructure;
pub mod verbose;
```

- [ ] **Step 3: Verify skeleton compiles**

Run: `cargo check -p colmena_dag_engine 2>&1 | grep -E 'error\[' | head -10`
Expected: errors only in `api.rs`, `main.rs`, `registry.rs`, `nodes/sql.rs`, worker — not in engine.rs itself. The struct body is currently unused, which is fine.

---

### Task 2.6: Implement `ColmenaEngine::new`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/engine.rs`

- [ ] **Step 1: Add the `new` constructor**

Append to the existing `engine.rs`:

```rust
impl ColmenaEngine {
    /// Build the engine: pin the internal pool, migrate state + secure-values
    /// schemas on it, build the node registry, and wire the `DagRunUseCase`.
    pub async fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let registry = Arc::new(PgPoolRegistry::new(config.pool_config));

        // Pin the internal DB. The returned Arc<PgPool> is the sole Postgres
        // connection pool used by state + secure-value repositories, and is
        // shared with any graph node that happens to reference the same URL.
        let internal_pool = registry.pin(&config.internal_database_url).await?;

        let state_repo = Arc::new(PostgresDagStateRepository::new(
            (*internal_pool).clone(),
        ));
        state_repo
            .migrate()
            .await
            .map_err(|e| EngineError::Migration(format!("{:?}", e)))?;

        let secure_value_repo = Arc::new(PostgresSecureValueRepository::new(
            (*internal_pool).clone(),
        ));
        secure_value_repo
            .migrate()
            .await
            .map_err(|e| EngineError::Migration(format!("{:?}", e)))?;

        let secure_value_service = Arc::new(SecureValueService::new(secure_value_repo));

        let conversation_factory = Arc::new(ConversationRepositoryFactory::new(registry.clone()));
        let sql_port_factory = Arc::new(SqlPortFactory::new(registry.clone()));

        let node_registry = HashMapNodeRegistry::new_with_secure_values(
            conversation_factory,
            sql_port_factory,
            Some(state_repo.clone() as Arc<dyn DagTaskMemoryRepository>),
            Some(secure_value_service.clone()),
        );

        let use_case = Arc::new(DagRunUseCase::with_secure_values_and_service(
            node_registry.clone(),
            Some(state_repo.clone()),
            secure_value_service,
        ));
        node_registry.set_subgraph_executor(use_case.clone());

        tracing::info!(
            target = "colmena::engine",
            pinned_pool_count = 1,
            "engine_started"
        );

        Ok(Self {
            registry,
            use_case,
            closed: AtomicBool::new(false),
        })
    }
}
```

> **Note:** `HashMapNodeRegistry::new_with_secure_values` currently takes 3 args; after Task 2.7 it takes 4 (adding `Arc<SqlPortFactory>`). The engine is written for the new signature — keep in mind that Task 2.7 must land in the same commit.

- [ ] **Step 2: Do not run tests yet**

The engine's `new` depends on Task 2.7 refactoring the node registry. Proceed.

---

### Task 2.7: Update `HashMapNodeRegistry` to accept `SqlPortFactory`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

- [ ] **Step 1: Add the parameter and plumb it to `SqlNode`**

Change `new` and `new_with_secure_values` signatures:

```rust
pub fn new(
    repository_factory: Arc<ConversationRepositoryFactory>,
    sql_port_factory: Arc<crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory>,
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
) -> Arc<Self> {
    HashMapNodeRegistry::new_with_secure_values(
        repository_factory,
        sql_port_factory,
        task_memory_repo,
        None,
    )
}

pub fn new_with_secure_values(
    repository_factory: Arc<ConversationRepositoryFactory>,
    sql_port_factory: Arc<crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory>,
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
    secure_value_service: Option<Arc<SecureValueService>>,
) -> Arc<Self> {
```

Inside the closure, find the line registering `sql_query`:

```rust
// before
nodes.insert("sql_query".to_string(), Arc::new(SqlNode::new()));

// after
nodes.insert(
    "sql_query".to_string(),
    Arc::new(SqlNode::new(sql_port_factory.clone())),
);
```

- [ ] **Step 2: Do not compile yet**

`SqlNode::new` still takes zero args — we fix that next.

---

### Task 2.8: Refactor `SqlNode` to use the factory

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`

- [ ] **Step 1: Update struct and constructor**

Replace the `SqlNode` struct and its `new` / `Default` impls:

```rust
use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;

pub struct SqlNode {
    factory: Arc<SqlPortFactory>,
    /// Lazily populated on first call to `initialize()`.
    pool_adapter: Arc<RwLock<Option<Arc<PgPoolAdapter>>>>,
    initialized: Arc<RwLock<bool>>,
    cached_description: Arc<RwLock<Option<String>>>,
}

impl SqlNode {
    pub fn new(factory: Arc<SqlPortFactory>) -> Self {
        Self {
            factory,
            pool_adapter: Arc::new(RwLock::new(None)),
            initialized: Arc::new(RwLock::new(false)),
            cached_description: Arc::new(RwLock::new(None)),
        }
    }
```

Remove the `impl Default for SqlNode { fn default() -> Self { Self::new() } }` block — callers supply the factory explicitly.

- [ ] **Step 2: Update `initialize()` to get adapter from factory**

In `InitializableNode::initialize`, replace the block that called `self.pool_adapter.connect(...)`:

```rust
// before: connect via trait
{
    let conn: &dyn SqlConnectionPort = &*self.pool_adapter;
    conn.connect(&connection_url, statement_timeout_ms, work_mem_mb)
        .await
        .map_err(|e| format!("Failed to initialize SQL pool: {}", e))?;
}

// after: acquire adapter from factory
let adapter = self
    .factory
    .get_adapter(&connection_url, statement_timeout_ms, work_mem_mb)
    .await
    .map_err(|e| format!("Failed to acquire SQL pool: {}", e))?;
*self.pool_adapter.write().await = Some(adapter.clone());
```

- [ ] **Step 3: Replace every `&*self.pool_adapter` and `self.pool_adapter.pool_ref()` usage**

Search the file for `pool_adapter`. For every place that used `&*self.pool_adapter`, obtain the adapter from the RwLock:

```rust
// Helper method at the top of `impl SqlNode { ... }`:
async fn adapter(&self) -> Result<Arc<PgPoolAdapter>, Box<dyn StdError + Send + Sync>> {
    self.pool_adapter
        .read()
        .await
        .clone()
        .ok_or_else(|| "SqlNode not initialized".into())
}
```

Then in `initialize()` and `execute()`, replace:

```rust
// before: registry uses pool_ref (an Arc<RwLock<Option<PgPool>>>)
let pool_ref = self.pool_adapter.pool_ref();
let registry = PgRegistryAdapter::new(pool_ref, sandbox_schema);

// after: registry uses the adapter's Arc<PgPool> directly
let adapter = self.adapter().await?;
let registry = PgRegistryAdapter::new(adapter.pool(), sandbox_schema);
```

And replace the `SqlExecutionService::new(self.pool_adapter.clone() as Arc<dyn SqlConnectionPort>, ...)` line with:

```rust
let adapter = self.adapter().await?;
let service = SqlExecutionService::new(
    adapter.clone() as Arc<dyn crate::dag_engine::domain::sql_ports::SqlConnectionPort>,
    validator,
    critic,
    registry,
);
```

RLS calls (`self.pool_adapter.setup_rls_for_table(...)`) become:

```rust
let adapter = self.adapter().await?;
adapter.setup_rls_for_table(...).await
```

- [ ] **Step 4: Verify**

Run: `cargo check -p colmena_dag_engine 2>&1 | grep -E 'error\[' | head -20`
Expected: errors only in `api.rs`, CLI `main.rs`, and the worker — the lib-side refactor is otherwise type-correct.

---

### Task 2.9: Add `ColmenaEngine` execution + shutdown methods

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/engine.rs`

- [ ] **Step 1: Append execution methods**

```rust
impl ColmenaEngine {
    pub async fn run_dag(
        &self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
    ) -> Result<Value, DagError> {
        self.use_case
            .execute(graph, resume_session_id, resume_answer, include_extra_info)
            .await
    }

    pub fn execute_stream(
        &self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
    ) -> impl Stream<Item = Result<DagExecutionEvent, DagError>> + Send + '_ {
        self.use_case.clone().execute_stream(
            graph,
            resume_session_id,
            resume_answer,
            include_extra_info,
        )
    }

    pub fn registry_metrics(&self) -> RegistryMetrics {
        self.registry.snapshot_metrics()
    }

    /// Close every pool in the registry. Idempotent.
    pub async fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let start = std::time::Instant::now();
        let pool_count = self.registry.snapshot_metrics().cached_pools;
        self.registry.close_all().await;
        tracing::info!(
            target = "colmena::engine",
            pools_closed = pool_count,
            duration_ms = start.elapsed().as_millis() as u64,
            "engine_shutdown"
        );
    }
}

impl Drop for ColmenaEngine {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::SeqCst) {
            tracing::warn!(
                target = "colmena::engine",
                "engine_dropped_without_shutdown"
            );
        }
    }
}
```

> **DagRunUseCase::execute_stream:** this method currently takes `self` by value (the use_case is `Clone`). We call `self.use_case.clone().execute_stream(...)` to avoid lifetime problems with `Arc<DagRunUseCase>`. Verify by checking the signature at [run_use_case.rs](../../../src/libs/colmena/src/dag_engine/application/run_use_case.rs) and adjust the call if the trait is different.

- [ ] **Step 2: Fix the `api.rs`, `main.rs`, registry.rs call-sites so the lib compiles**

Do **not** refactor them into the engine yet — that's commit 4. For now, patch the three call-sites minimally so the library builds. This means updating them to the new `ConversationRepositoryFactory::new(registry)` and `HashMapNodeRegistry::new_with_secure_values(..., sql_port_factory, ...)` signatures while still creating the pool ad-hoc. Each file needs a `let pool_registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));` at the top, then `let pool_arc = pool_registry.pin(&db_url).await?;` in place of the current `sqlx::postgres::PgPoolOptions::new().connect(...)`. Then:
- `PostgresDagStateRepository::new((*pool_arc).clone())`
- `ConversationRepositoryFactory::new(pool_registry.clone())`
- `SqlPortFactory::new(pool_registry.clone())` → wire into `HashMapNodeRegistry::new_with_secure_values(repo_factory, sql_port_factory, state_repo, secure_value_service)`.

Apply the same change to `api.rs::run_dag`, `api.rs::serve_dag`, and CLI `main.rs`.

- [ ] **Step 3: Verify the library compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: PASS (library code), but the **worker** (separate crate) will still fail against the new types. That's expected; commit 4 fixes it.

- [ ] **Step 4: Run all unit tests in the lib**

Run: `cargo test -p colmena_dag_engine --lib`
Expected: all existing tests pass; new pool_registry tests pass. No integration tests broken.

---

### Task 2.10: Clippy + commit

**Files:** all of the above.

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -p colmena_dag_engine --lib -- -D warnings`
Expected: PASS.

- [ ] **Step 2: Commit**

```bash
git add src/libs/colmena/src/dag_engine/engine.rs \
        src/libs/colmena/src/dag_engine/mod.rs \
        src/libs/colmena/src/dag_engine/domain/sql_ports.rs \
        src/libs/colmena/src/dag_engine/infrastructure/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs \
        src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs \
        src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs \
        src/libs/colmena/src/dag_engine/infrastructure/registry.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs \
        src/libs/colmena/src/dag_engine/api.rs \
        src/libs/colmena/src/dag_engine/main.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs
git commit -m "feat(engine): introduce ColmenaEngine owning shared PgPoolRegistry

Refactors ConversationRepositoryFactory + PgPoolAdapter to consume pools
from the registry. Adds ColmenaEngine as the single-process entry point
with explicit async shutdown(). The library compiles against the new API;
CLI + api.rs still build pools inline (will switch to ColmenaEngine in
the next commit)."
```

---

## Commit 3 — `api.rs` + CLI switch to `ColmenaEngine`

This commit replaces the hand-rolled bootstrap in `api.rs` and CLI `main.rs` with a single `ColmenaEngine::new()` call, plus `shutdown().await` on exit. The behavior (streaming, SSE, JSON) is preserved.

### Task 3.1: Refactor `run_dag` in `api.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/api.rs`

- [ ] **Step 1: Replace the bootstrap block**

At the top of `run_dag`, delete all code between `dotenvy::dotenv().ok();` and the `// Load and execute the graph` comment. Replace with:

```rust
    dotenvy::dotenv().ok();

    let engine_config = crate::dag_engine::engine::EngineConfig::from_env()
        .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;
    let engine = crate::dag_engine::engine::ColmenaEngine::new(engine_config)
        .await
        .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;
```

- [ ] **Step 2: Route streaming/non-streaming calls through the engine**

Replace `run_use_case.execute_stream(graph, ...)` with `engine.execute_stream(graph, ...)`. Replace `run_use_case.execute(graph, ...).await` (in the non-streaming branch) with `engine.run_dag(graph, ...).await`.

- [ ] **Step 3: Call shutdown on exit**

Wrap the body of `run_dag` in a `let result = async { ... }.await;`, then call `engine.shutdown().await;` regardless of outcome, then return `result`:

```rust
    let result: Result<Value, Box<dyn std::error::Error>> = async {
        // ... existing body (streaming + non-streaming branches)
    }
    .await;
    engine.shutdown().await;
    result
```

- [ ] **Step 4: Drop unused imports**

Remove: `use crate::dag_engine::application::run_use_case::DagRunUseCase;`, `use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;`, `use crate::llm::infrastructure::ConversationRepositoryFactory;`.

---

### Task 3.2: Refactor `serve_dag` in `api.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/api.rs`

- [ ] **Step 1: Replace the bootstrap and `AppState`**

Change `AppState` to hold the engine instead of the use_case:

```rust
#[derive(Clone)]
struct AppState {
    graph: Arc<Graph>,
    engine: Arc<crate::dag_engine::engine::ColmenaEngine>,
}
```

At the top of `serve_dag`, replace the bootstrap block with:

```rust
    dotenvy::dotenv().ok();
    let engine_config = crate::dag_engine::engine::EngineConfig::from_env()
        .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;
    let engine = Arc::new(crate::dag_engine::engine::ColmenaEngine::new(engine_config).await
        .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?);
```

- [ ] **Step 2: Wire the engine into handlers**

Every `state.use_case.execute(...)` or `state.use_case.clone().execute_stream(...)` becomes `state.engine.run_dag(...)` / `state.engine.execute_stream(...)`.

- [ ] **Step 3: Graceful shutdown**

Axum 0.7 uses `axum::serve(listener, app).with_graceful_shutdown(future)`. Wire a `tokio::signal::ctrl_c()` future and after `axum::serve(...).await?`, call `engine.shutdown().await`. Final block:

```rust
    let listener = tokio::net::TcpListener::bind(&addr_str).await?;
    let shutdown_future = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_future)
        .await?;
    engine.shutdown().await;
    Ok(())
```

- [ ] **Step 4: Verify build**

Run: `cargo check -p colmena_dag_engine --bin dag_engine`
Expected: PASS.

---

### Task 3.3: Refactor CLI `main.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/main.rs`

- [ ] **Step 1: Replace the bootstrap in the `Run` branch**

Replace everything from `dotenvy::dotenv().ok();` inside the `Run` arm through `let run_use_case = DagRunUseCase::with_secure_values_and_service(...)` + `registry.set_subgraph_executor(...)` with:

```rust
            dotenvy::dotenv().ok();
            let engine_config = colmena::dag_engine::engine::EngineConfig::from_env()?;
            let engine = colmena::dag_engine::engine::ColmenaEngine::new(engine_config).await?;
```

- [ ] **Step 2: Replace `run_use_case.execute_stream(...)` with `engine.execute_stream(...)`**

And replace `pool.close().await;` at the bottom with `engine.shutdown().await;`. Drop the old imports (`ConversationRepositoryFactory`, `HashMapNodeRegistry`, `DagRunUseCase`).

- [ ] **Step 3: Verify full lib + bins build**

Run: `cargo build -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 4: Run existing integration tests that exercise `run_dag`**

Run an existing graph end-to-end:

```bash
source .env 2>/dev/null || true
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```

Expected: the graph runs to completion, and the logs show `engine_started` and `engine_shutdown` events.

---

### Task 3.4: Commit

- [ ] **Step 1: Clippy + fmt**

Run: `cargo clippy -p colmena_dag_engine -- -D warnings && cargo fmt -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 2: Commit**

```bash
git add src/libs/colmena/src/dag_engine/api.rs \
        src/libs/colmena/src/dag_engine/main.rs
git commit -m "refactor(api,cli): route run_dag/serve_dag/CLI through ColmenaEngine

All three entry points now build a single ColmenaEngine per invocation,
reuse it, and call engine.shutdown().await before exit. No more ad-hoc
PgPoolOptions::connect() in the library."
```

---

## Commit 4 — Worker (`platform/worker`) switches to `ColmenaEngine`

This commit lives in the `adp` repo's worker crate, not in `colmena`. Because the worker depends on `colmena_dag_engine` via git branch `develop`, ensure commits 1–3 are pushed to `develop` first (or use a temporary `path = "../../..../colmena/src/libs/colmena"` override while testing locally).

### Task 4.1: Build the engine once in `main`

**Files:**
- Modify: `adp/apps/service/ia/platform/worker/src/main.rs`

- [ ] **Step 1: Extend `AppState`**

Replace the `Arc<Client>` state with a struct holding the Redis client and the engine:

```rust
use std::sync::Arc;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};

#[derive(Clone)]
struct AppState {
    redis: Arc<redis::Client>,
    engine: Arc<ColmenaEngine>,
}
```

- [ ] **Step 2: Construct the engine in `main`**

Replace:

```rust
let app = Router::new()
    .route("/", get(health_check))
    .route("/process", post(process_jobs_handler))
    .with_state(Arc::new(client));
```

with:

```rust
let engine_config = EngineConfig::from_env()
    .map_err(|e| format!("engine config: {}", e))?;
let engine = Arc::new(
    ColmenaEngine::new(engine_config)
        .await
        .map_err(|e| format!("engine init: {}", e))?,
);

let state = AppState {
    redis: Arc::new(client),
    engine: engine.clone(),
};

let app = Router::new()
    .route("/", get(health_check))
    .route("/process", post(process_jobs_handler))
    .with_state(state);
```

- [ ] **Step 3: Add graceful-shutdown**

Replace `axum::serve(listener, app).await?;` with:

```rust
let shutdown_signal = async {
    let _ = tokio::signal::ctrl_c().await;
    info!("SIGINT/SIGTERM received — starting graceful shutdown");
};
axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal)
    .await?;
engine.shutdown().await;
info!("Worker shut down cleanly");
```

---

### Task 4.2: Delete the per-job bootstrap from `process_job`

**Files:**
- Modify: `adp/apps/service/ia/platform/worker/src/main.rs`

- [ ] **Step 1: Update handler signatures**

Change:

```rust
async fn process_jobs_handler(State(client): State<Arc<Client>>) -> impl IntoResponse {
    match process_jobs_inline(&client).await { ... }
}

async fn process_jobs_inline(client: &Client) -> Result<u32, Box<dyn std::error::Error>> {
    let mut con = client.get_async_connection().await?;
    ...
    if let Err(e) = process_job(&job, &mut con).await { ... }
}

async fn process_job(job: &platform_shared::JobRequest, redis_con: &mut redis::aio::Connection) -> Result<(), Box<dyn std::error::Error>> {
```

to:

```rust
async fn process_jobs_handler(State(state): State<AppState>) -> impl IntoResponse {
    match process_jobs_inline(&state).await { ... }
}

async fn process_jobs_inline(state: &AppState) -> Result<u32, Box<dyn std::error::Error>> {
    let mut con = state.redis.get_async_connection().await?;
    ...
    if let Err(e) = process_job(&job, &mut con, &state.engine).await { ... }
}

async fn process_job(
    job: &platform_shared::JobRequest,
    redis_con: &mut redis::aio::Connection,
    engine: &ColmenaEngine,
) -> Result<(), Box<dyn std::error::Error>> {
```

- [ ] **Step 2: Delete the old bootstrap block in `process_job`**

Delete lines currently handling `pool`, `repo_factory`, `state_repo`, `secure_value_repo`, `secure_value_service`, `registry`, `use_case`, `registry.set_subgraph_executor(...)`. After deletion, the body jumps directly from graph deserialization (`let graph: Graph = ...`) to the streaming call:

```rust
let stream = engine.execute_stream(
    graph,
    job.session_id.clone(),
    job.resume_answer.clone(),
    true,
);
```

Remove the now-unused imports (`DagRunUseCase`, `HashMapNodeRegistry`, `ConversationRepositoryFactory`).

- [ ] **Step 3: Verify build**

Run: `cargo build -p worker`
Expected: PASS. Address any clippy warnings about unused imports.

---

### Task 4.3: Smoke-test against a local Postgres + Redis

**Files:** (no code changes — verification only)

- [ ] **Step 1: Boot dependencies**

Run: `docker compose -f adp/apps/service/ia/platform/docker-compose.yml up -d postgres redis` (or whatever bootstraps the local fixtures — adjust per team convention).

- [ ] **Step 2: Start the worker**

```bash
cd adp/apps/service/ia/platform/worker
DATABASE_URL=postgres://... REDIS_URL=redis://127.0.0.1:6379 \
  cargo run --release
```

Expected: logs show `engine_started pinned_pool_count=1`.

- [ ] **Step 3: Enqueue a job and trigger `/process`**

Use your existing test harness (an API that pushes to `job_queue` and hits `POST /process`). Watch the worker logs: you should see `pool_created pinned=true` once at startup, **not** on every job.

- [ ] **Step 4: Verify connection reuse via `pg_stat_activity`**

```sql
SELECT application_name, count(*)
FROM pg_stat_activity
WHERE datname = 'colmena_internal'
GROUP BY application_name;
```

Expected: count ≤ `COLMENA_POOL_MAX_CONN_PER_URL` (default 2).

---

### Task 4.4: Commit

- [ ] **Step 1: Commit in the `adp` repo**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/main.rs
git commit -m "refactor(worker): use shared ColmenaEngine instead of per-job pools

The engine is built once at worker startup and kept in AppState. process_job
now takes &ColmenaEngine and reuses the pinned internal pool across every
Redis job. Adds SIGTERM-aware graceful shutdown that drains all pools before
Cloud Run kills the container."
```

---

## Commit 5 — Observability: `/debug/pools` + tracing events

### Task 5.1: Expose metrics via HTTP

**Files:**
- Modify: `adp/apps/service/ia/platform/worker/src/main.rs`

- [ ] **Step 1: Add the handler**

```rust
async fn debug_pools(State(state): State<AppState>) -> impl IntoResponse {
    let metrics = state.engine.registry_metrics();
    (StatusCode::OK, axum::Json(metrics))
}
```

- [ ] **Step 2: Register the route**

```rust
let app = Router::new()
    .route("/", get(health_check))
    .route("/process", post(process_jobs_handler))
    .route("/debug/pools", get(debug_pools))
    .with_state(state);
```

- [ ] **Step 3: Verify**

Run the worker locally, then:

```bash
curl http://localhost:8080/debug/pools | jq
```

Expected JSON:

```json
{
  "cached_pools": 1,
  "pinned_pools": 1,
  "evictions_total": 0,
  "get_or_create_total": 1,
  "cache_hits_total": 0,
  "per_url": [
    {"url_hash": "...", "size": 1, "idle": 1, "pinned": true}
  ]
}
```

- [ ] **Step 4: Confirm no raw URL in response**

Grep: `curl -s http://localhost:8080/debug/pools | grep -oE 'postgres://|password|host=' || echo "clean"`
Expected: `clean`.

---

### Task 5.2: Add integration test for engine boot + factory sharing

**Files:**
- Create: `src/libs/colmena/tests/engine_pool_sharing.rs`

This test requires a real Postgres. Gated by env var so CI without DB skips cleanly.

- [ ] **Step 1: Write the test**

```rust
//! Integration test: verify `ColmenaEngine` wires a single pool end-to-end
//! and shares it between state persistence + conversation factory.
//!
//! Requires `TEST_DATABASE_URL` to be set. Otherwise the test skips.

use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::infrastructure::pool_registry::PoolConfig;

fn database_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL").ok()
}

#[tokio::test]
async fn engine_boots_with_pinned_pool_and_migrates() {
    let Some(db) = database_url() else {
        eprintln!("skip: TEST_DATABASE_URL not set");
        return;
    };
    let engine = ColmenaEngine::new(EngineConfig {
        internal_database_url: db,
        pool_config: PoolConfig::defaults(),
    })
    .await
    .expect("engine boots");

    let metrics = engine.registry_metrics();
    assert_eq!(metrics.pinned_pools, 1);
    assert_eq!(metrics.cached_pools, 1);
    assert_eq!(metrics.per_url.len(), 1);
    assert!(metrics.per_url[0].pinned);

    engine.shutdown().await;
    let after = engine.registry_metrics();
    assert_eq!(after.cached_pools, 0);
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    let Some(db) = database_url() else {
        return;
    };
    let engine = ColmenaEngine::new(EngineConfig {
        internal_database_url: db,
        pool_config: PoolConfig::defaults(),
    })
    .await
    .unwrap();
    engine.shutdown().await;
    engine.shutdown().await; // must not panic or log errors
}
```

- [ ] **Step 2: Run (with the env var set)**

```bash
TEST_DATABASE_URL=postgres://... cargo test -p colmena_dag_engine --test engine_pool_sharing
```

Expected: 2 passed.

Without the env var: tests skip gracefully (`cargo test` still returns 0).

---

### Task 5.3: Post-deploy validation runbook

**Files:**
- Create: `docs/superpowers/runbooks/connection-pool-management-validation.md`

- [ ] **Step 1: Write the runbook**

```markdown
# Connection Pool Management — Post-Deploy Validation

Validation checklist for the Phase 1 rollout of `ColmenaEngine` + `PgPoolRegistry`
(spec: `docs/superpowers/specs/2026-04-20-connection-pool-management-design.md`).

## Staging

1. `gcloud run deploy` the worker to staging.
2. Watch logs for the `engine_started` event on boot. Confirm `pinned_pool_count=1`.
3. Submit 10 representative jobs. In the worker logs you should see **no**
   `pool_created` event between them — only `pool_evicted` events are worth
   noticing.
4. `curl https://<staging-worker>/debug/pools`. Expected:
   - `cached_pools == 1` (internal only) if no graph used a second DB.
   - `pinned_pools == 1`.
   - `evictions_total == 0` under normal traffic.
5. On Cloud SQL, check:
   ```sql
   SELECT application_name, count(*)
   FROM pg_stat_activity
   WHERE datname = current_database()
   GROUP BY application_name;
   ```
   Expected: count ≤ `COLMENA_POOL_MAX_CONN_PER_URL` (default 2) per worker instance.
6. Test suspend/resume: trigger a `suspend` node graph, then resume. Confirm the
   state persists and no new pool is opened.
7. Test with a graph referencing an external DB in `connection_url`: expect a
   second entry in `/debug/pools` after the first call; reuse on the second call.

## Production

1. Deploy off-peak. Watch the first 15 minutes of logs for `pool_evicted`
   events — any warn-level eviction during low traffic is a red flag.
2. Monitor Cloud SQL connection count for 1 hour. If it exceeds 50% of
   `max_connections` sustained, open an incident and consider rollback.
3. Rollback is a plain `gcloud run services update-traffic` to the prior revision.
   No schema or Redis changes to undo.
```

- [ ] **Step 2:** No runtime action — this is documentation only.

---

### Task 5.4: Commit + push

- [ ] **Step 1: Commit worker + runbook (in the `adp` repo)**

```bash
cd /home/daniel-garcia4/startti/adp
git add apps/service/ia/platform/worker/src/main.rs
git commit -m "feat(worker): add /debug/pools endpoint for registry metrics"
```

- [ ] **Step 2: Commit the runbook + integration test (in the `colmena` repo)**

```bash
cd /home/daniel-garcia4/startti/colmena
git add src/libs/colmena/tests/engine_pool_sharing.rs \
        docs/superpowers/runbooks/connection-pool-management-validation.md
git commit -m "test,docs: engine boot integration test + post-deploy runbook"
```

- [ ] **Step 3: Run the full test suite once more**

Run: `cargo test -p colmena_dag_engine --lib && cargo clippy -p colmena_dag_engine --all-targets -- -D warnings`
Expected: PASS.

---

## Self-Review Checklist

Run this checklist after completing all commits:

- [ ] **Spec §Goals coverage:** One pool per URL via registry ✓; internal URL pinned ✓; LRU cap ✓; `min_connections=0 + idle_timeout` ✓; explicit async shutdown ✓; env vars with defaults ✓; preserved HTTP/Redis contract ✓.
- [ ] **Spec §Components coverage:** `PoolConfig` (Task 1.4), `EngineConfig` (Task 2.5), `ColmenaEngine` (Tasks 2.5+2.6+2.9), `PgPoolRegistry` (Tasks 1.6–1.10), `SqlPortFactory` (Task 2.4), refactored `ConversationRepositoryFactory` (2.1), refactored `PgPoolAdapter` (2.2), refactored `nodes/sql.rs` (2.8). ✓
- [ ] **Spec §Shutdown semantics:** `shutdown()` idempotent via `AtomicBool` (Task 2.9); `Drop` logs warn if skipped (Task 2.9); worker wires SIGTERM (Task 4.1). ✓
- [ ] **Spec §Config:** All six env vars validated in Task 1.4 with the exact defaults and ranges. ✓
- [ ] **Spec §Observability:** `RegistryMetrics` (Task 1.5), `/debug/pools` endpoint (Task 5.1), `engine_started`/`engine_shutdown`/`pool_created`/`pool_evicted` tracing events wired in Tasks 1.7, 1.8, 2.6, 2.9. ✓
- [ ] **Spec §Testing strategy:** Unit tests in Tasks 1.3, 1.4, 1.5, 1.7–1.10; integration test in Task 5.2. ✓
- [ ] **Spec §Migration plan:** 5 commits match the 5 phases in the spec. ✓
- [ ] **No placeholders** — every step has concrete code or commands.
- [ ] **Type consistency** — `ColmenaEngine`, `EngineConfig`, `PoolConfig`, `PgPoolRegistry`, `UrlKey`, `SqlPortFactory::get_adapter`, `HashMapNodeRegistry::new_with_secure_values(repo_factory, sql_port_factory, task_memory_repo, secure_value_service)` are used identically across tasks.
- [ ] **Risks called out in spec §Open risks:**
  - Shared `PgPoolAdapter` config isolation: covered by the `SET LOCAL`-based approach inherited from the existing code (no plan change needed, but add an integration test with two SQL nodes + same URL + different `statement_timeout_ms` as a follow-up if issues appear in staging).
  - URL normalization collisions: conservative — tests in Task 1.3 lock it in.
  - Eviction with in-flight queries: `close_all` awaits sqlx graceful close (Task 1.9).
  - sqlx prepared-statement cache: not material; only rebuilt on eviction.

**If any checkbox is unchecked after walking the plan, fix it before starting execution.**
