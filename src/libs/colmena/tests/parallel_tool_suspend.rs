//! E2E of «esperar y preguntar»: one question per turn inside a group of
//! `parallel` tool calls, through a real `ColmenaEngine` and Postgres.
//!
//! The parent agent calls `Run` — `parallel: true`, `dynamic` memory with the
//! thread fixed to `${agentId}`, as ADP's Run My Agent — once per agent in ONE
//! message, so the calls form one group of two chains. Each child is an agent
//! with memory that can ask the user through `Preguntar` (a `suspend`). One
//! scripted model answers every agent, by who calls it (the system prompt says
//! PADRE or HIJO); a child's task says what it does ("preguntá" asks,
//! "terminá" answers, "lento" waits first), and every request is kept.
//!
//! - A: one child asks, its sibling (called first) finishes: the turn waits
//!   for the group, then suspends on the question; the resume answers it
//!   under the same `Run#<k>`, with k = 1.
//! - B: both children ask: the first in the model's order is kept, the other
//!   child's row is FAILED and the model reads why.
//! - C: after B, a fresh turn runs the closed child again, on the same memory
//!   thread: its request carries no open tool call id.
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
use colmena::llm::domain::{LlmMessage, MessageRole};
use colmena::llm::infrastructure::OverrideGuard;
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;

/// What the model reads for a question the turn did not keep.
const CLOSED: &str = include_str!("../text/prompts/agent_loop/closed_by_parallel_suspend.md");
/// What a fresh run answers for a call its thread left open.
const ABANDONED: &str = include_str!("../text/prompts/agent_loop/abandoned_tool_call.md");

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

    /// The final text of the one child that ran in this turn.
    fn child_result(&self) -> &str {
        let ends: Vec<&Value> = self
            .frames
            .iter()
            .filter(|f| f["type"] == "subgraph-node-end" && f["node_id"] == "hijo")
            .collect();
        assert_eq!(ends.len(), 1, "one child ran: {ends:?}");
        ends[0]["output"]["result"].as_str().unwrap()
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

/// What the human answers to a child's question.
const HUMAN: &str = "sí";

/// The answer to the one question a child asks, in the resume format.
fn answer(to: &str) -> String {
    format!("Q[pregunta_hijo]: {to}\nA[pregunta_hijo]: {HUMAN}")
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

async fn suspended_children(pool: &sqlx::PgPool, chat: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM dag_runs WHERE agent_session_id = $1 \
         AND parent_session_id IS NOT NULL AND status = 'SUSPENDED'",
    )
    .bind(chat)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// `(tool_call_id, content)` of every `tool` message of the thread of
/// `node_id` (`agent` is the parent; a child is `tool/Run/<agentId>/hijo`).
async fn tool_messages(pool: &sqlx::PgPool, chat: &str, node_id: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT tool_call_id, content FROM llm_node_history \
         WHERE agent_session_id = $1 AND node_id = $2 AND role = 'tool' ORDER BY created_at",
    )
    .bind(chat)
    .bind(node_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Every id an assistant message declares that no `tool` message answers:
/// Anthropic and OpenAI reject a request that carries one with a 400.
fn open_ids(messages: &[LlmMessage]) -> Vec<String> {
    let answered: Vec<&str> = messages.iter().filter_map(|m| m.tool_call_id()).collect();
    messages
        .iter()
        .filter_map(|m| m.tool_calls())
        .flatten()
        .map(|c| c.id.clone())
        .filter(|id| !answered.contains(&id.as_str()))
        .collect()
}

/// The content of every `tool` message that answers `id`.
fn answers_to(messages: &[LlmMessage], id: &str) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.role() == &MessageRole::Tool && m.tool_call_id() == Some(id))
        .map(|m| m.content().to_string())
        .collect()
}

/// The call ids of the assistant message that makes `call`, and the ids of
/// the `tool` messages right after it, in the order the model received them.
fn calls_and_results(messages: &[LlmMessage], call: &str) -> (Vec<String>, Vec<String>) {
    let at = messages
        .iter()
        .position(|m| {
            m.tool_calls()
                .is_some_and(|c| c.iter().any(|c| c.id == call))
        })
        .unwrap_or_else(|| panic!("no message makes {call}"));
    let calls = messages[at].tool_calls().unwrap().iter();
    let results = messages[at + 1..]
        .iter()
        .take_while(|m| m.role() == &MessageRole::Tool)
        .filter_map(|m| m.tool_call_id());
    (
        calls.map(|c| c.id.clone()).collect(),
        results.map(str::to_string).collect(),
    )
}

/// The child's final text in the result its parent read.
fn child_said(result: &str) -> String {
    let result: Value = serde_json::from_str(result).unwrap_or_else(|_| panic!("{result}"));
    result["hijo"]["result"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The last request the parent's model received.
fn last_parent_request(model: &CallerModel) -> Request {
    model
        .seen()
        .into_iter()
        .rev()
        .find(|r| r.system.contains("PADRE"))
        .unwrap()
}

/// The requests a child's model received on the run whose prompt is `task`.
fn child_requests(model: &CallerModel, task: &str) -> Vec<Request> {
    model
        .seen()
        .into_iter()
        .filter(|r| r.system.contains("HIJO") && r.last_user() == task)
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

/// B and C: both ask, beta first (alfa waits `SLOW`); in C a third turn runs
/// beta again.
const TURNS_BC: Turns = &[
    (
        "turno 1",
        &[
            ("call_alfa", "alfa", "alfa: preguntá lento"),
            ("call_beta", "beta", "beta: preguntá"),
        ],
    ),
    ("turno 3", &[("call_beta_2", "beta", "beta: terminá")]),
];

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
    // The resumed child streams under the scope its call started with, and
    // it read the human's answer.
    let paths = second.child_paths();
    assert!(!paths.is_empty(), "the resumed child streamed nothing");
    let under = format!("agent>{scope}>");
    assert!(
        paths.iter().all(|p| format!("{p}>").starts_with(&under)),
        "{paths:?}"
    );
    let said = second.child_result();
    assert!(
        said.starts_with("alfa: hecho, con") && said.contains(HUMAN),
        "{said}"
    );
    let rows = children(&pool, &chat).await;
    assert_eq!(status_of(&rows, "alfa"), ["COMPLETED"], "{rows:?}");
    assert_eq!(status_of(&rows, "beta"), ["COMPLETED"], "{rows:?}");

    // The parent read both results, and nothing was left open.
    let last = last_parent_request(&model);
    assert_eq!(open_ids(&last.messages), Vec::<String>::new());
    assert_eq!(
        child_said(&answers_to(&last.messages, "call_alfa")[0]),
        said
    );
    assert!(answers_to(&last.messages, "call_beta")[0].contains("beta: hecho"));

    cleanup(&pool, &chat).await;
    eng.shutdown().await;
}

/// Turns 1 and 2 of B, for C: both children ask, beta first, and the turn
/// keeps alfa's question. Turn 2 answers it.
async fn two_questions_then_the_answer(eng: &ColmenaEngine, chat: &str, sse: &str) {
    let first = turn(eng, "turno 1", None, chat, &format!("{sse}1")).await;
    first.assert_suspended_on(ALFA_ASKS, "call_alfa");
    let answer = answer(ALFA_ASKS);
    let second = turn(eng, "turno 1", Some(&answer), chat, &format!("{sse}2")).await;
    second.assert_done();
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn two_questions_in_a_group_keep_the_first_and_close_the_other_child() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("b");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script(TURNS_BC)));
    let _guard = OverrideGuard::install(model.clone());

    let first = turn(&eng, "turno 1", None, &chat, "b1").await;
    // The first question in the model's order leads, though beta asked first.
    first.assert_suspended_on(ALFA_ASKS, "call_alfa");
    let (beta_asked, _) = first.one("subgraph-tool-input-available", "ask_beta");
    let (alfa_asked, _) = first.one("subgraph-tool-input-available", "ask_alfa");
    assert!(beta_asked < alfa_asked, "beta did not ask first");
    // The other question is closed: the model reads why, as its result.
    let (closed_at, closed) = first.one("tool-output-available", "call_beta");
    assert_eq!(closed["output"], CLOSED.trim(), "{closed}");
    assert_eq!(closed["childScope"], "Run#1", "{closed}");
    assert!(closed_at < first.finish_at());
    assert!(first.all("tool-output-available", "call_alfa").is_empty());
    assert_eq!(
        tool_messages(&pool, &chat, "agent").await,
        [("call_beta".to_string(), CLOSED.trim().to_string())]
    );
    // Its child's row is closed too: the parent keeps one suspended child,
    // the one the resume will find.
    let rows = children(&pool, &chat).await;
    assert_eq!(suspended_children(&pool, &chat).await, 1, "{rows:?}");
    assert_eq!(status_of(&rows, "alfa"), ["SUSPENDED"], "{rows:?}");
    assert_eq!(status_of(&rows, "beta"), ["FAILED"], "{rows:?}");

    let second = turn(&eng, "turno 1", Some(&answer(ALFA_ASKS)), &chat, "b2").await;
    second.assert_done();
    let paths = second.child_paths();
    assert!(!paths.is_empty(), "the resumed child streamed nothing");
    assert!(
        paths
            .iter()
            .all(|p| format!("{p}>").starts_with("agent>Run#0>")),
        "{paths:?}"
    );
    let said = second.child_result();
    assert!(
        said.starts_with("alfa: hecho, con") && said.contains(HUMAN),
        "{said}"
    );
    let rows = children(&pool, &chat).await;
    assert_eq!(status_of(&rows, "alfa"), ["COMPLETED"], "{rows:?}");
    assert_eq!(status_of(&rows, "beta"), ["FAILED"], "{rows:?}");
    let last = last_parent_request(&model);
    assert_eq!(open_ids(&last.messages), Vec::<String>::new());
    assert_eq!(answers_to(&last.messages, "call_beta"), [CLOSED.trim()]);
    assert_eq!(
        child_said(&answers_to(&last.messages, "call_alfa")[0]),
        said
    );
    // History holds beta's result first (written when the group closed) and
    // alfa's last (written on resume). The model reads them in the order of
    // its calls: Gemini pairs a result with its call by position.
    let stored = tool_messages(&pool, &chat, "agent").await;
    let stored: Vec<&str> = stored.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(stored, ["call_beta", "call_alfa"]);
    let (calls, results) = calls_and_results(&last.messages, "call_alfa");
    assert_eq!(calls, ["call_alfa", "call_beta"]);
    assert_eq!(results, calls, "results out of the order of the calls");

    cleanup(&pool, &chat).await;
    eng.shutdown().await;
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn the_closed_child_runs_again_on_its_thread_with_no_open_call() {
    let eng = engine().await;
    let pool = pool().await;
    let chat = unique_chat("c");
    cleanup(&pool, &chat).await;
    let model = Arc::new(CallerModel::new(script(TURNS_BC)));
    let _guard = OverrideGuard::install(model.clone());
    two_questions_then_the_answer(&eng, &chat, "c").await;
    // Beta's thread ends on its question, which no one will answer.
    let beta_thread = "tool/Run/beta/hijo";
    assert_eq!(tool_messages(&pool, &chat, beta_thread).await, []);

    // A fresh turn runs beta again, on the same memory thread.
    let third = turn(&eng, "turno 3", None, &chat, "c3").await;
    third.assert_done();
    let (_, beta) = third.one("tool-output-available", "call_beta_2");
    assert_eq!(beta["output"]["hijo"]["result"], "beta: hecho", "{beta}");

    // What beta's model received: its first turn, its question answered once
    // with the abandoned-call text, then the new task. No id is left open.
    let requests = child_requests(&model, "beta: terminá");
    assert_eq!(requests.len(), 1);
    let sent = &requests[0].messages;
    assert_eq!(
        open_ids(sent),
        Vec::<String>::new(),
        "a 400 on Anthropic/OpenAI"
    );
    assert_eq!(answers_to(sent, "ask_beta"), [ABANDONED.trim()]);
    let turns: Vec<(MessageRole, &str)> = sent
        .iter()
        .filter(|m| m.role() != &MessageRole::System)
        .map(|m| (m.role().clone(), m.tool_call_id().unwrap_or(m.content())))
        .collect();
    assert_eq!(
        turns,
        [
            (MessageRole::User, "beta: preguntá"),
            (MessageRole::Assistant, ""),
            (MessageRole::Tool, "ask_beta"),
            (MessageRole::User, "beta: terminá"),
        ]
    );
    assert_eq!(
        tool_messages(&pool, &chat, beta_thread).await,
        [("ask_beta".to_string(), ABANDONED.trim().to_string())]
    );
    let rows = children(&pool, &chat).await;
    assert_eq!(
        status_of(&rows, "beta"),
        ["FAILED", "COMPLETED"],
        "{rows:?}"
    );
    assert_eq!(
        open_ids(&last_parent_request(&model).messages),
        Vec::<String>::new()
    );

    cleanup(&pool, &chat).await;
    eng.shutdown().await;
}
