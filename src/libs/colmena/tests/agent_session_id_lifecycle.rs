//! End-to-end tests for the agent_session_id lifecycle (spec §4.1).
//!
//! Requires `DATABASE_URL` to be set and reachable. Each test cleans up
//! its own `dag_runs` rows.

use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use futures::StreamExt;
use serde_json::json;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().await.unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

fn trivial_graph() -> Graph {
    let raw = json!({
        "nodes": {
            "log": { "type": "log", "config": { "message": "hello" } }
        },
        "edges": []
    });
    serde_json::from_value(raw).unwrap()
}

async fn cleanup(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn first_run_under_new_chat_creates_root_with_agent_id() {
    let chat = "test_first_run";
    cleanup(chat).await;

    let eng = engine().await;
    let mut s =
        Box::pin(eng.execute_stream(trivial_graph(), None, None, false, None, Some(chat.into())));
    while s.next().await.is_some() {}
    drop(s);

    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT session_id, agent_session_id, parent_session_id \
         FROM dag_runs WHERE agent_session_id = $1",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.1, Some(chat.into()));
    assert_eq!(row.2, None);

    cleanup(chat).await;
    eng.shutdown().await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn second_run_same_chat_after_completed_creates_new_run() {
    let chat = "test_second_run";
    cleanup(chat).await;

    let eng = engine().await;

    // First run.
    let mut s1 =
        Box::pin(eng.execute_stream(trivial_graph(), None, None, false, None, Some(chat.into())));
    while s1.next().await.is_some() {}
    drop(s1);

    // Second run (no SUSPENDED state, so a fresh root run with same chat).
    let mut s2 =
        Box::pin(eng.execute_stream(trivial_graph(), None, None, false, None, Some(chat.into())));
    while s2.next().await.is_some() {}
    drop(s2);

    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count.0, 2,
        "two distinct runs should exist under the same chat"
    );

    cleanup(chat).await;
    eng.shutdown().await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn conflict_between_session_id_and_agent_session_id_errors() {
    let chat_a = "test_conflict_chat_a";
    let chat_b = "test_conflict_chat_b";
    cleanup(chat_a).await;
    cleanup(chat_b).await;

    let eng = engine().await;

    // Create a run under chat_a.
    let mut s = Box::pin(eng.execute_stream(
        trivial_graph(),
        None,
        None,
        false,
        None,
        Some(chat_a.into()),
    ));
    while s.next().await.is_some() {}
    drop(s);

    // Read its session_id.
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let (sid,): (String,) =
        sqlx::query_as("SELECT session_id FROM dag_runs WHERE agent_session_id = $1 LIMIT 1")
            .bind(chat_a)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Now try to resume that session_id while passing chat_b → must error.
    let mut s2 = Box::pin(eng.execute_stream(
        trivial_graph(),
        Some(sid),
        None,
        false,
        None,
        Some(chat_b.into()),
    ));
    let mut got_error = false;
    while let Some(item) = s2.next().await {
        if item.is_err() {
            got_error = true;
        }
    }
    assert!(got_error, "must surface the conflict as a stream error");

    cleanup(chat_a).await;
    cleanup(chat_b).await;
    eng.shutdown().await;
}
