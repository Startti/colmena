//! E2E of a group of `parallel` tool calls running at the same time, through a
//! real `ColmenaEngine`. The model answers ONE message with two calls to `Run`
//! — `parallel: true` and stateless, so each call is its own chain and the two
//! form one group — then with text. Each child sleeps in a `python_script`:
//! 2.3 s for `clima`, the first call, and 2 s for `precios`.
//!
//! - Parallel: both children run at once, so the group takes about the longer
//!   child, not the sum; `precios` finishes first and its Finish frame comes
//!   first, yet the history keeps the model's order.
//! - Baseline: the same graph with `parallel: false` runs the calls one after
//!   the other and takes at least the sum.
//!
//! Writes the SSE the CLI would print, each frame preceded by its arrival time
//! as an SSE comment (`: +<ms>`), to
//! `/tmp/colmena_e2e/parallel_tool_groups.sse` and
//! `/tmp/colmena_e2e/parallel_tool_groups_serial.sse`.
//!
//! Run with (the engine refuses to start without `SECURE_VALUES_KEY`):
//!   SECURE_VALUES_KEY=$(openssl rand -hex 24) DATABASE_URL=postgres:///colmena_e2e_par \
//!     cargo test -p colmena_dag_engine --test parallel_tool_groups -- --ignored --nocapture

mod parallel_turn_model;

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::infrastructure::OverrideGuard;
use futures::StreamExt;
use parallel_turn_model::{Call, ParallelTurnModel};
use serde_json::Value;
use serial_test::serial;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `(id, tool, arguments)` in the order the model puts them in its message.
const CALLS: [Call; 2] = [
    ("call_clima", "Run", r#"{"task":"clima"}"#),
    ("call_precios", "Run", r#"{"task":"precios"}"#),
];

/// What the shorter child sleeps (`precios`); `clima` sleeps 2.3 s.
const SHORTER_CHILD: Duration = Duration::from_secs(2);

/// The committed graph, with a stand-in key (the scripted model never uses it)
/// and `Run`'s `parallel` set to `parallel`.
fn graph(parallel: bool) -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/agents/parallel_tool_groups.json"
    );
    let mut graph: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let agent = &mut graph["nodes"]["agent"]["config"];
    agent["api_key"] = "scripted".into();
    agent["tool_configurations"]["Run"]["parallel"] = parallel.into();
    serde_json::from_value(graph).unwrap()
}

/// A frame and when it reached the consumer, from the start of the run.
struct Timed {
    at: Duration,
    frame: Value,
}

fn write_sse(name: &str, frames: &[Timed]) {
    std::fs::create_dir_all("/tmp/colmena_e2e").unwrap();
    let body: String = frames
        .iter()
        .map(|t| format!(": +{}ms\ndata: {}\n\n", t.at.as_millis(), t.frame))
        .collect();
    std::fs::write(
        format!("/tmp/colmena_e2e/{name}.sse"),
        body + "data: [DONE]\n\n",
    )
    .unwrap();
}

/// Runs the graph with the scripted model and returns the SSE frames with
/// their arrival times, and the `tool_call_id`s of the history the model read
/// on its second turn.
async fn run(parallel: bool, sse: &str) -> (Vec<Timed>, Vec<String>) {
    dotenvy::dotenv().ok();
    // The child is a `python_script`; the binary initializes pyo3 in main, a
    // test must too.
    pyo3::Python::initialize();
    let eng = ColmenaEngine::new(EngineConfig::from_env().await.unwrap())
        .await
        .unwrap();
    let model = Arc::new(ParallelTurnModel::new(&CALLS));
    let _guard = OverrideGuard::install(model.clone());
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let mut finish = Value::Null;
    let started = Instant::now();
    let mut stream = Box::pin(eng.execute_stream(graph(parallel), None, None, false, None, None));
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        let at = started.elapsed();
        frames.extend(mapper.map(&ev).into_iter().map(|frame| Timed { at, frame }));
        if let DagExecutionEvent::GraphFinish { output } = &ev {
            finish = output.clone();
        }
    }
    drop(stream);
    write_sse(sse, &frames);
    assert!(finish.to_string().contains("Listo."), "{finish}");
    eng.shutdown().await;
    (frames, model.results_seen())
}

/// Position and frame of the one frame of `kind` for `call_id`.
fn frame<'a>(frames: &'a [Timed], kind: &str, call_id: &str) -> (usize, &'a Timed) {
    let found: Vec<_> = frames
        .iter()
        .enumerate()
        .filter(|(_, t)| t.frame["type"] == kind && t.frame["toolCallId"] == call_id)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} for {call_id}");
    found[0]
}

/// Positions of the boundary frames of `kind` (`subgraph-node-start` or
/// `subgraph-node-end`) of the tool's children, with their paths.
fn boundaries<'a>(frames: &'a [Timed], kind: &str) -> Vec<(usize, &'a str)> {
    frames
        .iter()
        .enumerate()
        .filter(|(_, t)| t.frame["type"] == kind && t.frame["node_type"] == "subgraph")
        .map(|(i, t)| (i, t.frame["path"].as_str().unwrap()))
        .collect()
}

/// For each call, in the model's order: when its Start frame
/// (`tool-input-available`) and its Finish frame (`tool-output-available`)
/// arrived. Each child really ran: its output carries its task, and it took at
/// least its sleep.
fn spans(frames: &[Timed]) -> Vec<(Duration, Duration)> {
    CALLS
        .iter()
        .map(|(id, _, args)| {
            let (_, input) = frame(frames, "tool-input-available", id);
            let (_, output) = frame(frames, "tool-output-available", id);
            let task: Value = serde_json::from_str(args).unwrap();
            let task = task["task"].as_str().unwrap();
            assert!(
                output.frame["output"].to_string().contains(task),
                "{id}: {}",
                output.frame
            );
            let span = output.at - input.at;
            assert!(
                span >= SHORTER_CHILD,
                "{id} took {span:?}: it did not sleep"
            );
            (input.at, output.at)
        })
        .collect()
}

/// From the first Start frame to the last Finish frame, and the shorter
/// child's own span.
fn group_and_child(spans: &[(Duration, Duration)]) -> (Duration, Duration) {
    let first = spans.iter().map(|(s, _)| *s).min().unwrap();
    let last = spans.iter().map(|(_, e)| *e).max().unwrap();
    let child = spans.iter().map(|(s, e)| *e - *s).min().unwrap();
    (last - first, child)
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn a_group_of_parallel_calls_runs_its_children_at_the_same_time() {
    let (frames, history) = run(true, "parallel_tool_groups").await;

    // The timing: from the first Start frame to the last Finish frame, the
    // group takes about one child, not two.
    let (group, child) = group_and_child(&spans(&frames));
    eprintln!("parallel: group {group:?}, shorter child {child:?}");
    assert!(
        group < child * 3 / 2,
        "the group took {group:?}, the shorter child {child:?}: not concurrent"
    );

    // Two boundaries, one per call, named by k; both open before either closes.
    let opened = boundaries(&frames, "subgraph-node-start");
    let closed = boundaries(&frames, "subgraph-node-end");
    let mut paths: Vec<&str> = opened.iter().map(|(_, p)| *p).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["agent>Run#0", "agent>Run#1"]);
    let last_open = opened.iter().map(|(i, _)| *i).max().unwrap();
    let first_close = closed.iter().map(|(i, _)| *i).min().unwrap();
    assert!(last_open < first_close, "the children did not overlap");

    // Each call's two frames name the boundary it opened, which sits between them.
    for (k, (id, _, _)) in CALLS.iter().enumerate() {
        let scope = format!("Run#{k}");
        let (input_at, input) = frame(&frames, "tool-input-available", id);
        let (output_at, output) = frame(&frames, "tool-output-available", id);
        assert_eq!(input.frame["childScope"], scope.as_str(), "{}", input.frame);
        assert_eq!(
            output.frame["childScope"],
            scope.as_str(),
            "{}",
            output.frame
        );
        let path = format!("agent>{scope}");
        let (open_at, _) = opened.iter().find(|(_, p)| *p == path).unwrap();
        let (close_at, _) = closed.iter().find(|(_, p)| *p == path).unwrap();
        assert!(input_at < *open_at && *close_at < output_at, "{id}");
    }

    // Finish frames as they happen: `precios`, the shorter child, ends first.
    let (clima_end, _) = frame(&frames, "tool-output-available", "call_clima");
    let (precios_end, _) = frame(&frames, "tool-output-available", "call_precios");
    assert!(
        precios_end < clima_end,
        "the Finish frames waited for the model's order"
    );
    // The history, in the model's order all the same.
    assert_eq!(history, ["call_clima", "call_precios"]);
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn the_same_calls_without_parallel_run_one_after_the_other() {
    let (frames, history) = run(false, "parallel_tool_groups_serial").await;

    // The bare name, twice; the second child opens after the first closed.
    let opened = boundaries(&frames, "subgraph-node-start");
    let closed = boundaries(&frames, "subgraph-node-end");
    let paths: Vec<&str> = opened.iter().map(|(_, p)| *p).collect();
    assert_eq!(paths, ["agent>Run", "agent>Run"]);
    assert!(closed[0].0 < opened[1].0, "the children overlapped");
    for (id, _, _) in CALLS {
        let (_, input) = frame(&frames, "tool-input-available", id);
        let (_, output) = frame(&frames, "tool-output-available", id);
        assert!(input.frame.get("childScope").is_none(), "{}", input.frame);
        assert!(output.frame.get("childScope").is_none(), "{}", output.frame);
    }
    assert_eq!(history, ["call_clima", "call_precios"]);

    let (group, child) = group_and_child(&spans(&frames));
    eprintln!("serial: calls {group:?}, shorter child {child:?}");
    assert!(
        group >= child * 2,
        "the calls took {group:?}, the shorter child {child:?}: not serial"
    );
}
