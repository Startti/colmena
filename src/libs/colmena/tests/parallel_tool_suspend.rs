//! E2E of «esperar y preguntar»: one question per turn inside a group of
//! `parallel` tool calls, through a real `ColmenaEngine` and Postgres.
//!
//! The parent agent calls `Run` — `parallel: true`, `dynamic` memory with the
//! thread fixed to `${agentId}`, as ADP's Run My Agent — once per agent in ONE
//! message, so the calls form one group of two chains. Each child is an agent
//! with memory that can ask the user through `Preguntar` (a `suspend`). One
//! scripted model answers every agent, by who calls it (the system prompt says
//! PADRE or HIJO); a child's task says what it does ("preguntá" asks,
//! "terminá" answers, "lento" waits first).
//!
//! - A: one child asks, its sibling (called first) finishes: the turn waits
//!   for the group, then suspends on the question; the resume answers it
//!   under the same `Run#<k>`, with k = 1.
//!
//! Writes each turn's SSE to `/tmp/colmena_e2e/parallel_tool_suspend_<x>.sse`.
//!
//! Run with (the engine refuses to start without `SECURE_VALUES_KEY`):
//!   SECURE_VALUES_KEY=$(openssl rand -hex 24) DATABASE_URL=postgres:///colmena_e2e_par \
//!     cargo test -p colmena_dag_engine --test parallel_tool_suspend -- --ignored --nocapture

mod caller_model;

use caller_model::{CallerModel, Reply, Request};
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::domain::MessageRole;
use colmena::llm::infrastructure::OverrideGuard;
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;

/// How long a child whose task says "lento" waits before it answers.
const SLOW: Duration = Duration::from_millis(600);

/// A parent's call to `Run`: `(tool call id, agentId, task)`.
type Call = (&'static str, &'static str, &'static str);

/// The calls the parent makes on the turn whose prompt contains the key.
type Turns = &'static [(&'static str, &'static [Call])];

/// The model of every agent of the graph. The parent makes the calls of its
/// turn and, once it reads their results, says "Listo.". A child does what
/// its task says; given the answer to its question, it finishes.
fn script(turns: Turns) -> impl Fn(&Request) -> (Duration, Reply) + Send + Sync {
    move |req| {
        let now = Duration::ZERO;
        if req.system.contains("PADRE") {
            if req.last_role() == MessageRole::Tool {
                return (now, Reply::Text("Listo.".into()));
            }
            let prompt = req.last_user();
            let Some((_, calls)) = turns.iter().find(|(key, _)| prompt.contains(key)) else {
                return (now, Reply::Text(format!("SIN GUION: {prompt}")));
            };
            let calls = calls
                .iter()
                .map(|(id, agent, task)| {
                    let args = json!({ "agentId": agent, "task": task }).to_string();
                    (id.to_string(), "Run".to_string(), args)
                })
                .collect();
            return (now, Reply::Calls(calls));
        }
        let task = req.last_user();
        let agent = task.split(':').next().unwrap_or_default().to_string();
        if req.last_role() == MessageRole::Tool {
            let answer = format!("{agent}: hecho, con {}", req.last_content());
            return (now, Reply::Text(answer));
        }
        let wait = if task.contains("lento") { SLOW } else { now };
        if task.contains("preguntá") {
            let args = json!({ "question": format!("¿{agent}: seguimos?") }).to_string();
            let ask = (format!("ask_{agent}"), "Preguntar".to_string(), args);
            (wait, Reply::Calls(vec![ask]))
        } else {
            (wait, Reply::Text(format!("{agent}: hecho")))
        }
    }
}

/// The committed graph with `prompt` as the parent's prompt, and stand-in
/// keys: the scripted model never uses them.
fn graph(prompt: &str) -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/agents/parallel_tool_suspend.json"
    );
    let mut graph: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    graph["nodes"]["pedido"]["config"]["prompt"] = prompt.into();
    let agent = &mut graph["nodes"]["agent"]["config"];
    agent["api_key"] = "scripted".into();
    let child = &mut agent["tool_configurations"]["Run"]["node_schema"]["child_graph_inline"]
        ["fixed"]["nodes"]["hijo"]["config"];
    child["api_key"] = "scripted".into();
    serde_json::from_value(graph).unwrap()
}

fn unique_chat(scenario: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("par_suspend_{scenario}_{nanos}")
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

/// The frames of one turn, and its `finish` frame.
struct Turn {
    frames: Vec<Value>,
    finish: Value,
}

impl Turn {
    /// Positions and frames of `kind` for the tool call `id`.
    fn all(&self, kind: &str, id: &str) -> Vec<(usize, &Value)> {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f["type"] == kind && f["toolCallId"] == id)
            .collect()
    }

    /// Position and frame of the one frame of `kind` for the tool call `id`.
    fn one(&self, kind: &str, id: &str) -> (usize, &Value) {
        let found = self.all(kind, id);
        assert_eq!(found.len(), 1, "one {kind} for {id}: {found:?}");
        found[0]
    }

    /// Position of the `finish` frame.
    fn finish_at(&self) -> usize {
        self.frames
            .iter()
            .position(|f| f["type"] == "finish")
            .unwrap()
    }

    /// Paths of the frames a child emitted (the parent's own are at level 0).
    fn child_paths(&self) -> Vec<&str> {
        self.frames
            .iter()
            .filter(|f| f["level"].as_u64().unwrap_or(0) > 0)
            .map(|f| f["path"].as_str().unwrap_or_default())
            .collect()
    }

    fn assert_suspended_on(&self, question: &str, call: &str) {
        assert_eq!(self.finish["finishReason"], "suspended", "{}", self.finish);
        let output = &self.finish["output"];
        assert_eq!(output["questions"][0]["question"], question, "{output}");
        assert_eq!(output["questions"].as_array().unwrap().len(), 1, "{output}");
        assert_eq!(output["_pending_tool_call_id"], call, "{output}");
    }

    fn assert_done(&self) {
        assert_eq!(self.finish["finishReason"], "stop", "{}", self.finish);
        assert_eq!(
            self.finish["output"]["salida"]["result"], "Listo.",
            "{}",
            self.finish
        );
    }
}

/// Runs one turn of the chat; `answer` resumes it.
async fn turn(
    eng: &ColmenaEngine,
    prompt: &str,
    answer: Option<&str>,
    chat: &str,
    sse: &str,
) -> Turn {
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
        format!("/tmp/colmena_e2e/parallel_tool_suspend_{sse}.sse"),
        body + "data: [DONE]\n\n",
    )
    .unwrap();
    let finish = frames
        .iter()
        .find(|f| f["type"] == "finish")
        .cloned()
        .unwrap_or_else(|| panic!("no finish frame: {frames:?}"));
    Turn { frames, finish }
}

/// The answer to the one question a child asks, in the resume format.
fn answer(to: &str) -> String {
    format!("Q[pregunta_hijo]: {to}\nA[pregunta_hijo]: sí")
}

/// `(status, all_outputs)` of every child run of the chat (a row with a
/// parent), oldest first.
async fn children(pool: &sqlx::PgPool, chat: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT status, all_outputs::text FROM dag_runs \
         WHERE agent_session_id = $1 AND parent_session_id IS NOT NULL ORDER BY created_at",
    )
    .bind(chat)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The status of each child run whose outputs name `agent` (a session id is
/// hex, so it never spells an agent's name).
fn status_of<'a>(rows: &'a [(String, String)], agent: &str) -> Vec<&'a str> {
    rows.iter()
        .filter(|(_, outputs)| outputs.contains(agent))
        .map(|(status, _)| status.as_str())
        .collect()
}

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    ColmenaEngine::new(EngineConfig::from_env().await.unwrap())
        .await
        .unwrap()
}

/// A: beta, called first, takes `SLOW` to finish; alfa asks at once. The
/// question kept is `Run#1`, so its scope on resume is not the default k = 0.
const TURNS_A: Turns = &[(
    "turno 1",
    &[
        ("call_beta", "beta", "beta: terminá lento"),
        ("call_alfa", "alfa", "alfa: preguntá"),
    ],
)];

const ALFA_ASKS: &str = "¿alfa: seguimos?";

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_question_waits_for_its_group_and_the_resume_answers_it_under_its_scope() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("a");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script(TURNS_A)));
    let _guard = OverrideGuard::install(model.clone());

    let first = turn(&eng, "turno 1", None, &chat, "a1").await;
    first.assert_suspended_on(ALFA_ASKS, "call_alfa");
    // The turn waited for the group: alfa asked, beta finished, and only then
    // did the turn suspend. Beta's result reached the stream.
    let (asked, _) = first.one("subgraph-tool-input-available", "ask_alfa");
    let (done, beta) = first.one("tool-output-available", "call_beta");
    assert!(
        asked < done && done < first.finish_at(),
        "{:?}",
        first.frames
    );
    assert_eq!(beta["childScope"], "Run#0", "{beta}");
    assert_eq!(beta["output"]["hijo"]["result"], "beta: hecho", "{beta}");
    // The question has no result yet, so no Finish frame.
    assert!(first.all("tool-output-available", "call_alfa").is_empty());
    let (_, start) = first.one("tool-input-available", "call_alfa");
    let scope = start["childScope"].as_str().unwrap().to_string();
    assert_eq!(scope, "Run#1");

    let rows = children(&pool, &chat).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(status_of(&rows, "alfa"), ["SUSPENDED"], "{rows:?}");
    assert_eq!(status_of(&rows, "beta"), ["COMPLETED"], "{rows:?}");

    let second = turn(&eng, "turno 1", Some(&answer(ALFA_ASKS)), &chat, "a2").await;
    second.assert_done();
    // The resumed child streams under the scope its call started with.
    let paths = second.child_paths();
    assert!(!paths.is_empty(), "the resumed child streamed nothing");
    let under = format!("agent>{scope}>");
    assert!(
        paths.iter().all(|p| format!("{p}>").starts_with(&under)),
        "{paths:?}"
    );
    let rows = children(&pool, &chat).await;
    assert_eq!(status_of(&rows, "alfa"), ["COMPLETED"], "{rows:?}");
    assert_eq!(status_of(&rows, "beta"), ["COMPLETED"], "{rows:?}");

    cleanup(&pool, &chat).await;
    eng.shutdown().await;
}
