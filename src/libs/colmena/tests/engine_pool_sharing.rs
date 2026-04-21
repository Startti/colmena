//! Integration test: verify `ColmenaEngine` wires a single pool end-to-end
//! and shares it between state persistence + conversation factory.
//!
//! Requires `TEST_DATABASE_URL` to be set. Otherwise the test skips.

use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::infrastructure::pool_registry::PoolConfig;

fn database_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL").ok()
}

#[tokio::test]
async fn engine_boots_with_pinned_pool_and_migrates() {
    let Some(db) = database_url() else {
        eprintln!("skip: TEST_DATABASE_URL not set");
        return;
    };
    let engine = ColmenaEngine::new(EngineConfig {
        internal_database_url: db,
        pool_config: PoolConfig::defaults(),
    })
    .await
    .expect("engine boots");

    let metrics = engine.registry_metrics();
    assert_eq!(metrics.pinned_pools, 1);
    assert_eq!(metrics.cached_pools, 1);
    assert_eq!(metrics.per_url.len(), 1);
    assert!(metrics.per_url[0].pinned);

    engine.shutdown().await;
    let after = engine.registry_metrics();
    assert_eq!(after.cached_pools, 0);
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    let Some(db) = database_url() else {
        return;
    };
    let engine = ColmenaEngine::new(EngineConfig {
        internal_database_url: db,
        pool_config: PoolConfig::defaults(),
    })
    .await
    .unwrap();
    engine.shutdown().await;
    engine.shutdown().await; // must not panic or log errors
}
