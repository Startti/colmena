//! Exposing a remote MCP server's tools inside an `llm_call`.
//!
//! Split so the two halves stay reviewable on their own: [`expose`] turns a
//! server's catalog into `ToolDefinition`s the provider can see, and dispatch
//! (a later slice) routes a call back to the server.

pub mod expose;

pub use expose::{collect_mcp_tool_configs, drop_colliding, exposed_definitions};
