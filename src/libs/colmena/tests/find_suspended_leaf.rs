//! Integration test: find_suspended_leaf returns the deepest SUSPENDED run
//! for an agent.

use colmena::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository};
use colmena::dag_engine::infrastructure::persistence::PostgresDagStateRepository;
use serde_json::json;
use std::collections::{HashMap, VecDeque};

fn fake_state(
    session_id: &str,
    agent: Option<&str>,
    parent: Option<&str>,
    status: DagRunStatus,
) -> DagRunState {
    DagRunState {
        session_id: session_id.to_string(),
        agent_session_id: agent.map(|s| s.to_string()),
        parent_session_id: parent.map(|s| s.to_string()),
        graph_json: json!({"nodes": {}, "edges": []}),
        all_outputs: HashMap::new(),
        status,
        global_shared_state: json!({}),
        active_queue: VecDeque::new(),
        execution_history: Vec::new(),
        global_calls: HashMap::new(),
        caller_specific_calls: HashMap::new(),
    }
}

#[tokio::test]
async fn finds_leaf_in_three_level_tree() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = PostgresDagStateRepository::new(pool);

    let chat = "test_chat_leaf_three";
    // Cleanup any leftover rows from a previous failed run.
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(repo.pool())
        .await
        .ok();

    let root = format!("{}_root", chat);
    let sub = format!("{}_sub", chat);
    let subsub = format!("{}_subsub", chat);

    repo.save(&fake_state(&root, Some(chat), None, DagRunStatus::Suspended)).await.unwrap();
    repo.save(&fake_state(&sub,  Some(chat), Some(&root),    DagRunStatus::Suspended)).await.unwrap();
    repo.save(&fake_state(&subsub, Some(chat), Some(&sub),  DagRunStatus::Suspended)).await.unwrap();

    let leaf = repo.find_suspended_leaf(chat).await.unwrap();
    assert_eq!(leaf, Some(subsub.clone()));

    // Cleanup.
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(repo.pool())
        .await
        .ok();
}

#[tokio::test]
async fn returns_none_when_no_suspended_run() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = PostgresDagStateRepository::new(pool);

    let chat = "test_chat_no_suspend";
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(repo.pool())
        .await
        .ok();

    let root = format!("{}_root", chat);
    repo.save(&fake_state(&root, Some(chat), None, DagRunStatus::Completed)).await.unwrap();

    let leaf = repo.find_suspended_leaf(chat).await.unwrap();
    assert_eq!(leaf, None);

    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(repo.pool())
        .await
        .ok();
}
