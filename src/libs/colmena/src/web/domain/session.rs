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

        let got = reg.with_entry(&key, |e| *e).await;
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
}
