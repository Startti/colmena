//! E2E of a tool's memory thread belonging to its caller, through a real
//! `ColmenaEngine` and Postgres: the same tool with memory, called by the
//! root and from inside another tool's child, keeps two threads.
//!
//! The parent agent (PADRE) has X — `parallel`, `dynamic` memory with the
//! thread fixed to `${agentId}`, as ADP's Run My Agent — and Y (`parallel`,
//! `persistent`). Y's child (HIJO-Y) has X too, with the same configuration.
//! X's child is an agent with memory that can ask the user through
//! `Preguntar` (a `suspend`). One scripted model answers every agent, by who
//! calls it (the system prompt says PADRE, HIJO-Y or AGENTE-X); a task says
//! what the agent does ("preguntá" asks, "terminá" answers, "lento" waits
//! first), and every request is kept.
//!
//! - A: in one message, X{x} asks and Y runs X{x} after a wait. The nested
//!   X runs on its own thread, so it does not heal the root's open question,
//!   and the resume answers it.
//! - C, in A: the root's keys are today's (`tool/X/x/agente_x`).
//!
//! Writes each turn's SSE to `/tmp/colmena_e2e/nested_tool_memory_<x>.sse`.
//!
//! Run with (the engine refuses to start without `SECURE_VALUES_KEY`):
//!   SECURE_VALUES_KEY=$(openssl rand -hex 24) DATABASE_URL=postgres:///colmena_e2e_par \
//!     cargo test -p colmena_dag_engine --test nested_tool_memory -- --ignored --nocapture

mod caller_model;

use caller_model::{CallerModel, Reply, Request};
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::domain::{LlmMessage, MessageRole};
use colmena::llm::infrastructure::OverrideGuard;
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;

/// How long HIJO-Y waits before it calls X when its task says "lento".
const SLOW: Duration = Duration::from_millis(600);

/// X's thread `x` called by the root: today's key.
const ROOT_X: &str = "tool/X/x/agente_x";
/// Y's child, called by the root.
const HIJO_Y: &str = "tool/Y/hijo_y";
/// X's thread `x` called by Y's child: under its caller's path.
const NESTED_X: &str = "tool/Y/hijo_y/tool/X/x/agente_x";

/// X's question, the id of its call, and what the human answers.
const ASKS: &str = "¿x: seguimos?";
const ASK: &str = "ask_x";
const HUMAN: &str = "sí, con el plan B";

/// A parent's call: `(tool call id, tool, agentId, task)`. Y takes no
/// agentId; its task is `<lento?> → <X's task>`.
type Call = (&'static str, &'static str, &'static str, &'static str);

/// The calls the parent makes on the turn whose prompt contains the key.
type Turns = &'static [(&'static str, &'static [Call])];

fn args(agent: &str, task: &str) -> String {
    match agent {
        "" => json!({ "task": task }),
        _ => json!({ "agentId": agent, "task": task }),
    }
    .to_string()
}

/// A reply given at once.
fn now(reply: Reply) -> (Duration, Reply) {
    (Duration::ZERO, reply)
}

/// The model of every agent of the graph. The parent makes the calls of its
/// turn and, once it reads their results, says "Listo.". HIJO-Y calls X with
/// the task after its `→`. X does what its task says; given the answer to
/// its question, it finishes.
fn script(turns: Turns) -> impl Fn(&Request) -> (Duration, Reply) + Send + Sync {
    move |req| {
        let task = req.last_user();
        let answered = req.last_role() == MessageRole::Tool;
        if req.system.contains("PADRE") {
            if answered {
                return now(Reply::Text("Listo.".into()));
            }
            let Some((_, calls)) = turns.iter().find(|(key, _)| task.contains(key)) else {
                return now(Reply::Text(format!("SIN GUION: {task}")));
            };
            let calls = calls
                .iter()
                .map(|(id, tool, agent, task)| {
                    (id.to_string(), tool.to_string(), args(agent, task))
                })
                .collect();
            return now(Reply::Calls(calls));
        }
        let result = req.last_content();
        if req.system.contains("HIJO-Y") {
            if answered {
                return now(Reply::Text(format!("y: hecho, con {result}")));
            }
            let (how, x_task) = task.split_once("→ ").unwrap_or_else(|| panic!("{task}"));
            let wait = if how.contains("lento") {
                SLOW
            } else {
                Duration::ZERO
            };
            let agent = x_task.split(':').next().unwrap_or_default();
            let call = ("call_y_x".to_string(), "X".to_string(), args(agent, x_task));
            return (wait, Reply::Calls(vec![call]));
        }
        let agent = task.split(':').next().unwrap_or_default();
        if answered {
            return now(Reply::Text(format!("{agent}: hecho, con {result}")));
        }
        if task.contains("preguntá") {
            let ask = json!({ "question": ASKS }).to_string();
            return now(Reply::Calls(vec![(ASK.into(), "Preguntar".into(), ask)]));
        }
        now(Reply::Text(format!("{agent}: hecho")))
    }
}

/// The committed graph with `prompt` as the parent's prompt, and a stand-in
/// key: the scripted model never uses it.
fn graph(prompt: &str) -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/agents/nested_tool_memory.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let mut graph: Value =
        serde_json::from_str(&text.replace("${GEMINI_API_KEY}", "scripted")).unwrap();
    graph["nodes"]["pedido"]["config"]["prompt"] = prompt.into();
    serde_json::from_value(graph).unwrap()
}

fn unique_chat(scenario: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("nested_memory_{scenario}_{nanos}")
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

/// Runs one turn of the chat (`answer` resumes it) and returns its `finish`
/// frame.
async fn turn(
    eng: &ColmenaEngine,
    prompt: &str,
    answer: Option<&str>,
    chat: &str,
    sse: &str,
) -> Value {
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let mut stream = Box::pin(eng.execute_stream(
        graph(prompt),
        None,
        answer.map(str::to_string),
        false,
        None,
        Some(chat.to_string()),
    ));
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        frames.extend(mapper.map(&ev));
    }
    drop(stream);
    std::fs::create_dir_all("/tmp/colmena_e2e").unwrap();
    let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
    std::fs::write(
        format!("/tmp/colmena_e2e/nested_tool_memory_{sse}.sse"),
        body + "data: [DONE]\n\n",
    )
    .unwrap();
    frames
        .into_iter()
        .find(|f| f["type"] == "finish")
        .expect("no finish frame")
}

fn assert_suspended_on_x(finish: &Value, call: &str) {
    assert_eq!(finish["finishReason"], "suspended", "{finish}");
    let output = &finish["output"];
    assert_eq!(output["questions"][0]["question"], ASKS, "{output}");
    assert_eq!(output["questions"].as_array().unwrap().len(), 1, "{output}");
    assert_eq!(output["_pending_tool_call_id"], call, "{output}");
}

fn assert_done(finish: &Value) {
    assert_eq!(finish["finishReason"], "stop", "{finish}");
    assert_eq!(finish["output"]["salida"]["result"], "Listo.", "{finish}");
}

/// The answer to X's question, in the resume format.
fn answer() -> String {
    format!("Q[pregunta_x]: {ASKS}\nA[pregunta_x]: {HUMAN}")
}

/// The thread `node_id` keeps, oldest first and without its system prompt:
/// `(role, what it says)`, where a `tool` message says `<id>: <content>` and
/// an assistant's call says `→ <id>`.
async fn thread(pool: &sqlx::PgPool, chat: &str, node_id: &str) -> Vec<(String, String)> {
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT role, content, tool_call_id, tool_calls::text FROM llm_node_history \
         WHERE agent_session_id = $1 AND node_id = $2 AND role <> 'system' ORDER BY created_at",
    )
    .bind(chat)
    .bind(node_id)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(role, content, id, calls)| {
            let calls: Vec<Value> = calls
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default();
            let said = match (id, calls.as_slice()) {
                (Some(id), _) => format!("{id}: {content}"),
                (None, [call]) => format!("→ {}", call["id"].as_str().unwrap()),
                _ => content,
            };
            (role, said)
        })
        .collect()
}

fn said(role: &str, what: &str) -> (String, String) {
    (role.to_string(), what.to_string())
}

/// X's thread once it asked: its task, and the call no result answers yet.
fn asked() -> [(String, String); 2] {
    [said("user", "x: preguntá"), said("assistant", "→ ask_x")]
}

/// Asserts the thread at `key` holds X's question, the human's answer to it
/// and X's reply, and nothing else.
async fn assert_answered_on(pool: &sqlx::PgPool, chat: &str, key: &str) {
    let thread = thread(pool, chat, key).await;
    assert_eq!(thread.len(), 4, "{thread:?}");
    assert_eq!(thread[..2], asked());
    let answer = &thread[2].1;
    assert!(
        answer.starts_with("ask_x: ") && answer.contains(HUMAN),
        "{thread:?}"
    );
}

/// Every memory key the chat wrote, sorted.
async fn keys(pool: &sqlx::PgPool, chat: &str) -> Vec<String> {
    let mut keys: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT node_id FROM llm_node_history WHERE agent_session_id = $1",
    )
    .bind(chat)
    .fetch_all(pool)
    .await
    .unwrap();
    keys.sort();
    keys
}

/// The requests X's model received on the run whose prompt is `task`.
fn x_requests(model: &CallerModel, task: &str) -> Vec<Request> {
    model
        .seen()
        .into_iter()
        .filter(|r| r.system.contains("AGENTE-X") && r.last_user() == task)
        .collect()
}

/// The messages of a request without its system prompt: `(role, content)`,
/// or the id a `tool` message answers.
fn turns_of(messages: &[LlmMessage]) -> Vec<(MessageRole, &str)> {
    messages
        .iter()
        .filter(|m| m.role() != &MessageRole::System)
        .map(|m| (m.role().clone(), m.tool_call_id().unwrap_or(m.content())))
        .collect()
}

/// Asserts X's model read the human's answer as the result of its question,
/// once: it asked on one request and finished on the next.
fn assert_x_read_the_answer(model: &CallerModel) {
    let requests = x_requests(model, "x: preguntá");
    assert_eq!(requests.len(), 2, "X asked once, then finished");
    let last = &requests[1];
    assert_eq!(
        turns_of(&last.messages),
        [
            (MessageRole::User, "x: preguntá"),
            (MessageRole::Assistant, ""),
            (MessageRole::Tool, ASK),
        ]
    );
    assert!(
        last.last_content().contains(HUMAN),
        "{}",
        last.last_content()
    );
}

/// A: X{x} asks at once; Y, called in the same message, waits `SLOW` and then
/// runs X{x}: after the root's question is in its thread.
const TURNS_A: Turns = &[(
    "turno 1",
    &[
        ("call_x", "X", "x", "x: preguntá"),
        ("call_y", "Y", "", "lento → x: terminá"),
    ],
)];

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_nested_call_leaves_the_roots_question_open_and_the_resume_answers_it() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("a");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script(TURNS_A)));
    let _guard = OverrideGuard::install(model.clone());

    let first = turn(&eng, "turno 1", None, &chat, "a1").await;
    assert_suspended_on_x(&first, "call_x");
    // The root's X keeps its question open: no marker answered it.
    assert_eq!(thread(&pool, &chat, ROOT_X).await, asked());
    // The nested X ran on its own thread, with only its own messages.
    assert_eq!(
        thread(&pool, &chat, NESTED_X).await,
        [said("user", "x: terminá"), said("assistant", "x: hecho")]
    );
    // C: the root's X keeps today's key.
    assert_eq!(
        keys(&pool, &chat).await,
        ["agent", ROOT_X, HIJO_Y, NESTED_X]
    );

    let second = turn(&eng, "turno 1", Some(&answer()), &chat, "a2").await;
    assert_done(&second);
    assert_x_read_the_answer(&model);
    assert_answered_on(&pool, &chat, ROOT_X).await;

    cleanup(&pool, &chat).await;
    eng.shutdown().await;
}
