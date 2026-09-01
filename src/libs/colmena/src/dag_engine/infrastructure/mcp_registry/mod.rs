//! Connection identity and pooling for remote MCP servers.
//!
//! [`McpServerKey`] is the identity: two configurations that reach the same
//! endpoint carrying the same RESOLVED credentials are the same server. The
//! identity follows the credential values, not the references naming them, so
//! rotating a secret yields a new connection instead of silently reusing one
//! built with the retired value.
//! [`McpConnectionRegistry`] is the pool that hands one live client back for
//! that identity, so an agent loop does not re-handshake every turn.

pub mod key;
pub mod registry;

pub use key::{CredentialFingerprint, McpServerKey};
pub use registry::McpConnectionRegistry;
