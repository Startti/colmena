//! Pluggable conversation-lifecycle hooks used by session-bearing web nodes.
//!
//! The DAG engine currently keys runs by a `dag_run_id`, but conversations
//! (external sessions) can span multiple runs. When a conversation concludes
//! we want each `SessionRegistry` to drop entries scoped to it. Registrars
//! implement `ConversationLifecycleSubscriber` and are invoked by the engine
//! on conversation close.

use std::sync::Arc;

/// Callback invoked on the "conversation closed" edge.
///
/// Implementors typically hold a `SessionRegistry` and call
/// `cleanup_conversation(conversation_id, ...)` to evict scoped state.
#[async_trait::async_trait]
pub trait ConversationLifecycleSubscriber: Send + Sync {
    /// Called once per subscriber when the engine declares a conversation closed.
    async fn on_conversation_closed(&self, conversation_id: &str);
}

/// Fan-out bus: multiple registries can subscribe.
#[derive(Default, Clone)]
pub struct ConversationLifecycleBus {
    subs: Arc<tokio::sync::Mutex<Vec<Arc<dyn ConversationLifecycleSubscriber>>>>,
}

impl ConversationLifecycleBus {
    /// Constructs an empty bus with no subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a subscriber. Subscribers added during or after a
    /// `notify_conversation_closed` call do not receive that prior event.
    pub async fn subscribe(&self, s: Arc<dyn ConversationLifecycleSubscriber>) {
        self.subs.lock().await.push(s);
    }

    /// Fans out the close event to every subscriber sequentially.
    /// The internal lock is released before awaiting each subscriber so
    /// `subscribe()` does not block on slow callbacks.
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
        last_id: Arc<tokio::sync::Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ConversationLifecycleSubscriber for Counter {
        async fn on_conversation_closed(&self, conversation_id: &str) {
            self.n.fetch_add(1, Ordering::SeqCst);
            *self.last_id.lock().await = Some(conversation_id.to_string());
        }
    }

    fn counter(n: Arc<AtomicUsize>) -> Arc<Counter> {
        Arc::new(Counter {
            n,
            last_id: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    #[tokio::test]
    async fn fanout_invokes_every_subscriber() {
        let bus = ConversationLifecycleBus::new();
        let n = Arc::new(AtomicUsize::new(0));
        bus.subscribe(counter(n.clone())).await;
        bus.subscribe(counter(n.clone())).await;
        bus.notify_conversation_closed("conv-1").await;
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn notify_with_zero_subscribers_is_noop() {
        let bus = ConversationLifecycleBus::new();
        bus.notify_conversation_closed("conv-x").await;
    }

    #[tokio::test]
    async fn conversation_id_is_passed_through() {
        let bus = ConversationLifecycleBus::new();
        let sub = counter(Arc::new(AtomicUsize::new(0)));
        bus.subscribe(sub.clone()).await;
        bus.notify_conversation_closed("conv-42").await;
        assert_eq!(sub.last_id.lock().await.as_deref(), Some("conv-42"));
    }
}
