//! Unit tests for `crdt_doc_list_sheets_of` — cross-artifact peek.
//! Verifies no session ownership enforcement (any artifact in the registry
//! is visible). Mirrors test pattern of crdt_doc_recent_changes_test.rs.

use colmena::crdt_documents::{
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext, crdt_doc_tools::dispatch_crdt_doc_list_sheets_of,
};
use serde_json::json;
use std::sync::Arc;

async fn make_runtime() -> (Arc<CrdtDocumentsRuntime>, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("lso_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({"storage_backend": "localfs", "storage_root": tmp.to_str().unwrap()});
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    (rt, tmp)
}

#[tokio::test]
async fn list_sheets_of_returns_sheets_for_any_artifact() {
    let (rt, tmp) = make_runtime().await;
    // Two artifacts; ctx is pinned to artifact A but we query artifact B.
    let aid_a = ArtifactId::new();
    let aid_b = ArtifactId::new();
    let entry_b = rt.registry.get_or_create(&aid_b, "B");
    let sheet_id = apply_add_sheet(&entry_b.doc, "Inventory");
    let _ = apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "A1", &json!("Region"));
    let _ = apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "B1", &json!("Sales"));
    let _ = apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "A2", &json!("North"));
    let _ = apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "B2", &json!(100));
    let _entry_a = rt.registry.get_or_create(&aid_a, "A");

    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a.clone(), Some("s".to_string()));
    let result =
        dispatch_crdt_doc_list_sheets_of(&ctx, json!({"artifact_id": aid_b.to_string()})).await;
    assert_eq!(result["artifact_id"], aid_b.to_string());
    let sheets = result["sheets"].as_array().expect("sheets array");
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0]["name"], "Inventory");
    assert_eq!(sheets[0]["n_rows"], 2);
    assert_eq!(sheets[0]["n_cols"], 2);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_sheets_of_rejects_not_found() {
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let _entry = rt.registry.get_or_create(&aid_a, "A");
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a, Some("s".to_string()));
    let missing = ArtifactId::new();
    let result =
        dispatch_crdt_doc_list_sheets_of(&ctx, json!({"artifact_id": missing.to_string()})).await;
    assert_eq!(result["error"], "artifact_not_found");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_sheets_of_rejects_invalid_id() {
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_a, "A");
    let ctx = CrdtDocsContext::new_local(rt, aid_a, Some("s".to_string()));
    let result = dispatch_crdt_doc_list_sheets_of(&ctx, json!({"artifact_id": "not_a_ulid"})).await;
    assert_eq!(result["error"], "invalid_artifact_id");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_sheets_of_returns_multiple_sheets_in_order() {
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let aid_b = ArtifactId::new();
    let entry_b = rt.registry.get_or_create(&aid_b, "B");
    let sid1 = apply_add_sheet(&entry_b.doc, "First");
    let sid2 = apply_add_sheet(&entry_b.doc, "Second");
    let sid3 = apply_add_sheet(&entry_b.doc, "Third");
    let _ = apply_set_cell_in_proc(&entry_b.doc, &sid1, "A1", &json!("x"));
    let _entry_a = rt.registry.get_or_create(&aid_a, "A");
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a, Some("s".to_string()));
    let result =
        dispatch_crdt_doc_list_sheets_of(&ctx, json!({"artifact_id": aid_b.to_string()})).await;
    let sheets = result["sheets"].as_array().expect("sheets array");
    assert_eq!(sheets.len(), 3);
    // Order matches insertion order (Y.Array preserves order).
    assert_eq!(sheets[0]["name"], "First");
    assert_eq!(sheets[0]["sheet_id"], sid1);
    assert_eq!(sheets[1]["name"], "Second");
    assert_eq!(sheets[1]["sheet_id"], sid2);
    assert_eq!(sheets[2]["name"], "Third");
    assert_eq!(sheets[2]["sheet_id"], sid3);
    // The first sheet has 1 cell at A1 → n_rows=1, n_cols=1.
    assert_eq!(sheets[0]["n_rows"], 1);
    assert_eq!(sheets[0]["n_cols"], 1);
    // Second and third are empty → n_rows=0, n_cols=0.
    assert_eq!(sheets[1]["n_rows"], 0);
    assert_eq!(sheets[1]["n_cols"], 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_sheets_of_handles_sparse_cells_correctly() {
    // Sparse data: only C5 is set → n_rows=5, n_cols=3 (because C is col 3, 5th row).
    // Catches off-by-one bugs in the max_row+1 / max_col+1 math.
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let aid_b = ArtifactId::new();
    let entry_b = rt.registry.get_or_create(&aid_b, "B");
    let sid = apply_add_sheet(&entry_b.doc, "Sparse");
    let _ = apply_set_cell_in_proc(&entry_b.doc, &sid, "C5", &json!("only_cell"));
    let _ = rt.registry.get_or_create(&aid_a, "A");
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a, Some("s".to_string()));
    let result =
        dispatch_crdt_doc_list_sheets_of(&ctx, json!({"artifact_id": aid_b.to_string()})).await;
    let sheets = result["sheets"].as_array().expect("sheets array");
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0]["n_rows"], 5);
    assert_eq!(sheets[0]["n_cols"], 3);
    let _ = std::fs::remove_dir_all(&tmp);
}
