//! The node hands the large tabular switch to the tool executor, whose
//! `fetch_attachment_bytes` is the lowest shared point of every whole-object
//! attachment read. Through the real `llm_call` node: the model runs
//! `attachment_run_python` on a large registered file and gets the redirect,
//! with zero reads of the object.

use super::node_harness::{
    registry_with_storage, run_turn_with_tools, CountingStorage, RecordingModel,
};
use crate::llm::domain::large_tabular::refusal_text;
use crate::llm::infrastructure::ScriptedResponse;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
#[serial_test::serial]
async fn a_tool_that_reads_a_large_file_whole_gets_the_redirect_and_no_read_happens() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}", db.path().display());
    let storage = Arc::new(CountingStorage::default());
    let reg = registry_with_storage(Some(storage.clone()));
    reg.set_large_tabular(true);
    let model = RecordingModel::scripted(vec![
        ScriptedResponse::ToolCall {
            id: "c1".into(),
            tool_name: "attachment_run_python".into(),
            arguments: json!({"attachment_id": "doc-big", "code": "print(len(df))"}),
        },
        ScriptedResponse::Text("ok".into()),
    ]);
    let entry = json!({
        "id": "doc-big", "mime_type": "text/csv", "filename": "sales.csv",
        "size_bytes": 60 * 1024 * 1024, "storage_key": "chat-attachments/u/s/big.csv",
    });
    run_turn_with_tools(
        &reg,
        &url,
        vec![entry],
        json!({"attachment_run_python": {"node_type": "attachment_run_python"}}),
        &model,
    )
    .await
    .unwrap();

    let seen = model.seen();
    assert!(seen.contains(refusal_text()), "{seen}");
    assert_eq!(storage.reads(), 0, "the object was never read");
}
