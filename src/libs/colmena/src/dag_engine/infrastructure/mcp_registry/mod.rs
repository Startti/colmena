//! Connection identity and pooling for remote MCP servers.
//!
//! [`McpServerKey`] is the identity: two configurations that resolve to the
//! same endpoint under the same credential references are the same server.
//! [`McpConnectionRegistry`] is the pool that hands one live client back for
//! that identity, so an agent loop does not re-handshake every turn.

pub mod key;
pub mod registry;

pub use key::{CredentialScope, McpServerKey};
pub use registry::{McpConnectionRegistry, McpConnector};
