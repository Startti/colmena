//! Integration test for cross-session secure-value lookup via `agent_session_id`.
//!
//! ## What this validates
//! A secret persisted in run A (`session_id=session_a`, `agent_session_id=agent_X`)
//! is correctly resolved by run B (`session_id=session_b`, same `agent_session_id=agent_X`).
//! This exercises the agent-first fallback path in the secure-value repository and the
//! `inject_secrets` step inside `run_use_case` that covers both inputs and config.
//!
//! ## Graph
//! Uses `tests/graphs/basic/secure_value_in_config_smoke.json` whose single `log` node
//! has `config.marker_field = "<sv_smoke>"`. The test pre-populates `<sv_smoke>` under
//! `session_a` + `agent_X`, then runs the engine under `session_b` + `agent_X` and checks
//! that the `NodeStart` event for `show` has the resolved real value in `config.marker_field`.
//!
//! Run with:
//!   source .env && cargo test --test secure_values_cross_session_integration -- --ignored

use colmena::dag_engine::application::SecureValueService;
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::infrastructure::persistence::PostgresSecureValueRepository;
use futures::StreamExt;
use std::sync::Arc;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

/// Load the smoke graph from the repo's `tests/graphs/basic/` directory.
/// `CARGO_MANIFEST_DIR` points to `src/libs/colmena`; walk three levels up to reach the repo root.
fn smoke_config_graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/basic/secure_value_in_config_smoke.json"
    );
    let raw = std::fs::read_to_string(path)
        .expect("secure_value_in_config_smoke.json must exist");
    serde_json::from_str(&raw).expect("valid graph JSON")
}

/// Delete all secure-value rows associated with a session and all dag_runs rows for the agent.
async fn cleanup(session_id: &str, agent_id: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();

    // Remove the secure-value rows seeded under session_id.
    sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
        .bind(session_id)
        .execute(&pool)
        .await
        .ok();

    // Remove any dag_runs created by both engine invocations under this agent.
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(agent_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn agent_session_id_resolves_handle_persisted_in_another_session() {
    dotenvy::dotenv().ok();

    // Use a nanos-based suffix so parallel test runs don't collide.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let agent_id = format!("xs_test_{nanos}");
    let session_a = format!("{agent_id}_runA");
    let session_b = format!("{agent_id}_runB");

    // Start clean.
    cleanup(&session_a, &agent_id).await;

    // --- Step 1: pre-populate <sv_smoke> under session_a + agent_id ---
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = Arc::new(PostgresSecureValueRepository::new(pool));
    let svc = SecureValueService::new(repo);

    let handle = svc
        .persist_secret(
            &session_a,
            Some(&agent_id),
            "test_setup",
            "smoke",
            "the-real-cross-session-value",
        )
        .await
        .expect("persist_secret must succeed");

    assert_eq!(
        handle, "<sv_smoke>",
        "persist_secret must return <sv_smoke>"
    );

    // --- Step 2: run the engine under session_b with the SAME agent_id ---
    // Pass session_b as resume_session_id so the engine adopts that exact string
    // as its session_id. The agent_session_id triggers the agent-first lookup path
    // inside inject_secrets, which finds the row seeded under session_a.
    let eng = engine().await;

    let mut stream = Box::pin(eng.execute_stream(
        smoke_config_graph(),
        Some(session_b.clone()),
        None,
        false,
        None,
        Some(agent_id.clone()),
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

    // --- Step 4: assert cross-session injection resolved the handle ---
    let config = show_config.expect("NodeStart for 'show' must have fired");
    let marker = config
        .get("marker_field")
        .and_then(|v| v.as_str())
        .unwrap_or("<MISSING>");

    assert_eq!(
        marker, "the-real-cross-session-value",
        "config.marker_field must equal the value persisted in session_a via agent_session_id; \
         got: {marker:?}. Cross-session lookup is broken."
    );

    // --- Cleanup ---
    cleanup(&session_a, &agent_id).await;
    eng.shutdown().await;
}
