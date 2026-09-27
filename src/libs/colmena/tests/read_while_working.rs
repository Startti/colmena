//! E2E of «que Auto lea lo que escribís mientras trabaja», through a real
//! `ColmenaEngine`, Postgres, a `RunControl` with a steering inbox and the
//! scripted model.
//!
//! Reuses `tests/graphs/agents/cancel_one_child.json`: a parent agent
//! (PADRE) that runs children (HIJO) with `Run` (`parallel: true`, as ADP's
//! Run My Agent).
//!
//! - A: the parent runs alfa alone, and the test writes to the inbox while
//!   alfa's child runs. The parent's next request carries the message after
//!   alfa's result, its thread keeps it right there, the frame
//!   `user-message-consumed` goes out after the result, and the final answer
//!   closes the inbox.
//! - B: the parent runs alfa (slow) and beta (fast) in one message, and the
//!   test writes while beta's child runs. The message is read only after
//!   both results: a group is waited for whole.
//!
//! Writes each turn's SSE to `/tmp/colmena_e2e/read_while_working_<x>.sse`.
//!
//! Run with (the engine refuses to start without `SECURE_VALUES_KEY`):
//!   SECURE_VALUES_KEY=$(openssl rand -hex 24) DATABASE_URL=postgres:///colmena_e2e_par \
//!     cargo test -p colmena_dag_engine --test read_while_working -- --ignored --nocapture

// `Request::last_content` and `last_role` serve the sibling E2Es, not this one.
#[allow(dead_code)]
mod caller_model;

use caller_model::{CallerModel, Reply, Request};
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig, RunControl};
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::domain::steering::InMemorySteeringInbox;
use colmena::llm::domain::MessageRole;
use colmena::llm::infrastructure::OverrideGuard;
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// What the person writes while the parent works.
const WRITTEN: &str = "y contame también en inglés";

/// The calls the parent makes in its first message: `(id, agent, task)`.
type Calls = &'static [(&'static str, &'static str, &'static str)];

/// The parent runs `calls` in one message and, once it read their results,
/// says "Listo."; a child answers after 1.5 s if its task says "lento", and
/// after 300 ms if not.
fn script(calls: Calls) -> impl Fn(&Request) -> (Duration, Reply) + Send + Sync + 'static {
    move |req: &Request| {
        if req.system.contains("PADRE") {
            if req.messages.iter().any(|m| m.role() == &MessageRole::Tool) {
                return (Duration::ZERO, Reply::Text("Listo.".into()));
            }
            let calls = calls.iter().map(|(id, agent, task)| {
                let args = json!({ "agentId": agent, "task": task }).to_string();
                (id.to_string(), "Run".to_string(), args)
            });
            return (Duration::ZERO, Reply::Calls(calls.collect()));
        }
        let task = req.last_user().to_string();
        let wait = if task.contains("lento") {
            Duration::from_millis(1500)
        } else {
            Duration::from_millis(300)
        };
        (wait, Reply::Text(format!("{task}: hecho")))
    }
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
    format!("read_while_working_{scenario}_{nanos}")
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

/// Runs one turn with `inbox` and writes [`WRITTEN`] to it when the child
/// whose `path` contains `write_at` starts. Returns every frame.
async fn turn(
    eng: &ColmenaEngine,
    chat: &str,
    inbox: &Arc<InMemorySteeringInbox>,
    write_at: &str,
    sse: &str,
) -> Vec<Value> {
    let control = RunControl::new(CancellationToken::new()).with_steering(inbox.clone());
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let mut written = false;
    let mut stream = Box::pin(eng.execute_stream_controlled(
        graph(),
        None,
        None,
        false,
        None,
        Some(chat.to_string()),
        control,
    ));
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        for frame in mapper.map(&ev) {
            let child_started = frame["type"] == "subgraph-node-start"
                && frame["node_id"] == "hijo"
                && frame["path"].as_str().is_some_and(|p| p.contains(write_at));
            if child_started && !written {
                written = true;
                assert!(
                    inbox.push("m1", WRITTEN),
                    "the inbox is open while the turn runs"
                );
            }
            frames.push(frame);
        }
    }
    drop(stream);
    assert!(written, "the child never started: {frames:?}");
    std::fs::create_dir_all("/tmp/colmena_e2e").unwrap();
    let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
    std::fs::write(
        format!("/tmp/colmena_e2e/read_while_working_{sse}.sse"),
        body + "data: [DONE]\n\n",
    )
    .unwrap();
    frames
}

/// Position of the one frame of `kind` whose `key` is `value`.
fn at(frames: &[Value], kind: &str, key: &str, value: &str) -> usize {
    let found: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f["type"] == kind && f[key] == value)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} with {key}={value}: {frames:?}");
    found[0]
}

/// The parent's thread, oldest first and without its system prompt, as
/// `<role>: <content>`.
async fn parent_thread(pool: &sqlx::PgPool, chat: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT role || ': ' || content FROM llm_node_history \
         WHERE agent_session_id = $1 AND node_id = 'agent' AND role <> 'system' \
         ORDER BY created_at, id",
    )
    .bind(chat)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Where [`WRITTEN`] sits in the parent's thread, with the thread.
async fn written_in_thread(pool: &sqlx::PgPool, chat: &str) -> (usize, Vec<String>) {
    let thread = parent_thread(pool, chat).await;
    let msg = thread
        .iter()
        .position(|m| *m == format!("user: {WRITTEN}"))
        .unwrap_or_else(|| panic!("{thread:?}"));
    (msg, thread)
}

async fn root_statuses(pool: &sqlx::PgPool, chat: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT status FROM dag_runs WHERE agent_session_id = $1 AND parent_session_id IS NULL",
    )
    .bind(chat)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The roles of the last `n` messages of the parent's last request, newest
/// first, and the content of its last message.
fn last_parent_roles(model: &CallerModel, n: usize) -> (Vec<MessageRole>, String) {
    let last = model
        .seen()
        .into_iter()
        .rev()
        .find(|r| r.system.contains("PADRE"))
        .unwrap();
    let roles = last
        .messages
        .iter()
        .rev()
        .take(n)
        .map(|m| m.role().clone())
        .collect();
    (roles, last.messages.last().unwrap().content().to_string())
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_message_written_while_a_child_runs_is_read_after_its_result() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("a");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script(&[(
        "call_alfa",
        "alfa",
        "alfa: rápido",
    )])));
    let _guard = OverrideGuard::install(model.clone());
    let inbox = Arc::new(InMemorySteeringInbox::new());

    let frames = turn(&eng, &chat, &inbox, "Run#0", "a").await;

    // The frame: after alfa's result, from the root, never from a child.
    let read = at(&frames, "user-message-consumed", "id", "m1");
    assert_eq!(frames[read]["node_id"], "agent");
    assert!(at(&frames, "tool-output-available", "toolCallId", "call_alfa") < read);
    assert!(!frames
        .iter()
        .any(|f| f["type"] == "subgraph-user-message-consumed"));
    let finish = frames
        .iter()
        .position(|f| f["type"] == "finish")
        .expect("finish");
    assert!(read < finish);
    assert_eq!(frames[finish]["finishReason"], "stop");

    // The parent's second request: the call, its result, then the message.
    let (roles, last) = last_parent_roles(&model, 3);
    assert_eq!(
        roles,
        [MessageRole::User, MessageRole::Tool, MessageRole::Assistant]
    );
    assert_eq!(last, WRITTEN);

    // Its thread keeps it right there, and the turn is one root run.
    let (msg, thread) = written_in_thread(&pool, &chat).await;
    assert!(thread[msg - 1].starts_with("tool: "), "{thread:?}");
    assert_eq!(thread[msg + 1], "assistant: Listo.");
    assert_eq!(root_statuses(&pool, &chat).await, ["COMPLETED"]);
    // The final answer closed the inbox: a late message is refused.
    assert!(!inbox.push("m2", "tarde"));
    cleanup(&pool, &chat).await;
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_message_written_during_a_group_waits_for_the_whole_group() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("b");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script(&[
        ("call_alfa", "alfa", "alfa: lento"),
        ("call_beta", "beta", "beta: rápido"),
    ])));
    let _guard = OverrideGuard::install(model.clone());
    let inbox = Arc::new(InMemorySteeringInbox::new());

    // Written while beta (the fast one) runs: alfa is still running after it.
    let frames = turn(&eng, &chat, &inbox, "Run#1", "b").await;

    let read = at(&frames, "user-message-consumed", "id", "m1");
    assert!(at(&frames, "tool-output-available", "toolCallId", "call_beta") < read);
    assert!(at(&frames, "tool-output-available", "toolCallId", "call_alfa") < read);
    let (roles, last) = last_parent_roles(&model, 4);
    assert_eq!(
        roles,
        [
            MessageRole::User,
            MessageRole::Tool,
            MessageRole::Tool,
            MessageRole::Assistant
        ]
    );
    assert_eq!(last, WRITTEN);
    // In the thread too: both results, then the message.
    let (msg, thread) = written_in_thread(&pool, &chat).await;
    assert!(thread[msg - 2].starts_with("tool: "), "{thread:?}");
    assert!(thread[msg - 1].starts_with("tool: "), "{thread:?}");
    assert_eq!(root_statuses(&pool, &chat).await, ["COMPLETED"]);
    cleanup(&pool, &chat).await;
}
