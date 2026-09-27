//! E2E of «cancelar un agente hijo por separado», through a real
//! `ColmenaEngine`, Postgres and a `RunControl`.
//!
//! The parent agent calls `Run` — `parallel: true`, as ADP's Run My Agent —
//! for alfa and beta in ONE message. The scripted model makes alfa's child
//! wait `LONG` and beta's `SHORT`.
//!
//! - A: the test cancels alfa's call as soon as alfa's child starts. Alfa's
//!   row is CANCELLED, its node and its boundary close with
//!   `CANCELLED_BY_PERSON`, its call's result is the cancelled text with
//!   `status: "cancelled"`, beta finishes after that, and the parent reads the
//!   cancelled text and answers. The turn takes far less than `LONG`.
//! - B: the composer's Stop (the turn's token) still stops everything: the
//!   root and both children CANCELLED, `cancelled` + `finish`, no call is
//!   answered with the cancelled text, no frame says `status: "cancelled"`
//!   and nothing closes with `CANCELLED_BY_PERSON`.
//!
//! Writes each turn's SSE to `/tmp/colmena_e2e/cancel_one_child_<x>.sse`.
//!
//! Run with (the engine refuses to start without `SECURE_VALUES_KEY`):
//!   SECURE_VALUES_KEY=$(openssl rand -hex 24) DATABASE_URL=postgres:///colmena_e2e_par \
//!     cargo test -p colmena_dag_engine --test cancel_one_child -- --ignored --nocapture

// `Request::last_content` serves the sibling E2Es, not this one.
#[allow(dead_code)]
mod caller_model;

use caller_model::{CallerModel, Reply, Request};
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig, RunControl};
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::domain::MessageRole;
use colmena::llm::infrastructure::OverrideGuard;
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// What the model reads for a call the person cancelled.
const CANCELLED: &str = include_str!("../text/prompts/agent_loop/cancelled_by_person.md");

/// How long alfa's child takes: the turn must end far sooner.
const LONG: Duration = Duration::from_secs(30);
/// How long beta's child takes: long enough to finish after alfa is cut.
const SHORT: Duration = Duration::from_millis(1500);

/// The parent calls Run for alfa and beta in one message and says "Listo."
/// once it reads their results. A child answers after `LONG` ("tarda") or
/// `SHORT`.
fn script(req: &Request) -> (Duration, Reply) {
    let now = Duration::ZERO;
    if req.system.contains("PADRE") {
        if req.last_role() == MessageRole::Tool {
            return (now, Reply::Text("Listo.".into()));
        }
        let call = |id: &str, agent: &str, task: &str| {
            let args = json!({ "agentId": agent, "task": task }).to_string();
            (id.to_string(), "Run".to_string(), args)
        };
        return (
            now,
            Reply::Calls(vec![
                call("call_alfa", "alfa", "alfa: tarda"),
                call("call_beta", "beta", "beta: terminá"),
            ]),
        );
    }
    let task = req.last_user().to_string();
    let wait = if task.contains("tarda") { LONG } else { SHORT };
    (wait, Reply::Text(format!("{task}: hecho")))
}

/// The committed graph with stand-in keys: the scripted model never uses them.
fn graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/agents/cancel_one_child.json"
    );
    let mut graph: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let agent = &mut graph["nodes"]["agent"]["config"];
    agent["api_key"] = "scripted".into();
    agent["tool_configurations"]["Run"]["node_schema"]["child_graph_inline"]["fixed"]["nodes"]
        ["hijo"]["config"]["api_key"] = "scripted".into();
    serde_json::from_value(graph).unwrap()
}

fn unique_chat(scenario: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("cancel_child_{scenario}_{nanos}")
}

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    sqlx::PgPool::connect(&url).await.unwrap()
}

async fn cleanup(pool: &sqlx::PgPool, chat: &str) {
    for table in ["dag_runs", "llm_node_history", "secure_value_mappings"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE agent_session_id = $1"))
            .bind(chat)
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    ColmenaEngine::new(EngineConfig::from_env().await.unwrap())
        .await
        .unwrap()
}

/// What cuts the turn when alfa's child starts.
enum Cut {
    AlfaCall,
    WholeTurn,
}

/// Runs one turn and applies `cut` at the first frame of alfa's child
/// (`Run#0`: alfa is the model's first call). Returns every frame.
async fn turn(eng: &ColmenaEngine, chat: &str, cut: Cut, sse: &str) -> Vec<Value> {
    let control = RunControl::new(CancellationToken::new());
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let mut cut_done = false;
    let mut stream = Box::pin(eng.execute_stream_controlled(
        graph(),
        None,
        None,
        false,
        None,
        Some(chat.to_string()),
        control.clone(),
    ));
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        for frame in mapper.map(&ev) {
            let alfa_child_started = frame["type"] == "subgraph-node-start"
                && frame["node_id"] == "hijo"
                && frame["path"].as_str().is_some_and(|p| p.contains("Run#0"));
            if alfa_child_started && !cut_done {
                cut_done = true;
                match cut {
                    Cut::AlfaCall => {
                        assert!(control.cancel_call("call_alfa"), "alfa's call is running")
                    }
                    Cut::WholeTurn => control.cancel_token().cancel(),
                }
            }
            frames.push(frame);
        }
    }
    drop(stream);
    assert!(cut_done, "alfa's child never started: {frames:?}");
    std::fs::create_dir_all("/tmp/colmena_e2e").unwrap();
    let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
    std::fs::write(
        format!("/tmp/colmena_e2e/cancel_one_child_{sse}.sse"),
        body + "data: [DONE]\n\n",
    )
    .unwrap();
    frames
}

/// `(agentId, status)` of every child run of the chat, by agent.
async fn children(pool: &sqlx::PgPool, chat: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT (global_shared_state::jsonb)->>'agentId' AS agent, status FROM dag_runs \
         WHERE agent_session_id = $1 AND parent_session_id IS NOT NULL ORDER BY 1",
    )
    .bind(chat)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn root_status(pool: &sqlx::PgPool, chat: &str) -> String {
    sqlx::query_scalar(
        "SELECT status FROM dag_runs WHERE agent_session_id = $1 AND parent_session_id IS NULL",
    )
    .bind(chat)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// How many stored tool messages of the chat are the cancelled text.
async fn answered_cancelled(pool: &sqlx::PgPool, chat: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM llm_node_history \
         WHERE agent_session_id = $1 AND role = 'tool' AND content = $2",
    )
    .bind(chat)
    .bind(CANCELLED.trim())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Position of the one frame of `kind` for the tool call `id`.
fn at(frames: &[Value], kind: &str, id: &str) -> usize {
    let found: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f["type"] == kind && f["toolCallId"] == id)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} for {id}: {frames:?}");
    found[0]
}

/// Every frame that closes something with the per-call cancel's text.
fn closed_by_the_cancel(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|f| {
            f["errorText"]
                .as_str()
                .is_some_and(|t| t.starts_with("CANCELLED_BY_PERSON"))
        })
        .collect()
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn cancelling_one_child_cuts_only_it_and_the_parent_carries_on() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("a");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script));
    let _guard = OverrideGuard::install(model.clone());

    let started = Instant::now();
    let frames = turn(&eng, &chat, Cut::AlfaCall, "a").await;
    let took = started.elapsed();
    eprintln!("cancel_one_child A: the turn took {took:?}");
    assert!(took < LONG / 2, "alfa was not cut: {took:?}");

    let alfa = &frames[at(&frames, "tool-output-available", "call_alfa")];
    assert_eq!(alfa["status"], "cancelled", "{alfa}");
    assert_eq!(alfa["output"], CANCELLED.trim());
    assert_eq!(alfa["childScope"], "Run#0");
    let beta = &frames[at(&frames, "tool-output-available", "call_beta")];
    assert!(beta.get("status").is_none(), "{beta}");
    let statuses = frames.iter().filter(|f| f["status"] == "cancelled").count();
    assert_eq!(statuses, 1, "only alfa's result says cancelled");
    // Beta finished after alfa was cut: the sibling kept running.
    assert!(
        at(&frames, "tool-output-available", "call_alfa")
            < at(&frames, "tool-output-available", "call_beta")
    );

    // Alfa's child closed its node and its boundary with the cancel's text,
    // and nothing else did.
    let closes: Vec<&Value> = frames
        .iter()
        .filter(|f| {
            f["type"] == "subgraph-node-end"
                && f["path"].as_str().is_some_and(|p| p.contains("Run#0"))
        })
        .collect();
    assert_eq!(closes.len(), 2, "{closes:?}");
    for close in &closes {
        assert_eq!(close["status"], "error", "{close}");
        let text = close["errorText"].as_str().unwrap_or_default();
        assert!(text.starts_with("CANCELLED_BY_PERSON"), "{close}");
    }
    assert_eq!(closed_by_the_cancel(&frames), closes);

    let finish = frames
        .iter()
        .find(|f| f["type"] == "finish")
        .expect("finish");
    assert_eq!(finish["finishReason"], "stop", "{finish}");
    assert!(!frames.iter().any(|f| f["type"] == "cancelled"));

    // The parent read the cancelled text for alfa, and beta's answer.
    let last = model
        .seen()
        .into_iter()
        .rev()
        .find(|r| r.system.contains("PADRE"))
        .unwrap();
    let answer_to = |id: &str| {
        last.messages
            .iter()
            .find(|m| m.role() == &MessageRole::Tool && m.tool_call_id() == Some(id))
            .map(|m| m.content().to_string())
    };
    assert_eq!(answer_to("call_alfa").as_deref(), Some(CANCELLED.trim()));
    assert!(answer_to("call_beta").is_some_and(|a| a.contains("beta: terminá: hecho")));

    // In the base: alfa's child CANCELLED, beta's COMPLETED, the root
    // COMPLETED, and the parent's thread keeps alfa's answer (what B's zero
    // is measured against).
    assert_eq!(
        children(&pool, &chat).await,
        [
            ("alfa".to_string(), "CANCELLED".to_string()),
            ("beta".to_string(), "COMPLETED".to_string())
        ]
    );
    assert_eq!(root_status(&pool, &chat).await, "COMPLETED");
    assert_eq!(answered_cancelled(&pool, &chat).await, 1);
    cleanup(&pool, &chat).await;
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn the_turn_stop_still_stops_everything_and_answers_no_call() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("b");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script));
    let _guard = OverrideGuard::install(model.clone());

    let started = Instant::now();
    let frames = turn(&eng, &chat, Cut::WholeTurn, "b").await;
    eprintln!("cancel_one_child B: the turn took {:?}", started.elapsed());
    assert!(
        frames.iter().any(|f| f["type"] == "cancelled"),
        "{frames:?}"
    );
    let finish = frames
        .iter()
        .find(|f| f["type"] == "finish")
        .expect("finish");
    assert_eq!(finish["finishReason"], "cancelled", "{finish}");
    assert!(
        !frames.iter().any(|f| f["status"] == "cancelled"),
        "a turn Stop answered a call"
    );
    assert!(
        !frames.iter().any(|f| f["output"] == CANCELLED.trim()),
        "a turn Stop answered a call"
    );
    let cut = closed_by_the_cancel(&frames);
    assert!(
        cut.is_empty(),
        "a turn Stop closed as a call cancel: {cut:?}"
    );

    assert_eq!(root_status(&pool, &chat).await, "CANCELLED");
    assert_eq!(
        children(&pool, &chat).await,
        [
            ("alfa".to_string(), "CANCELLED".to_string()),
            ("beta".to_string(), "CANCELLED".to_string())
        ]
    );
    assert_eq!(answered_cancelled(&pool, &chat).await, 0);
    cleanup(&pool, &chat).await;
}
