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
