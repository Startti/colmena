//! A tool call over a large file, through the real `llm_call` node: the file is
//! registered as a host reference, the model calls `attachment_run_python`, and
//! the call reaches the runtime the engine wired, or the refusal it always had.
//! A real SQLite attachment registry, a real in-memory storage that counts, a
//! real SQLite preparation registry, and a recording mounted executor.

use super::node_harness::{
    registry_with_storage, run_turn, run_turn_with_tools, CountingStorage, RecordingModel,
};
use crate::llm::infrastructure::ScriptedResponse;
use crate::tabular_run::testkit::{prepared, runtime, Recorder, SOURCE};
use serde_json::{json, Value};
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;

fn entry() -> Value {
    json!({
        "id": "doc-big", "mime_type": "text/csv", "filename": "sales.csv",
        "size_bytes": 60 * MIB, "storage_key": SOURCE,
    })
}

fn tool_call() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "attachment_run_python".into(),
            arguments: json!({"attachment_id": "doc-big", "code": "result = 42"}),
        },
        ScriptedResponse::Text("ok".into()),
    ]
}

fn tools() -> Value {
    json!({"attachment_run_python": {"node_type": "attachment_run_python"}})
}

/// Registers the file in one turn, then plays `script` in a second. Returns what
/// the model saw, the tools it was offered and the storage.
async fn two_turns(
    switch: bool,
    wire: Option<Arc<Recorder>>,
    off_in_runtime: bool,
) -> (Arc<RecordingModel>, Arc<CountingStorage>) {
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let storage = Arc::new(CountingStorage::default());
    let reg = registry_with_storage(Some(storage.clone()));
    reg.set_large_tabular(true);
    run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
        .await
        .unwrap();
    reg.set_large_tabular(switch);
    if let Some(exec) = wire {
        let p = prepared(&[("sales", 2), ("stores", 1)], 4).await;
        reg.set_large_tabular_runtime(Arc::new(runtime(&p, exec, !off_in_runtime)));
        // Keep the fixture alive for the turn.
        std::mem::forget(p);
    }
    let model = RecordingModel::scripted(tool_call());
    run_turn_with_tools(&reg, &url, vec![], tools(), &model)
        .await
        .unwrap();
    (model, storage)
}

#[tokio::test]
#[serial_test::serial]
async fn a_call_over_a_prepared_file_runs_over_its_tables_through_the_node() {
    let exec = Recorder::ok(json!({"total": 42}));
    let (model, storage) = two_turns(true, Some(exec.clone()), false).await;
    let seen = model.seen();
    assert!(
        seen.contains("\"total\":42"),
        "the result reached the model: {seen}"
    );
    assert!(
        seen.contains("\"stores\"") && seen.contains("\"tables\""),
        "{seen}"
    );
    assert!(!seen.contains(SOURCE), "no key shown: {seen}");
    assert_eq!(exec.calls(), 1);
    assert_eq!(exec.seen.lock().unwrap()[0].1, vec![0, 1], "all tables");
    assert_eq!(
        storage.reads() + storage.stores(),
        0,
        "the original is never read"
    );
    let offered = model.tools_offered().join("\n");
    assert!(offered.contains("Large files (over 50 MiB)"), "{offered}");
    assert!(
        offered.contains("\"tables\"") || offered.contains("tables"),
        "{offered}"
    );
}

/// Wired but the switch is off now: the file keeps the refusal it has always
/// had, nothing is run, and the tool text is the usual one.
#[tokio::test]
#[serial_test::serial]
async fn with_the_switch_off_the_refusal_is_the_one_it_always_was() {
    let exec = Recorder::ok(Value::Null);
    let (model, storage) = two_turns(false, Some(exec.clone()), false).await;
    let seen = model.seen();
    assert!(seen.contains("large_tabular_file"), "{seen}");
    assert_eq!(exec.calls(), 0);
    assert_eq!(storage.reads() + storage.stores(), 0);
    assert!(!model
        .tools_offered()
        .join("\n")
        .contains("Large files (over 50 MiB)"));
}

/// The switch on but nothing wired (the host gave the engine no registry or no
/// executor): the usual refusal and the usual tool text.
#[tokio::test]
#[serial_test::serial]
async fn with_no_runtime_wired_nothing_is_routed_and_the_tool_is_unchanged() {
    let (model, storage) = two_turns(true, None, false).await;
    let seen = model.seen();
    assert!(seen.contains("large_tabular_file"), "{seen}");
    assert_eq!(storage.reads() + storage.stores(), 0);
    assert!(!model
        .tools_offered()
        .join("\n")
        .contains("Large files (over 50 MiB)"));
}

/// The runtime's own switch (`EngineConfig.prepare`) off: a typed refusal.
#[tokio::test]
#[serial_test::serial]
async fn a_runtime_that_is_off_answers_with_the_typed_refusal() {
    let exec = Recorder::ok(Value::Null);
    let (model, _storage) = two_turns(true, Some(exec.clone()), true).await;
    let seen = model.seen();
    assert!(seen.contains("large_tabular_disabled"), "{seen}");
    assert_eq!(exec.calls(), 0);
}
