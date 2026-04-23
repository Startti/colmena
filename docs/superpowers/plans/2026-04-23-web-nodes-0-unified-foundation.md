# Web Nodes — Unified Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the shared foundation that the three web toolkit nodes (`tavily_client`, `api_explorer`, `browser`) depend on: the `web/` hexagonal module skeleton, the `ToolkitNode` trait, the multi-tool-per-node runtime extension in `DagToolExecutor`, the shared `SessionRegistry<T>`, and the shared `WebDomainError` enum. Ship with a tiny stub toolkit node so the dispatch path is exercised end-to-end in tests before any real node lands.

**Architecture:** New top-level module `src/libs/colmena/src/web/` with empty `domain/`, `application/`, `infrastructure/` subdirs populated in this plan only with the cross-cutting pieces. `ToolkitNode` extends `ExecutableNode` in `dag_engine/domain/node.rs`. `ToolConfiguration` gains three new optional fields (`node_type` already exists; `node_config` and `expose_sub_tools` are new). `DagToolExecutor` gains a new `generate_toolkit_tool_definitions()` and dispatches `__sub_tool` into `execute()` when the config is a toolkit entry. The `HashMapNodeRegistry` exposes a `get_toolkit_node` helper. `SessionRegistry<T>` lives in `web/domain/session.rs` and uses `tokio::sync::Mutex` + a lazy sweeper task.

**Tech Stack:** Rust (async/await + tokio), `serde` / `serde_json`, `thiserror`, `async-trait`, `mockall` (dev). No new runtime dependencies introduced by this plan — `lru` and `dashmap` are already present; `chrono` is already present (used for TTL timestamps).

**Design spec:** [docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md](../specs/2026-04-23-web-nodes-unified-design.md)

---

## Conventions for this plan

- All commits use: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib <module>` — the crate is named `colmena_dag_engine`.
- After each task, run `cargo check --lib` before committing; it must pass.
- After tasks that touch `tool_configuration.rs` or `dag_tool_executor.rs`, also run `cargo test --lib tool_configuration dag_tool_executor` before committing.
- Every task ends with a commit. Don't batch.
- This plan delivers **no user-facing nodes**. All tests here are internal. The three nodes themselves ship in plans A / C / B.

---

## Task 0: Create the `web/` module skeleton

**Files:**
- Create: `src/libs/colmena/src/web/mod.rs`
- Create: `src/libs/colmena/src/web/domain/mod.rs`
- Create: `src/libs/colmena/src/web/application/mod.rs`
- Create: `src/libs/colmena/src/web/infrastructure/mod.rs`
- Modify: `src/libs/colmena/src/lib.rs`

- [ ] **Step 1: Create the module tree**

Create `src/libs/colmena/src/web/mod.rs`:

```rust
//! Web toolkit nodes — shared foundation.
//!
//! See `docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md`.
//!
//! This module hosts the ports, use cases, and adapters for three toolkit nodes
//! (`tavily_client`, `api_explorer`, `browser`) plus cross-cutting pieces that
//! all of them use: [`session::SessionRegistry`], [`errors::WebDomainError`].

pub mod domain;
pub mod application;
pub mod infrastructure;
```

Create `src/libs/colmena/src/web/domain/mod.rs`:

```rust
//! Ports and value objects for the `web` toolkit nodes.
```

Create `src/libs/colmena/src/web/application/mod.rs`:

```rust
//! Use cases orchestrating web-toolkit ports.
```

Create `src/libs/colmena/src/web/infrastructure/mod.rs`:

```rust
//! Adapters implementing the web-toolkit ports.
```

- [ ] **Step 2: Register the module in `lib.rs`**

Edit `src/libs/colmena/src/lib.rs`. After line 4 (`pub mod skills;`), add:

```rust
pub mod web;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --lib 2>&1 | tail -20`
Expected: clean build (warnings about unused modules are OK).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web src/libs/colmena/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(web): add web module skeleton for toolkit nodes

Scaffolding for the shared web/ module that hosts the domain, application,
and infrastructure layers of the three incoming toolkit nodes. No behavior
yet — subsequent tasks populate it.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: Shared `WebDomainError`

**Files:**
- Create: `src/libs/colmena/src/web/domain/errors.rs`
- Modify: `src/libs/colmena/src/web/domain/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/web/domain/errors.rs`:

```rust
//! Domain errors shared across the three web-toolkit ports (search, api_spec, browser).
//!
//! The convention (per spec): variants whose `Display` message is stable and
//! LLM-addressable are returned to the LLM as structured tool results. Variants
//! categorized as "configuration/init" failures crash the DAG. `WebDomainError::is_llm_recoverable()`
//! classifies which is which for use-case layers.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WebDomainError {
    // Crash the DAG (config/init). Not recoverable by the LLM.
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("adapter init failed: {0}")]
    AdapterInit(String),

    // Returned to the LLM as structured results (recoverable).
    #[error("rate limit exceeded ({calls_used}/{cap})")]
    RateLimit { calls_used: u32, cap: u32 },

    #[error("session lost")]
    SessionLost { last_known_url: Option<String> },

    #[error("selector not found: {selector} on {page_url}")]
    SelectorNotFound {
        selector: String,
        page_url: String,
        hints: Vec<String>,
    },

    #[error("navigation failed: {0}")]
    NavigationFailed(String),

    #[error("timeout after {ms}ms")]
    Timeout { ms: u64 },

    #[error("spec parse failed: {0}")]
    SpecParseError(String),

    #[error("endpoint not found: {searched_for}")]
    EndpointNotFound {
        searched_for: String,
        did_you_mean: Vec<String>,
    },

    #[error("upstream {status}: {body}")]
    Upstream { status: u16, body: String },

    #[error("session cap reached ({active}/{cap})")]
    SessionCapReached { active: u32, cap: u32 },

    #[error("unexpected HTML response from {url}")]
    UnexpectedHtmlResponse { url: String, resolved_url: String },
}

impl WebDomainError {
    /// Returns `true` when this error should be surfaced to the LLM as a structured
    /// tool result. Returns `false` for configuration / adapter init failures that
    /// should bubble up and crash the DAG.
    pub fn is_llm_recoverable(&self) -> bool {
        !matches!(self, Self::InvalidConfig(_) | Self::AdapterInit(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_crashes_dag() {
        assert!(!WebDomainError::InvalidConfig("bad".into()).is_llm_recoverable());
    }

    #[test]
    fn adapter_init_crashes_dag() {
        assert!(!WebDomainError::AdapterInit("no token".into()).is_llm_recoverable());
    }

    #[test]
    fn rate_limit_is_recoverable() {
        assert!(WebDomainError::RateLimit { calls_used: 51, cap: 50 }.is_llm_recoverable());
    }

    #[test]
    fn session_lost_is_recoverable() {
        assert!(WebDomainError::SessionLost { last_known_url: None }.is_llm_recoverable());
    }

    #[test]
    fn timeout_is_recoverable() {
        assert!(WebDomainError::Timeout { ms: 3000 }.is_llm_recoverable());
    }

    #[test]
    fn upstream_is_recoverable() {
        assert!(WebDomainError::Upstream {
            status: 502,
            body: "bad gateway".into()
        }
        .is_llm_recoverable());
    }

    #[test]
    fn display_uses_thiserror_message() {
        let e = WebDomainError::RateLimit { calls_used: 51, cap: 50 };
        assert_eq!(e.to_string(), "rate limit exceeded (51/50)");
    }
}
```

- [ ] **Step 2: Wire the module**

Edit `src/libs/colmena/src/web/domain/mod.rs`:

```rust
//! Ports and value objects for the `web` toolkit nodes.

pub mod errors;

pub use errors::WebDomainError;
```

- [ ] **Step 3: Run the tests — expect PASS**

Run: `cargo test --lib web::domain::errors`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/domain
git commit -m "$(cat <<'EOF'
feat(web): add WebDomainError enum

Shared domain error for the three web-toolkit ports. Includes an
is_llm_recoverable() classifier so use cases know which variants should
bubble up as structured tool results and which should crash the DAG.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `SessionKey` and `SessionRegistry<T>` — part 1: types + basic insert/get/remove

**Files:**
- Create: `src/libs/colmena/src/web/domain/session.rs`
- Modify: `src/libs/colmena/src/web/domain/mod.rs`

The spec calls this a domain-layer type with a generic `T`. The registry owns storage and a sweeper; cleanup logic for each session type (`T`) is provided as a closure at registry construction. We build it in three incremental tasks: types+CRUD (this task), TTL sweep (Task 3), and eager `cleanup_conversation` (Task 4).

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/web/domain/session.rs`:

```rust
//! Conversation-scoped session registry shared by the three web-toolkit nodes.
//!
//! Generic over the session state type `T`. Each node constructs its own
//! `Arc<SessionRegistry<MyState>>` and looks entries up by
//! `SessionKey { conversation_id, session_name }`.
//!
//! The registry supports three scopes of cleanup:
//! - Explicit removal via `remove()`.
//! - Passive TTL-based eviction via a background sweeper (Task 3).
//! - Eager removal of all entries for a given `conversation_id` via
//!   `cleanup_conversation()` (Task 4).

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub type ConversationId = String;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub conversation_id: ConversationId,
    pub session_name: String,
}

impl SessionKey {
    pub fn new(conversation_id: impl Into<String>, session_name: impl Into<String>) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            session_name: session_name.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TtlConfig {
    pub idle_ttl_seconds: u64,
    pub max_lifetime_seconds: u64,
    pub max_active_sessions: u32,
}

impl Default for TtlConfig {
    fn default() -> Self {
        Self {
            idle_ttl_seconds: 900,
            max_lifetime_seconds: 3600,
            max_active_sessions: 50,
        }
    }
}

pub struct SessionEntry<T> {
    pub value: T,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

pub struct SessionRegistry<T> {
    inner: Arc<Mutex<HashMap<SessionKey, SessionEntry<T>>>>,
    ttl: TtlConfig,
}

impl<T> SessionRegistry<T> {
    pub fn new(ttl: TtlConfig) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        })
    }

    pub fn ttl(&self) -> &TtlConfig {
        &self.ttl
    }

    /// Insert a new entry (or replace if one exists). Returns the previous entry if any.
    pub async fn insert(&self, key: SessionKey, value: T) -> Option<T> {
        let mut map = self.inner.lock().await;
        let now = Utc::now();
        let prev = map.remove(&key);
        map.insert(
            key,
            SessionEntry {
                value,
                created_at: now,
                last_activity: now,
            },
        );
        prev.map(|e| e.value)
    }

    /// Get the current number of entries.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Return `true` if the key is present.
    pub async fn contains(&self, key: &SessionKey) -> bool {
        self.inner.lock().await.contains_key(key)
    }

    /// Remove a single entry by key. Returns the extracted value if any.
    pub async fn remove(&self, key: &SessionKey) -> Option<T> {
        self.inner.lock().await.remove(key).map(|e| e.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_and_contains() {
        let reg: Arc<SessionRegistry<String>> = SessionRegistry::new(TtlConfig::default());
        let key = SessionKey::new("conv-1", "default");
        assert!(!reg.contains(&key).await);
        reg.insert(key.clone(), "hello".into()).await;
        assert!(reg.contains(&key).await);
        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn insert_replaces_and_returns_prev() {
        let reg: Arc<SessionRegistry<String>> = SessionRegistry::new(TtlConfig::default());
        let key = SessionKey::new("conv-1", "default");
        reg.insert(key.clone(), "first".into()).await;
        let prev = reg.insert(key.clone(), "second".into()).await;
        assert_eq!(prev, Some("first".into()));
        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn remove_returns_value() {
        let reg: Arc<SessionRegistry<String>> = SessionRegistry::new(TtlConfig::default());
        let key = SessionKey::new("conv-1", "default");
        reg.insert(key.clone(), "bye".into()).await;
        let removed = reg.remove(&key).await;
        assert_eq!(removed, Some("bye".into()));
        assert!(!reg.contains(&key).await);
    }

    #[tokio::test]
    async fn remove_missing_returns_none() {
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(TtlConfig::default());
        let key = SessionKey::new("conv-1", "default");
        assert!(reg.remove(&key).await.is_none());
    }
}
```

- [ ] **Step 2: Wire the module**

Edit `src/libs/colmena/src/web/domain/mod.rs`:

```rust
//! Ports and value objects for the `web` toolkit nodes.

pub mod errors;
pub mod session;

pub use errors::WebDomainError;
pub use session::{ConversationId, SessionEntry, SessionKey, SessionRegistry, TtlConfig};
```

- [ ] **Step 3: Run tests — expect PASS**

Run: `cargo test --lib web::domain::session`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/domain
git commit -m "$(cat <<'EOF'
feat(web): add SessionKey, TtlConfig, SessionRegistry CRUD

First slice of the shared conversation-scoped session registry. Later
tasks add the TTL sweeper and eager conversation cleanup.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `SessionRegistry<T>` — TTL sweep + activity touch

**Files:**
- Modify: `src/libs/colmena/src/web/domain/session.rs`

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `src/libs/colmena/src/web/domain/session.rs`:

```rust
    #[tokio::test]
    async fn get_touches_last_activity() {
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(TtlConfig::default());
        let key = SessionKey::new("conv-1", "default");
        reg.insert(key.clone(), 7).await;

        let first = reg
            .inner
            .lock()
            .await
            .get(&key)
            .map(|e| e.last_activity)
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let got = reg.with_entry(&key, |e| e.value).await;
        assert_eq!(got, Some(7));

        let second = reg
            .inner
            .lock()
            .await
            .get(&key)
            .map(|e| e.last_activity)
            .unwrap();
        assert!(second > first, "with_entry must update last_activity");
    }

    #[tokio::test]
    async fn sweep_removes_idle_expired_entries() {
        let ttl = TtlConfig {
            idle_ttl_seconds: 0, // everything is immediately idle-expired
            max_lifetime_seconds: 3600,
            max_active_sessions: 50,
        };
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(ttl);
        reg.insert(SessionKey::new("c1", "default"), 1).await;
        reg.insert(SessionKey::new("c2", "default"), 2).await;
        assert_eq!(reg.len().await, 2);

        // allow wall-clock to advance
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let evicted = reg.sweep_expired(|_v| {}).await;
        assert_eq!(evicted, 2);
        assert_eq!(reg.len().await, 0);
    }

    #[tokio::test]
    async fn sweep_removes_max_lifetime_expired() {
        let ttl = TtlConfig {
            idle_ttl_seconds: 3600,
            max_lifetime_seconds: 0, // expire on lifetime
            max_active_sessions: 50,
        };
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(ttl);
        reg.insert(SessionKey::new("c1", "default"), 1).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let evicted = reg.sweep_expired(|_v| {}).await;
        assert_eq!(evicted, 1);
    }

    #[tokio::test]
    async fn sweep_calls_cleanup_closure_per_evicted() {
        let ttl = TtlConfig {
            idle_ttl_seconds: 0,
            max_lifetime_seconds: 3600,
            max_active_sessions: 50,
        };
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(ttl);
        reg.insert(SessionKey::new("c1", "default"), 10).await;
        reg.insert(SessionKey::new("c2", "default"), 20).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let collected = Arc::new(Mutex::new(Vec::<u32>::new()));
        let collected_clone = collected.clone();
        reg.sweep_expired(move |v| {
            let c = collected_clone.clone();
            // Note: cleanup closure is sync; accumulate via blocking lock. Tests only.
            let mut guard = c.try_lock().unwrap();
            guard.push(v);
        })
        .await;

        let guard = collected.lock().await;
        let mut vals: Vec<u32> = guard.clone();
        vals.sort_unstable();
        assert_eq!(vals, vec![10, 20]);
    }
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --lib web::domain::session::tests::get_touches_last_activity`
Expected: FAIL — `with_entry` and `sweep_expired` do not exist yet.

- [ ] **Step 3: Add `with_entry` and `sweep_expired`**

Add the following methods inside `impl<T> SessionRegistry<T>` in `src/libs/colmena/src/web/domain/session.rs`:

```rust
    /// Apply a closure to the entry for `key` if present. Updates `last_activity` on
    /// each call. Returns `Some(f(&entry.value))` or `None`.
    pub async fn with_entry<R>(&self, key: &SessionKey, f: impl FnOnce(&T) -> R) -> Option<R>
    where
        T: Clone,
    {
        let mut map = self.inner.lock().await;
        if let Some(entry) = map.get_mut(key) {
            entry.last_activity = Utc::now();
            Some(f(&entry.value))
        } else {
            None
        }
    }

    /// Remove entries whose idle TTL or max-lifetime has been exceeded. The
    /// provided cleanup closure is invoked once per evicted value. Returns the
    /// number of entries removed.
    ///
    /// The closure runs synchronously inside the registry's critical section;
    /// callers that need async cleanup should spawn it from the closure using
    /// `tokio::spawn`.
    pub async fn sweep_expired(&self, mut on_evicted: impl FnMut(T)) -> usize {
        use chrono::Duration as ChronoDuration;

        let now = Utc::now();
        let idle_cap = ChronoDuration::seconds(self.ttl.idle_ttl_seconds as i64);
        let life_cap = ChronoDuration::seconds(self.ttl.max_lifetime_seconds as i64);

        let mut map = self.inner.lock().await;
        let expired_keys: Vec<SessionKey> = map
            .iter()
            .filter(|(_, entry)| {
                (now - entry.last_activity) > idle_cap || (now - entry.created_at) > life_cap
            })
            .map(|(k, _)| k.clone())
            .collect();

        let count = expired_keys.len();
        for k in expired_keys {
            if let Some(entry) = map.remove(&k) {
                on_evicted(entry.value);
            }
        }
        count
    }
```

- [ ] **Step 4: Run the tests — expect PASS**

Run: `cargo test --lib web::domain::session`
Expected: 8 tests pass (4 from Task 2 + 4 new).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/domain/session.rs
git commit -m "$(cat <<'EOF'
feat(web): add with_entry + sweep_expired to SessionRegistry

with_entry touches last_activity on each access (used by use cases to
hold off idle eviction while the session is in use). sweep_expired
removes idle/lifetime-expired entries and fires a per-value cleanup
closure so adapters can close connections / free resources.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `SessionRegistry<T>` — conversation cleanup + capacity eviction

**Files:**
- Modify: `src/libs/colmena/src/web/domain/session.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `session.rs`:

```rust
    #[tokio::test]
    async fn cleanup_conversation_removes_matching_entries() {
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(TtlConfig::default());
        reg.insert(SessionKey::new("conv-a", "s1"), 1).await;
        reg.insert(SessionKey::new("conv-a", "s2"), 2).await;
        reg.insert(SessionKey::new("conv-b", "s1"), 3).await;

        let removed = reg.cleanup_conversation("conv-a", |_v| {}).await;
        assert_eq!(removed, 2);
        assert_eq!(reg.len().await, 1);
        assert!(reg.contains(&SessionKey::new("conv-b", "s1")).await);
    }

    #[tokio::test]
    async fn insert_evicts_lru_when_over_cap() {
        let ttl = TtlConfig {
            idle_ttl_seconds: 3600,
            max_lifetime_seconds: 3600,
            max_active_sessions: 2,
        };
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(ttl);

        let k1 = SessionKey::new("c1", "default");
        let k2 = SessionKey::new("c2", "default");
        let k3 = SessionKey::new("c3", "default");

        let evicted_keys: Arc<Mutex<Vec<SessionKey>>> = Arc::new(Mutex::new(Vec::new()));

        // k1 is inserted first, then touched; k2 is inserted next; k3 triggers eviction → k2 should go.
        reg.insert_with_capacity(k1.clone(), 1, {
            let ek = evicted_keys.clone();
            move |k, _v| {
                let mut g = ek.try_lock().unwrap();
                g.push(k);
            }
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        reg.insert_with_capacity(k2.clone(), 2, {
            let ek = evicted_keys.clone();
            move |k, _v| {
                let mut g = ek.try_lock().unwrap();
                g.push(k);
            }
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        // Touch k1 so k2 becomes the LRU victim.
        reg.with_entry(&k1, |_| ()).await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        reg.insert_with_capacity(k3.clone(), 3, {
            let ek = evicted_keys.clone();
            move |k, _v| {
                let mut g = ek.try_lock().unwrap();
                g.push(k);
            }
        })
        .await;

        assert_eq!(reg.len().await, 2);
        let evicted = evicted_keys.lock().await.clone();
        assert_eq!(evicted, vec![k2]);
    }
```

- [ ] **Step 2: Run the test — expect FAIL**

Run: `cargo test --lib web::domain::session::tests::cleanup_conversation_removes_matching_entries`
Expected: FAIL — methods do not exist yet.

- [ ] **Step 3: Implement the two methods**

Add to `impl<T> SessionRegistry<T>` in `session.rs`:

```rust
    /// Remove every entry whose `SessionKey.conversation_id` matches `conversation_id`.
    /// Returns the number of entries removed. The cleanup closure fires once per evicted value.
    pub async fn cleanup_conversation(
        &self,
        conversation_id: &str,
        mut on_evicted: impl FnMut(T),
    ) -> usize {
        let mut map = self.inner.lock().await;
        let matching: Vec<SessionKey> = map
            .keys()
            .filter(|k| k.conversation_id == conversation_id)
            .cloned()
            .collect();
        let count = matching.len();
        for k in matching {
            if let Some(entry) = map.remove(&k) {
                on_evicted(entry.value);
            }
        }
        count
    }

    /// Insert respecting `max_active_sessions`. If at or over capacity, evict the
    /// LRU (oldest `last_activity`) entry before inserting. The cleanup closure fires
    /// once if an entry was evicted, with the evicted key and value.
    pub async fn insert_with_capacity(
        &self,
        key: SessionKey,
        value: T,
        on_evicted: impl FnOnce(SessionKey, T),
    ) -> Option<T> {
        let mut map = self.inner.lock().await;
        let now = Utc::now();

        if (map.len() as u32) >= self.ttl.max_active_sessions && !map.contains_key(&key) {
            if let Some((victim_key, _)) = map
                .iter()
                .min_by_key(|(_, e)| e.last_activity)
                .map(|(k, e)| (k.clone(), e.last_activity))
            {
                if let Some(entry) = map.remove(&victim_key) {
                    on_evicted(victim_key, entry.value);
                }
            }
        }

        let prev = map.remove(&key);
        map.insert(
            key,
            SessionEntry {
                value,
                created_at: now,
                last_activity: now,
            },
        );
        prev.map(|e| e.value)
    }
```

- [ ] **Step 4: Run the tests — expect PASS**

Run: `cargo test --lib web::domain::session`
Expected: 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/domain/session.rs
git commit -m "$(cat <<'EOF'
feat(web): add cleanup_conversation + insert_with_capacity

cleanup_conversation is called by the engine when a conversation closes.
insert_with_capacity enforces max_active_sessions via LRU eviction. Both
fire per-value cleanup closures so adapters can release resources
synchronously with the registry update.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Background sweeper

**Files:**
- Modify: `src/libs/colmena/src/web/domain/session.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `session.rs`:

```rust
    #[tokio::test]
    async fn start_sweeper_evicts_on_tick() {
        let ttl = TtlConfig {
            idle_ttl_seconds: 0,
            max_lifetime_seconds: 3600,
            max_active_sessions: 50,
        };
        let reg: Arc<SessionRegistry<u32>> = SessionRegistry::new(ttl);

        let evicted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let evicted_clone = evicted.clone();

        let handle = reg.clone().start_sweeper(
            std::time::Duration::from_millis(50),
            move |_v| {
                evicted_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        );

        reg.insert(SessionKey::new("c1", "default"), 1).await;
        reg.insert(SessionKey::new("c2", "default"), 2).await;

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        handle.abort();

        assert_eq!(evicted.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(reg.len().await, 0);
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::domain::session::tests::start_sweeper_evicts_on_tick`
Expected: FAIL — method missing.

- [ ] **Step 3: Implement**

Add to `impl<T> SessionRegistry<T>` in `session.rs`:

```rust
    /// Spawn a background tokio task that periodically calls `sweep_expired`.
    /// Returns the task handle; callers retain it and call `.abort()` during shutdown.
    ///
    /// The cleanup closure must be `Send + 'static + Clone` because it is shared
    /// across ticks of the sweeper.
    pub fn start_sweeper<F>(
        self: Arc<Self>,
        period: std::time::Duration,
        cleanup: F,
    ) -> tokio::task::JoinHandle<()>
    where
        T: Send + 'static,
        F: Fn(T) + Send + Sync + Clone + 'static,
    {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let c = cleanup.clone();
                self.sweep_expired(c).await;
            }
        })
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::domain::session`
Expected: 11 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/domain/session.rs
git commit -m "$(cat <<'EOF'
feat(web): add start_sweeper for background TTL eviction

Spawns a tokio task that calls sweep_expired on a fixed period. Nodes
that own a SessionRegistry call this once at construction and keep the
JoinHandle for shutdown abort.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Extend `ToolConfiguration` with toolkit fields

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`

- [ ] **Step 1: Write the failing test**

Append inside the existing `#[cfg(test)] mod tests` in `tool_configuration.rs`:

```rust
    #[test]
    fn deserialize_toolkit_config_all() {
        let json = serde_json::json!({
            "name": "web",
            "description": "Web search",
            "node_type": "tavily_client",
            "node_config": { "api_key": "${TAVILY_API_KEY}" },
            "expose_sub_tools": "all"
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.node_type, "tavily_client");
        assert!(cfg.is_toolkit());
        match cfg.expose_sub_tools {
            Some(SubToolFilter::All) => {}
            other => panic!("expected All, got {:?}", other),
        }
        assert_eq!(
            cfg.node_config
                .as_ref()
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str()),
            Some("${TAVILY_API_KEY}")
        );
    }

    #[test]
    fn deserialize_toolkit_config_list() {
        let json = serde_json::json!({
            "name": "browser",
            "description": "",
            "node_type": "browser",
            "node_config": { "browserless_ws_url": "ws://localhost:3000" },
            "expose_sub_tools": ["navigate", "click"]
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(cfg.is_toolkit());
        match cfg.expose_sub_tools {
            Some(SubToolFilter::List(ref v)) => {
                assert_eq!(v, &vec!["navigate".to_string(), "click".to_string()])
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn legacy_config_is_not_toolkit() {
        let json = serde_json::json!({
            "name": "fetch_users",
            "description": "List users",
            "node_type": "http_request",
            "fixed_config": { "base_url": "https://api.example.com" }
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(!cfg.is_toolkit());
        assert!(cfg.node_config.is_none());
        assert!(cfg.expose_sub_tools.is_none());
    }
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo test --lib tool_configuration::tests::deserialize_toolkit_config_all`
Expected: FAIL — `SubToolFilter`, `node_config`, `expose_sub_tools`, and `is_toolkit` do not exist.

- [ ] **Step 3: Add the fields and helper**

Edit `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`:

Add the new enum above `pub struct ToolConfiguration`:

```rust
/// Selector for which sub-tools of a toolkit node to expose to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubToolFilter {
    /// String `"all"` — expose every sub-tool the node declares.
    All,
    /// An explicit allow-list of sub-tool names (without the `toolkit_alias__` prefix).
    List(Vec<String>),
}
```

Wait — `untagged` alone won't distinguish `"all"` from an arbitrary string. Replace with a custom deserializer via a proxy enum:

```rust
/// Selector for which sub-tools of a toolkit node to expose to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubToolFilter {
    /// An explicit allow-list of sub-tool names (without the `toolkit_alias__` prefix).
    List(Vec<String>),
    /// String `"all"` — expose every sub-tool the node declares.
    Keyword(SubToolKeyword),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubToolKeyword {
    #[serde(rename = "all")]
    All,
}

impl SubToolFilter {
    pub fn all() -> Self {
        Self::Keyword(SubToolKeyword::All)
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Self::Keyword(SubToolKeyword::All))
    }

    /// Return `true` if the given sub-tool should be exposed.
    pub fn includes(&self, sub_tool: &str) -> bool {
        match self {
            Self::Keyword(SubToolKeyword::All) => true,
            Self::List(v) => v.iter().any(|s| s == sub_tool),
        }
    }
}
```

Add the two new fields to `pub struct ToolConfiguration`, after `pub node_schema: Option<NodeSchema>`:

```rust
    /// Per-toolkit static node configuration passed to the toolkit node at runtime.
    /// Only meaningful for toolkit entries (where `expose_sub_tools` is set).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub node_config: Option<Value>,

    /// Which sub-tools of this toolkit to expose to the LLM. When present, the entry
    /// is treated as a toolkit entry and the generator expands it into N ToolDefinitions.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expose_sub_tools: Option<SubToolFilter>,
```

Also adjust the matching test-expectations for the two new tests, and add the helper `is_toolkit()` below the struct:

```rust
impl ToolConfiguration {
    /// Whether this configuration represents a **toolkit** entry (a node that
    /// exposes multiple sub-tools to the LLM) rather than a legacy single-tool
    /// configuration.
    pub fn is_toolkit(&self) -> bool {
        self.expose_sub_tools.is_some()
    }
}
```

Update the matching pattern in the new tests to match the `SubToolFilter::Keyword(SubToolKeyword::All)` variant:

```rust
    #[test]
    fn deserialize_toolkit_config_all() {
        let json = serde_json::json!({
            "name": "web",
            "description": "Web search",
            "node_type": "tavily_client",
            "node_config": { "api_key": "${TAVILY_API_KEY}" },
            "expose_sub_tools": "all"
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.node_type, "tavily_client");
        assert!(cfg.is_toolkit());
        assert!(cfg.expose_sub_tools.as_ref().unwrap().is_all());
        assert_eq!(
            cfg.node_config
                .as_ref()
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str()),
            Some("${TAVILY_API_KEY}")
        );
    }

    #[test]
    fn deserialize_toolkit_config_list() {
        let json = serde_json::json!({
            "name": "browser",
            "description": "",
            "node_type": "browser",
            "node_config": { "browserless_ws_url": "ws://localhost:3000" },
            "expose_sub_tools": ["navigate", "click"]
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(cfg.is_toolkit());
        let filter = cfg.expose_sub_tools.as_ref().unwrap();
        assert!(!filter.is_all());
        assert!(filter.includes("navigate"));
        assert!(filter.includes("click"));
        assert!(!filter.includes("fill"));
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib tool_configuration`
Expected: all existing tests plus the 3 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/tool_configuration.rs
git commit -m "$(cat <<'EOF'
feat(tool_configuration): add toolkit fields to ToolConfiguration

Adds node_config (Option<Value>) and expose_sub_tools (Option<SubToolFilter>)
so a tool_configurations entry can declare a toolkit node. SubToolFilter
accepts either the keyword string "all" or an explicit allow-list.
is_toolkit() distinguishes new entries from legacy single-tool entries.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Add `ToolkitNode` trait and `SubToolDefinition`

**Files:**
- Create: `src/libs/colmena/src/dag_engine/domain/toolkit_node.rs`
- Modify: `src/libs/colmena/src/dag_engine/domain/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/dag_engine/domain/toolkit_node.rs`:

```rust
//! ToolkitNode — a node that exposes multiple sub-tools to the LLM.
//!
//! See `docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md` §
//! "Runtime extension: multi-tool per node".
//!
//! The reserved input key `__sub_tool` identifies which sub-tool the LLM invoked.
//! Toolkit nodes branch on this key in their `execute()` implementation.

use crate::dag_engine::domain::node::ExecutableNode;
use crate::llm::domain::ParameterProperty;
use serde_json::Value;
use std::collections::HashMap;

/// Reserved input key injected by `DagToolExecutor` to identify which sub-tool
/// of a toolkit node the LLM invoked.
pub const SUB_TOOL_INPUT_KEY: &str = "__sub_tool";

/// One sub-tool within a toolkit node.
#[derive(Debug, Clone)]
pub struct SubToolDefinition {
    /// Short programmatic name (no toolkit prefix). Examples: `"search"`, `"navigate"`.
    pub name: &'static str,
    /// Rich description shown to the LLM. Accuracy relies on this.
    pub description: String,
    /// JSON-Schema-style properties map for the LLM-visible parameters.
    pub properties: HashMap<String, ParameterProperty>,
    /// Names of the parameters the LLM is required to supply.
    pub required: Vec<String>,
}

/// Marker trait for nodes that expose multiple sub-tools.
///
/// A node that implements `ToolkitNode` is also an `ExecutableNode`; the runtime
/// dispatches on the reserved `__sub_tool` input key when executing the node.
///
/// `sub_tool_catalog(&config)` may return a **static** list (most toolkits here)
/// or a **dynamic** list computed from the node configuration (future work —
/// e.g. exposing each endpoint of an HTTP spec as its own sub-tool).
pub trait ToolkitNode: ExecutableNode {
    fn sub_tool_catalog(&self, config: &Value) -> Vec<SubToolDefinition>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_tool_input_key_is_reserved_constant() {
        assert_eq!(SUB_TOOL_INPUT_KEY, "__sub_tool");
    }

    #[test]
    fn sub_tool_definition_clone_is_cheap() {
        let def = SubToolDefinition {
            name: "search",
            description: "search the web".into(),
            properties: HashMap::new(),
            required: Vec::new(),
        };
        let cloned = def.clone();
        assert_eq!(cloned.name, "search");
    }
}
```

- [ ] **Step 2: Register in the domain mod**

Edit `src/libs/colmena/src/dag_engine/domain/mod.rs`. Add:

```rust
pub mod toolkit_node;
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib toolkit_node`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/toolkit_node.rs src/libs/colmena/src/dag_engine/domain/mod.rs
git commit -m "$(cat <<'EOF'
feat(domain): add ToolkitNode trait and SubToolDefinition

Introduces the trait that the three upcoming web nodes implement.
SUB_TOOL_INPUT_KEY is the reserved inputs key the executor injects at
dispatch time, letting a single node branch on sub-tool at execute()
time.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Add a `ToolkitNodeRegistryPort` helper and in-memory lookup

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/ports.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

The executor needs to ask the registry "is this node a toolkit node?" without downcasting. We add a second `HashMap<String, Arc<dyn ToolkitNode>>` side-by-side with the existing one.

- [ ] **Step 1: Read the ports file to find insertion point**

Run: `cargo check --lib 2>&1 | head -5` (sanity check before editing).

- [ ] **Step 2: Extend `NodeRegistryPort`**

Edit `src/libs/colmena/src/dag_engine/application/ports.rs`. Find `pub trait NodeRegistryPort` and add a new method with a default implementation so existing impls don't break:

```rust
    /// Return the node as a `ToolkitNode` if it was registered as one; `None`
    /// otherwise (including for standalone ExecutableNode registrations). Default
    /// impl returns `None` so existing registries don't need changes.
    fn get_toolkit_node(
        &self,
        _node_type: &str,
    ) -> Option<std::sync::Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>> {
        None
    }
```

- [ ] **Step 3: Implement it on `HashMapNodeRegistry`**

Edit `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`. Replace the existing struct, `new_with_secure_values` builder, and `NodeRegistryPort` impl with the versions below (keeping all existing node registrations unchanged; only the storage and the new method are added):

Find the struct definition:

```rust
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    subgraph_node: Option<Arc<SubGraphNode>>,
}
```

Replace with:

```rust
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    toolkit_nodes:
        HashMap<String, Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>>,
    subgraph_node: Option<Arc<SubGraphNode>>,
}
```

Find the closing `Self { nodes, subgraph_node: Some(sub_node) }` at the end of `new_with_secure_values` and replace with:

```rust
            Self {
                nodes,
                toolkit_nodes: HashMap::new(),
                subgraph_node: Some(sub_node),
            }
```

At the bottom of the file, extend the `impl NodeRegistryPort for HashMapNodeRegistry` block with the new method:

```rust
    fn get_toolkit_node(
        &self,
        node_type: &str,
    ) -> Option<Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>> {
        self.toolkit_nodes.get(node_type).cloned()
    }
```

Also add a public helper on the struct itself so the registry builder can register toolkit nodes. Add a new `impl HashMapNodeRegistry` block:

```rust
impl HashMapNodeRegistry {
    /// Register a toolkit node. The node is stored in both maps (as
    /// `ExecutableNode` for normal DAG use and as `ToolkitNode` for sub-tool
    /// dispatch in the executor).
    pub fn register_toolkit_node<N>(self: &mut Arc<Self>, node_type: impl Into<String>, node: Arc<N>)
    where
        N: crate::dag_engine::domain::toolkit_node::ToolkitNode + 'static,
    {
        // SAFETY: called only before the Arc is handed to the engine; there is
        // a single reference at this point. Converting via Arc::get_mut is
        // correct while we're still in construction.
        if let Some(this) = Arc::get_mut(self) {
            let name = node_type.into();
            this.nodes.insert(name.clone(), node.clone() as Arc<dyn ExecutableNode>);
            this.toolkit_nodes
                .insert(name, node as Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>);
        }
    }
}
```

> **Note:** `Arc::get_mut` returning `Some` requires that no other Arc / Weak references exist. The current registry constructor uses `Arc::new_cyclic`, which creates a `Weak` reference. The helper is intended for **test** construction (where you have a fresh `Arc`), not for mutating the shared runtime registry. Production registration of the three real nodes in plans A/C/B happens inside `new_with_secure_values` directly (where it has `&mut self` on the inner value), not through this helper. This helper exists so Task 11's integration test can register a stub toolkit node.

- [ ] **Step 4: Verify the whole crate still compiles**

Run: `cargo check --lib 2>&1 | tail -20`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/ports.rs src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "$(cat <<'EOF'
feat(registry): add toolkit-node lookup to NodeRegistryPort

Extends HashMapNodeRegistry with a parallel HashMap keyed by node_type
holding Arc<dyn ToolkitNode>. Executor uses get_toolkit_node() to
distinguish toolkit entries from legacy single-tool entries without
downcasting.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Add a stub `EchoToolkitNode` for testing

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

This stub has no dependencies and exists solely to drive integration tests of the toolkit runtime. It exposes two sub-tools: `echo` (returns its input string) and `double` (returns its input number × 2).

- [ ] **Step 1: Write the node + unit tests**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs`:

```rust
//! Internal stub toolkit node used by runtime tests.
//!
//! Not registered in the default `HashMapNodeRegistry`. Construct directly in
//! tests.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::toolkit_node::{SubToolDefinition, ToolkitNode, SUB_TOOL_INPUT_KEY};
use crate::llm::domain::ParameterProperty;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

pub struct EchoToolkitNode;

#[async_trait::async_trait]
impl ExecutableNode for EchoToolkitNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub_tool = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("missing __sub_tool")?;
        match sub_tool {
            "echo" => {
                let msg = inputs
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(json!({ "output": msg }))
            }
            "double" => {
                let n = inputs
                    .get("n")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                Ok(json!({ "output": n * 2.0 }))
            }
            other => Err(format!("unknown sub_tool: {other}").into()),
        }
    }

    fn schema(&self) -> Value {
        json!({ "inputs": {}, "outputs": { "output": "any" } })
    }

    fn description(&self) -> Option<&str> {
        Some("Echo toolkit stub — internal test use only.")
    }
}

impl ToolkitNode for EchoToolkitNode {
    fn sub_tool_catalog(&self, _config: &Value) -> Vec<SubToolDefinition> {
        let mut echo_props = HashMap::new();
        echo_props.insert(
            "message".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "Text to echo back".to_string(),
                enum_values: None,
                pattern: None,
            },
        );

        let mut double_props = HashMap::new();
        double_props.insert(
            "n".to_string(),
            ParameterProperty {
                property_type: "number".to_string(),
                description: "Number to double".to_string(),
                enum_values: None,
                pattern: None,
            },
        );

        vec![
            SubToolDefinition {
                name: "echo",
                description: "Return the input string unchanged.".to_string(),
                properties: echo_props,
                required: vec!["message".to_string()],
            },
            SubToolDefinition {
                name: "double",
                description: "Return twice the input number.".to_string(),
                properties: double_props,
                required: vec!["n".to_string()],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatches_on_sub_tool_echo() {
        let node = EchoToolkitNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("echo"));
        inputs.insert("message".into(), json!("hi"));
        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        assert_eq!(out.get("output").unwrap().as_str(), Some("hi"));
    }

    #[tokio::test]
    async fn dispatches_on_sub_tool_double() {
        let node = EchoToolkitNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("double"));
        inputs.insert("n".into(), json!(4));
        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        assert_eq!(out.get("output").unwrap().as_f64(), Some(8.0));
    }

    #[tokio::test]
    async fn catalog_has_two_entries() {
        let node = EchoToolkitNode;
        let cat = node.sub_tool_catalog(&json!({}));
        assert_eq!(cat.len(), 2);
        assert!(cat.iter().any(|d| d.name == "echo"));
        assert!(cat.iter().any(|d| d.name == "double"));
    }
}
```

- [ ] **Step 2: Register as a module (but NOT in the default registry)**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`. Add:

```rust
pub mod echo_toolkit;
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib echo_toolkit`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "$(cat <<'EOF'
feat(nodes): add EchoToolkitNode stub for runtime tests

Two-sub-tool (echo, double) stub used only in the dag_tool_executor
integration tests that come next. Not registered in the default
HashMapNodeRegistry.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `DagToolExecutor` — generate tool definitions for toolkit entries

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Write the failing test**

Append a new test module at the end of `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` (or extend the existing one if present):

```rust
#[cfg(test)]
mod toolkit_runtime_tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::domain::toolkit_node::ToolkitNode;
    use crate::dag_engine::infrastructure::nodes::echo_toolkit::EchoToolkitNode;
    use crate::dag_engine::domain::tool_configuration::{SubToolFilter, ToolConfiguration};
    use crate::llm::domain::ToolExecutor;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;

    /// Test registry that returns the same `Arc<EchoToolkitNode>` for both
    /// `get_node()` and `get_toolkit_node()`.
    struct EchoRegistry {
        node: Arc<EchoToolkitNode>,
    }

    impl crate::dag_engine::application::ports::NodeRegistryPort for EchoRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            if node_type == "echo_toolkit" {
                Some(self.node.clone() as Arc<dyn ExecutableNode>)
            } else {
                None
            }
        }

        fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>> {
            let mut m = std::collections::HashMap::new();
            m.insert("echo_toolkit".to_string(), self.node.clone() as Arc<dyn ExecutableNode>);
            m
        }

        fn get_toolkit_node(&self, node_type: &str) -> Option<Arc<dyn ToolkitNode>> {
            if node_type == "echo_toolkit" {
                Some(self.node.clone() as Arc<dyn ToolkitNode>)
            } else {
                None
            }
        }
    }

    fn build_executor_with_toolkit_all() -> DagToolExecutor {
        let registry = Arc::new(EchoRegistry {
            node: Arc::new(EchoToolkitNode),
        });
        let mut configs = HashMap::new();
        configs.insert(
            "web".to_string(),
            ToolConfiguration {
                name: "web".to_string(),
                description: "echo toolkit".to_string(),
                node_type: "echo_toolkit".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: Some(json!({})),
                expose_sub_tools: Some(SubToolFilter::all()),
            },
        );
        DagToolExecutor::new(registry, configs)
    }

    #[tokio::test]
    async fn toolkit_expands_to_one_tooldef_per_sub_tool() {
        let exec = build_executor_with_toolkit_all();
        let tools = exec.available_tools().await;
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        // Prefixed by alias "web__"
        assert!(names.contains(&"web__echo".to_string()));
        assert!(names.contains(&"web__double".to_string()));
    }

    #[tokio::test]
    async fn toolkit_filter_list_only_exposes_listed_sub_tools() {
        let registry = Arc::new(EchoRegistry {
            node: Arc::new(EchoToolkitNode),
        });
        let mut configs = HashMap::new();
        configs.insert(
            "web".to_string(),
            ToolConfiguration {
                name: "web".to_string(),
                description: "".to_string(),
                node_type: "echo_toolkit".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: Some(SubToolFilter::List(vec!["echo".to_string()])),
            },
        );
        let exec = DagToolExecutor::new(registry, configs);
        let tools = exec.available_tools().await;
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"web__echo".to_string()));
        assert!(!names.contains(&"web__double".to_string()));
    }
}
```

- [ ] **Step 2: Run — expect FAIL (toolkit expansion not wired yet)**

Run: `cargo test --lib dag_tool_executor::toolkit_runtime_tests::toolkit_expands_to_one_tooldef_per_sub_tool`
Expected: FAIL — only one `web` tool (using the fallback `node.schema()` path), not two prefixed sub-tool entries.

- [ ] **Step 3: Wire toolkit expansion into `available_tools`**

Edit `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`. Find `async fn available_tools`. Before the `for (name, config) in &self.tool_configurations` loop, replace that loop's body with a conditional that expands toolkit entries.

Locate the block:

```rust
        // 1. Add configured tools first
        for (name, config) in &self.tool_configurations {
            if let Some(node) = self.registry.get_node(&config.node_type) {
                tools.push(self.generate_tool_definition(name, config, &node));
            }
        }
```

Replace with:

```rust
        // 1. Add configured tools first
        for (name, config) in &self.tool_configurations {
            if config.is_toolkit() {
                // Toolkit: expand one ToolDefinition per declared sub-tool.
                let Some(toolkit) = self.registry.get_toolkit_node(&config.node_type) else {
                    colmena_log!(
                        "WARN: toolkit config '{}' references unknown toolkit node_type '{}'",
                        name,
                        config.node_type
                    );
                    continue;
                };
                let node_cfg = config.node_config.clone().unwrap_or(Value::Object(Default::default()));
                let catalog = toolkit.sub_tool_catalog(&node_cfg);
                let filter = config.expose_sub_tools.as_ref().expect("is_toolkit → filter present");
                for sub in catalog {
                    if !filter.includes(sub.name) {
                        continue;
                    }
                    tools.push(crate::llm::domain::ToolDefinition {
                        name: format!("{}__{}", name, sub.name),
                        description: sub.description,
                        parameters: crate::llm::domain::ToolParameters {
                            schema_type: "object".to_string(),
                            properties: sub.properties,
                            required: sub.required,
                        },
                    });
                }
            } else if let Some(node) = self.registry.get_node(&config.node_type) {
                tools.push(self.generate_tool_definition(name, config, &node));
            }
        }
```

- [ ] **Step 4: Run — expect PASS on the first test; the second test (FAIL) is also expected now to PASS since we implemented filtering**

Run: `cargo test --lib dag_tool_executor::toolkit_runtime_tests`
Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "$(cat <<'EOF'
feat(dag_tool_executor): expand toolkit entries into N ToolDefinitions

available_tools() now branches on ToolConfiguration::is_toolkit(). For
toolkit entries, it calls the registered ToolkitNode's sub_tool_catalog()
and filters by expose_sub_tools, producing one ToolDefinition per exposed
sub-tool with an alias-prefixed name (e.g. web__search).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: `DagToolExecutor` — dispatch toolkit calls with `__sub_tool` injection

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Write the failing test**

Append inside `mod toolkit_runtime_tests`:

```rust
    #[tokio::test]
    async fn toolkit_dispatch_echo_returns_message() {
        use crate::llm::domain::{FunctionCall, ToolCall};

        let exec = build_executor_with_toolkit_all();

        let call = ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "web__echo".to_string(),
                arguments: r#"{"message":"hola"}"#.to_string(),
            },
        };
        let result = exec.execute(&call).await.expect("execute ok");
        assert!(result.success, "got error: {:?}", result.error);
        // Output is a JSON-stringified value.
        let parsed: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed.get("output").unwrap().as_str(), Some("hola"));
    }

    #[tokio::test]
    async fn toolkit_dispatch_unknown_sub_tool_errors_cleanly() {
        use crate::llm::domain::{FunctionCall, ToolCall};

        let exec = build_executor_with_toolkit_all();

        let call = ToolCall {
            id: "call-2".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "web__does_not_exist".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let result = exec.execute(&call).await.expect("execute returns ToolResult");
        assert!(!result.success);
        assert!(result
            .output
            .to_lowercase()
            .contains("unknown sub-tool"));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib dag_tool_executor::toolkit_runtime_tests::toolkit_dispatch_echo_returns_message`
Expected: FAIL — `web__echo` is not resolved (tool_call.function.name is not found in `tool_configurations` because the map key is `web`, not `web__echo`).

- [ ] **Step 3: Implement dispatch**

Edit the top of `async fn execute(&self, tool_call: &ToolCall)` in `dag_tool_executor.rs`. Immediately after the `load_skill` branch and before the existing `let node_type = &tool_call.function.name;` line, insert toolkit-dispatch logic. The full replacement (from the `load_skill` block through the execution step) is:

```rust
        // --- Toolkit dispatch: names of the form "<alias>__<sub_tool>" ---
        if let Some((alias, sub_tool)) = tool_call.function.name.split_once("__") {
            if let Some(cfg) = self.tool_configurations.get(alias) {
                if cfg.is_toolkit() {
                    return self.execute_toolkit(alias, sub_tool, cfg, tool_call).await;
                }
            }
        }
```

Place that block immediately after the `load_skill` early-return and before `let node_type = &tool_call.function.name;`.

Now add the helper `execute_toolkit` as a new method on `DagToolExecutor`. Add a new `impl DagToolExecutor` block (or append inside the existing one) near the bottom of the file, above `impl ToolExecutor for DagToolExecutor`:

```rust
impl DagToolExecutor {
    async fn execute_toolkit(
        &self,
        alias: &str,
        sub_tool: &str,
        cfg: &ToolConfiguration,
        tool_call: &crate::llm::domain::ToolCall,
    ) -> Result<crate::llm::domain::ToolResult, crate::llm::domain::LlmError> {
        use crate::dag_engine::domain::toolkit_node::SUB_TOOL_INPUT_KEY;
        use crate::llm::domain::{LlmError, ToolResult};

        // Resolve the toolkit node.
        let toolkit = self.registry.get_toolkit_node(&cfg.node_type).ok_or_else(|| {
            LlmError::ToolNotFound {
                name: cfg.node_type.clone(),
            }
        })?;

        // Confirm this sub-tool is actually in the filter / catalogue.
        let node_cfg = cfg.node_config.clone().unwrap_or(Value::Object(Default::default()));
        let catalog = toolkit.sub_tool_catalog(&node_cfg);
        let known = catalog.iter().any(|d| d.name == sub_tool);
        let exposed = cfg
            .expose_sub_tools
            .as_ref()
            .map(|f| f.includes(sub_tool))
            .unwrap_or(false);
        if !known || !exposed {
            return Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: format!(
                    "unknown sub-tool '{}' for toolkit '{}'",
                    sub_tool, alias
                ),
                error: Some("unknown sub-tool".to_string()),
            });
        }

        // Parse LLM arguments.
        let mut inputs: HashMap<String, Value> = serde_json::from_str(&tool_call.function.arguments)
            .map_err(|e| LlmError::InvalidToolCall {
                reason: format!("Failed to parse arguments for {}: {}", tool_call.function.name, e),
            })?;

        // Inject the reserved sub-tool discriminator.
        inputs.insert(SUB_TOOL_INPUT_KEY.to_string(), Value::String(sub_tool.to_string()));

        // Execute the underlying toolkit node as a plain ExecutableNode.
        // node_exec_config is the per-toolkit static node_config from the entry
        // (e.g. { "api_key": "..." }).
        let exec_node = self
            .registry
            .get_node(&cfg.node_type)
            .ok_or_else(|| LlmError::ToolNotFound { name: cfg.node_type.clone() })?;

        let mut state = serde_json::json!({});
        let result = exec_node.execute(&inputs, &node_cfg, &mut state, None).await;

        match result {
            Ok(value) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: true,
                output: value.to_string(),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: format!("Error executing toolkit {}__{}: {}", alias, sub_tool, e),
                error: Some(e.to_string()),
            }),
        }
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib dag_tool_executor::toolkit_runtime_tests`
Expected: all four tests pass (the two from Task 10 + two new).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "$(cat <<'EOF'
feat(dag_tool_executor): dispatch toolkit sub-tool calls via __sub_tool

When the LLM invokes a tool whose name matches the "alias__sub_tool"
shape AND the alias resolves to a toolkit ToolConfiguration, the
executor injects __sub_tool into inputs, passes node_config as the
per-node exec config, and delegates to the ExecutableNode path.
Non-toolkit entries are unaffected.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Conversation-close hook for registry cleanup

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`
- Create: `src/libs/colmena/src/web/domain/lifecycle.rs`
- Modify: `src/libs/colmena/src/web/domain/mod.rs`

The spec asks for an engine hook that fires when a conversation closes so registries can eagerly clean up. Rather than touching the engine for every new registry, we expose a lightweight subscriber pattern.

- [ ] **Step 1: Create the subscriber trait**

Create `src/libs/colmena/src/web/domain/lifecycle.rs`:

```rust
//! Pluggable conversation-lifecycle hooks used by session-bearing web nodes.
//!
//! The DAG engine currently keys runs by a `dag_run_id`, but conversations
//! (external sessions) can span multiple runs. When a conversation concludes
//! we want each `SessionRegistry` to drop entries scoped to it. Registrars
//! implement `ConversationLifecycleSubscriber` and are invoked by the engine
//! on conversation close.

use std::sync::Arc;

#[async_trait::async_trait]
pub trait ConversationLifecycleSubscriber: Send + Sync {
    async fn on_conversation_closed(&self, conversation_id: &str);
}

/// Fan-out bus: multiple registries can subscribe.
#[derive(Default, Clone)]
pub struct ConversationLifecycleBus {
    subs: Arc<tokio::sync::Mutex<Vec<Arc<dyn ConversationLifecycleSubscriber>>>>,
}

impl ConversationLifecycleBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn subscribe(&self, s: Arc<dyn ConversationLifecycleSubscriber>) {
        self.subs.lock().await.push(s);
    }

    pub async fn notify_conversation_closed(&self, conversation_id: &str) {
        let subs = self.subs.lock().await.clone();
        for s in subs {
            s.on_conversation_closed(conversation_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter {
        n: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ConversationLifecycleSubscriber for Counter {
        async fn on_conversation_closed(&self, _conversation_id: &str) {
            self.n.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn fanout_invokes_every_subscriber() {
        let bus = ConversationLifecycleBus::new();
        let n = Arc::new(AtomicUsize::new(0));
        bus.subscribe(Arc::new(Counter { n: n.clone() })).await;
        bus.subscribe(Arc::new(Counter { n: n.clone() })).await;
        bus.notify_conversation_closed("conv-1").await;
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/web/domain/mod.rs`. Append:

```rust
pub mod lifecycle;
pub use lifecycle::{ConversationLifecycleBus, ConversationLifecycleSubscriber};
```

- [ ] **Step 3: Wire a single bus into the DAG engine run orchestrator**

Edit `src/libs/colmena/src/dag_engine/application/run_use_case.rs`. Find the struct that owns the run orchestration (grep: `pub struct DagRun` or `pub struct RunUseCase` — whichever hosts the run lifecycle). Add a new field and setter (without coupling the engine to any specific registry):

```rust
    /// Optional bus notified when a conversation is considered finished.
    /// Registries (web nodes) subscribe to eagerly evict scoped state.
    pub conversation_lifecycle: Option<crate::web::domain::ConversationLifecycleBus>,
```

Provide a constructor / setter and call `conversation_lifecycle.notify_conversation_closed(conversation_id)` when the run finishes (non-suspended) in whatever path the use case already has for "run complete, no follow-up".

> **If you are uncertain where the boundary is**, use this heuristic: wire the notification into the code path that runs when the HTTP session / conversation receives a "final" terminating signal — in the current engine, that is the same place where persistence of the final run state occurs. A one-line `if let Some(bus) = &self.conversation_lifecycle { bus.notify_conversation_closed(&conversation_id).await; }` is sufficient. If there is no existing hook, add it where `run.complete` or equivalent is called.

If this is too invasive for the current task, add the field and setter but leave the notification for Plan A's session-lifetime integration task — this is acceptable because Plan A's tavily_client is stateless (no sessions needed); Plan C's first use of sessions comes with its own integration task that will cover notification too.

- [ ] **Step 4: Run the full build**

Run: `cargo check --lib 2>&1 | tail -20`
Expected: no errors.

Run: `cargo test --lib web::domain::lifecycle`
Expected: 1 test passes.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/domain src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "$(cat <<'EOF'
feat(web): add ConversationLifecycleBus and subscriber trait

Fan-out bus that session-bearing web registries subscribe to. The DAG
engine's run orchestrator gains an optional field and calls
notify_conversation_closed when a conversation concludes. Plans C and
B wire their registries to this bus as a subscriber; plan A is stateless
and does not subscribe.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Developer docs stub for web nodes

**Files:**
- Create: `docs/developer_guide/25_web_nodes.md`
- Modify: `docs/DEVELOPER_GUIDE.md`

Plans A, C, B will each populate a subsection. This task seeds the skeleton.

- [ ] **Step 1: Create the file**

Create `docs/developer_guide/25_web_nodes.md`:

```markdown
# Web Nodes (tavily_client, api_explorer, browser)

Three toolkit nodes that give LLM agents internet capabilities:

- **tavily_client** — web search + URL fetch (Tavily API). See [Spec A](../superpowers/specs/2026-04-23-web-nodes-a-tavily-client-design.md).
- **api_explorer** — OpenAPI/Swagger discovery and http_request builder. See [Spec C](../superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md).
- **browser** — self-hosted headless browser (Browserless + chromiumoxide). See [Spec B](../superpowers/specs/2026-04-23-web-nodes-b-browser-design.md).

## Shared runtime: toolkit nodes

A *toolkit node* is an `ExecutableNode` that also implements `ToolkitNode` and exposes
multiple *sub-tools* to the LLM via a single node instance.

Declaring one in an `llm_call`:

\`\`\`json
"tool_configurations": {
  "web": {
    "node_type": "tavily_client",
    "node_config": { "api_key": "${TAVILY_API_KEY}" },
    "expose_sub_tools": "all"
  }
}
\`\`\`

- `node_type` must point to a registered toolkit node.
- `node_config` is the static per-instance configuration handed to the node at execution time.
- `expose_sub_tools` is either the string `"all"` or an array of sub-tool names.

Runtime flow:

1. The engine calls `ToolkitNode.sub_tool_catalog(&node_config)` to get the list of sub-tools.
2. Filtered by `expose_sub_tools`, each sub-tool becomes one `ToolDefinition` named `"{alias}__{sub_tool}"` (e.g. `web__search`).
3. When the LLM invokes one, `DagToolExecutor` injects `__sub_tool` into the node inputs, passes `node_config` as the node execution config, and calls `execute()` once.

## Sub-sections

- [tavily_client](#tavily_client) — populated by Spec A.
- [api_explorer](#api_explorer) — populated by Spec C.
- [browser](#browser) — populated by Spec B.
```

- [ ] **Step 2: Add to the developer-guide index**

Edit `docs/DEVELOPER_GUIDE.md`. Find the list of guide sections (grep for a line containing `24_skills.md` if present, or the last numbered guide entry). Insert:

```markdown
- [25. Web Nodes](developer_guide/25_web_nodes.md) — `tavily_client`, `api_explorer`, `browser`
```

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/25_web_nodes.md docs/DEVELOPER_GUIDE.md
git commit -m "$(cat <<'EOF'
docs(web): seed developer guide section 25 for web toolkit nodes

Stub introducing the toolkit runtime for upcoming tavily_client,
api_explorer, and browser nodes. Plans A/C/B populate the node-specific
subsections.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Final verification

- [ ] **Step 1: Full build + test**

Run: `cargo check --lib 2>&1 | tail -5`
Expected: clean.

Run: `cargo test --lib web tool_configuration toolkit_node echo_toolkit dag_tool_executor 2>&1 | tail -20`
Expected: all new tests pass (15 new tests across the plan).

- [ ] **Step 2: Final smoke-check commit (no code change needed if everything is green)**

This task introduces no new artifact. If you made any cleanup edits, commit them:

```bash
git status
# If there are uncommitted changes, commit them with an appropriate message.
# Otherwise, skip to the next plan.
```

---

## Plan summary

By the end of this plan:
- `src/libs/colmena/src/web/` module exists with `domain/{errors,session,lifecycle}.rs`.
- `ToolkitNode` trait and `SubToolDefinition` live in `dag_engine/domain/toolkit_node.rs`.
- `ToolConfiguration` carries `node_config` + `expose_sub_tools`.
- `NodeRegistryPort::get_toolkit_node()` exists; `HashMapNodeRegistry` stores toolkit nodes in a parallel map.
- `DagToolExecutor` expands toolkit configs into N `ToolDefinition`s and dispatches `{alias}__{sub_tool}` calls via `__sub_tool` injection.
- `EchoToolkitNode` stub verifies the runtime end-to-end.
- `ConversationLifecycleBus` + subscriber trait exist for session cleanup hooks.
- Developer guide section 25 skeleton exists.

No user-facing node ships in this plan. The three node plans (A, C, B) each layer on this foundation.
