//! `node-skipped` reporting contract.
//!
//! The event exists so that a node which never ran is never invisible. That
//! only holds if the frame means what it says, so these tests drive the real
//! run loop (no database required: `DagRunUseCase::new(registry, None)`) and
//! assert the event against what actually executed.
//!
//! The invariant under test: **a `NodeSkipped` frame is emitted if and only if
//! the node produced no output during the run, and at most once per node.**

use async_trait::async_trait;
use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::application::run_use_case::DagRunUseCase;
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use colmena::dag_engine::domain::observer::ExecutionObserver;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

/// Emits whatever its `config.value` holds. With no config it emits an object,
/// which is what the engine treats as "this branch carries data".
struct EmitNode;

#[async_trait]
impl ExecutableNode for EmitNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        Ok(config
            .get("value")
            .cloned()
            .unwrap_or(json!({ "ok": true })))
    }
    fn schema(&self) -> Value {
        json!({})
    }
}

/// Emits `null` — the deliberate "skip stub" that stops a branch.
struct NullNode;

#[async_trait]
impl ExecutableNode for NullNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        Ok(Value::Null)
    }
    fn schema(&self) -> Value {
        json!({})
    }
}

struct TestRegistry;

impl NodeRegistryPort for TestRegistry {
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
        match node_type {
            "emit" => Some(Arc::new(EmitNode)),
            "null_emit" => Some(Arc::new(NullNode)),
            _ => None,
        }
    }
    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        HashMap::new()
    }
}

/// Run a graph to completion and return every event it emitted.
async fn run(graph_json: Value) -> Vec<DagExecutionEvent> {
    let graph: Graph = serde_json::from_value(graph_json).expect("valid graph");
    let use_case = DagRunUseCase::new(Arc::new(TestRegistry), None);
    let mut stream = Box::pin(use_case.execute_stream(graph, None, None, false, None, None, None));
    let mut events = Vec::new();
    while let Some(res) = stream.next().await {
        events.push(res.expect("stream must not error"));
    }
    events
}

fn skipped(events: &[DagExecutionEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            DagExecutionEvent::NodeSkipped { node_id, reason } => {
                Some((node_id.clone(), reason.clone()))
            }
            _ => None,
        })
        .collect()
}

fn started(events: &[DagExecutionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            DagExecutionEvent::NodeStart { node_id, .. } => Some(node_id.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_node_that_runs_is_never_reported_as_skipped() {
    // `sink` has two upstreams: `dead` stops its branch with a null output,
    // `alive` carries data. `sink` runs. Reporting it as skipped because one of
    // its incoming branches went nowhere would make the event a lie.
    let events = run(json!({
        "nodes": {
            "dead":  { "type": "null_emit", "config": {} },
            "alive": { "type": "emit", "config": {} },
            "sink":  { "type": "emit", "config": {} }
        },
        "edges": [
            { "from": "dead", "to": "sink" },
            { "from": "alive", "to": "sink" }
        ]
    }))
    .await;

    assert!(
        started(&events).contains(&"sink".to_string()),
        "precondition: sink must actually run; started = {:?}",
        started(&events)
    );
    assert!(
        !skipped(&events).iter().any(|(id, _)| id == "sink"),
        "sink ran, so it must not be reported as skipped; skipped = {:?}",
        skipped(&events)
    );
}

#[tokio::test]
async fn a_node_that_never_runs_is_reported_exactly_once() {
    // `orphan` is reachable only through an edge whose pointer never resolves,
    // and it is the target of two such edges. It never runs, so it must be
    // reported — but once, not once per edge.
    let events = run(json!({
        "nodes": {
            "a":      { "type": "emit", "config": {} },
            "b":      { "type": "emit", "config": {} },
            "orphan": { "type": "emit", "config": {} }
        },
        "edges": [
            { "from": "a.missing_field", "to": "orphan" },
            { "from": "b.missing_field", "to": "orphan" }
        ]
    }))
    .await;

    assert!(
        !started(&events).contains(&"orphan".to_string()),
        "precondition: orphan must not run"
    );
    let orphan_frames: Vec<_> = skipped(&events)
        .into_iter()
        .filter(|(id, _)| id == "orphan")
        .collect();
    assert_eq!(
        orphan_frames.len(),
        1,
        "a skipped node must produce exactly one frame, got: {orphan_frames:?}"
    );
}

#[tokio::test]
async fn a_branch_stopped_by_a_null_output_is_reported() {
    // The single-upstream case: `dead` emits null, so `after` genuinely never
    // runs and the operator must be told.
    let events = run(json!({
        "nodes": {
            "dead":  { "type": "null_emit", "config": {} },
            "after": { "type": "emit", "config": {} }
        },
        "edges": [{ "from": "dead", "to": "after" }]
    }))
    .await;

    assert!(!started(&events).contains(&"after".to_string()));
    assert_eq!(
        skipped(&events),
        vec![("after".to_string(), "upstream_null_output".to_string())],
        "the stopped branch must be reported with the null-output reason"
    );
}

#[tokio::test]
async fn a_clean_graph_reports_no_skips() {
    let events = run(json!({
        "nodes": {
            "a": { "type": "emit", "config": {} },
            "b": { "type": "emit", "config": {} }
        },
        "edges": [{ "from": "a", "to": "b" }]
    }))
    .await;

    assert_eq!(started(&events).len(), 2, "both nodes must run");
    assert!(
        skipped(&events).is_empty(),
        "a graph where every node ran must emit no skip frames, got: {:?}",
        skipped(&events)
    );
}

#[tokio::test]
async fn a_node_behind_a_skipped_node_is_also_reported() {
    // `dead` stops its branch, so `first` never runs — and because `first`
    // never runs, nothing ever walks *its* outgoing edge, so `second` is never
    // even considered. Both are invisible to the user unless the run reports
    // every node that produced no output, not just the ones an edge passed over.
    //
    // This is the same hole that swallows a skipped node when the run suspends
    // in another branch: the marking happens in one run, the report in another.
    let events = run(json!({
        "nodes": {
            "dead":   { "type": "null_emit", "config": {} },
            "first":  { "type": "emit", "config": {} },
            "second": { "type": "emit", "config": {} }
        },
        "edges": [
            { "from": "dead", "to": "first" },
            { "from": "first", "to": "second" }
        ]
    }))
    .await;

    assert_eq!(
        skipped(&events)
            .into_iter()
            .find(|(id, _)| id == "second")
            .map(|(_, r)| r),
        Some("never_reached".to_string()),
        "no edge ever passed over `second`, so it carries the no-observed-cause reason"
    );
}

#[tokio::test]
async fn a_node_waiting_on_a_dependency_that_never_ran_is_reported() {
    // `sink` is enqueued by `alive`, but it also depends on `orphaned`, which
    // never runs. It is dropped at the readiness gate rather than by an edge.
    let events = run(json!({
        "nodes": {
            "dead":     { "type": "null_emit", "config": {} },
            "orphaned": { "type": "emit", "config": {} },
            "alive":    { "type": "emit", "config": {} },
            "sink":     { "type": "emit", "config": {} }
        },
        "edges": [
            { "from": "dead", "to": "orphaned" },
            { "from": "orphaned", "to": "sink" },
            { "from": "alive", "to": "sink" }
        ]
    }))
    .await;

    assert!(!started(&events).contains(&"sink".to_string()));
    assert_eq!(
        skipped(&events)
            .into_iter()
            .find(|(id, _)| id == "sink")
            .map(|(_, r)| r),
        Some("upstream_never_fired".to_string()),
        "the readiness gate is the cause here, and the reason must say so"
    );
}

#[tokio::test]
async fn an_edge_to_an_unknown_node_is_reported() {
    // `Graph::validate` rejects this at load, but `validate_persisted` lets it
    // through when resuming a frozen snapshot, so the run loop must still
    // account for it. This harness calls the loop directly, like that path does.
    let events = run(json!({
        "nodes": { "a": { "type": "emit", "config": {} } },
        "edges": [{ "from": "a", "to": "ghost" }]
    }))
    .await;

    assert_eq!(
        skipped(&events),
        vec![("ghost".to_string(), "unknown_target".to_string())],
        "an edge naming a node that does not exist must still be reported"
    );
}

#[tokio::test]
async fn a_run_cut_short_by_a_call_limit_says_so() {
    // `gate` is capped at zero calls, so the run trips the limit on its first
    // node and abandons the queue. Nothing downstream was passed over for a
    // routing reason, so calling it `never_reached` would read as ordinary
    // routing and hide the truncation from whoever is debugging the graph.
    let events = run(json!({
        "nodes": {
            "gate": { "type": "emit", "config": {}, "max_total_calls": 0 },
            "tail": { "type": "emit", "config": {} }
        },
        "edges": [{ "from": "gate", "to": "tail" }]
    }))
    .await;

    assert!(started(&events).is_empty(), "precondition: nothing runs");
    assert_eq!(
        skipped(&events)
            .into_iter()
            .find(|(id, _)| id == "tail")
            .map(|(_, r)| r),
        Some("run_stopped_early".to_string()),
        "a run stopped by a call limit must say so, not blame ordinary routing"
    );
}
