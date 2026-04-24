//! Ports and value objects for the `web` toolkit nodes.

pub mod errors;
pub mod lifecycle;
pub mod search_port;
pub mod session;

pub use errors::WebDomainError;
pub use lifecycle::{ConversationLifecycleBus, ConversationLifecycleSubscriber};
pub use search_port::{
    ExtractFormat, FetchRequest, FetchResponse, SearchDepth, SearchPort, SearchRequest,
    SearchResponse, SearchResult, TimeRange,
};
pub use session::{ConversationId, SessionEntry, SessionKey, SessionRegistry, TtlConfig};
