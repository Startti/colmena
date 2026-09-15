//! Regression test proving that `inject_secrets` covers node `config`.
//!
//! 1. Persists `SOURCE` — Python source — under a handle for a unique session.
//! 2. Runs a `python_script` node `show` whose `config.code` is only that handle, so the
//!    script runs only if `inject_secrets` resolved it in config (an input edge cannot
//!    stand in: there is none).
//! 3. Stream frames are masked back to the handle, so the source appears in none.
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
    // `show` is a python_script; the binary initializes pyo3 in main, a test must too.
    pyo3::Python::initialize();
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().await.unwrap();
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

/// Quote-free on purpose: serialized frames would escape quotes and hide a leak.
const SOURCE: &str = "output = dict(config_injected=True)";

fn smoke_graph(handle: &str) -> Graph {
    let raw = json!({
        "nodes": {
            "show": { "type": "python_script", "config": { "sandbox_mode": "none", "code": handle } }
        },
        "edges": []
    });
    serde_json::from_value(raw).expect("valid graph JSON")
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn config_handle_is_injected_before_the_node_runs() {
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
        .persist_secret(&session_id, None, "test_setup", "smoke", SOURCE)
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

    // --- Step 3: the source ran; no frame carries it ---
    let mut show = serde_json::Value::Null;
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        let raw = serde_json::to_string(&ev).unwrap();
        assert!(
            !raw.contains(SOURCE),
            "stream frame leaked the secret: {raw}"
        );
        if let DagExecutionEvent::NodeFinish {
            node_id, output, ..
        } = ev
        {
            if node_id == "show" {
                show = output;
            }
        }
    }
    drop(stream);

    assert_eq!(
        show,
        json!({ "config_injected": true }),
        "inject_secrets must resolve the config handle before the node runs"
    );

    cleanup(&session_id).await;
    eng.shutdown().await;
}
