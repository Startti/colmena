//! Engine-level proof that `COLMENA_LARGE_TABULAR` (`EngineConfig.prepare`)
//! reaches the `llm_call` node: a large `storage_key`-only attachment is
//! registered by the host's key when the switch is on, and skipped as before
//! when it is off. Needs `DATABASE_URL` (a real Postgres attachment registry):
//! `DATABASE_URL=postgres://... cargo test --test large_tabular_turn -- --ignored`.

use async_trait::async_trait;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::llm::infrastructure::{OverrideGuard, ScriptedAdapter, ScriptedResponse};
use colmena::tabular_prepare::ports::{
    InlineTrigger, PrepareConfig, PrepareRequest, PrepareRunner,
};
use serde_json::json;
use serial_test::serial;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

const KEY: &str = "chat-attachments/u/s/engine-level.csv";

/// A converter that does nothing: the switch needs a wired trigger to start.
struct Idle;

#[async_trait]
impl PrepareRunner for Idle {
    async fn run(&self, _req: PrepareRequest) {}
}

fn graph(url: &str, size: u64) -> Graph {
    serde_json::from_value(json!({
        "nodes": {
            "viewer": { "type": "llm_call", "config": {
                "provider": "openai", "model": "m", "api_key": "scripted",
                "connection_url": url, "attachments_enabled": true,
                "summary_enabled": false, "stream": false, "prompt": "go",
                "files": [{
                    "id": "doc-big", "mime_type": "text/csv", "filename": "big.csv",
                    "size_bytes": size, "storage_key": KEY
                }]
            }},
            "out": { "type": "log" }
        },
        "edges": [{ "from": "viewer", "to": "out" }]
    }))
    .unwrap()
}

/// `(provider_file_id, storage_key, origin)` of the registered row, if any.
async fn registered(
    url: &str,
    agent_session: &str,
) -> Option<(String, Option<String>, Option<String>)> {
    let pool = PgPoolOptions::new().connect(url).await.unwrap();
    sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
        "SELECT provider_file_id, storage_key, origin FROM conversation_attachments \
         WHERE agent_session_id = $1 AND document_id = 'doc-big'",
    )
    .bind(agent_session)
    .fetch_optional(&pool)
    .await
    .unwrap()
}

async fn run(
    switch: bool,
    size: u64,
    agent_session: &str,
) -> Option<(String, Option<String>, Option<String>)> {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let mut cfg = EngineConfig::from_env().await.unwrap();
    cfg.prepare = PrepareConfig {
        large_tabular: switch,
        trigger: Arc::new(InlineTrigger::new(Arc::new(Idle))),
        ..PrepareConfig::default()
    };
    let engine = ColmenaEngine::new(cfg).await.unwrap();
    let _model = OverrideGuard::install(Arc::new(ScriptedAdapter::new(vec![
        ScriptedResponse::Text("ok".into()),
        ScriptedResponse::Text("ok".into()),
    ])));
    engine
        .run_dag(
            graph(&url, size),
            None,
            None,
            false,
            Some(agent_session.to_string()),
        )
        .await
        .expect("the turn runs");
    registered(&url, agent_session).await
}

fn session(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("c9_{tag}_{nanos}")
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn the_engine_switch_reaches_the_node() {
    let on = run(true, 60 * 1024 * 1024, &session("on")).await;
    assert_eq!(
        on,
        Some((
            String::new(),
            Some(KEY.to_string()),
            Some("host_storage_ref".to_string())
        ))
    );

    assert_eq!(
        run(false, 60 * 1024 * 1024, &session("off")).await,
        None,
        "switch off: the key-only entry is skipped as before"
    );
    assert_eq!(
        run(true, 50 * 1024 * 1024, &session("exact")).await,
        None,
        "exactly 50 MiB is small"
    );
}
