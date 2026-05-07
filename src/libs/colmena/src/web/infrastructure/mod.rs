//! Adapters implementing the web-toolkit ports.

pub mod openapi_adapter;
pub mod tavily_adapter;

pub use openapi_adapter::{OpenApiAdapter, OpenApiAdapterConfig};
pub use tavily_adapter::TavilyAdapter;
