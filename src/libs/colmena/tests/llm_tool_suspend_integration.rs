//! Deterministic coverage of llm_call's SUSPENDED-propagation path
//! (Spec 5) using ScriptedAdapter — no real LLM provider needed.
//!
//! These tests install a process-global ScriptedAdapter override via
//! `OverrideGuard`, which holds a serialization mutex against other
//! override tests.
//!
//! Run with:
//!   source .env && RUST_LOG=info,colmena=info,colmena_dag_engine=info \
//!     cargo test --test llm_tool_suspend_integration -- --ignored --nocapture

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::llm::infrastructure::{OverrideGuard, ScriptedAdapter, ScriptedResponse};
use futures::StreamExt;
use std::sync::Arc;

fn init_logs() {
    // Idempotent — can be called multiple times across tests safely.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();
}

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().await.unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

/// Reset DB state for a test agent_session_id and (when applicable) seed
/// the `agent_session` parent row that other tables may reference via FK.
///
/// Two deployment shapes:
///   1. Standalone colmena DB (`colmena_llm_memory` or similar) — only
///      colmena's own tables. No FK to `agent_session`. We just clean up.
///   2. Shared ADP DB — `llm_node_history.agent_session_id` and
///      `dag_runs.agent_session_id` are FKs into `agent_session(id)`. We
///      seed that row so inserts pass the FK; ON DELETE CASCADE cleans up
///      dependents on next run.
///
/// We auto-detect by checking `information_schema.tables`.
async fn cleanup(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();

    let has_agent_session: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = 'agent_session'
           )"#,
    )
    .fetch_one(&pool)
    .await
    .expect("probe agent_session existence");

    if has_agent_session {
        // CASCADE on agent_session covers dag_runs + llm_node_history.
        sqlx::query("DELETE FROM agent_session WHERE id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
    } else {
        // No FK — clean the engine tables directly.
        sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM llm_node_history WHERE agent_session_id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
    }

    // secure_value_mappings has no FK on agent_session — clean explicitly
    // either way.
    sqlx::query("DELETE FROM secure_value_mappings WHERE agent_session_id = $1")
        .bind(chat)
        .execute(&pool)
        .await
        .ok();

    if has_agent_session {
        sqlx::query(
            r#"INSERT INTO agent_session (id, "updatedAt")
               VALUES ($1, NOW())
               ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(chat)
        .execute(&pool)
        .await
        .expect("seed agent_session row");
    }
}

fn smoke_graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/advanced/llm_tool_suspend_smoke.json"
    );
    let raw = std::fs::read_to_string(path).expect("smoke graph must exist");
    serde_json::from_str(&raw).expect("valid graph JSON")
}

fn unique_chat(prefix: &str) -> String {
    format!(
        "{}_{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn run_until_finish(
    eng: &ColmenaEngine,
    graph: Graph,
    resume_answer: Option<String>,
    chat: &str,
) -> serde_json::Value {
    let mut stream = Box::pin(eng.execute_stream(
        graph,
        None,
        resume_answer,
        false,
        None,
        Some(chat.to_string()),
    ));
    let mut last_output = serde_json::Value::Null;
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        if let DagExecutionEvent::GraphFinish { ref output } = ev {
            last_output = output.clone();
        }
    }
    last_output
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn suspend_propagates_when_tool_returns_suspended() {
    init_logs();
    let chat = unique_chat("agent_test_suspend_propagate");
    cleanup(&chat).await;

    tracing::info!("=== TEST: suspend_propagates_when_tool_returns_suspended (chat={chat}) ===");

    let adapter = Arc::new(ScriptedAdapter::new(vec![ScriptedResponse::ToolCall {
        id: "call_1".into(),
        tool_name: "ask_secret".into(),
        arguments: serde_json::json!({
            "secrets": [{"question": "What is your username?", "name": "username"}]
        }),
    }]));
    let _guard = OverrideGuard::install(adapter);

    let eng = engine().await;
    let output = run_until_finish(&eng, smoke_graph(), None, &chat).await;

    tracing::info!(?output, "test: graph finished");

    assert_eq!(
        output.get("__colmena_status").and_then(|v| v.as_str()),
        Some("SUSPENDED"),
        "expected SUSPENDED, got: {output}"
    );
    let questions = output
        .get("questions")
        .and_then(|v| v.as_array())
        .expect("questions array");
    assert_eq!(questions.len(), 1);
    assert_eq!(
        questions[0].get("question").and_then(|v| v.as_str()),
        Some("What is your username?")
    );

    eng.shutdown().await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn resume_replays_and_completes() {
    init_logs();
    let chat = unique_chat("agent_test_resume_replay");
    cleanup(&chat).await;

    tracing::info!("=== TEST: resume_replays_and_completes (chat={chat}) ===");

    let eng = engine().await;

    // Run 1: tool_call → SUSPEND.
    {
        let adapter = Arc::new(ScriptedAdapter::new(vec![ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "ask_secret".into(),
            arguments: serde_json::json!({
                "secrets": [{"question": "User?", "name": "user"}]
            }),
        }]));
        let _g = OverrideGuard::install(adapter);
        tracing::info!("--- run 1: expecting SUSPENDED ---");
        let output = run_until_finish(&eng, smoke_graph(), None, &chat).await;
        assert_eq!(
            output.get("__colmena_status").and_then(|v| v.as_str()),
            Some("SUSPENDED"),
            "run 1 expected SUSPENDED, got: {output}"
        );
    }

    // Run 2: resume. Script provides ONE Text entry — the agent loop calls
    // the LLM once after replaying the tool, and that text response closes
    // the loop.
    {
        let adapter = Arc::new(ScriptedAdapter::new(vec![ScriptedResponse::Text(
            "Saved username.".into(),
        )]));
        let _g = OverrideGuard::install(adapter);
        tracing::info!("--- run 2: resume with Q[user]: User?\\nA[user]: alice ---");
        let output = run_until_finish(
            &eng,
            smoke_graph(),
            Some("Q[user]: User?\nA[user]: alice".into()),
            &chat,
        )
        .await;
        tracing::info!(?output, "test: run 2 graph finished");
        assert!(
            output.get("__colmena_status").and_then(|v| v.as_str()) != Some("SUSPENDED"),
            "run 2 expected COMPLETED (not SUSPENDED), got: {output}"
        );
    }

    eng.shutdown().await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn multiple_secrets_resolved_via_qa_format() {
    init_logs();
    let chat = unique_chat("agent_test_multi_secret");
    cleanup(&chat).await;

    tracing::info!("=== TEST: multiple_secrets_resolved_via_qa_format (chat={chat}) ===");

    let eng = engine().await;

    {
        let adapter = Arc::new(ScriptedAdapter::new(vec![ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "ask_secret".into(),
            arguments: serde_json::json!({
                "secrets": [
                    {"question": "User?", "name": "user"},
                    {"question": "Pass?", "name": "pass"}
                ]
            }),
        }]));
        let _g = OverrideGuard::install(adapter);
        tracing::info!("--- run 1: expecting SUSPENDED with 2 questions ---");
        let output = run_until_finish(&eng, smoke_graph(), None, &chat).await;
        let qs = output
            .get("questions")
            .and_then(|v| v.as_array())
            .expect("questions");
        assert_eq!(qs.len(), 2, "expected 2 questions, got: {output}");
    }

    {
        let adapter = Arc::new(ScriptedAdapter::new(vec![ScriptedResponse::Text(
            "Saved both credentials.".into(),
        )]));
        let _g = OverrideGuard::install(adapter);
        tracing::info!("--- run 2: resume with two Q/A pairs ---");
        let _output = run_until_finish(
            &eng,
            smoke_graph(),
            Some("Q[user]: User?\nA[user]: alice\nQ[pass]: Pass?\nA[pass]: hunter2".into()),
            &chat,
        )
        .await;
    }

    // With the sliding TTL change (spec 2026-05-11), live rows survive
    // the end-of-run sweep. Assert both <sv_user_*> and <sv_pass_*> handles
    // are present for this agent_session_id.
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT hash_key FROM secure_value_mappings \
         WHERE agent_session_id = $1 ORDER BY hash_key",
    )
    .bind(&chat)
    .fetch_all(&pool)
    .await
    .unwrap();
    let handles: Vec<String> = rows.into_iter().map(|r| r.0).collect();
    tracing::info!(?handles, "test: handles persisted for chat");

    assert!(
        handles.iter().any(|h| h.starts_with("<sv_user_")),
        "expected <sv_user_*> handle, got: {handles:?}"
    );
    assert!(
        handles.iter().any(|h| h.starts_with("<sv_pass_")),
        "expected <sv_pass_*> handle, got: {handles:?}"
    );

    eng.shutdown().await;
}
