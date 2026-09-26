//! E2E of D2 (parallel tool call identity) through a real `ColmenaEngine`.
//! The model answers ONE message with three calls — `Run`, `Nota`, `Run` —
//! then with text. `Run` declares `parallel: true`, so each of its calls opens
//! its boundary as `Run#<k>` (k = the call's index in that message: 0 and 2)
//! and its tool frames carry `childScope`; `Nota` did not opt in and keeps the
//! bare name and frames without the field. `Nota` is a barrier between the two
//! `Run` calls, so each forms a group of one and the three run one after
//! another.
//! Writes the SSE the CLI would print to
//! `/tmp/colmena_e2e/parallel_tool_identity.sse` and asserts on its frames.
//!
//! Run with (the engine refuses to start without `SECURE_VALUES_KEY`):
//!   SECURE_VALUES_KEY=$(openssl rand -hex 24) DATABASE_URL=postgres:///colmena_e2e_par \
//!     cargo test -p colmena_dag_engine --test parallel_tool_identity -- --ignored --nocapture

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

/// `(id, tool, arguments)` in the order the model puts them in its message.
const CALLS: [Call; 3] = [
    ("call_clima", "Run", r#"{"task":"clima"}"#),
    ("call_nota", "Nota", r#"{"texto":"empecé"}"#),
    ("call_precios", "Run", r#"{"task":"precios"}"#),
];

/// The committed graph, with a stand-in key: the scripted model never uses it.
fn graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/agents/parallel_tool_identity.json"
    );
    let mut graph: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    graph["nodes"]["agent"]["config"]["api_key"] = "scripted".into();
    serde_json::from_value(graph).unwrap()
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

/// Position and frame of the one frame of `kind` for `call_id`.
fn frame<'a>(frames: &'a [Value], kind: &str, call_id: &str) -> (usize, &'a Value) {
    let found: Vec<_> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f["type"] == kind && f["toolCallId"] == call_id)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} for {call_id}: {found:?}");
    found[0]
}

#[tokio::test]
#[serial]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn each_parallel_call_opens_its_own_boundary_and_its_frames_name_it() {
    dotenvy::dotenv().ok();
    let eng = ColmenaEngine::new(EngineConfig::from_env().await.unwrap())
        .await
        .unwrap();
    let model = Arc::new(ParallelTurnModel::new(&CALLS));
    let _guard = OverrideGuard::install(model.clone());
    let mut mapper = SseMapper::new();
    let mut frames = Vec::new();
    let mut finish = Value::Null;
    let mut stream = Box::pin(eng.execute_stream(graph(), None, None, false, None, None));
    while let Some(item) = stream.next().await {
        let ev = item.expect("stream event must not error");
        frames.extend(mapper.map(&ev));
        if let DagExecutionEvent::GraphFinish { output } = &ev {
            finish = output.clone();
        }
    }
    drop(stream);
    write_sse("parallel_tool_identity", &frames);
    assert!(finish.to_string().contains("Listo."), "{finish}");

    let boundaries: Vec<&str> = frames
        .iter()
        .filter(|f| f["type"] == "subgraph-node-start" && f["node_type"] == "subgraph")
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(boundaries, ["agent>Run#0", "agent>Nota", "agent>Run#2"]);

    let expected_scope = [Some("Run#0"), None, Some("Run#2")];
    let mut previous_output = 0;
    for ((id, _, _), scope) in CALLS.iter().zip(expected_scope) {
        let (input_at, input) = frame(&frames, "tool-input-available", id);
        let (output_at, output) = frame(&frames, "tool-output-available", id);
        assert_eq!(input["childScope"].as_str(), scope, "{input}");
        assert_eq!(output["childScope"].as_str(), scope, "{output}");
        // Absent, not null: a frame of a tool that did not opt in is unchanged.
        assert_eq!(input.get("childScope").is_some(), scope.is_some());
        assert_eq!(output.get("childScope").is_some(), scope.is_some());

        // The scope names the boundary this call opened, between its frames.
        let name = scope.unwrap_or("Nota");
        let path = format!("{}>{}", input["path"].as_str().unwrap(), name);
        let (boundary_at, _) = frames
            .iter()
            .enumerate()
            .find(|(_, f)| f["type"] == "subgraph-node-start" && f["path"] == path.as_str())
            .unwrap_or_else(|| panic!("no boundary at {path}"));
        assert!(input_at < boundary_at && boundary_at < output_at, "{id}");

        // Serial: `Nota` is a barrier, so no two of these calls share a group
        // and each starts after the previous one finished.
        assert!(
            previous_output < input_at,
            "{id} started before the last ended"
        );
        previous_output = output_at;
    }

    // Only the two frames of a tool call carry the field. `tool-input-start`
    // comes from the streamed chunk, before the call has its k: never.
    let carriers: Vec<&str> = frames
        .iter()
        .filter(|f| f.get("childScope").is_some())
        .map(|f| f["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        carriers,
        [
            "tool-input-available",
            "tool-output-available",
            "tool-input-available",
            "tool-output-available"
        ]
    );
    for (id, _, _) in CALLS {
        frame(&frames, "tool-input-start", id);
    }

    // The model read the three results in the order it asked for them.
    let ids: Vec<&str> = CALLS.iter().map(|(id, _, _)| *id).collect();
    assert_eq!(model.results_seen(), ids);
    eng.shutdown().await;
}
