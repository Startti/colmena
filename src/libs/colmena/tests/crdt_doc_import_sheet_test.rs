//! Unit tests for crdt_doc_import_sheet (F-T2).
//! Covers: happy clone, name collision, default name format, all 6 error paths,
//! audit event recording, dirty flag side-effect.

use colmena::crdt_documents::{
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_import_sheet::{dispatch_crdt_doc_import_sheet, MAX_SHEETS_PER_ARTIFACT},
};
use serde_json::json;
use std::sync::Arc;

async fn make_two_artifacts() -> (
    Arc<CrdtDocumentsRuntime>,
    ArtifactId, // principal (ctx)
    ArtifactId, // secondary (source)
    String,     // source sheet_id with seeded 2x2 data
    std::path::PathBuf,
) {
    let tmp = std::env::temp_dir().join(format!("imp_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({"storage_backend": "localfs", "storage_root": tmp.to_str().unwrap()});
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid_p = ArtifactId::new();
    let aid_s = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_p, "principal");
    let entry_s = rt.registry.get_or_create(&aid_s, "secondary");
    let sid = apply_add_sheet(&entry_s.doc, "Inventory");
    apply_set_cell_in_proc(&entry_s.doc, &sid, "A1", &json!("Region"));
    apply_set_cell_in_proc(&entry_s.doc, &sid, "B1", &json!("Sales"));
    apply_set_cell_in_proc(&entry_s.doc, &sid, "A2", &json!("North"));
    apply_set_cell_in_proc(&entry_s.doc, &sid, "B2", &json!(100));
    (rt, aid_p, aid_s, sid, tmp)
}

#[tokio::test]
async fn import_sheet_clones_cells_and_headers() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    let result = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
        }),
    )
    .await;
    assert!(
        result["error"].is_null(),
        "got error: {:?}",
        result["error"]
    );
    assert_eq!(result["n_rows"], 2);
    assert_eq!(result["n_cols"], 2);
    assert_eq!(result["source"]["artifact_id"], aid_s.to_string());
    // Verify the principal now has the cloned sheet with same values.
    let entry_p = rt.registry.get(&aid_p).unwrap();
    let proj = colmena::crdt_documents::projection::project(&entry_p.doc);
    let sheets = proj["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0]["cells"]["A1"], json!("Region"));
    // Numbers round-trip as f64 through the projection (matches set_range
    // behavior in crdt_doc_tools tests — `json!(100)` would fail the eq).
    assert_eq!(sheets[0]["cells"]["B2"], json!(100.0));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_default_name_includes_short_source_id() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p, Some("s".to_string()));
    let result = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
        }),
    )
    .await;
    let aid_s_str = aid_s.to_string();
    // Default format: "<original> (from art_xxxx)" where xxxx = first 4 chars of ULID (after "art_" prefix)
    let expected_suffix = format!("(from art_{})", &aid_s_str[4..8]);
    let name = result["name"].as_str().expect("name string");
    assert!(name.starts_with("Inventory ("), "name was: {name}");
    assert!(
        name.contains(&expected_suffix),
        "name was: {name}, expected to contain {expected_suffix}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_auto_suffixes_on_name_collision() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    // First import — succeeds with "Mirror".
    let r1 = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
            "new_name": "Mirror",
        }),
    )
    .await;
    assert_eq!(r1["name"], "Mirror");
    // Second import with same name — should become "Mirror (2)".
    let r2 = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
            "new_name": "Mirror",
        }),
    )
    .await;
    assert_eq!(r2["name"], "Mirror (2)");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_source_not_found() {
    let (rt, aid_p, _aid_s, _sid, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt, aid_p, Some("s".to_string()));
    let missing = ArtifactId::new();
    let r = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": missing.to_string(),
            "source_sheet_id": "sh_anything",
        }),
    )
    .await;
    assert_eq!(r["error"], "source_artifact_not_found");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_sheet_not_found() {
    let (rt, aid_p, aid_s, _sid, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt, aid_p, Some("s".to_string()));
    let r = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": "sh_doesnotexist",
        }),
    )
    .await;
    assert_eq!(r["error"], "source_sheet_not_found");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_self_import() {
    let (rt, aid_p, _aid_s, _sid_src, tmp) = make_two_artifacts().await;
    // Add a sheet to principal so we have something to "self-import".
    let entry_p = rt.registry.get(&aid_p).unwrap();
    let own_sid = apply_add_sheet(&entry_p.doc, "Owned");
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    let r = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_p.to_string(), // same as ctx → forbidden
            "source_sheet_id": own_sid,
        }),
    )
    .await;
    assert_eq!(r["error"], "self_import_forbidden");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_invalid_source_id() {
    let (rt, aid_p, _aid_s, _sid, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt, aid_p, Some("s".to_string()));
    let r = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": "not_a_ulid",
            "source_sheet_id": "sh_x",
        }),
    )
    .await;
    assert_eq!(r["error"], "invalid_artifact_id");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_max_sheets_in_dest() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    // Pre-fill the principal with MAX_SHEETS_PER_ARTIFACT sheets.
    let entry_p = rt.registry.get(&aid_p).unwrap();
    for i in 0..MAX_SHEETS_PER_ARTIFACT {
        let _ = apply_add_sheet(&entry_p.doc, &format!("filler_{i}"));
    }
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    let r = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
        }),
    )
    .await;
    assert_eq!(r["error"], "max_sheets_in_artifact_exceeded");
    assert_eq!(r["current"], MAX_SHEETS_PER_ARTIFACT);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_records_audit_event_with_source() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s_audit".to_string()));
    let _ = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
        }),
    )
    .await;
    // Audit log of the principal should have at least one event mentioning the source.
    // We don't exclude origin (None) so we also see the import event written under
    // origin=agent:s_audit.
    let events = ctx
        .backend()
        .events_since(&aid_p, 0, None, None, 10)
        .await
        .expect("events_since");
    assert!(!events.is_empty());
    let import_event = events
        .iter()
        .find(|e| e.summary.contains("imported sheet"))
        .expect("missing import event");
    let aid_s_str = aid_s.to_string();
    // Source artifact recognizable via the first 4 chars of ULID
    assert!(
        import_event.summary.contains(&aid_s_str[4..8])
            || import_event.summary.contains(&aid_s_str),
        "summary: {}",
        import_event.summary
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_marks_dirty_for_snapshot_writer() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    let entry_p = rt.registry.get(&aid_p).unwrap();
    // Reset dirty flag to false to detect that import sets it.
    entry_p
        .dirty
        .store(false, std::sync::atomic::Ordering::Release);
    let _ = dispatch_crdt_doc_import_sheet(
        &ctx,
        json!({
            "source_artifact_id": aid_s.to_string(),
            "source_sheet_id": sid_src,
        }),
    )
    .await;
    assert!(entry_p.dirty.load(std::sync::atomic::Ordering::Acquire));
    let _ = std::fs::remove_dir_all(&tmp);
}
