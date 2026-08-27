//! `rmcp`-backed adapter for [`crate::llm::domain::mcp::McpClientPort`].
//!
//! This is the ONLY module in the crate allowed to name an `rmcp` type
//! (design §1, ADR-1) — every other layer sees only the domain port and its
//! value objects.

pub mod rmcp_http_client;

pub use rmcp_http_client::RmcpHttpClient;
