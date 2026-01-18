use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeEvent {
    LlmToken { token: String },
}

pub trait ExecutionObserver: Send + Sync {
    fn on_event(&self, event: NodeEvent);
}
