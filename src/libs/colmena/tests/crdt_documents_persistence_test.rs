//! Build a runtime, mutate state, drop the runtime, rebuild from disk,
//! verify the state survived.

use colmena::crdt_documents::{
    projection::project,
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn write_drop_reload_survives() {
    let tmp = std::env::temp_dir().join(format!("persist_{}", ulid::Ulid::new()));
    let cfg = json!({ "storage_root": tmp.to_str().unwrap() });

    let id = ArtifactId::new();

    {
        let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
        let entry = rt.registry.get_or_create(&id, "persist-test");
        let s = apply_add_sheet(&entry.doc, "S");
        apply_set_cell_in_proc(&entry.doc, &s, "A1", &json!("hello"));
        entry.mark_dirty();

        // Wait past the 5s snapshot tick so the writer task flushes.
        tokio::time::sleep(Duration::from_millis(6_000)).await;
    }
    // Original runtime dropped here. Brief sleep so any in-flight write completes.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let rt2 = CrdtDocumentsRuntime::from_config(&cfg).await.unwrap();
    let entry = rt2.registry.get(&id).expect("artifact reloaded from disk");
    let proj = project(&entry.doc);
    assert_eq!(proj["sheets"][0]["cells"]["A1"], "hello");
    assert_eq!(proj["sheets"][0]["name"], "S");

    let _ = std::fs::remove_dir_all(&tmp);
}
