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
use futures::StreamExt;
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::{Arc, Mutex};

/// A model that calls `add` until the request carries `tool_turns` tool
/// results, then answers `answer`. Call `k` (from 0) reports a usage of its
/// own, so a call counted twice, or not at all, changes every total.
struct UsageModel {
    tool_turns: usize,
    answer: &'static str,
    reported: Mutex<Vec<LlmUsage>>,
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

    /// The usage of the next call, recorded as reported.
    fn next_usage(&self) -> LlmUsage {
        let mut reported = self.reported.lock().unwrap();
        let k = reported.len() as u32;
        let usage = LlmUsage::new(1000 + 100 * k, 10 + k)
            .with_thinking_tokens(7)
            .with_cache_read_tokens(50 + k)
            .with_cache_write_tokens(3);
        reported.push(usage.clone());
        usage
    }

    /// `(calls, sum of their usage)`: what the provider bills.
    fn billed(&self) -> (usize, LlmUsage) {
        let reported = self.reported.lock().unwrap();
        let mut sum = LlmUsage::default();
        reported.iter().for_each(|u| sum.add(u));
        (reported.len(), sum)
    }

    /// `Some((id, args))` for a tool call, `None` for the answer.
    fn decide(&self, request: &LlmRequest) -> Option<(String, String)> {
        let messages = request.messages().iter();
        let done = messages.filter(|m| m.role() == &MessageRole::Tool).count();
        (done < self.tool_turns).then(|| (format!("call_{done}"), r#"{"a":1,"b":2}"#.into()))
    }
}

#[async_trait]
impl LlmRepository for UsageModel {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let (id, provider) = (request.id().clone(), request.config().provider().clone());
        let response = match self.decide(&request) {
            Some((call, args)) => LlmResponse::new(id, String::new(), provider)?.with_tool_calls(
                vec![ToolCall::new(call, FunctionCall::new("add".into(), args))],
            ),
            None => LlmResponse::new(id, self.answer.into(), provider)?,
        };
        Ok(response.with_usage(self.next_usage()))
    }

    /// The answer, then one `Usage` part, as the providers stream it.
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let first = match self.decide(&request) {
            Some((id, args_chunk)) => LlmStreamPart::ToolCallChunk(ToolCallChunk {
                index: 0,
                id,
                name: "add".into(),
                args_chunk,
                provider_signature: None,
            }),
            None => LlmStreamPart::Content(self.answer.into()),
        };
        let (id, provider) = (request.id(), request.config().provider());
        let chunks: Vec<Result<LlmStreamChunk, LlmError>> = vec![
            Ok(LlmStreamChunk::new(
                id.clone(),
                first,
                provider.clone(),
                false,
            )),
            Ok(LlmStreamChunk::new(
                id.clone(),
                LlmStreamPart::Usage(self.next_usage()),
                provider.clone(),
                true,
            )),
        ];
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
    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let registry = HashMapNodeRegistry::new(
        Arc::new(ConversationRepositoryFactory::new(pools.clone())),
        Arc::new(SqlPortFactory::new(pools)),
        Some(Arc::new(NoTaskMemory)),
    );
    let use_case = DagRunUseCase::new(registry.clone(), None);
    registry.set_subgraph_executor(Arc::new(use_case.clone()));

    let graph: Graph = serde_json::from_value(graph).unwrap();
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let mut stream = Box::pin(use_case.execute_stream(graph, None, None, true, None, None, None));
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
    let rows = one(frames, kind)["nodes"].as_array().unwrap().iter();
    let agent: Vec<&Value> = rows.filter(|r| r["node_id"] == "agent").collect();
    assert_eq!(agent.len(), 1, "one {kind} row for agent: {frames:#?}");
    usage(agent[0])
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
