//! Integration tests for spec
//! `docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`.
//!
//! Verifies the engine no longer cascades `__colmena_resume_answer` into
//! nodes that weren't suspended.
//!
//! Uses `ScriptedAdapter` for deterministic LLM responses (no real API call).
//!
//! Run with:
//!   source .env && cargo test --test suspend_resume_routing -- --ignored --nocapture

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::llm::infrastructure::{OverrideGuard, ScriptedAdapter, ScriptedResponse};
use futures::StreamExt;
use serial_test::serial;
use std::sync::Arc;

fn init_logs() {
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
/// Auto-detects standalone colmena DB vs shared ADP DB by probing
/// `information_schema.tables`.
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
        sqlx::query("DELETE FROM agent_session WHERE id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
    } else {
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

fn repro_graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/basic/suspend_then_llm_resume.json"
    );
    let raw = std::fs::read_to_string(path).expect("suspend_then_llm_resume.json must exist");
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

/// Drain the stream and return (all events, GraphFinish output).
async fn run_until_finish(
    eng: &ColmenaEngine,
    graph: Graph,
    resume_answer: Option<String>,
    chat: &str,
) -> (Vec<DagExecutionEvent>, serde_json::Value) {
    let mut stream = Box::pin(eng.execute_stream(
        graph,
        None,
        resume_answer,
        false,
        None,
        Some(chat.to_string()),
    ));
    let mut events = Vec::new();
    let mut finish_output = serde_json::Value::Null;
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        if let DagExecutionEvent::GraphFinish { ref output } = ev {
            finish_output = output.clone();
        }
        events.push(ev);
    }
    (events, finish_output)
}

/// Verifies the suspend → llm_call resume bug (ADP 2026-06-04).
///
/// Run 1: the graph suspends at `ask_name`. GraphFinish should carry
/// `__colmena_status == "SUSPENDED"`.
///
/// Run 2: resume with the user answer. The engine should route the resume
/// answer only to the `ask_name` node, let `poet` run fresh, and complete
/// without a NodeError. Before the fix, the engine injects
/// `__colmena_resume_answer` into `poet`, which enters the resume-tool
/// branch and panics with "no pending tool call found in conversation
/// history".
#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn suspend_then_llm_resume_runs_llm_fresh() {
    init_logs();
    let chat = unique_chat("srr_suspend_then_llm");
    cleanup(&chat).await;

    tracing::info!("=== TEST: suspend_then_llm_resume_runs_llm_fresh (chat={chat}) ===");

    let eng = engine().await;

    // --- Run 1: expect SUSPENDED at ask_name ---
    {
        tracing::info!("--- run 1: expecting SUSPENDED ---");
        // No LLM call reaches poet in run 1 (graph suspends before poet).
        // Install an empty script — any call to ScriptedAdapter would error,
        // which would catch a regression where poet runs unexpectedly.
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let _guard = OverrideGuard::install(adapter);

        let (events, output) = run_until_finish(&eng, repro_graph(), None, &chat).await;
        tracing::info!(?output, "run 1: graph finished");

        let has_error = events.iter().any(|ev| matches!(ev, DagExecutionEvent::Error { .. }));
        assert!(
            !has_error,
            "run 1 should not produce any Error events, got: {events:?}"
        );
        assert_eq!(
            output.get("__colmena_status").and_then(|v| v.as_str()),
            Some("SUSPENDED"),
            "run 1 expected SUSPENDED, got: {output}"
        );
    }

    // --- Run 2: resume, poet must run fresh without entering resume-tool branch ---
    {
        tracing::info!("--- run 2: resume with answer for ask_name ---");
        // Script one text response for the poet llm_call.
        let adapter = Arc::new(ScriptedAdapter::new(vec![ScriptedResponse::Text(
            "¡Hola Julián! Me alegra conocerte.".into(),
        )]));
        let _guard = OverrideGuard::install(adapter);

        let (events, output) = run_until_finish(
            &eng,
            repro_graph(),
            Some("Q[ask_name]: ¿Cuál es tu nombre?\nA[ask_name]: Julián".into()),
            &chat,
        )
        .await;
        tracing::info!(?output, "run 2: graph finished");

        // Before the fix this fires: poet enters its resume branch, finds no
        // pending tool call, and emits an Error event.
        let error_events: Vec<_> = events
            .iter()
            .filter(|ev| matches!(ev, DagExecutionEvent::Error { .. }))
            .collect();
        assert!(
            error_events.is_empty(),
            "run 2 must produce NO Error events (bug: poet entered resume-tool branch). Errors: {error_events:?}"
        );

        // poet must have emitted a NodeFinish containing the scripted greeting.
        let poet_finish = events.iter().find(|ev| {
            matches!(ev,
                DagExecutionEvent::NodeFinish { node_id, output }
                if node_id == "poet"
                    && output.to_string().contains("Hola")
            )
        });
        assert!(
            poet_finish.is_some(),
            "poet NodeFinish with greeting not found. Events: {events:?}"
        );

        assert!(
            output.get("__colmena_status").and_then(|v| v.as_str()) != Some("SUSPENDED"),
            "run 2 must complete (not stay SUSPENDED), got: {output}"
        );
    }

    eng.shutdown().await;
}

fn cascade_graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/basic/suspend_cascade.json"
    );
    let raw = std::fs::read_to_string(path).expect("suspend_cascade.json must exist");
    serde_json::from_str(&raw).expect("valid graph JSON")
}

/// Verifies the suspend → suspend cascade resume scenario (spec §5 row 2).
///
/// Run 1: the graph suspends at `ask_one`. GraphFinish carries
/// `__colmena_status == "SUSPENDED"` and `questions[0].id == "ask_one"`.
///
/// Run 2: answer `ask_one` only. Engine must route the resume answer only to
/// `ask_one`, then run `ask_two` fresh and suspend there. Before the fix, the
/// engine cascaded `__colmena_resume_answer` into `ask_two`, which then failed
/// with "missing answer for ask_two".
///
/// Run 3: answer `ask_two`. Engine must route correctly and reach `finish`.
#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn suspend_cascade_resumes_each_node_independently() {
    init_logs();
    let chat = unique_chat("srr_cascade");
    cleanup(&chat).await;

    tracing::info!(
        "=== TEST: suspend_cascade_resumes_each_node_independently (chat={chat}) ==="
    );

    let eng = engine().await;

    // --- Run 1: must suspend at ask_one ---
    {
        tracing::info!("--- run 1: expecting SUSPENDED at ask_one ---");
        let (_events, output) = run_until_finish(&eng, cascade_graph(), None, &chat).await;
        tracing::info!(?output, "run 1: graph finished");

        assert_eq!(
            output.get("__colmena_status").and_then(|v| v.as_str()),
            Some("SUSPENDED"),
            "run 1 expected SUSPENDED, got: {output}"
        );
        assert_eq!(
            output
                .get("questions")
                .and_then(|q| q.as_array())
                .and_then(|a| a.first())
                .and_then(|q| q.get("id"))
                .and_then(|v| v.as_str()),
            Some("ask_one"),
            "run 1 must pause at ask_one, got: {output:#}"
        );
    }

    // --- Run 2: answer ask_one. Must suspend fresh at ask_two ---
    {
        tracing::info!("--- run 2: answering ask_one, expecting SUSPENDED at ask_two ---");
        let ans1 = "Q[ask_one]: Primera pregunta?\nA[ask_one]: alfa".to_string();
        let (_events, output) =
            run_until_finish(&eng, cascade_graph(), Some(ans1), &chat).await;
        tracing::info!(?output, "run 2: graph finished");

        assert_eq!(
            output.get("__colmena_status").and_then(|v| v.as_str()),
            Some("SUSPENDED"),
            "run 2 expected SUSPENDED at ask_two, got: {output}"
        );
        assert_eq!(
            output
                .get("questions")
                .and_then(|q| q.as_array())
                .and_then(|a| a.first())
                .and_then(|q| q.get("id"))
                .and_then(|v| v.as_str()),
            Some("ask_two"),
            "run 2 must pause at ask_two, got: {output:#}"
        );
    }

    // --- Run 3: answer ask_two. Must reach finish ---
    {
        tracing::info!("--- run 3: answering ask_two, expecting finish node ---");
        let ans2 = "Q[ask_two]: Segunda pregunta?\nA[ask_two]: beta".to_string();
        let (events, output) =
            run_until_finish(&eng, cascade_graph(), Some(ans2), &chat).await;
        tracing::info!(?output, "run 3: graph finished");

        let reached_finish = events.iter().any(|e| {
            matches!(e, DagExecutionEvent::NodeFinish { node_id, .. } if node_id == "finish")
        });
        assert!(reached_finish, "run 3 must reach `finish` node, events: {events:?}");

        assert!(
            output.get("__colmena_status").and_then(|v| v.as_str()) != Some("SUSPENDED"),
            "run 3 must complete (not stay SUSPENDED), got: {output}"
        );
    }

    cleanup(&chat).await;
    eng.shutdown().await;
}
