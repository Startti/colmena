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

use std::sync::{Arc, OnceLock};

/// The one pool for this process.
///
/// Process-level and not per-node on purpose. Colmena is embedded as a library
/// by the ADP worker and never calls `main()`, so there is no startup hook to
/// build this in; a `OnceLock` lets the first `llm_call` that needs MCP create
/// it and every later turn reuse it.
///
/// A per-node pool would defeat the whole design: the LRU bound and the catalog
/// TTL exist so an agent loop stops re-handshaking every turn, and a pool that
/// dies with the node would re-handshake on every single one.
///
/// Sized from `COLMENA_MAX_POOLED_MCP_SERVERS` when it parses as a positive
/// integer, and from [`registry::DEFAULT_MAX_POOLED_SERVERS`] otherwise —
/// unset, empty, zero and malformed all fall back rather than failing, because
/// a bad env var must not be the thing that takes MCP down.
pub fn global_mcp_registry() -> &'static Arc<McpConnectionRegistry> {
    static POOL: OnceLock<Arc<McpConnectionRegistry>> = OnceLock::new();
    POOL.get_or_init(|| {
        let raw = std::env::var("COLMENA_MAX_POOLED_MCP_SERVERS").ok();
        Arc::new(McpConnectionRegistry::with_max_entries(pool_size_from(
            raw.as_deref(),
        )))
    })
}

/// The pool size a raw env value asks for, or the default when it asks for
/// nothing usable.
///
/// Split out of the `OnceLock` closure so it can be tested at all: the closure
/// runs exactly once per process, and the test binary shares that process, so
/// every branch below would otherwise be unreachable from a test.
///
/// Absent, empty, non-numeric, negative and zero all fall back rather than
/// failing. A pool sized by a mistyped environment variable is a worse outcome
/// than one sized by the default, and a bad env var must never be the thing
/// that takes MCP down.
fn pool_size_from(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(registry::DEFAULT_MAX_POOLED_SERVERS)
}

pub use key::{CredentialFingerprint, McpServerKey};
pub use registry::{McpConnectionRegistry, DEFAULT_MAX_POOLED_SERVERS};

#[cfg(test)]
mod tests {
    use super::*;

    /// A value the operator actually meant is honoured.
    #[test]
    fn a_valid_size_is_used_as_given() {
        assert_eq!(pool_size_from(Some("64")), 64);
        assert_eq!(pool_size_from(Some("  64  ")), 64, "surrounding space");
        assert_eq!(pool_size_from(Some("1")), 1);
    }

    /// Every unusable form falls back instead of failing. Zero is included on
    /// purpose: a pool that can hold nothing is not a smaller pool, it is a
    /// broken one, and `with_max_entries` would silently clamp it to 1 anyway.
    #[test]
    fn an_unusable_value_falls_back_to_the_default() {
        for raw in [
            None,
            Some(""),
            Some("   "),
            Some("0"),
            Some("-5"),
            Some("abc"),
            Some("12x"),
        ] {
            assert_eq!(
                pool_size_from(raw),
                registry::DEFAULT_MAX_POOLED_SERVERS,
                "{raw:?} should have fallen back"
            );
        }
    }

    /// A value too large for `usize` is malformed, not huge — it must fall back
    /// rather than saturate to something the operator never asked for.
    #[test]
    fn an_oversized_value_falls_back() {
        let too_big = "9".repeat(40);
        assert_eq!(
            pool_size_from(Some(&too_big)),
            registry::DEFAULT_MAX_POOLED_SERVERS
        );
    }

    /// The whole point of the singleton: two turns must share one pool. If this
    /// ever returns distinct instances, the LRU bound and the catalog TTL stop
    /// meaning anything because every turn starts from an empty pool.
    /// Same-thread sharing is not the claim that matters: `llm_call` runs under
    /// a multi-threaded tokio runtime, so turns land on different threads. A
    /// `thread_local!` implementation would satisfy `every_turn_shares_one_pool`
    /// and still hand each worker thread its own pool — every turn starting cold
    /// while the code looks correct. This is the test that kills it.
    #[test]
    fn threads_share_the_same_pool() {
        let addr = |r: &Arc<McpConnectionRegistry>| Arc::as_ptr(r) as usize;
        let mine = addr(global_mcp_registry());

        let theirs: Vec<usize> = (0..4)
            .map(|_| std::thread::spawn(move || addr(global_mcp_registry())))
            .map(|h| h.join().expect("thread panicked"))
            .collect();

        assert!(
            theirs.iter().all(|t| *t == mine),
            "a worker thread got its own pool: {theirs:?} vs {mine}"
        );
    }

    #[test]
    fn every_turn_shares_one_pool() {
        assert!(
            Arc::ptr_eq(global_mcp_registry(), global_mcp_registry()),
            "a second call built a second pool"
        );
    }
}
