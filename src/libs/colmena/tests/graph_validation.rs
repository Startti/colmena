//! Structural validation of a graph, and — the part that matters — that it is
//! actually wired into the path every caller takes.
//!
//! The library entry points (`execute_stream`, `execute_stream_cancellable`,
//! `run_dag`, `stream_sse_parts`) used to take a `Graph` and run it unchecked.
//! Only the CLI validated, so ADP's worker — which calls
//! `execute_stream_cancellable` directly — ran graphs the CLI would have
//! rejected. Testing `Graph::validate` on its own would not have caught that:
//! the function was correct and simply never called.

use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::application::run_use_case::DagRunUseCase;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::node::ExecutableNode;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

/// A registry with nothing in it.
///
/// The validation under test runs before any node is looked up, so the test
/// needs no real nodes — and this keeps it free of a database, which is what
/// lets it run in CI instead of behind `--ignored`.
struct EmptyRegistry;

impl NodeRegistryPort for EmptyRegistry {
    fn get_node(&self, _node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
        None
    }
    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        HashMap::new()
    }
}

fn graph_with_slash_id() -> serde_json::Value {
    serde_json::json!({
        "nodes": { "bad/id": { "type": "log", "config": {} } },
        "edges": []
    })
}

/// Serialises the two tests that depend on `COLMENA_GRAPH_VALIDATION`.
///
/// `set_var` is process-global, so with the default test parallelism the
/// kill-switch test switched validation off underneath the test asserting it is
/// on — which failed with a node-lookup error instead, looking like the wiring
/// was broken. The lock is the fix; `--test-threads=1` would only hide it from
/// this file while CI still runs the suite in parallel.
/// A `tokio` mutex, not a `std` one: the guard is held across an `.await`, and
/// a blocking guard there can stall the runtime (clippy flags it).
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn use_case() -> DagRunUseCase {
    DagRunUseCase::new(Arc::new(EmptyRegistry), None)
}

#[test]
fn graph_with_slash_in_node_id_fails_validation() {
    let g: Graph = serde_json::from_value(graph_with_slash_id()).unwrap();
    let err = g.validate().expect_err("validation must fail");
    assert!(err.to_string().contains("bad/id"));
}

/// The wiring test, and the reason this file exists.
///
/// It drives `DagRunUseCase::execute_stream` — the one function every entry
/// point converges on, including the `execute_stream_cancellable` ADP's worker
/// calls — rather than invoking `Graph::validate` by hand. Testing the helper
/// alone would have passed happily while nothing called it, which is exactly
/// the state this fixes.
///
/// Builds without a database: the registry's pool is lazy and the state
/// repository is optional, so this runs in CI rather than behind `--ignored`.
#[tokio::test]
async fn the_engine_path_rejects_an_invalid_graph_before_executing_it() {
    let _guard = ENV_LOCK.lock().await;
    let mut stream = Box::pin(use_case().execute_stream(
        serde_json::from_value(graph_with_slash_id()).unwrap(),
        None,
        None,
        false,
        None,
        None,
        None,
    ));

    let first = stream
        .next()
        .await
        .expect("the stream must yield an error rather than run the graph");
    let err = first.expect_err("an invalid graph must not start executing");
    assert!(
        err.to_string().contains("bad/id"),
        "the error must name the offending node: {err}"
    );
}

/// The safety valve, so an operator bitten by this can turn it off without
/// waiting for a rollback — same shape as `COLMENA_PREFLIGHT_HEALTH=off`.
#[tokio::test]
async fn the_kill_switch_lets_an_invalid_graph_through() {
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("COLMENA_GRAPH_VALIDATION", "off");
    let mut stream = Box::pin(use_case().execute_stream(
        serde_json::from_value(graph_with_slash_id()).unwrap(),
        None,
        None,
        false,
        None,
        None,
        None,
    ));
    let first = stream.next().await;
    std::env::remove_var("COLMENA_GRAPH_VALIDATION");

    // With validation off the run gets past this check. It may still fail for
    // an unrelated reason; what must not happen is the validation error.
    if let Some(Err(e)) = &first {
        assert!(
            !e.to_string()
                .contains("reserved for subgraph path qualifiers"),
            "the kill switch must suppress the validation error, got: {e}"
        );
    }
}
