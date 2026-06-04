//! End-to-end test for `crdt_doc_run_python` tool. Exercises:
//! - Reading a sheet's data as a pandas DataFrame.
//! - Computing aggregations server-side and returning to LLM.
//! - Writing a DataFrame back as a new sheet.
//! - Name collision resolution.
//!
//! These tests are #[ignore] because they require pandas + numpy + scipy
//! installed in the system Python that PyO3 links against. Install in the
//! project venv:
//!   .venv/bin/pip install pandas numpy scipy
//! Then run with: cargo test --test crdt_doc_run_python_test -- --ignored

use colmena::crdt_documents::{
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_run_python::{execute_run_python, RunPythonArgs},
};
use serde_json::json;
use std::sync::Arc;

async fn make_test_ctx() -> (
    CrdtDocsContext,
    ArtifactId,
    Arc<CrdtDocumentsRuntime>,
    std::path::PathBuf,
    String, // inventory sheet_id
) {
    let tmp = std::env::temp_dir().join(format!("rp_test_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({
        "storage_backend": "localfs",
        "storage_root": tmp.to_str().unwrap(),
    });
    let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid = ArtifactId::new();
    let entry = runtime.registry.get_or_create(&aid, "test");

    // Seed Inventory with sample data: 4 rows of (Region, Sales).
    let sheet_id = apply_add_sheet(&entry.doc, "Inventory");
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A1", &json!("Region"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B1", &json!("Sales"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A2", &json!("North"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B2", &json!(100));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A3", &json!("North"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B3", &json!(200));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A4", &json!("South"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B4", &json!(150));

    let ctx = CrdtDocsContext::new_local(
        runtime.clone(),
        aid.clone(),
        Some("test_session".to_string()),
    );
    (ctx, aid, runtime, tmp, sheet_id)
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python — install with .venv/bin/pip install pandas numpy scipy"]
async fn run_python_aggregation_returns_output_to_llm() {
    let (ctx, _aid, _rt, tmp, sheet_id) = make_test_ctx().await;

    let args = RunPythonArgs {
        sheet_ids: vec![sheet_id.clone()],
        code: format!(
            r#"df = dfs["{sheet_id}"]
totals = df.groupby('Region')['Sales'].sum()
output = totals.to_dict()
"#
        ),
        write_to_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(
        result["error"].is_null(),
        "got error: {:?}",
        result["error"]
    );
    let totals = result["output"].as_object().expect("output is object");
    // Note: Y.Doc serializes numbers as f64, so 300 round-trips as 300.0.
    // pandas aggregations preserve the float type.
    assert_eq!(totals["North"], json!(300.0));
    assert_eq!(totals["South"], json!(150.0));
    assert!(result["wrote_sheet"].is_null());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn run_python_write_to_sheet_creates_new_sheet() {
    let (ctx, _aid, runtime, tmp, sheet_id) = make_test_ctx().await;

    let args = RunPythonArgs {
        sheet_ids: vec![sheet_id.clone()],
        code: format!(
            r#"df = dfs["{sheet_id}"]
output_sheet = df.groupby('Region')['Sales'].sum().reset_index()
output = "summary written"
"#
        ),
        write_to_sheet: Some("Summary".to_string()),
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(
        result["error"].is_null(),
        "got error: {:?}",
        result["error"]
    );
    let wrote = &result["wrote_sheet"];
    assert_eq!(wrote["name"], "Summary");
    assert_eq!(wrote["n_rows"], 2);
    assert_eq!(wrote["n_cols"], 2);

    // Verify the new sheet exists in the runtime's projection.
    let entry = runtime.registry.get(ctx.artifact_id()).unwrap();
    let proj = colmena::crdt_documents::projection::project(&entry.doc);
    let sheets = proj["sheets"].as_array().unwrap();
    let summary = sheets
        .iter()
        .find(|s| s["name"] == "Summary")
        .expect("Summary sheet exists");
    // Headers written to row 1.
    assert_eq!(summary["cells"]["A1"], json!("Region"));
    assert_eq!(summary["cells"]["B1"], json!("Sales"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn run_python_name_collision_appends_suffix() {
    let (ctx, _aid, runtime, tmp, sheet_id) = make_test_ctx().await;
    // Pre-create a sheet named "Summary" so the run_python writeback hits a collision.
    let entry = runtime.registry.get(ctx.artifact_id()).unwrap();
    let _ = apply_add_sheet(&entry.doc, "Summary");

    let args = RunPythonArgs {
        sheet_ids: vec![sheet_id.clone()],
        code: format!(
            r#"df = dfs["{sheet_id}"]
output_sheet = df.head(1)
output = "ok"
"#
        ),
        write_to_sheet: Some("Summary".to_string()),
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(
        result["error"].is_null(),
        "got error: {:?}",
        result["error"]
    );
    assert_eq!(result["wrote_sheet"]["name"], "Summary (2)");

    let _ = std::fs::remove_dir_all(&tmp);
}
