//! Import the spike fixture → mutate via tool helpers → export → re-import
//! and verify isomorphism on cell values.

use colmena::crdt_documents::{
    projection::project,
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    xlsx_export::export_doc_to_xlsx,
    xlsx_import::import_xlsx_into_doc,
};
use serde_json::json;
use yrs::Doc;

#[test]
fn import_mutate_export_reimport_isomorphic_on_values() {
    // Try a couple of paths since cargo test --test changes the CWD.
    let fixture = std::fs::read("spike/fixtures/test.xlsx")
        .or_else(|_| std::fs::read("../../spike/fixtures/test.xlsx"))
        .or_else(|_| std::fs::read("../../../spike/fixtures/test.xlsx"))
        .expect("spike fixture should be reachable");

    let doc = Doc::new();
    let stats = import_xlsx_into_doc(&doc, &fixture).unwrap();
    assert!(
        stats.cells_imported >= 700,
        "expected ≥700 cells, got {}",
        stats.cells_imported
    );

    // Add a new sheet with two cells.
    let new_sheet = apply_add_sheet(&doc, "Notes");
    let _ = apply_set_cell_in_proc(&doc, &new_sheet, "A1", &json!("Hello"));
    let _ = apply_set_cell_in_proc(&doc, &new_sheet, "B1", &json!(123));

    // Export.
    let exported = export_doc_to_xlsx(&doc).unwrap();
    assert_eq!(&exported[..2], b"PK");

    // Re-import into a fresh doc.
    let doc2 = Doc::new();
    import_xlsx_into_doc(&doc2, &exported).unwrap();

    let v = project(&doc2);
    let sheets = v["sheets"].as_array().unwrap();

    // The notes sheet survived.
    let notes = sheets
        .iter()
        .find(|s| s["name"] == "Notes")
        .expect("Notes sheet present after re-import");
    assert_eq!(notes["cells"]["A1"], "Hello");
    assert_eq!(notes["cells"]["B1"], json!(123.0));

    // The original fixture's sheet survived.
    let hoja1 = sheets
        .iter()
        .find(|s| s["name"] == "Hoja1")
        .expect("Hoja1 sheet present after re-import");
    assert_eq!(hoja1["cells"]["A3"], "SKU-0001");
}
