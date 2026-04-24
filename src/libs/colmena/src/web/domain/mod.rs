//! Ports and value objects for the `web` toolkit nodes.

pub mod errors;
pub mod lifecycle;
pub mod session;

pub use errors::WebDomainError;
pub use lifecycle::{ConversationLifecycleBus, ConversationLifecycleSubscriber};
pub use session::{ConversationId, SessionEntry, SessionKey, SessionRegistry, TtlConfig};
