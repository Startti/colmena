//! Adapters implementing the web-toolkit ports.

pub mod tavily_adapter;
pub mod openapi_adapter;

pub use tavily_adapter::TavilyAdapter;
pub use openapi_adapter::{OpenApiAdapter, OpenApiAdapterConfig};
