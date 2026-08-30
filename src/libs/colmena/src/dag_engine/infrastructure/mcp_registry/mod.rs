//! Connection identity for remote MCP servers.
//!
//! The process-level connection registry that consumes [`McpServerKey`] lands
//! in the next slice; this module currently carries the identity alone.

pub mod key;

pub use key::McpServerKey;
