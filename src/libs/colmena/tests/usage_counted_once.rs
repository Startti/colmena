//! Every provider call is billed exactly once.
//!
//! Hosts bill from `usage-summary` (and show `finish.usage`), so a call that
//! reaches those totals twice charges the customer twice. A streamed
//! `llm_call` used to do exactly that: each call's `Usage` stream part became
//! an `LlmUsage` event, and the node then emitted the run's cumulative usage
//! again at the end, so `usage-summary` read 2x what `node-end`'s
//! `extra_info.usage` said.
//!
//! These tests drive the real run loop and the real `llm_call`/`subgraph`
//! nodes (no database: `DagRunUseCase::new(registry, None)`) against a model
//! that reports a distinct usage on each call, and assert, through the
//! `SseMapper`, that `usage-summary`, `subgraph-usage-summary`, `finish.usage`
//! and `extra_info.usage` all equal the sum of what the provider reported.

use async_trait::async_trait;
use colmena::dag_engine::application::run_use_case::DagRunUseCase;
use colmena::dag_engine::domain::error::DagError;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::state::{DagPhaseSummary, DagTask, DagTaskMemoryRepository};
use colmena::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
use colmena::dag_engine::infrastructure::registry::HashMapNodeRegistry;
use colmena::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::domain::{
    FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk,
    LlmStreamPart, LlmUsage, MessageRole, ToolCall, ToolCallChunk,
};
use colmena::llm::infrastructure::{ConversationRepositoryFactory, OverrideGuard};
use colmena::storage::infrastructure::LocalCacheStorageAdapter;
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::{Arc, Mutex};

/// A model that calls `add` (or, offered no `add`, `Sub`) until the request
/// carries `tool_turns` tool results, then answers `answer`; offered neither,
/// it answers. Call `k` (from 0) reports a usage of its
/// own, so a call counted twice, or not at all, changes every total.
struct UsageModel {
    tool_turns: usize,
    answer: &'static str,
    /// The model each call asked for, and the usage it reported.
    reported: Mutex<Vec<(String, LlmUsage)>>,
}

impl UsageModel {
    fn new(tool_turns: usize, answer: &'static str) -> Arc<Self> {
        let reported = Mutex::new(Vec::new());
        Arc::new(Self {
            tool_turns,
            answer,
            reported,
        })
    }

    /// The usage of the next call, `request`, recorded as reported.
    fn next_usage(&self, request: &LlmRequest) -> LlmUsage {
        let mut reported = self.reported.lock().unwrap();
        let k = reported.len() as u32;
        let usage = LlmUsage::new(1000 + 100 * k, 10 + k)
            .with_thinking_tokens(7)
            .with_cache_read_tokens(50 + k)
            .with_cache_write_tokens(3);
        let model = request.config().provider().model().to_string();
        reported.push((model, usage.clone()));
        usage
    }

    /// `(calls, sum of their usage)`: what the provider bills.
    fn billed(&self) -> (usize, LlmUsage) {
        self.billed_if(|_| true)
    }

    /// `billed`, of the calls that asked for `model` only.
    fn billed_for(&self, model: &str) -> (usize, LlmUsage) {
        self.billed_if(|m| m == model)
    }

    fn billed_if(&self, keep: impl Fn(&str) -> bool) -> (usize, LlmUsage) {
        let reported = self.reported.lock().unwrap();
        let mut sum = LlmUsage::default();
        let kept: Vec<_> = reported.iter().filter(|(m, _)| keep(m)).collect();
        kept.iter().for_each(|(_, u)| sum.add(u));
        (kept.len(), sum)
    }

    /// `Some((id, tool, args))` for a tool call, `None` for the answer.
    fn decide(&self, request: &LlmRequest) -> Option<(String, String, String)> {
        let offers = |name: &str| {
            request
                .tools()
                .unwrap_or(&[])
                .iter()
                .any(|t| t.name == name)
        };
        let messages = request.messages().iter();
        let done = messages.filter(|m| m.role() == &MessageRole::Tool).count();
        let (tool, args) = if offers("add") {
            ("add", r#"{"a":1,"b":2}"#.to_string())
        } else if offers("Sub") {
            ("Sub", format!(r#"{{"prompt":"task {done}"}}"#))
        } else {
            return None;
        };
        (done < self.tool_turns).then(|| (format!("call_{done}"), tool.into(), args))
    }
}

#[async_trait]
impl LlmRepository for UsageModel {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let (id, provider) = (request.id().clone(), request.config().provider().clone());
        let response = match self.decide(&request) {
            Some((call, tool, args)) => LlmResponse::new(id, String::new(), provider)?
                .with_tool_calls(vec![ToolCall::new(call, FunctionCall::new(tool, args))]),
            None => LlmResponse::new(id, self.answer.into(), provider)?,
        };
        Ok(response.with_usage(self.next_usage(&request)))
    }

    /// An interim cumulative `Usage` part (as a provider with continuous
    /// usage stats streams), the answer, then the call's final `Usage`. Only
    /// the final one is billed.
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let first = match self.decide(&request) {
            Some((id, name, args_chunk)) => LlmStreamPart::ToolCallChunk(ToolCallChunk {
                index: 0,
                id,
                name,
                args_chunk,
                provider_signature: None,
            }),
            None => LlmStreamPart::Content(self.answer.into()),
        };
        let interim = LlmStreamPart::Usage(LlmUsage::new(1, 1));
        let last = LlmStreamPart::Usage(self.next_usage(&request));
        let (id, provider) = (request.id(), request.config().provider());
        let chunks: Vec<Result<LlmStreamChunk, LlmError>> = [interim, first, last]
            .into_iter()
            .enumerate()
            .map(|(i, p)| Ok(LlmStreamChunk::new(id.clone(), p, provider.clone(), i == 2)))
            .collect();
        Ok(Box::pin(futures::stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "usage-model"
    }
}

/// The registry wants a task memory (`reactor`); these graphs never touch it.
struct NoTaskMemory;

#[async_trait]
impl DagTaskMemoryRepository for NoTaskMemory {
    async fn add_task(&self, _: &DagTask) -> Result<(), DagError> {
        Ok(())
    }
    async fn update_task_result(&self, _: &str, _: Value) -> Result<(), DagError> {
        Ok(())
    }
    async fn get_tasks_for_run(&self, _: &str) -> Result<Vec<DagTask>, DagError> {
        Ok(vec![])
    }
    async fn get_first_uncompleted_task(&self, _: &str) -> Result<Option<DagTask>, DagError> {
        Ok(None)
    }
    async fn delete_task(&self, _: &str) -> Result<(), DagError> {
        Ok(())
    }
    async fn clear_tasks_for_run(&self, _: &str) -> Result<(), DagError> {
        Ok(())
    }
    async fn get_current_phase(&self, _: &str) -> Result<Option<i32>, DagError> {
        Ok(None)
    }
    async fn get_uncompleted_tasks_for_phase(
        &self,
        _: &str,
        _: i32,
    ) -> Result<Vec<DagTask>, DagError> {
        Ok(vec![])
    }
    async fn save_phase_summary(&self, _: &str, _: i32, _: &str) -> Result<(), DagError> {
        Ok(())
    }
    async fn get_phase_summaries(&self, _: &str) -> Result<Vec<DagPhaseSummary>, DagError> {
        Ok(vec![])
    }
}

fn agent(stream: bool) -> Value {
    json!({ "nodes": { "agent": { "type": "llm_call", "config": {
        "provider": "mock", "api_key": "unused", "model": "usage-model",
        "prompt": "add 1 and 2", "stream": stream, "enabled_tools": ["add"]
    } } }, "edges": [] })
}

/// Runs `graph` through the real run loop and maps every event to SSE frames.
async fn run(graph: Value) -> Vec<Value> {
    run_in(graph, None).await
}

/// `run`, as a turn of the conversation `agent_session_id` when given, with
/// an in-process store for attachment bytes.
async fn run_in(graph: Value, agent_session_id: Option<&str>) -> Vec<Value> {
    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let registry = HashMapNodeRegistry::new_with_secure_values(
        Arc::new(ConversationRepositoryFactory::new(pools.clone())),
        Arc::new(SqlPortFactory::new(pools)),
        Some(Arc::new(NoTaskMemory)),
        None,
        Some(Arc::new(LocalCacheStorageAdapter::new())),
        None,
        None,
    );
    let use_case = DagRunUseCase::new(registry.clone(), None);
    registry.set_subgraph_executor(Arc::new(use_case.clone()));
    registry.set_foreach_registry(registry.clone());

    let graph: Graph = serde_json::from_value(graph).unwrap();
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let session = agent_session_id.map(str::to_string);
    let mut stream =
        Box::pin(use_case.execute_stream(graph, None, None, true, None, session, None));
    while let Some(event) = stream.next().await {
        frames.extend(mapper.map(&event.expect("the run must not fail")));
    }
    frames
}

fn one<'a>(frames: &'a [Value], kind: &str) -> &'a Value {
    let found: Vec<&Value> = frames.iter().filter(|f| f["type"] == kind).collect();
    assert_eq!(found.len(), 1, "exactly one {kind}: {frames:#?}");
    found[0]
}

fn usage(value: &Value) -> LlmUsage {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{e}: {value:#}"))
}

/// The `agent` row of a `usage-summary`/`subgraph-usage-summary`.
fn row(frames: &[Value], kind: &str) -> LlmUsage {
    usage(entry(frames, kind, "agent"))
}

/// The `node_id` row of a `usage-summary`/`subgraph-usage-summary`.
fn entry<'a>(frames: &'a [Value], kind: &str, node_id: &str) -> &'a Value {
    let rows = one(frames, kind)["nodes"].as_array().unwrap().iter();
    let found: Vec<&Value> = rows.filter(|r| r["node_id"] == node_id).collect();
    assert_eq!(found.len(), 1, "one {kind} row for {node_id}: {frames:#?}");
    found[0]
}

/// `finish.usage`, in the field names of `LlmUsage`.
fn finish(frames: &[Value]) -> LlmUsage {
    let u = &one(frames, "finish")["usage"];
    usage(&json!({
        "prompt_tokens": u["promptTokens"], "completion_tokens": u["completionTokens"],
        "thinking_tokens": u["thinkingTokens"], "cache_read_tokens": u["cacheReadTokens"],
        "cache_write_tokens": u["cacheWriteTokens"], "total_tokens": u["totalTokens"]
    }))
}

/// `extra_info.usage` of the agent's `kind` frame (`node-end`/`subgraph-node-end`).
fn node_end(frames: &[Value], kind: &str) -> LlmUsage {
    let end = frames
        .iter()
        .find(|f| f["type"] == kind && f["node_id"] == "agent");
    usage(&end.unwrap_or_else(|| panic!("no {kind}: {frames:#?}"))["output"]["extra_info"]["usage"])
}

/// `llm_call` with 3 ReAct tool turns and with a single call, streamed or
/// not: `usage-summary`, `finish.usage` and `extra_info.usage` all equal what
/// the provider reported. Before the fix a streamed run billed 2x.
#[tokio::test]
#[serial]
async fn llm_call_is_billed_once() {
    for (stream, tool_turns) in [(true, 3), (false, 3), (true, 0), (false, 0)] {
        let model = UsageModel::new(tool_turns, "done");
        let _guard = OverrideGuard::install(model.clone());
        let frames = run(agent(stream)).await;
        let (calls, billed) = model.billed();
        let case = format!("stream={stream} tool_turns={tool_turns}");
        assert_eq!(calls, tool_turns + 1, "{case}: one call per turn");
        assert_eq!(
            row(&frames, "usage-summary"),
            billed,
            "{case}: usage-summary"
        );
        assert_eq!(finish(&frames), billed, "{case}: finish.usage");
        assert_eq!(
            node_end(&frames, "node-end"),
            billed,
            "{case}: extra_info.usage"
        );
    }
}

/// The same agent inside a `subgraph`: the child's summary, the parent's
/// (which re-counts the child's `LlmUsage` events) and `finish` match.
#[tokio::test]
#[serial]
async fn child_graph_is_billed_once() {
    for stream in [true, false] {
        let model = UsageModel::new(2, "done");
        let _guard = OverrideGuard::install(model.clone());
        let sub = json!({ "type": "subgraph", "config": { "child_graph_inline": agent(stream) } });
        let frames = run(json!({ "nodes": { "sub": sub }, "edges": [] })).await;
        let (calls, billed) = model.billed();
        assert_eq!(calls, 3, "stream={stream}");
        assert_eq!(
            row(&frames, "subgraph-usage-summary"),
            billed,
            "stream={stream}"
        );
        assert_eq!(row(&frames, "usage-summary"), billed, "stream={stream}");
        assert_eq!(finish(&frames), billed, "stream={stream}");
        assert_eq!(
            node_end(&frames, "subgraph-node-end"),
            billed,
            "stream={stream}"
        );
    }
}

/// `critic`, `planner` and `reactor` make one call each, billed once whether
/// they stream or not. Not streaming (their default, and how an
/// `orchestrator` runs them) it used not to be billed at all.
#[tokio::test]
#[serial]
async fn review_nodes_are_billed_once() {
    const REVIEW: &str = r#"{"task_ok":true,"response":"ok","tasks":[],"suspend":false}"#;
    for kind in ["critic", "planner", "reactor"] {
        for streaming in [true, false] {
            let model = UsageModel::new(0, REVIEW);
            let _guard = OverrideGuard::install(model.clone());
            let frames = run(json!({ "nodes": { "agent": { "type": kind, "config": {
                "provider": "openai", "api_key": "unused",
                "texts": { "task": "review this" }, "streaming": streaming
            } } }, "edges": [] }))
            .await;
            let (calls, billed) = model.billed();
            let case = format!("{kind} streaming={streaming}");
            assert_eq!(calls, 1, "{case}");
            assert_eq!(
                row(&frames, "usage-summary"),
                billed,
                "{case}: usage-summary"
            );
            assert_eq!(finish(&frames), billed, "{case}: finish.usage");
        }
    }
}

/// An answer long enough (over 250 characters) that a later turn compacts
/// it with the cheap-model summarizer instead of quoting it.
const LONG_ANSWER: &str = "The sum of one and two is three. Adding the two numbers \
    together, one plus two, gives three, which is the result that was asked for. \
    Nothing else was needed to answer: no tool, no lookup, no assumption. Three is \
    the final answer, and it was checked twice before it was written down here.";

/// An agent with a conversation that persists across runs (`sqlite`).
fn remembering_agent(db: &std::path::Path, extra: Value) -> Value {
    let mut config = json!({
        "provider": "openai", "api_key": "unused", "model": "usage-model",
        "prompt": "add 1 and 2", "stream": false, "provider_key_id": "pk-1",
        "connection_url": format!("sqlite://{}", db.display()),
    });
    config
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({ "nodes": { "agent": { "type": "llm_call", "config": config } }, "edges": [] })
}

/// The second turn of a conversation compacts the first turn's long answer
/// with a call of its own to the provider: that call is billed too, on its
/// own row with its own model. It used not to reach `usage-summary` at all.
#[tokio::test]
#[serial]
async fn history_compaction_is_billed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("memory.db");
    std::fs::File::create(&db).unwrap();
    let graph = remembering_agent(&db, json!({}));

    let first = UsageModel::new(0, LONG_ANSWER);
    {
        let _guard = OverrideGuard::install(first.clone());
        run_in(graph.clone(), Some("conversation")).await;
    }
    assert_eq!(first.billed().0, 1, "the first turn makes one call");

    let model = UsageModel::new(0, "three");
    let _guard = OverrideGuard::install(model.clone());
    let frames = run_in(graph, Some("conversation")).await;
    assert_eq!(
        model.billed().0,
        2,
        "the summary of the long answer, then the answer"
    );
    assert_side_call_billed(&model, &frames, "history_compaction");
}

/// What a run with one side call of `purpose` bills: the node's row is its
/// own model's calls (and equals `extra_info.usage`); the side call has a row
/// of its own, `agent::<purpose>`, with the model it asked for (the cheap
/// tier of the node's provider); `finish.usage` is both.
fn assert_side_call_billed(model: &UsageModel, frames: &[Value], purpose: &str) {
    let (calls, own) = model.billed_for("usage-model");
    assert_eq!(calls, 1, "{purpose}: one answer");
    assert_eq!(row(frames, "usage-summary"), own, "{purpose}: agent row");
    assert_eq!(node_end(frames, "node-end"), own, "{purpose}: extra_info");
    let side = entry(frames, "usage-summary", &format!("agent::{purpose}"));
    let (calls, side_usage) = model.billed_for("gpt-4o-mini");
    assert_eq!(calls, 1, "{purpose}: one side call, with the cheap tier");
    assert_eq!(usage(side), side_usage, "{purpose}: side row");
    assert_eq!(side["model"], "gpt-4o-mini", "{purpose}: side row model");
    assert_eq!(side["provider"], "openai", "{purpose}: side row provider");
    assert_eq!(side["provider_key_id"], "pk-1", "{purpose}: the node's key");
    assert_eq!(finish(frames), model.billed().1, "{purpose}: finish.usage");
}

/// A text attachment without a description is summarized by the provider
/// while the node answers: that call is billed too, on its own row with its
/// own model. It used not to reach `usage-summary` at all.
#[tokio::test]
#[serial]
async fn attachment_summary_is_billed() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("memory.db");
    std::fs::File::create(&db).unwrap();
    let text = "Quarterly report. Revenue grew 12% over the previous quarter.";
    let graph = remembering_agent(
        &db,
        json!({ "files": [{
            "id": "doc-1", "mime_type": "text/plain", "filename": "report.txt",
            "data": STANDARD.encode(text)
        }] }),
    );

    let model = UsageModel::new(0, "three");
    let _guard = OverrideGuard::install(model.clone());
    let frames = run_in(graph, Some("conversation")).await;
    assert_eq!(
        model.billed().0,
        2,
        "the attachment's summary and the answer"
    );
    assert_side_call_billed(&model, &frames, "attachment_summary");
}

/// Two attachments summarized inside a `subgraph`: the child's summary and
/// the parent's (which re-counts the child's events) give both calls one row
/// of their own, with their model.
#[tokio::test]
#[serial]
async fn child_side_call_keeps_its_model() {
    let (_dir, db) = memory_db();
    let files = json!({ "files": [text_file("d1"), text_file("d2")] });
    let child = remembering_agent(&db, files);
    let sub = json!({ "type": "subgraph", "config": { "child_graph_inline": child } });
    let model = UsageModel::new(0, "three");
    let _guard = OverrideGuard::install(model.clone());
    let frames = run_in(json!({ "nodes": { "sub": sub }, "edges": [] }), Some("c")).await;
    let (calls, side_usage) = model.billed_for("gpt-4o-mini");
    assert_eq!(calls, 2, "two summaries");
    for kind in ["subgraph-usage-summary", "usage-summary"] {
        let side = entry(&frames, kind, "agent::attachment_summary");
        assert_eq!(usage(side), side_usage, "{kind}");
        assert_eq!(side["model"], "gpt-4o-mini", "{kind}");
    }
}

/// A text attachment `id` with no description.
fn text_file(id: &str) -> Value {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let data = STANDARD.encode("Revenue grew 12%.");
    json!({ "id": id, "mime_type": "text/plain", "filename": "a.txt", "data": data })
}

/// A memory file for one test.
fn memory_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("memory.db");
    std::fs::File::create(&db).unwrap();
    (dir, db)
}

/// `finish.usage` and the sum of every `usage-summary` row both equal what
/// the provider reported, and one row, `*::<purpose>`, is the cheap model's.
fn assert_all_billed(model: &UsageModel, frames: &[Value], purpose: &str) {
    let rows = one(frames, "usage-summary")["nodes"].as_array().unwrap();
    let mut sum = LlmUsage::default();
    rows.iter().for_each(|r| sum.add(&usage(r)));
    assert_eq!(sum, model.billed().1, "usage-summary rows: {rows:#?}");
    assert_eq!(finish(frames), model.billed().1, "finish.usage");
    let suffix = format!("::{purpose}");
    let is_side = |r: &&Value| r["node_id"].as_str().unwrap().ends_with(&suffix);
    let side: Vec<&Value> = rows.iter().filter(is_side).collect();
    assert_eq!(side.len(), 1, "one {purpose} row: {rows:#?}");
    let cheap = model.billed_for("gpt-4o-mini").1;
    assert_eq!(
        (usage(side[0]), &side[0]["model"]),
        (cheap, &json!("gpt-4o-mini"))
    );
}

/// The `node_schema` of an `llm_call` that remembers (`sqlite`), as a tool
/// or a `for_each` target: only `prompt` is the caller's.
fn remembering_schema(db: &std::path::Path) -> Value {
    let url = format!("sqlite://{}", db.display());
    json!({
        "provider": { "fixed": "openai" }, "api_key": { "fixed": "unused" },
        "model": { "fixed": "usage-model" }, "stream": { "fixed": false },
        "connection_url": { "fixed": url },
        "prompt": { "type": "string", "required": true, "description": "p" }
    })
}

/// An `llm_call` dispatched as a tool, called twice: the second call compacts
/// the first one's long answer. That side call is billed under the tool.
#[tokio::test]
#[serial]
async fn a_side_call_inside_a_tool_is_billed() {
    let (_dir, db) = memory_db();
    let sub = json!({ "name": "Sub", "description": "d", "node_type": "llm_call",
        "memory_mode": "persistent", "node_schema": remembering_schema(&db) });
    let graph = json!({ "nodes": { "agent": { "type": "llm_call", "config": {
        "provider": "openai", "api_key": "unused", "model": "usage-model",
        "prompt": "go", "stream": false, "enabled_tools": ["Sub"],
        "tool_configurations": { "Sub": sub }
    } } }, "edges": [] });
    let model = UsageModel::new(2, LONG_ANSWER);
    let _guard = OverrideGuard::install(model.clone());
    let frames = run_in(graph, Some("c")).await;
    assert_eq!(model.billed().0, 6, "agent x3, Sub x2, one summary");
    assert_all_billed(&model, &frames, "history_compaction");
}

/// Two `for_each` rows share a conversation: the second compacts the first's
/// long answer. That side call is billed under its row.
#[tokio::test]
#[serial]
async fn a_side_call_in_a_for_each_row_is_billed() {
    let (_dir, db) = memory_db();
    let graph = json!({ "nodes": { "fe": { "type": "for_each", "config": {
        "items": [{ "prompt": "a" }, { "prompt": "b" }], "concurrency": 1,
        "target": { "node_type": "llm_call", "node_schema": remembering_schema(&db) }
    } } }, "edges": [] });
    let model = UsageModel::new(0, LONG_ANSWER);
    let _guard = OverrideGuard::install(model.clone());
    let frames = run_in(graph, Some("c")).await;
    assert_eq!(model.billed().0, 3, "two rows, one summary");
    assert_all_billed(&model, &frames, "history_compaction");
}
