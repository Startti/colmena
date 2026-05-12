//! Integration tests for orchestrator agent-suspend propagation
//! (spec docs/superpowers/specs/2026-04-29-orchestrator-agent-suspend-design.md).
//!
//! Requires DATABASE_URL and GEMINI_API_KEY (the orchestrator's planner / reactor
//! components are LLM nodes). Each test cleans up its own dag_runs rows.

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use futures::StreamExt;
use serde_json::json;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
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

/// A minimal orchestrator with one agent whose subgraph just suspends.
/// We use a free-form Gemini planner because the orchestrator requires one,
/// but the only "real" work happens in the suspend node inside the agent.
fn single_agent_suspend_graph() -> Graph {
    let raw = json!({
        "nodes": {
            "trigger": {
                "type": "input",
                "config": { "prompt": "Ask the user for confirmation." }
            },
            "orch": {
                "type": "orchestrator",
                "config": {
                    "max_phases": 3,
                    "verbose": false,
                    "include_extra_info": false,
                    "planner": {
                        "provider": "google",
                        "model": "gemini-2.5-flash",
                        "api_key": "${GEMINI_API_KEY}",
                        "system_message": "Break the request into exactly ONE task assigned to 'asker'."
                    },
                    "agents": {
                        "asker": {
                            "description": "Asks the user one yes/no question.",
                            "child_graph_inline": {
                                "nodes": {
                                    "ask_in": { "type": "input", "config": {} },
                                    "ask": {
                                        "type": "suspend",
                                        "config": {
                                            "id": "confirm",
                                            "question": "Do you confirm?",
                                            "question_type": "choice",
                                            "options": ["yes", "no"]
                                        }
                                    },
                                    "ask_out": { "type": "output", "config": {} }
                                },
                                "edges": [
                                    { "from": "ask_in", "to": "ask" },
                                    { "from": "ask", "to": "ask_out" }
                                ]
                            }
                        }
                    },
                    "phase_reactor": {
                        "provider": "google",
                        "model": "gemini-2.5-flash",
                        "api_key": "${GEMINI_API_KEY}",
                        "system_message": "Summarize phase. Set task_ok=true."
                    },
                    "final_reactor": {
                        "provider": "google",
                        "model": "gemini-2.5-flash",
                        "api_key": "${GEMINI_API_KEY}",
                        "system_message": "Reply with what the user confirmed."
                    }
                }
            }
        },
        "edges": [
            { "from": "trigger", "to": "orch" }
        ]
    });
    serde_json::from_value(raw).unwrap()
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn orchestrator_propagates_agent_suspend() {
    let chat = "test_orch_agent_suspend";
    cleanup(chat).await;

    let eng = engine().await;
    let mut s = Box::pin(eng.execute_stream(
        single_agent_suspend_graph(),
        None,
        None,
        false,
        None,
        Some(chat.into()),
    ));
    let mut saw_suspended = false;
    while let Some(item) = s.next().await {
        let ev = item.expect("event");
        if let DagExecutionEvent::GraphFinish { output } = &ev {
            if output.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
                saw_suspended = true;
            }
        }
    }
    drop(s);
    assert!(
        saw_suspended,
        "orchestrator should bubble up SUSPENDED GraphFinish event"
    );

    // Verify dag_runs: the orchestrator's root run AND the asker subgraph run
    // should both be SUSPENDED.
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let suspended_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        suspended_count.0 >= 2,
        "expected at least 2 SUSPENDED rows (orchestrator root + asker subgraph), got {}",
        suspended_count.0
    );

    cleanup(chat).await;
    eng.shutdown().await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn orchestrator_resumes_agent_suspend_end_to_end() {
    let chat = "test_orch_resume_e2e";
    cleanup(chat).await;

    let eng = engine().await;

    // Run 1: trigger the suspend.
    let mut s1 = Box::pin(eng.execute_stream(
        single_agent_suspend_graph(),
        None,
        None,
        false,
        None,
        Some(chat.into()),
    ));
    while s1.next().await.is_some() {}
    drop(s1);

    // Run 2: resume by agent_session_id only with an answer in the canonical
    // ID-keyed Q/A format. The id ("confirm") matches the suspend node's config.id.
    let mut s2 = Box::pin(eng.execute_stream(
        single_agent_suspend_graph(),
        None,
        Some("Q[confirm]: Do you confirm?\nA[confirm]: yes".into()),
        false,
        None,
        Some(chat.into()),
    ));
    while s2.next().await.is_some() {}
    drop(s2);

    // Verify all rows are now COMPLETED.
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let still_suspended: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        still_suspended.0, 0,
        "after resume, no rows should remain SUSPENDED for this chat"
    );

    let completed: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'COMPLETED'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(completed.0 >= 2, "expected COMPLETED rows after resume");

    cleanup(chat).await;
    eng.shutdown().await;
}

/// E2E with the pre-existing fixture at tests/graphs/advanced/nested_orchestrators_suspend.json.
/// That graph has 3 levels: outer_orch → team_leader subgraph → leader_orch → confirm_specialist
/// subgraph → ask_user (suspend). Tests that the resume cascade unwinds 3 levels in one
/// invocation with just --agent-session-id and --answer.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn nested_orchestrators_suspend_cascades_3_levels() {
    let chat = "test_nested_3_levels";
    cleanup(chat).await;

    // The fixture lives at the workspace root; CARGO_MANIFEST_DIR points to the
    // crate root (src/libs/colmena), so we traverse three levels up.
    let graph_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/advanced/nested_orchestrators_suspend.json"
    );
    let raw = tokio::fs::read_to_string(graph_path)
        .await
        .expect("graph file must exist");
    let graph: Graph = serde_json::from_str(&raw).expect("parse graph");

    let eng = engine().await;

    // Run 1: should suspend with 3 SUSPENDED rows.
    let mut s1 =
        Box::pin(eng.execute_stream(graph.clone(), None, None, false, None, Some(chat.into())));
    while s1.next().await.is_some() {}
    drop(s1);

    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let suspended_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        suspended_count.0, 3,
        "expected 3 SUSPENDED rows after run 1 (root + 2 subgraphs)"
    );

    // Run 2: resume — the deepest suspend node (id="confirm_meeting") in the
    // fixture expects a canonical ID-keyed Q/A answer.
    let mut s2 = Box::pin(eng.execute_stream(
        graph,
        None,
        Some(
            "Q[confirm_meeting]: Please confirm: shall we schedule the meeting for Tuesday at 10am?\nA[confirm_meeting]: Yes, Tuesday at 10am works for me."
                .into(),
        ),
        false,
        None,
        Some(chat.into()),
    ));
    while s2.next().await.is_some() {}
    drop(s2);

    // All 3 rows should now be COMPLETED.
    let still_suspended: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        still_suspended.0, 0,
        "all rows should be COMPLETED after resume"
    );

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        total.0 >= 3,
        "expected at least 3 dag_runs rows in the chat tree"
    );

    cleanup(chat).await;
    eng.shutdown().await;
}
