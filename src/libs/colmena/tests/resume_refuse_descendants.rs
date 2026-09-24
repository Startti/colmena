//! Integration test: `fail_if_suspended` + `fail_suspended_descendants`, the
//! pair `close_refused` uses to close a refused child's row *and* its own
//! SUSPENDED descendants, atomically and without touching rows that aren't
//! SUSPENDED or aren't its descendants.

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

async fn cleanup(pool: &sqlx::PgPool, agent_session_ids: &[&str]) {
    for chat in agent_session_ids {
        sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
            .bind(chat)
            .execute(pool)
            .await
            .ok();
    }
}

/// Seeds root(SUSPENDED) -> a(SUSPENDED) -> b(SUSPENDED) -> c(COMPLETED) under
/// one chat, plus an unrelated SUSPENDED row under a different chat. Closes
/// `a` via `fail_if_suspended` + `fail_suspended_descendants`: `a` and `b`
/// must become FAILED; `root` (a's parent, not its descendant), `c` (already
/// terminal — the SUSPENDED guard in `fail_suspended_descendants`'s SQL must
/// not touch it) and the unrelated row must all stay exactly as seeded.
/// `find_resume_entry` for the chat then returns `root`, not `b` — the bug
/// this pair of methods exists to close.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn refusing_a_child_closes_its_suspended_descendants_and_fixes_find_resume_entry() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = PostgresDagStateRepository::new(pool);

    let chat = "test_chat_refuse_descendants";
    let other_chat = "test_chat_refuse_descendants_unrelated";
    cleanup(repo.pool(), &[chat, other_chat]).await;

    let root = format!("{}_root", chat);
    let a = format!("{}_a", chat);
    let b = format!("{}_b", chat);
    let c = format!("{}_c", chat);
    let unrelated = format!("{}_unrelated_root", other_chat);

    repo.save(&fake_state(
        &root,
        Some(chat),
        None,
        DagRunStatus::Suspended,
    ))
    .await
    .unwrap();
    repo.save(&fake_state(
        &a,
        Some(chat),
        Some(&root),
        DagRunStatus::Suspended,
    ))
    .await
    .unwrap();
    repo.save(&fake_state(
        &b,
        Some(chat),
        Some(&a),
        DagRunStatus::Suspended,
    ))
    .await
    .unwrap();
    repo.save(&fake_state(
        &c,
        Some(chat),
        Some(&b),
        DagRunStatus::Completed,
    ))
    .await
    .unwrap();
    repo.save(&fake_state(
        &unrelated,
        Some(other_chat),
        None,
        DagRunStatus::Suspended,
    ))
    .await
    .unwrap();

    let flipped_a = repo.fail_if_suspended(&a).await.unwrap();
    assert!(flipped_a, "a was SUSPENDED and must be closed");
    let flipped_count = repo.fail_suspended_descendants(&a).await.unwrap();
    assert_eq!(flipped_count, 1, "only b is a SUSPENDED descendant of a");

    async fn status_of(repo: &PostgresDagStateRepository, sid: &str) -> DagRunStatus {
        repo.get_by_id(sid).await.unwrap().unwrap().status
    }

    assert_eq!(status_of(&repo, &a).await, DagRunStatus::Failed);
    assert_eq!(status_of(&repo, &b).await, DagRunStatus::Failed);
    assert_eq!(
        status_of(&repo, &c).await,
        DagRunStatus::Completed,
        "already terminal, and not SUSPENDED: the guard must leave it"
    );
    assert_eq!(
        status_of(&repo, &root).await,
        DagRunStatus::Suspended,
        "the parent is not a's descendant"
    );
    assert_eq!(
        status_of(&repo, &unrelated).await,
        DagRunStatus::Suspended,
        "a different chat's chain must stay untouched"
    );

    let entry = repo.find_resume_entry(chat).await.unwrap();
    assert_eq!(
        entry,
        Some(root.clone()),
        "root is the only SUSPENDED row left in the chain; b must not be picked"
    );

    cleanup(repo.pool(), &[chat, other_chat]).await;
}

/// `fail_if_suspended` on an already-COMPLETED row is a no-op: returns
/// `false`, and the row's status is untouched. Pins the `AND status =
/// 'SUSPENDED'` guard in the Postgres UPDATE.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn fail_if_suspended_leaves_a_completed_row_untouched() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = PostgresDagStateRepository::new(pool);

    let chat = "test_chat_refuse_completed_guard";
    cleanup(repo.pool(), &[chat]).await;

    let done = format!("{}_done", chat);
    repo.save(&fake_state(
        &done,
        Some(chat),
        None,
        DagRunStatus::Completed,
    ))
    .await
    .unwrap();

    let flipped = repo.fail_if_suspended(&done).await.unwrap();
    assert!(!flipped);

    let row = repo.get_by_id(&done).await.unwrap().unwrap();
    assert_eq!(row.status, DagRunStatus::Completed);

    cleanup(repo.pool(), &[chat]).await;
}
