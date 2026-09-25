//! E2E of a `child_graph_ref` resume through a real `ColmenaEngine`: a stub
//! resolver stands in for the embedder (the CLI has none) and ScriptedAdapter
//! for the model. The stub answers v1 when the child starts and v2 — or
//! `forbidden` — when it resumes. Writes the SSE the CLI would print to
//! `/tmp/colmena_e2e/child_graph_ref_resume_{v2,forbidden}.sse`.
//!
//! Run with:
//!   source .env && cargo test -p colmena_dag_engine --test child_graph_ref_resume -- --ignored --nocapture

use async_trait::async_trait;
use colmena::dag_engine::application::ports::{
    ChildGraphRequest, ChildGraphResolveError, ChildGraphResolverPort, ResolvedChildGraph,
};
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::infrastructure::{OverrideGuard, ScriptedAdapter, ScriptedResponse};
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const ANSWER: &str = "Q[pregunta_sello]: ¿Seguimos?\nA[pregunta_sello]: sí";

/// The child the stub hands out. The top-level marker proves the resolved
/// graph as a whole never reaches a frame (a node's own config does, as for
/// any inline child).
fn child_graph(stamp: &str) -> Value {
    json!({
        "marker": "sk-stub-graph-marker",
        "nodes": {
            "entrada": { "type": "input", "config": {} },
            "pregunta": { "type": "suspend", "config": { "id": "pregunta_sello", "question": "¿Seguimos?" } },
            "sello": { "type": "input", "config": { "data": { "sello": format!("SELLO={stamp}") } } },
            "fin": { "type": "output", "config": {} }
        },
        "edges": [
            { "from": "entrada", "to": "pregunta" },
            { "from": "pregunta", "to": "sello" },
            { "from": "sello", "to": "fin" }
        ]
    })
}

/// Answers each resolve with the next scripted outcome and records the request.
struct StubResolver {
    script: Mutex<VecDeque<Result<&'static str, ChildGraphResolveError>>>,
    requests: Mutex<Vec<ChildGraphRequest>>,
}
impl StubResolver {
    fn new(script: Vec<Result<&'static str, ChildGraphResolveError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::default(),
        })
    }
}
#[async_trait]
impl ChildGraphResolverPort for StubResolver {
    async fn resolve(
        &self,
        req: ChildGraphRequest,
    ) -> Result<ResolvedChildGraph, ChildGraphResolveError> {
        self.requests.lock().unwrap().push(req);
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted resolve");
        next.map(|stamp| ResolvedChildGraph {
            graph: child_graph(stamp),
            display_name: "Agente sello".into(),
        })
    }
}

async fn engine_with(resolver: Arc<StubResolver>) -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let mut cfg = EngineConfig::from_env().await.unwrap();
    cfg.child_graph_resolver = Some(resolver as Arc<dyn ChildGraphResolverPort>);
    ColmenaEngine::new(cfg).await.unwrap()
}

fn parent() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/agents/child_graph_ref_resume.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn unique_chat(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}_{nanos}")
}

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap()
}

/// A shared ADP database requires the `agent_session` row by FK; a standalone
/// Colmena database has no such table. Chats are unique per run.
async fn seed_chat(chat: &str) {
    let pool = pool().await;
    let shared: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'agent_session')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    if shared {
        sqlx::query(r#"INSERT INTO agent_session (id, "updatedAt") VALUES ($1, NOW()) ON CONFLICT (id) DO NOTHING"#)
            .bind(chat)
            .execute(&pool)
            .await
            .unwrap();
    }
}

async fn child_statuses(chat: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT status FROM dag_runs WHERE agent_session_id = $1 AND parent_session_id IS NOT NULL",
    )
    .bind(chat)
    .fetch_all(&pool().await)
    .await
    .unwrap()
}

/// A resumed tool's result never gets an SSE frame of its own — `llm.rs`
/// replays it straight into conversation history via
/// `execute_with_resume_answer` (no `LlmToolCallStart`/`Finish` pair fires on
/// that path, unlike a fresh dispatch). The `CHILD_GRAPH_RESOLVE_FAILED:`
/// text the model actually sees lives here, as the `tool`-role message.
async fn last_tool_message(chat: &str) -> String {
    sqlx::query_scalar(
        "SELECT content FROM llm_node_history WHERE agent_session_id = $1 AND role = 'tool' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(chat)
    .fetch_one(&pool().await)
    .await
    .unwrap()
}

/// One turn; appends its SSE, as the CLI prints it, to `sse`.
async fn turn(
    eng: &ColmenaEngine,
    chat: &str,
    answer: Option<&str>,
    script: Vec<ScriptedResponse>,
    sse: &mut Vec<Value>,
) -> Value {
    let _guard = OverrideGuard::install(Arc::new(ScriptedAdapter::new(script)));
    let mut mapper = SseMapper::new();
    let mut stream = Box::pin(eng.execute_stream(
        parent(),
        None,
        answer.map(str::to_string),
        false,
        None,
        Some(chat.to_string()),
    ));
    let mut finish = Value::Null;
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        sse.extend(mapper.map(&ev));
        if let DagExecutionEvent::GraphFinish { output } = &ev {
            finish = output.clone();
        }
    }
    finish
}

fn write_sse(name: &str, frames: &[Value]) {
    std::fs::create_dir_all("/tmp/colmena_e2e").unwrap();
    let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
    std::fs::write(
        format!("/tmp/colmena_e2e/{name}.sse"),
        body + "data: [DONE]\n\n",
    )
    .unwrap();
}

fn sello_outputs(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter(|f| f["type"] == "subgraph-node-end" && f["node_id"] == "sello")
        .map(|f| f["output"].to_string())
        .collect()
}

fn run_my_agent() -> ScriptedResponse {
    ScriptedResponse::ToolCall {
        id: "call_1".into(),
        tool_name: "Run_My_Agent".into(),
        arguments: json!({ "agentId": "agt_sello", "prompt": "sellá el documento" }),
    }
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_ref_child_resumes_with_the_graph_the_resolver_gives_now() {
    let chat = unique_chat("cgr_resume_v2");
    seed_chat(&chat).await;
    let resolver = StubResolver::new(vec![Ok("v1"), Ok("v2")]);
    let eng = engine_with(resolver.clone()).await;
    let mut sse = Vec::new();

    let first = turn(&eng, &chat, None, vec![run_my_agent()], &mut sse).await;
    assert_eq!(first["__colmena_status"], json!("SUSPENDED"), "{first}");
    turn(
        &eng,
        &chat,
        Some(ANSWER),
        vec![ScriptedResponse::Text("Listo.".into())],
        &mut sse,
    )
    .await;
    write_sse("child_graph_ref_resume_v2", &sse);

    let sellos = sello_outputs(&sse);
    assert_eq!(sellos.len(), 1, "{sellos:?}");
    assert!(
        sellos[0].contains("SELLO=v2"),
        "the child ran the graph resolved on resume: {sellos:?}"
    );
    assert!(
        !serde_json::to_string(&sse)
            .unwrap()
            .contains("sk-stub-graph-marker"),
        "no frame carries the resolved graph"
    );
    let reqs = resolver.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        (&reqs[1].agent_id, &reqs[1].context, &reqs[1].parent_path),
        (&reqs[0].agent_id, &reqs[0].context, &reqs[0].parent_path)
    );
    assert_eq!(child_statuses(&chat).await, vec!["COMPLETED".to_string()]);
    eng.shutdown().await;
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_revoked_ref_fails_on_resume_and_closes_the_child() {
    let chat = unique_chat("cgr_resume_forbidden");
    seed_chat(&chat).await;
    let resolver = StubResolver::new(vec![
        Ok("v1"),
        Err(ChildGraphResolveError::Forbidden(
            "the agent is not in their selector".into(),
        )),
    ]);
    let eng = engine_with(resolver).await;
    let mut sse = Vec::new();

    turn(&eng, &chat, None, vec![run_my_agent()], &mut sse).await;
    let done = turn(
        &eng,
        &chat,
        Some(ANSWER),
        vec![ScriptedResponse::Text("No está disponible.".into())],
        &mut sse,
    )
    .await;
    write_sse("child_graph_ref_resume_forbidden", &sse);

    assert_ne!(
        done["__colmena_status"],
        json!("SUSPENDED"),
        "the parent goes on: {done}"
    );
    let tool_message = last_tool_message(&chat).await;
    assert!(
        tool_message
            .contains("CHILD_GRAPH_RESOLVE_FAILED:forbidden: the agent is not in their selector"),
        "{tool_message}"
    );
    assert!(sello_outputs(&sse).is_empty(), "nothing of the child ran");
    assert_eq!(child_statuses(&chat).await, vec!["FAILED".to_string()]);
    eng.shutdown().await;
}
