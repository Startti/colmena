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
