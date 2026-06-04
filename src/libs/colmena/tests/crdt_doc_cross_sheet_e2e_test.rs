//! End-to-end integration test for subsystem F.
//! Exercises: 2 artifacts → import_sheet from B to A → run_python
//! with multi-sheet (original A + cloned B) → row-diff by key.
//!
//! #[ignore] because it requires pandas+numpy in the embedded Python (PyO3).
//! Install in .venv: pip install pandas numpy scipy
//! Run with: source .env && cargo test --test crdt_doc_cross_sheet_e2e_test -- --ignored

use colmena::crdt_documents::{
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_import_sheet::{execute_import_sheet, ImportSheetArgs},
    crdt_doc_run_python::{execute_run_python, RunPythonArgs},
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python — pip install pandas numpy scipy"]
async fn cross_sheet_row_diff_via_clone_plus_run_python() {
    // Ensure the embedded Python interpreter is initialized BEFORE the sandbox
    // helper spawns a blocking task that calls into pyo3 (the `auto-initialize`
    // feature is intentionally off for this crate). Safe to call repeatedly.
    pyo3::prepare_freethreaded_python();

    // Force the in-memory ChangeTrackerStore for this test even when the test
    // harness was invoked with a Postgres `DATABASE_URL` in the environment
    // (e.g. via `source .env`). The sqlx Any driver does not support
    // `timestamptz`, so picking up a Postgres URL here would fail at decode
    // time when we read `events_since`.
    // SAFETY: env mutation is process-global. This is the only `#[test]` in
    // this binary so no concurrent test races us.
    unsafe {
        std::env::remove_var("DATABASE_URL");
    }

    // 1. Two artifacts, both with an "Inventory" sheet.
    let tmp = std::env::temp_dir().join(format!("cs_e2e_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({"storage_backend": "localfs", "storage_root": tmp.to_str().unwrap()});
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid_q3 = ArtifactId::new();
    let aid_q4 = ArtifactId::new();
    let entry_q3 = rt.registry.get_or_create(&aid_q3, "Q3");
    let entry_q4 = rt.registry.get_or_create(&aid_q4, "Q4");

    // Q3: header SKU/Price, rows A/100, B/200, C/300
    let sid_q3 = apply_add_sheet(&entry_q3.doc, "Inventory");
    apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, "A1", &json!("SKU"));
    apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, "B1", &json!("Price"));
    for (i, (sku, price)) in [("A", 100), ("B", 200), ("C", 300)].iter().enumerate() {
        apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, &format!("A{}", i + 2), &json!(sku));
        apply_set_cell_in_proc(
            &entry_q3.doc,
            &sid_q3,
            &format!("B{}", i + 2),
            &json!(price),
        );
    }

    // Q4: B/250 (changed), C/300 (same), D/400 (new). A is gone.
    let sid_q4 = apply_add_sheet(&entry_q4.doc, "Inventory");
    apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, "A1", &json!("SKU"));
    apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, "B1", &json!("Price"));
    for (i, (sku, price)) in [("B", 250), ("C", 300), ("D", 400)].iter().enumerate() {
        apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, &format!("A{}", i + 2), &json!(sku));
        apply_set_cell_in_proc(
            &entry_q4.doc,
            &sid_q4,
            &format!("B{}", i + 2),
            &json!(price),
        );
    }

    // 2. ctx is pinned to Q3 (the principal). Import the Q4 sheet into it.
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_q3.clone(), Some("agent_e2e".to_string()));
    let import_r = execute_import_sheet(
        &ctx,
        ImportSheetArgs {
            source_artifact_id: aid_q4.to_string(),
            source_sheet_id: sid_q4.clone(),
            new_name: Some("Q4_Inventory".to_string()),
        },
    )
    .await;
    assert!(import_r["error"].is_null(), "import error: {import_r:?}");
    let cloned_sid = import_r["sheet_id"].as_str().expect("sheet_id").to_string();

    // 3. run_python with both sheets — row-diff by SKU (Pattern B simplified).
    //    Headers are in row 1 of the Y.Doc, so they ALREADY become DataFrame
    //    column names automatically (no title row to promote). Merge by SKU
    //    with outer + indicator to get the diff status per row.
    let code = format!(
        r#"
a, b = dfs["{sid_q3}"], dfs["{cloned}"]
m = a.merge(b, on='SKU', how='outer', suffixes=('_q3','_q4'), indicator=True)
m['_status'] = m['_merge'].map({{'left_only':'only_in_Q3','right_only':'only_in_Q4','both':'present_in_both'}})
output_sheet = m.drop(columns='_merge')
output = m['_status'].value_counts().to_dict()
"#,
        sid_q3 = sid_q3,
        cloned = cloned_sid,
    );
    let py = execute_run_python(
        &ctx,
        RunPythonArgs {
            sheet_ids: vec![sid_q3.clone(), cloned_sid.clone()],
            code,
            write_to_sheet: Some("Diff Q3 vs Q4".to_string()),
        },
    )
    .await;
    assert!(py["error"].is_null(), "py error: {py:?}");

    // 4. Assertions on the row-diff output:
    //    - SKU A: only_in_Q3
    //    - SKU B: both (price changed)
    //    - SKU C: both (price same)
    //    - SKU D: only_in_Q4
    let counts = py["output"].as_object().expect("output dict");
    assert_eq!(counts["only_in_Q3"].as_i64().unwrap(), 1);
    assert_eq!(counts["only_in_Q4"].as_i64().unwrap(), 1);
    assert_eq!(counts["present_in_both"].as_i64().unwrap(), 2);

    // 5. Principal now has 3 sheets: original + cloned + diff.
    let proj = colmena::crdt_documents::projection::project(&entry_q3.doc);
    assert_eq!(proj["sheets"].as_array().unwrap().len(), 3);

    // 6. Two events recorded for Q3: import + write_to_sheet. We use the same
    //    audit-listing API as the F-T2 unit test (events_since with the same
    //    signature: artifact_id, after_id, since_ts, origin_filter, limit).
    let events = ctx
        .backend()
        .events_since(&aid_q3, 0, None, None, 10)
        .await
        .expect("events");
    assert!(events.len() >= 2);
    assert!(events.iter().any(|e| e.summary.contains("imported sheet")));
    assert!(events.iter().any(|e| e.summary.contains("Diff Q3 vs Q4")));

    let _ = std::fs::remove_dir_all(&tmp);
}
