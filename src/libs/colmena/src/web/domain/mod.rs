//! Ports and value objects for the `web` toolkit nodes.

pub mod api_spec_port;
pub mod errors;
pub mod lifecycle;
pub mod search_port;
pub mod session;

pub use api_spec_port::{
    ApiKeyLocation, ApiSpecPort, Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec,
    RequestBodySpec, ResponseSpec, SecurityRequirement, SecurityScheme, SpecFetchResult,
    SpecFormat,
};
pub use errors::WebDomainError;
pub use lifecycle::{ConversationLifecycleBus, ConversationLifecycleSubscriber};
pub use search_port::{
    ExtractFormat, FetchRequest, FetchResponse, SearchDepth, SearchPort, SearchRequest,
    SearchResponse, SearchResult, TimeRange,
};
pub use session::{ConversationId, SessionEntry, SessionKey, SessionRegistry, TtlConfig};
