//! Domain port and value objects for remote MCP (Model Context Protocol)
//! servers exposed as `llm_call` tools.
//!
//! **Hard architecture rule (CLAUDE.md): this module has ZERO infrastructure
//! dependencies.** Imports are limited to `std`, `serde`, `serde_json`,
//! `thiserror`, and `async_trait`. No `reqwest`, no `rmcp`, no HTTP types.
//! The `rmcp`-backed adapter that implements [`McpClientPort`] lives in
//! `llm/infrastructure/mcp_client/` (a later slice).

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants (design §4) — single source of truth; tests assert against these
// symbols rather than duplicating magic numbers.
// ---------------------------------------------------------------------------

/// Cap for a top-level tool description forwarded to the model (bytes).
pub const MCP_MAX_DESCRIPTION_BYTES: usize = 4 * 1024;

/// Cap for the short lazy-loading catalog summary (bytes), matching the
/// documented `ToolDefinition::summary` contract.
pub const MCP_MAX_SUMMARY_BYTES: usize = 200;

/// Aggregate byte-size ceiling for a tool's forwarded JSON Schema. Schemas
/// over this ceiling are excluded from exposure (never truncated mid-JSON).
pub const MCP_MAX_SCHEMA_BYTES: usize = 32 * 1024;

/// Cap for server-authored error text (bytes) — see design §2b: this text
/// bypasses the outer result scrubber via `ToolResult::failure`, so the MCP
/// dispatcher must cap and delimit it itself.
pub const MCP_MAX_ERROR_BYTES: usize = 4 * 1024;

/// Maximum number of tools accepted from a single MCP server per exposure
/// pass (also bounds an adversarial server's ability to flood context).
pub const MCP_MAX_TOOLS_PER_SERVER: usize = 64;

/// Maximum length, in characters, of an exposed `<alias>__<tool>` tool name
/// — matches the tightest LLM-provider name constraint.
pub const MCP_MAX_EXPOSED_NAME_LEN: usize = 64;

/// Number of hex characters kept from the `sha256` digest when a name must
/// be truncated deterministically (design §4a).
pub const MCP_NAME_HASH_LEN: usize = 8;

// ---------------------------------------------------------------------------
// Port
// ---------------------------------------------------------------------------

/// Port for a live connection to a single remote MCP server.
///
/// `Send + Sync` is mandatory: every existing port in this crate is
/// (`ToolExecutor: Send + Sync`, [`crate::llm::domain::tool_executor`]), and
/// the connection is shared across concurrent `llm_call` executions via a
/// process-level registry (design §3, a later slice).
#[async_trait]
pub trait McpClientPort: Send + Sync {
    /// Server's `tools/list`, verbatim. Never filtered, capped, or renamed
    /// at this layer — containment happens at the exposure stage.
    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError>;

    /// Server's `tools/call`. `arguments` is the model's raw JSON object,
    /// forwarded unchanged.
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResult, McpError>;

    /// Stable human label for warnings/delimiters — the operator-chosen
    /// `tool_configurations` alias, never the server URL.
    fn server_label(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Value objects (immutable data — R1.4)
// ---------------------------------------------------------------------------

/// A single tool as reported by an MCP server's `tools/list`.
///
/// `input_schema` is forwarded verbatim (design R4.4) — never flattened,
/// downgraded, or lossily mapped. `outputSchema` is deliberately not
/// represented here (ADR-3): `ToolDefinition` has no output-schema slot and
/// no provider adapter forwards one.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDescriptor {
    /// Server's tool name, verbatim (may contain hyphens, e.g.
    /// `resolve-library-id`).
    pub name: String,
    pub title: Option<String>,
    /// Third-party authored, UNCAPPED at this layer.
    pub description: String,
    /// Verbatim JSON Schema for the tool's input arguments.
    pub input_schema: Value,
}

/// Result of a `tools/call` invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolResult {
    /// Content blocks already folded into a single string (text blocks
    /// preserved losslessly — R2.6). Uncapped and undelimited at this
    /// layer; containment is applied by the dispatcher (design §4b).
    pub content: String,
    /// Mirrors the JSON-RPC `isError` flag on the `tools/call` response.
    pub is_error: bool,
}

/// Transport used to reach an MCP server. Only remote transports are
/// supported — no stdio/process-spawn code path exists anywhere in this
/// crate (spec R2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum McpTransport {
    #[default]
    StreamableHttp,
    Sse,
}

/// Configuration for a single MCP server connection.
///
/// `header_refs` holds UNRESOLVED secure-value/`$DYNAMIC` references, never
/// resolved secret values (design §7, spec R3.6/G3) — the resolved values
/// are read at connect time and never stored on this struct's cache-key
/// representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpServerConfig {
    pub url: String,
    pub transport: McpTransport,
    pub header_refs: BTreeMap<String, String>,
    pub timeout: Duration,
    pub cache_ttl: Duration,
}

// ---------------------------------------------------------------------------
// Errors (R1.3) — one variant per failure class, each carrying enough
// context to build either an operator-facing message or a model-correctable
// tool error downstream (design §2a). Deliberately no catch-all `Other`.
// ---------------------------------------------------------------------------

#[derive(Debug, Error, Clone, PartialEq)]
pub enum McpError {
    #[error("MCP server '{server}' transport error: {reason}")]
    Transport { server: String, reason: String },

    #[error("MCP server '{server}' timed out after {seconds}s")]
    Timeout { server: String, seconds: u64 },

    #[error("MCP server '{server}' handshake failed: {detail}")]
    Handshake { server: String, detail: String },

    #[error("MCP server '{server}' protocol error: {detail}")]
    Protocol { server: String, detail: String },

    #[error("MCP server '{server}' has no tool named '{tool}'")]
    ToolNotFound { server: String, tool: String },

    #[error("MCP tool '{tool}' on server '{server}' failed: {message}")]
    ToolCallFailed {
        server: String,
        tool: String,
        message: String,
    },

    #[error("MCP tool '{tool}' schema is {bytes} bytes, exceeding the {limit}-byte limit")]
    SchemaTooLarge {
        tool: String,
        bytes: usize,
        limit: usize,
    },

    #[error("Invalid MCP server config: {detail}")]
    InvalidConfig { detail: String },
}

#[cfg(test)]
mod send_sync_tests {
    use super::McpClientPort;

    /// R1.1 — compile-time proof that any implementer of `McpClientPort`
    /// satisfies `Send + Sync`. This function never runs; it merely needs
    /// to compile, which is only possible if the trait bound holds.
    #[allow(dead_code)]
    fn mcp_client_port_is_send_sync<T: McpClientPort>() {
        fn assert_send_sync<U: Send + Sync>() {}
        assert_send_sync::<T>();
    }
}

#[cfg(test)]
mod error_variant_tests {
    //! R1.3 — one test per variant proves each is independently
    //! pattern-matchable (not folded into a catch-all `Other`) and carries
    //! the context described in design §2a. `let ... else` destructuring
    //! fails to compile/panics on a variant mismatch, so a passing test is
    //! proof the match succeeded on the exact variant constructed.
    use super::McpError;

    #[test]
    fn transport_carries_server_and_reason() {
        let McpError::Transport { server, reason } = (McpError::Transport {
            server: "s".into(),
            reason: "reset".into(),
        }) else {
            unreachable!()
        };
        assert_eq!((server, reason), ("s".into(), "reset".into()));
    }

    #[test]
    fn timeout_carries_server_and_seconds() {
        let McpError::Timeout { server, seconds } = (McpError::Timeout {
            server: "s".into(),
            seconds: 30,
        }) else {
            unreachable!()
        };
        assert_eq!((server.as_str(), seconds), ("s", 30));
    }

    #[test]
    fn handshake_carries_server_and_detail() {
        let McpError::Handshake { server, detail } = (McpError::Handshake {
            server: "s".into(),
            detail: "bad initialize".into(),
        }) else {
            unreachable!()
        };
        assert_eq!((server.as_str(), detail.as_str()), ("s", "bad initialize"));
    }

    #[test]
    fn protocol_carries_server_and_detail() {
        let McpError::Protocol { server, detail } = (McpError::Protocol {
            server: "s".into(),
            detail: "malformed json-rpc".into(),
        }) else {
            unreachable!()
        };
        assert_eq!(
            (server.as_str(), detail.as_str()),
            ("s", "malformed json-rpc")
        );
    }

    #[test]
    fn tool_not_found_carries_server_and_tool() {
        let McpError::ToolNotFound { server, tool } = (McpError::ToolNotFound {
            server: "s".into(),
            tool: "t".into(),
        }) else {
            unreachable!()
        };
        assert_eq!((server.as_str(), tool.as_str()), ("s", "t"));
    }

    #[test]
    fn tool_call_failed_carries_server_tool_and_message() {
        let McpError::ToolCallFailed {
            server,
            tool,
            message,
        } = (McpError::ToolCallFailed {
            server: "s".into(),
            tool: "t".into(),
            message: "bad args".into(),
        })
        else {
            unreachable!()
        };
        assert_eq!(
            (server.as_str(), tool.as_str(), message.as_str()),
            ("s", "t", "bad args")
        );
    }

    #[test]
    fn schema_too_large_carries_tool_bytes_and_limit() {
        let McpError::SchemaTooLarge { tool, bytes, limit } = (McpError::SchemaTooLarge {
            tool: "t".into(),
            bytes: 40_000,
            limit: super::MCP_MAX_SCHEMA_BYTES,
        }) else {
            unreachable!()
        };
        assert_eq!(
            (tool.as_str(), bytes, limit),
            ("t", 40_000, super::MCP_MAX_SCHEMA_BYTES)
        );
    }

    #[test]
    fn invalid_config_carries_detail() {
        let McpError::InvalidConfig { detail } = (McpError::InvalidConfig {
            detail: "missing url".into(),
        }) else {
            unreachable!()
        };
        assert_eq!(detail, "missing url");
    }
}
