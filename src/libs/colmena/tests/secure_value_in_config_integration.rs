//! Regression test proving that `inject_secrets` does NOT cover node `config`.
//!
//! ## Bug description
//! When a node's `config` contains a secure-value handle (e.g. `<sv_smoke>`),
//! the engine's injection step only processes `inputs`, leaving the handle literal
//! in `config`. This means any node that reads a secret from its own config (e.g.
//! for static API keys, base URLs, etc.) never receives the real value.
//!
//! ## What this test does
//! 1. Pre-populates `<sv_smoke>` → `"smoke-value-xyz"` in the Postgres secure-value
//!    store for a unique session.
//! 2. Runs a graph whose `log` node has `config.marker_field = "<sv_smoke>"`.
//! 3. Inspects the `NodeStart` event for the `show` node.
//! 4. Asserts that `config.marker_field` equals `"smoke-value-xyz"`.
//!
//! ## Expected outcome before the fix (Task 1 — TDD red phase)
//! The assertion FAILS: `config.marker_field` equals `"<sv_smoke>"` (the raw handle),
//! proving the bug. After Task 2 fixes the engine, the assertion will PASS.
//!
//! Run with:
//!   source .env && cargo test --test secure_value_in_config_integration -- --ignored

use colmena::dag_engine::application::SecureValueService;
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::infrastructure::persistence::PostgresSecureValueRepository;
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

async fn cleanup(session_id: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM dag_runs WHERE session_id = $1")
        .bind(session_id)
        .execute(&pool)
        .await
        .ok();
    // Also clean up the secure values for the session
    let repo = Arc::new(PostgresSecureValueRepository::new(pool.clone()));
    let svc = SecureValueService::new(repo);
    let _ = svc.cleanup(session_id).await;
}

fn smoke_graph(handle: &str) -> Graph {
    let raw = json!({
        "nodes": {
            "show": {
                "type": "log",
                "config": {
                    "marker_field": handle
                }
            }
        },
        "edges": []
    });
    serde_json::from_value(raw).expect("valid graph JSON")
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn config_handle_is_not_injected_exposes_bug() {
    dotenvy::dotenv().ok();

    let session_id = format!(
        "sv_config_smoke_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    cleanup(&session_id).await;

    // --- Step 1: pre-populate the secure value for this session ---
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = Arc::new(PostgresSecureValueRepository::new(pool));
    let svc = SecureValueService::new(repo);

    let handle = svc
        .persist_secret(&session_id, None, "test_setup", "smoke", "smoke-value-xyz")
        .await
        .expect("persist_secret must succeed");

    assert!(
        handle.starts_with("<sv_smoke_") && handle.ends_with('>'),
        "persist_secret must return a handle of the form <sv_smoke_<8hex>>, got: {handle}"
    );

    // --- Step 2: run the graph ---
    // Pass `session_id` as `resume_session_id` so the engine uses that exact string
    // as its internal `session_id` (no state row → fresh run with the known id).
    // This keeps the lookup key consistent: both `persist_secret` above and the
    // engine's `inject_secrets` call now use the same identifier.
    let eng = engine().await;

    let mut stream = Box::pin(eng.execute_stream(
        smoke_graph(&handle),
        Some(session_id.clone()),
        None,
        false,
        None,
        None,
    ));

    // --- Step 3: collect the NodeStart event for the "show" node ---
    let mut show_config: Option<serde_json::Value> = None;

    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");

        if let DagExecutionEvent::NodeStart {
            ref node_id,
            ref config,
            ..
        } = ev
        {
            if node_id == "show" {
                show_config = Some(config.clone());
            }
        }
    }

    drop(stream);

    // --- Step 4: assert the config field was injected ---
    let config = show_config.expect("NodeStart event for 'show' must have fired");
    let marker = config
        .get("marker_field")
        .and_then(|v| v.as_str())
        .unwrap_or("<MISSING>");

    // This assertion FAILS before the fix (marker == "<sv_smoke>"),
    // and PASSES after the fix (marker == "smoke-value-xyz").
    assert_eq!(
        marker, "smoke-value-xyz",
        "config.marker_field must be the injected real value, not the handle. \
         Got: {marker:?}. This means inject_secrets does not cover node config — the bug."
    );

    cleanup(&session_id).await;
    eng.shutdown().await;
}
