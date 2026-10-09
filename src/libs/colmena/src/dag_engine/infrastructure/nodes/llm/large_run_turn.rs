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

/// The files the code returns are attachments of the session afterwards: the
/// answer names them with the engine's own id, and the registry holds an
/// engine-owned row for each (never a host reference), so later tools can use them.
#[tokio::test]
#[serial_test::serial]
async fn files_the_code_returns_become_engine_owned_attachments() {
    use crate::llm::domain::AttachmentRegistry;
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let storage = Arc::new(CountingStorage::default());
    let reg = registry_with_storage(Some(storage.clone()));
    reg.set_large_tabular(true);
    run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
        .await
        .unwrap();
    let p = prepared(&[("sales", 1)], 4).await;
    let answer = json!({
        "__colmena_emitted": [{"name": "out.csv", "rows": 2, "dtypes": {"a": "int64"}, "size": 6}],
        "result": 7
    });
    let exec =
        Recorder::ok_with_files(answer, &[("out.csv", b"a\n1\n2\n"), ("bad name.csv", b"x")]);
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, exec, true)));
    let model = RecordingModel::scripted(tool_call());
    run_turn_with_tools(&reg, &url, vec![], tools(), &model)
        .await
        .unwrap();
    let seen = model.seen();
    assert!(
        seen.contains("\"document_id\":\"generated/out.csv\""),
        "{seen}"
    );
    assert!(
        seen.contains("\"rows_reported_by_code\":2") && seen.contains("\"result\":7"),
        "{seen}"
    );
    assert!(
        seen.contains("not_kept") && !seen.contains("bad name"),
        "{seen}"
    );
    assert!(!seen.contains(SOURCE), "{seen}");
    assert_eq!(
        *p.storage.stored.lock().unwrap(),
        [("out.csv".to_string(), b"a\n1\n2\n".to_vec())]
    );
    let attachments = crate::llm::infrastructure::persistence::SqliteAttachmentRegistry::new(&url)
        .await
        .unwrap();
    let row = attachments
        .lookup_by_document_id("agent_1", "generated/out.csv")
        .await
        .unwrap()
        .expect("registered");
    assert!(!row.is_host_storage_ref());
    assert_eq!(row.provider, crate::llm::domain::ProviderKind::Generated);
    assert_eq!(
        row.origin.as_deref(),
        Some("generated_by:attachment_run_python")
    );
    assert_eq!(
        (row.mime_type.as_str(), row.size_bytes),
        ("text/csv", Some(6))
    );
    assert_eq!(storage.reads() + storage.stores(), 0);
}

/// Switch on and a runtime wired, but this turn has only a small file: the tool
/// is exactly what it always was (no large-file text, no `tables` argument), and
/// the refusals do not point at it.
#[tokio::test]
#[serial_test::serial]
async fn the_tool_is_unchanged_on_a_turn_with_only_a_small_file() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let storage = Arc::new(CountingStorage::default());
    let reg = registry_with_storage(Some(storage));
    reg.set_large_tabular(true);
    let p = prepared(&[("sales", 1)], 4).await;
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, Recorder::ok(Value::Null), true)));
    let small = json!({"id": "doc-s", "mime_type": "text/csv", "filename": "s.csv",
                       "size_bytes": 8, "data": STANDARD.encode(b"a,b\n1,2\n")});
    let model = RecordingModel::scripted(vec![ScriptedResponse::Text("ok".into())]);
    run_turn_with_tools(&reg, &url, vec![small], tools(), &model)
        .await
        .unwrap();
    let offered = model.tools_offered().join("\n");
    assert!(offered.contains("attachment_run_python"), "{offered}");
    assert!(!offered.contains("Large files (over 50 MiB)"), "{offered}");
    assert!(!offered.contains("\"tables\":{"), "{offered}");
}

/// One source of truth for the wording: with the tool served the refusals send
/// the model to it; without, they say it is not available.
#[tokio::test]
#[serial_test::serial]
async fn the_refusal_points_at_the_tool_only_when_it_is_served() {
    use crate::llm::domain::large_tabular::refusal_text_for;
    for served in [true, false] {
        let db = tempfile::NamedTempFile::new().unwrap();
        let url = format!("sqlite://{}", db.path().display());
        let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
        reg.set_large_tabular(true);
        run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
            .await
            .unwrap();
        if served {
            let p = prepared(&[("sales", 1)], 4).await;
            reg.set_large_tabular_runtime(Arc::new(runtime(&p, Recorder::ok(Value::Null), true)));
            std::mem::forget(p);
        }
        let model = RecordingModel::scripted(vec![
            ScriptedResponse::ToolCall {
                id: "c1".into(),
                tool_name: "load_attachment".into(),
                arguments: json!({"document_id": "doc-big"}),
            },
            ScriptedResponse::Text("ok".into()),
        ]);
        run_turn_with_tools(&reg, &url, vec![], tools(), &model)
            .await
            .unwrap();
        let seen = model.seen();
        assert!(
            seen.contains(refusal_text_for(served)),
            "served={served}: {seen}"
        );
        assert!(
            !seen.contains(refusal_text_for(!served)),
            "served={served}: {seen}"
        );
    }
}

/// The wiring and the wording are decided together: with the switch on and a
/// runtime wired but `attachment_run_python` NOT among the node's tools, nothing
/// points the model at it and the call is not routed.
#[tokio::test]
#[serial_test::serial]
async fn a_node_that_does_not_offer_the_tool_never_points_at_it() {
    use crate::llm::domain::large_tabular::refusal_text_for;
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
    reg.set_large_tabular(true);
    run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
        .await
        .unwrap();
    let p = prepared(&[("sales", 1)], 4).await;
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, Recorder::ok(Value::Null), true)));
    std::mem::forget(p);
    let model = RecordingModel::scripted(vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "load_attachment".into(),
            arguments: json!({"document_id": "doc-big"}),
        },
        ScriptedResponse::Text("ok".into()),
    ]);
    run_turn_with_tools(&reg, &url, vec![], Value::Null, &model)
        .await
        .unwrap();
    let seen = model.seen();
    assert!(seen.contains(refusal_text_for(false)), "{seen}");
    assert!(!seen.contains("attachment_run_python"), "{seen}");
}

/// A whole turn through the node with the file loaded: what the refusal names, and
/// which tools were offered, for a node with this `tool_configurations` and these
/// extra config keys.
async fn refusal_for_node(tool_configurations: Value, extra: Value) -> (String, Vec<String>) {
    use super::node_harness::run_turn_with_config;
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
    reg.set_large_tabular(true);
    run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
        .await
        .unwrap();
    let p = prepared(&[("sales", 1)], 4).await;
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, Recorder::ok(Value::Null), true)));
    std::mem::forget(p);
    let model = RecordingModel::scripted(vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "load_attachment".into(),
            arguments: json!({"document_id": "doc-big"}),
        },
        ScriptedResponse::Text("ok".into()),
    ]);
    run_turn_with_config(&reg, &url, vec![], tool_configurations, extra, &model)
        .await
        .unwrap();
    (model.seen(), model.tools_offered())
}

/// A node that has `data_run_python` and not `attachment_run_python` is sent to the
/// tool it has.
#[tokio::test]
#[serial_test::serial]
async fn the_refusal_names_data_run_python_when_that_is_the_tool_the_node_has() {
    use crate::llm::domain::large_tabular::{refusal_text_for_tool, LargeTool};
    let (seen, offered) = refusal_for_node(
        json!({"data_run_python": {"node_type": "data_run_python"}}),
        json!({}),
    )
    .await;
    assert!(
        seen.contains(refusal_text_for_tool(Some(LargeTool::DataRunPython))),
        "{seen}"
    );
    assert!(!seen.contains("attachment_run_python"), "{seen}");
    assert!(offered.join("\n").contains("data_run_python"));
}

/// The owner's `!name` governs the real node: the tool is not offered, so the
/// refusal does not send the model to it.
#[tokio::test]
#[serial_test::serial]
async fn an_excluded_tool_is_neither_offered_nor_named_by_the_refusal() {
    use crate::llm::domain::large_tabular::refusal_text_for;
    for (tool, config) in [
        (
            "attachment_run_python",
            json!({"attachment_run_python": {"node_type": "attachment_run_python"}}),
        ),
        (
            "data_run_python",
            json!({"data_run_python": {"node_type": "data_run_python"}}),
        ),
    ] {
        let (seen, offered) =
            refusal_for_node(config, json!({"enabled_tools": [format!("!{tool}")]})).await;
        assert!(!offered.join("\n").contains(tool), "{tool}: {offered:?}");
        assert!(seen.contains(refusal_text_for(false)), "{tool}: {seen}");
        assert!(!seen.contains(&format!("use `{tool}`")), "{tool}: {seen}");
    }
}

// ---- the same path through `data_run_python` ----

fn drp_tools() -> Value {
    json!({"data_run_python": {"node_type": "data_run_python"}})
}

fn drp_call(bindings: Value) -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "data_run_python".into(),
            arguments: json!({"bindings": bindings, "code": "output = 42"}),
        },
        ScriptedResponse::Text("ok".into()),
    ]
}

/// Loads the large file in one turn, then plays `script` in a second on a node with
/// `data_run_python`. `switch` is the engine switch for the second turn.
async fn drp_turn(
    switch: bool,
    exec: Arc<Recorder>,
    script: Vec<ScriptedResponse>,
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
    let p = prepared(&[("sales", 2), ("stores", 1)], 4).await;
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, exec, true)));
    std::mem::forget(p);
    let model = RecordingModel::scripted(script);
    run_turn_with_tools(&reg, &url, vec![], drp_tools(), &model)
        .await
        .unwrap();
    (model, storage)
}

/// The model, using `data_run_python` on a node that has it, gets the prepared tables:
/// the call reaches the runtime, the answer carries `result` and `tables`, the original
/// is never read, and the tool is shown with the large-file text and the `tables` argument.
#[tokio::test]
#[serial_test::serial]
async fn data_run_python_over_a_prepared_file_runs_over_its_tables() {
    let exec = Recorder::ok(json!({"total": 42}));
    let script = drp_call(json!([{"var": "big", "attachment_id": "doc-big"}]));
    let (model, storage) = drp_turn(true, exec.clone(), script).await;
    let seen = model.seen();
    assert!(seen.contains("\"total\":42"), "{seen}");
    assert!(
        seen.contains("\"stores\"") && seen.contains("\"tables\""),
        "{seen}"
    );
    assert!(!seen.contains(SOURCE), "no key shown: {seen}");
    assert_eq!(exec.calls(), 1);
    assert_eq!(exec.seen.lock().unwrap()[0].1, vec![0, 1], "all tables");
    assert_eq!(storage.reads() + storage.stores(), 0);
    let offered = model.tools_offered().join("\n");
    assert!(offered.contains("data_run_python"), "{offered}");
    assert!(offered.contains("Large files (over 50 MiB)"), "{offered}");
    assert!(offered.contains("pass ONE binding"), "{offered}");
}

/// A large file is analysed on its own; a mix is refused with a sentence, nothing runs.
#[tokio::test]
#[serial_test::serial]
async fn a_large_file_mixed_with_other_bindings_is_refused_and_nothing_runs() {
    let exec = Recorder::ok(json!(1));
    let script = drp_call(json!([
        {"var": "big", "attachment_id": "doc-big"},
        {"var": "small", "data": [{"a": 1}]}
    ]));
    let (model, _) = drp_turn(true, exec.clone(), script).await;
    let seen = model.seen();
    assert!(seen.contains("analysed on its own"), "{seen}");
    assert!(seen.contains("large_tabular_file"), "{seen}");
    assert_eq!(exec.calls(), 0);
}

/// `code_ref` has no meaning over a large file.
#[tokio::test]
#[serial_test::serial]
async fn code_ref_over_a_large_file_is_refused() {
    let exec = Recorder::ok(json!(1));
    let script = vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "data_run_python".into(),
            arguments: json!({"bindings": [{"var": "big", "attachment_id": "doc-big"}], "code_ref": "k"}),
        },
        ScriptedResponse::Text("ok".into()),
    ];
    let (model, _) = drp_turn(true, exec.clone(), script).await;
    assert!(
        model.seen().contains("`code_ref` is not available"),
        "{}",
        model.seen()
    );
    assert_eq!(exec.calls(), 0);
}

/// The definition `data_run_python` had in v0.30.0, as a literal: name, description,
/// summary and input schema, captured from that release's builder (the builder and its
/// text are unchanged since). The checks below compare the node against THIS, not against
/// whatever the builder produces today.
const GOLDEN_DRP: &str = include_str!("golden_data_run_python_v0_30_0.json");

fn same_as_the_golden(offered: &crate::llm::domain::ToolDefinition) {
    let golden: Value = serde_json::from_str(GOLDEN_DRP).unwrap();
    assert_eq!(offered.name, golden["name"].as_str().unwrap());
    assert_eq!(offered.description, golden["description"].as_str().unwrap());
    assert_eq!(offered.summary.as_deref(), golden["summary"].as_str());
    assert_eq!(
        offered.input_schema_override.clone().unwrap_or(Value::Null),
        golden["input_schema_override"]
    );
}

/// With the engine switch off the tool is exactly what it always was: the same
/// description and schema, and the call over the host-owned file gets the refusal it
/// always had, with nothing routed.
#[tokio::test]
#[serial_test::serial]
async fn with_the_switch_off_data_run_python_is_unchanged() {
    use crate::llm::domain::large_tabular::refusal_text_for;
    let exec = Recorder::ok(json!(1));
    let script = drp_call(json!([{"var": "big", "attachment_id": "doc-big"}]));
    let (model, storage) = drp_turn(false, exec.clone(), script).await;
    same_as_the_golden(&model.tool_definition("data_run_python").expect("offered"));
    let seen = model.seen();
    assert!(seen.contains(refusal_text_for(false)), "{seen}");
    assert!(seen.contains("large_tabular_file"), "{seen}");
    assert_eq!(exec.calls(), 0);
    assert_eq!(storage.reads() + storage.stores(), 0);
}

/// A turn with only small files shows the usual tool even with the switch on.
#[tokio::test]
#[serial_test::serial]
async fn on_a_turn_with_only_a_small_file_data_run_python_is_unchanged() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
    reg.set_large_tabular(true);
    let p = prepared(&[("sales", 1)], 4).await;
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, Recorder::ok(Value::Null), true)));
    let small = json!({"id": "doc-s", "mime_type": "text/csv", "filename": "s.csv",
                       "size_bytes": 8, "data": STANDARD.encode(b"a,b\n1,2\n")});
    let model = RecordingModel::scripted(vec![ScriptedResponse::Text("ok".into())]);
    run_turn_with_tools(&reg, &url, vec![small], drp_tools(), &model)
        .await
        .unwrap();
    same_as_the_golden(&model.tool_definition("data_run_python").expect("offered"));
}

/// Files returned through `data_run_python` are engine-owned attachments tagged with
/// that tool's name (its quota counts them), never a host reference.
#[tokio::test]
#[serial_test::serial]
async fn files_returned_through_data_run_python_carry_its_name() {
    use crate::llm::domain::AttachmentRegistry;
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
    reg.set_large_tabular(true);
    run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
        .await
        .unwrap();
    let p = prepared(&[("sales", 1)], 4).await;
    let exec = Recorder::ok_with_files(
        json!({"__colmena_emitted": [{"name": "out.csv", "rows": 2, "dtypes": {}, "size": 6}], "result": 7}),
        &[("out.csv", b"a\n1\n2\n")],
    );
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, exec, true)));
    let model = RecordingModel::scripted(drp_call(
        json!([{"var": "big", "attachment_id": "doc-big"}]),
    ));
    run_turn_with_tools(&reg, &url, vec![], drp_tools(), &model)
        .await
        .unwrap();
    let attachments = crate::llm::infrastructure::persistence::SqliteAttachmentRegistry::new(&url)
        .await
        .unwrap();
    let row = attachments
        .lookup_by_document_id("agent_1", "generated/out.csv")
        .await
        .unwrap()
        .expect("registered");
    assert!(!row.is_host_storage_ref());
    assert_eq!(row.origin.as_deref(), Some("generated_by:data_run_python"));
}

// ---- a tool the node excluded never reaches the sandbox, even called by name ----

/// A turn on a node that declares BOTH Python tools and excludes `excluded`, where the
/// model calls `called` by name over the large file. Returns what the model saw and how
/// many times the sandbox ran.
async fn called_by_name(excluded: &str, called: &str, arguments: Value) -> (String, usize) {
    use super::node_harness::run_turn_with_config;
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let reg = registry_with_storage(Some(Arc::new(CountingStorage::default())));
    reg.set_large_tabular(true);
    run_turn(&reg, &url, vec![entry()], &RecordingModel::new(2))
        .await
        .unwrap();
    let p = prepared(&[("sales", 1)], 4).await;
    let exec = Recorder::ok(json!({"ran": true}));
    reg.set_large_tabular_runtime(Arc::new(runtime(&p, exec.clone(), true)));
    std::mem::forget(p);
    let model = RecordingModel::scripted(vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: called.into(),
            arguments,
        },
        ScriptedResponse::Text("ok".into()),
    ]);
    let both = json!({
        "attachment_run_python": {"node_type": "attachment_run_python"},
        "data_run_python": {"node_type": "data_run_python"},
    });
    run_turn_with_config(
        &reg,
        &url,
        vec![],
        both,
        json!({"enabled_tools": [format!("!{excluded}")]}),
        &model,
    )
    .await
    .unwrap();
    (model.seen(), exec.calls())
}

#[tokio::test]
#[serial_test::serial]
async fn an_excluded_python_tool_called_by_name_never_reaches_the_sandbox() {
    let (seen, runs) = called_by_name(
        "attachment_run_python",
        "attachment_run_python",
        json!({"attachment_id": "doc-big", "code": "result = 1"}),
    )
    .await;
    assert_eq!(runs, 0, "{seen}");
    assert!(!seen.contains("\"ran\":true"), "{seen}");
    let (seen, runs) = called_by_name(
        "data_run_python",
        "data_run_python",
        json!({"bindings": [{"var": "big", "attachment_id": "doc-big"}], "code": "output = 1"}),
    )
    .await;
    assert_eq!(runs, 0, "{seen}");
    assert!(!seen.contains("\"ran\":true"), "{seen}");
}

/// The other tool of the pair still works on the same node.
#[tokio::test]
#[serial_test::serial]
async fn the_tool_that_is_not_excluded_still_runs_on_that_node() {
    let (seen, runs) = called_by_name(
        "attachment_run_python",
        "data_run_python",
        json!({"bindings": [{"var": "big", "attachment_id": "doc-big"}], "code": "output = 1"}),
    )
    .await;
    assert_eq!(runs, 1, "{seen}");
    assert!(seen.contains("\"ran\":true"), "{seen}");
}
