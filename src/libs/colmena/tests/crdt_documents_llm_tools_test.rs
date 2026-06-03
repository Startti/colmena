//! Verifies that the synthetic LLM tools, when dispatched through their
//! `execute_*` functions, mutate the runtime's registered Doc as expected
//! and that the ChangeTracker captures their summaries.

use colmena::crdt_documents::{projection::project, ArtifactId, CrdtDocumentsRuntime};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_tools::*;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn full_tool_sequence_round_trips_through_runtime() {
    let tmp = std::env::temp_dir().join(format!("llmt_{}", ulid::Ulid::new()));
    let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let id = ArtifactId::new();
    let entry = rt.registry.get_or_create(&id, "test");
    let ctx = CrdtDocsContext::new_local(rt.clone(), id.clone(), Some("test_session".to_string()));

    // 1. add_sheet → "Sales"
    let resp = execute_add_sheet(
        &ctx,
        AddSheetArgs {
            name: "Sales".into(),
        },
    )
    .await;
    let sheet_id = resp["sheet_id"].as_str().unwrap().to_string();

    // 2. set_range — write a 3x2 block (Product/Qty headers + 2 rows)
    execute_set_range(
        &ctx,
        SetRangeArgs {
            sheet_id: sheet_id.clone(),
            start_addr: "A1".into(),
            values_2d: vec![
                vec![json!("Product"), json!("Qty")],
                vec![json!("Apple"), json!(10)],
                vec![json!("Pear"), json!(20)],
            ],
        },
    )
    .await;

    // 3. read with explicit range
    let v = execute_read(
        &ctx,
        ReadArgs {
            sheet_id: sheet_id.clone(),
            range: Some("A1:B3".into()),
        },
    );
    let cells = v["cells"].as_object().unwrap();
    assert_eq!(
        cells.len(),
        6,
        "expected 6 cells in A1:B3, got {}",
        cells.len()
    );
    assert_eq!(cells["A1"], "Product");
    assert_eq!(cells["B2"], json!(10.0));
    assert_eq!(cells["A3"], "Pear");

    // 4. list_sheets
    let v = execute_list_sheets(&ctx);
    let sheets = v["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0]["name"], "Sales");

    // 5. get_recent_changes — tracker should reflect add_sheet + set_range
    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: None,
        },
    )
    .await;
    let narration = v["narration"].as_str().unwrap();
    assert!(
        narration.contains("Sales"),
        "narration missing 'Sales': {narration}"
    );
    assert!(
        narration.contains("added sheet") || narration.contains("add"),
        "narration missing add_sheet event: {narration}"
    );

    // 6. Cursor: pass current_event_id to verify it filters new events.
    let current = v["current_event_id"].as_u64();
    let v2 = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: current,
        },
    )
    .await;
    assert_eq!(v2["narration"], "No changes since last check.");

    // Final: verify the projection reflects the writes.
    let proj = project(&entry.doc);
    assert_eq!(proj["sheets"][0]["cells"]["A1"], "Product");
    assert_eq!(proj["sheets"][0]["cells"]["B3"], json!(20.0));

    let _ = std::fs::remove_dir_all(&tmp);
}
