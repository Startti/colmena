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

    // 5. get_recent_changes — own-session events are filtered out so this
    //    starts empty even though we just made add_sheet + set_range.
    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: None,
            sheet_id: None,
            limit: None,
        },
    )
    .await;
    assert!(
        v["events"].as_array().unwrap().is_empty(),
        "own-session events must be filtered out, got: {:?}",
        v["events"]
    );

    // 6. Seed a peer-side event and verify it surfaces.
    let peer_event_id = rt
        .store
        .insert_event(colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: id.clone(),
            sheet_id: Some(sheet_id.clone()),
            origin: "agent:other_peer".to_string(),
            summary: "peer added a sheet 'Sales'".to_string(),
        })
        .await
        .unwrap();

    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: None,
            sheet_id: None,
            limit: None,
        },
    )
    .await;
    let events = v["events"].as_array().unwrap();
    assert!(
        events.iter().any(|e| e["id"] == json!(peer_event_id)
            && e["origin"] == "agent:other_peer"),
        "expected peer event in results: {events:?}"
    );

    // 7. Cursor: pass current_event_id to verify it filters new events.
    let current = v["current_event_id"].as_u64();
    let v2 = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: current,
            sheet_id: None,
            limit: None,
        },
    )
    .await;
    assert!(v2["events"].as_array().unwrap().is_empty());

    // Final: verify the projection reflects the writes.
    let proj = project(&entry.doc);
    assert_eq!(proj["sheets"][0]["cells"]["A1"], "Product");
    assert_eq!(proj["sheets"][0]["cells"]["B3"], json!(20.0));

    let _ = std::fs::remove_dir_all(&tmp);
}
