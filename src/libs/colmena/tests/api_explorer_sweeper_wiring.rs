//! Integration coverage for the TTL sweeper wiring inside `ApiExplorerNode::new`.
//!
//! These tests pin down two invariants of the lifecycle simplification
//! (commit 8a6a17a):
//!
//! 1. Inside a tokio runtime, `ApiExplorerNode::new` spawns the passive
//!    TTL sweeper task without panicking.
//! 2. Outside a tokio runtime, `ApiExplorerNode::new` skips the sweeper
//!    via the `Handle::try_current` guard — it MUST NOT panic, otherwise
//!    sync test harnesses constructing the node would crash.
//!
//! The sweeper period itself (`Duration::from_secs(60)`) and the
//! `TtlConfig::default()` policy (15min idle / 1h max) are already
//! exercised by unit tests in `web/domain/session.rs`. Here we only
//! prove the wiring at the construction site is correct.

use colmena::dag_engine::infrastructure::nodes::api_explorer::ApiExplorerNode;

#[tokio::test]
async fn new_constructs_inside_runtime_with_default_ttl() {
    let node = ApiExplorerNode::new();
    let registry = node.registry();

    let ttl = registry.ttl();
    assert_eq!(ttl.idle_ttl_seconds, 900, "idle TTL should match default");
    assert_eq!(
        ttl.max_lifetime_seconds, 3600,
        "max lifetime should match default"
    );
    assert_eq!(
        ttl.max_active_sessions, 50,
        "max active sessions should match default"
    );

    assert_eq!(
        registry.len().await,
        0,
        "fresh registry should hold no entries"
    );
}

#[test]
fn new_does_not_panic_outside_runtime() {
    let node = ApiExplorerNode::new();
    drop(node);
}
