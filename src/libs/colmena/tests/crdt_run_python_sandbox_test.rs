//! Verifies that `crdt_doc_run_python` enforces the sandbox: banned
//! imports/builtins are rejected; allowed ones pass.
//!
//! All tests are #[ignore] because they require pandas+numpy+scipy in
//! the system Python that PyO3 links against (the auto-prelude does
//! `import pandas as pd`). Run with:
//!   cargo test --test crdt_run_python_sandbox_test -- --ignored

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

async fn make_minimal_ctx() -> (CrdtDocsContext, String, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("rps_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({
        "storage_backend": "localfs",
        "storage_root": tmp.to_str().unwrap(),
    });
    let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid = ArtifactId::new();
    let entry = runtime.registry.get_or_create(&aid, "test");
    let sid = apply_add_sheet(&entry.doc, "S");
    let _ = apply_set_cell_in_proc(&entry.doc, &sid, "A1", &json!("x"));
    let _ = apply_set_cell_in_proc(&entry.doc, &sid, "A2", &json!("y"));
    let ctx = CrdtDocsContext::new_local(runtime, aid, Some("sb_test".to_string()));
    (ctx, sid, tmp)
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn sandbox_rejects_requests_import() {
    let (ctx, sid, tmp) = make_minimal_ctx().await;
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: "import requests\noutput = 1".to_string(),
        write_to_sheet: None,
        on_existing_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    let err = result["error"].as_str().expect("expected error string");
    assert!(
        err.contains("requests") || err.contains("not allowed"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn sandbox_rejects_open_call() {
    let (ctx, sid, tmp) = make_minimal_ctx().await;
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: "f = open('/etc/passwd', 'r')\noutput = f.read()".to_string(),
        write_to_sheet: None,
        on_existing_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    let err = result["error"].as_str().expect("expected error string");
    assert!(
        err.contains("open") || err.contains("not allowed"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn sandbox_allows_numpy_computation() {
    let (ctx, sid, tmp) = make_minimal_ctx().await;
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: "import numpy as np\noutput = int(np.array([1,2,3]).sum())".to_string(),
        write_to_sheet: None,
        on_existing_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(
        result["error"].is_null(),
        "unexpected error: {:?}",
        result["error"]
    );
    assert_eq!(result["output"], json!(6));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+scipy in system Python"]
async fn sandbox_allows_scipy_stats() {
    let (ctx, sid, tmp) = make_minimal_ctx().await;
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: r#"from scipy import stats
result = stats.describe([1,2,3,4,5])
output = {"mean": float(result.mean), "n": int(result.nobs)}
"#
        .to_string(),
        write_to_sheet: None,
        on_existing_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(
        result["error"].is_null(),
        "unexpected error: {:?}",
        result["error"]
    );
    assert_eq!(result["output"]["mean"], json!(3.0));
    assert_eq!(result["output"]["n"], json!(5));
    let _ = std::fs::remove_dir_all(&tmp);
}
