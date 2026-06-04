# Subsystem F (CRDT Cross-Sheet & Cross-Artifact Analysis) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Habilitar al agente para comparar, unir, enriquecer o transformar datos entre sheets dentro del mismo artifact o trayéndolas desde otros artifacts vía clonado. Reusa íntegramente `crdt_doc_run_python` (subsistema C); agrega 2 tools nuevos + 1 extensión + 1 skill.

**Architecture:** Modelo "principal + secundarios" — el agente opera desde un artifact pinneado y clona sheets de otros artifacts al actual. Una vez clonada, la sheet vive en el mismo Y.Doc del principal y se procesa con la infraestructura multi-sheet existente. Cross-session-friendly por diseño (los tools no enforcean session ownership; solo la discovery vía `list_my_artifacts` es session-scoped).

**Tech Stack:** Rust + yrs (Y.Doc CRDT) + PyO3 sandboxed pandas + schemars (tool schemas) + sqlx (audit log reuse from B) + axum WS sync. Cero cambios a ADP, todo en branch `feature/docs`.

**Spec:** `docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md`

---

## File Structure

**Crear:**

- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs` — módulo nuevo aislado para `crdt_doc_import_sheet`. Espejo en estructura a `crdt_doc_run_python.rs` (tool def + Args struct + execute fn + dispatch wrapper + helpers).
- `src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md` — skill builtin nueva.
- `src/libs/colmena/tests/crdt_doc_import_sheet_test.rs` — unit tests del clonado.
- `src/libs/colmena/tests/crdt_doc_list_sheets_of_test.rs` — unit tests del peek cross-artifact.
- `src/libs/colmena/tests/crdt_doc_cross_sheet_e2e_test.rs` — integration test e2e con pandas (`#[ignore]`).
- `tests/graphs/crdt_documents/f_cross_artifact_smoke.json` — browser smoke graph.
- `tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py` — generador de xlsx Q3/Q4 con overlap parcial.

**Modificar:**

- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` — agregar `TOOL_LIST_SHEETS_OF`, `tool_list_sheets_of()`, `execute_list_sheets_of()`, `dispatch_crdt_doc_list_sheets_of()`. También extender `GetRecentChangesArgs` con `artifact_id?` opcional.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — re-exportar nuevos símbolos.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — agregar las 2 ramas de dispatch nuevas.
- `src/libs/colmena/tests/crdt_doc_recent_changes_test.rs` — agregar 2 tests para `artifact_id?` opcional (filter + backward-compat).
- `docs/developer_guide/38_crdt_documents.md` — nueva sub-sección §5.7.
- `docs/node_configurations.json` — entries para los 2 tools nuevos + actualizar `crdt_doc_get_recent_changes` con el campo `artifact_id` opcional.
- `docs/BACKLOG.md` — 3 items v1.1 (workspace, live link, delete sheet).
- `docs/CHANGELOG_2026-06.md` — sección "4. F — cross-sheet analysis".

---

## Task 1 — F-T1: `crdt_doc_list_sheets_of` tool

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` (append after existing `TOOL_LIST_SHEETS` block, around line 60)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` (re-export new symbols)
- Test: `src/libs/colmena/tests/crdt_doc_list_sheets_of_test.rs` (new file)

- [ ] **Step 1: Write the failing test file**

Create `src/libs/colmena/tests/crdt_doc_list_sheets_of_test.rs`:

```rust
//! Unit tests for `crdt_doc_list_sheets_of` — cross-artifact peek.
//! Verifies no session ownership enforcement (any artifact in the registry
//! is visible). Mirrors test pattern of crdt_doc_recent_changes_test.rs.

use colmena::crdt_documents::{
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_tools::{dispatch_crdt_doc_list_sheets_of, ListSheetsOfArgs},
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
    apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "A1", &json!("Region"));
    apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "B1", &json!("Sales"));
    apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "A2", &json!("North"));
    apply_set_cell_in_proc(&entry_b.doc, &sheet_id, "B2", &json!(100));
    let _entry_a = rt.registry.get_or_create(&aid_a, "A");

    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a.clone(), Some("s".to_string()));
    let result = dispatch_crdt_doc_list_sheets_of(
        &ctx,
        json!({"artifact_id": aid_b.to_string()}),
    )
    .await;
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
    let result = dispatch_crdt_doc_list_sheets_of(
        &ctx,
        json!({"artifact_id": missing.to_string()}),
    )
    .await;
    assert_eq!(result["error"], "artifact_not_found");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn list_sheets_of_rejects_invalid_id() {
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_a, "A");
    let ctx = CrdtDocsContext::new_local(rt, aid_a, Some("s".to_string()));
    let result = dispatch_crdt_doc_list_sheets_of(
        &ctx,
        json!({"artifact_id": "not_a_ulid"}),
    )
    .await;
    assert_eq!(result["error"], "invalid_artifact_id");
    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test crdt_doc_list_sheets_of_test -- --nocapture 2>&1 | tail -10`
Expected: FAIL with "unresolved import" / "no function `dispatch_crdt_doc_list_sheets_of`".

- [ ] **Step 3: Add tool constant, definition, executor, dispatcher in crdt_doc_tools.rs**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`, add **after** the existing `TOOL_LIST_SHEETS` block (after line 62 where `TOOL_READ` etc. begin). Use schemars `JsonSchema` like the existing tools do:

```rust
// ─────────────────────────────────────────────────────────────────────────────
// crdt_doc_list_sheets_of — peek at another artifact's sheets (F)
// ─────────────────────────────────────────────────────────────────────────────

pub const TOOL_LIST_SHEETS_OF: &str = "crdt_doc_list_sheets_of";

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListSheetsOfArgs {
    /// ID del artifact cuyo listado de sheets queremos. Puede ser cualquier
    /// artifact del registry — NO enforce session ownership (el agente debe
    /// haber obtenido el ID legítimamente vía list_my_artifacts, prompt
    /// explícito, o futuro workspace listing).
    pub artifact_id: String,
}

pub fn tool_list_sheets_of() -> ToolDefinition {
    super::build_synthetic_tool::<ListSheetsOfArgs>(
        TOOL_LIST_SHEETS_OF,
        "List the sheets of a different artifact (not the current one). \
         Use this to peek at what's inside another workbook BEFORE deciding \
         to clone a sheet from it via crdt_doc_import_sheet. Returns \
         {artifact_id, name, sheets:[{sheet_id, name, n_rows, n_cols}]}. \
         The agent must already know the target artifact_id (from \
         crdt_doc_list_my_artifacts or from the user's prompt).",
    )
}

pub fn execute_list_sheets_of(
    ctx: &CrdtDocsContext,
    args: ListSheetsOfArgs,
) -> serde_json::Value {
    use crate::crdt_documents::ArtifactId;
    let aid: ArtifactId = match args.artifact_id.parse() {
        Ok(a) => a,
        Err(_) => {
            return serde_json::json!({
                "error": "invalid_artifact_id",
                "value": args.artifact_id,
            });
        }
    };
    let Some(entry) = ctx.runtime().registry.get(&aid) else {
        return serde_json::json!({
            "error": "artifact_not_found",
            "artifact_id": aid.to_string(),
        });
    };
    // Project sheets directly from the Y.Doc — counts computed on-the-fly
    // from each sheet's cells Y.Map (no SQL needed).
    use yrs::{Map, ReadTxn, Transact};
    let txn = entry.doc.transact();
    let Some(yrs::Out::YMap(workbook)) = txn.get_map("workbook").map(yrs::Out::YMap) else {
        return serde_json::json!({
            "artifact_id": aid.to_string(),
            "name": entry.meta.name.clone(),
            "sheets": [],
        });
    };
    let Some(yrs::Out::YArray(sheets_arr)) = workbook.get(&txn, "sheets") else {
        return serde_json::json!({
            "artifact_id": aid.to_string(),
            "name": entry.meta.name.clone(),
            "sheets": [],
        });
    };
    let mut sheets_out = Vec::new();
    for i in 0..sheets_arr.len(&txn) {
        let Some(yrs::Out::YMap(sheet_map)) = sheets_arr.get(&txn, i) else { continue };
        let sid = match sheet_map.get(&txn, "id") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
            _ => continue,
        };
        let name = match sheet_map.get(&txn, "name") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
            _ => String::new(),
        };
        // Compute n_rows / n_cols by walking cells addresses
        let (n_rows, n_cols) = if let Some(yrs::Out::YMap(cells_map)) = sheet_map.get(&txn, "cells") {
            let mut max_row = 0u32;
            let mut max_col = 0u32;
            for (addr, _) in cells_map.iter(&txn) {
                if let Some((r, c)) = parse_a1_to_rc(addr) {
                    if r > max_row { max_row = r; }
                    if c > max_col { max_col = c; }
                }
            }
            (max_row + 1, max_col + 1) // 1-indexed inclusive counts
        } else {
            (0, 0)
        };
        sheets_out.push(serde_json::json!({
            "sheet_id": sid,
            "name": name,
            "n_rows": n_rows,
            "n_cols": n_cols,
        }));
    }
    serde_json::json!({
        "artifact_id": aid.to_string(),
        "name": entry.meta.name.clone(),
        "sheets": sheets_out,
    })
}

pub async fn dispatch_crdt_doc_list_sheets_of(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ListSheetsOfArgs>(args) {
        Ok(a) => execute_list_sheets_of(ctx, a),
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// Internal helper — parses "A1", "AA12" into (row_idx0, col_idx0).
// Returns None if format is invalid.
fn parse_a1_to_rc(addr: &str) -> Option<(u32, u32)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    if split == 0 { return None; }
    let col_part = &addr[..split];
    let row_part = &addr[split..];
    let row: u32 = row_part.parse().ok()?;
    let row = row.checked_sub(1)?;
    let mut col: u32 = 0;
    for ch in col_part.chars() {
        if !ch.is_ascii_alphabetic() { return None; }
        col = col.checked_mul(26)?.checked_add((ch.to_ascii_uppercase() as u32) - ('A' as u32) + 1)?;
    }
    Some((row, col.checked_sub(1)?))
}
```

Add `tool_list_sheets_of()` call to `build_all_crdt_doc_tools()` (line ~579). After the existing tool list, append `tool_list_sheets_of(),`.

- [ ] **Step 4: Re-export in mod.rs**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, update the `pub use crdt_doc_tools::{...}` block. Find the existing line ~176-186 and add the new symbols:

```rust
pub use crdt_doc_tools::{
    build_all_crdt_doc_tools, dispatch_crdt_doc_add_sheet, dispatch_crdt_doc_create_artifact,
    dispatch_crdt_doc_get_recent_changes, dispatch_crdt_doc_list_my_artifacts,
    dispatch_crdt_doc_list_sheets, dispatch_crdt_doc_list_sheets_of, dispatch_crdt_doc_read,
    dispatch_crdt_doc_set_cell, dispatch_crdt_doc_set_range, CrdtDocsContext, ListSheetsOfArgs,
    TOOL_ADD_SHEET as CRDT_DOC_ADD_SHEET_TOOL,
    TOOL_CREATE_ARTIFACT as CRDT_DOC_CREATE_ARTIFACT_TOOL,
    TOOL_GET_RECENT_CHANGES as CRDT_DOC_GET_RECENT_CHANGES_TOOL,
    TOOL_LIST_MY_ARTIFACTS as CRDT_DOC_LIST_MY_ARTIFACTS_TOOL,
    TOOL_LIST_SHEETS as CRDT_DOC_LIST_SHEETS_TOOL,
    TOOL_LIST_SHEETS_OF as CRDT_DOC_LIST_SHEETS_OF_TOOL,
    TOOL_READ as CRDT_DOC_READ_TOOL, TOOL_SET_CELL as CRDT_DOC_SET_CELL_TOOL,
    TOOL_SET_RANGE as CRDT_DOC_SET_RANGE_TOOL,
};
```

- [ ] **Step 5: Wire the dispatch route in dag_tool_executor.rs**

In `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` around line 693-750, the synthetic crdt_doc tools section:

(a) Add to the `use` block (~line 695):

```rust
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    dispatch_crdt_doc_add_sheet, dispatch_crdt_doc_create_artifact,
    dispatch_crdt_doc_get_recent_changes, dispatch_crdt_doc_list_my_artifacts,
    dispatch_crdt_doc_list_sheets, dispatch_crdt_doc_list_sheets_of,
    dispatch_crdt_doc_read, dispatch_crdt_doc_run_python,
    dispatch_crdt_doc_set_cell, dispatch_crdt_doc_set_range,
    CRDT_DOC_ADD_SHEET_TOOL, CRDT_DOC_CREATE_ARTIFACT_TOOL,
    CRDT_DOC_GET_RECENT_CHANGES_TOOL, CRDT_DOC_LIST_MY_ARTIFACTS_TOOL,
    CRDT_DOC_LIST_SHEETS_OF_TOOL, CRDT_DOC_LIST_SHEETS_TOOL, CRDT_DOC_READ_TOOL,
    CRDT_DOC_RUN_PYTHON_TOOL, CRDT_DOC_SET_CELL_TOOL, CRDT_DOC_SET_RANGE_TOOL,
};
```

(b) Add `n == CRDT_DOC_LIST_SHEETS_OF_TOOL` to the boolean filter (around line 709-718).

(c) Add the match arm (around line 732-745) AFTER the existing `n == CRDT_DOC_LIST_SHEETS_TOOL` arm:

```rust
n if n == CRDT_DOC_LIST_SHEETS_OF_TOOL => {
    dispatch_crdt_doc_list_sheets_of(ctx, args).await
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test --test crdt_doc_list_sheets_of_test 2>&1 | tail -10`
Expected: PASS (3 tests).

- [ ] **Step 7: Verify clippy + fmt**

```bash
cargo clippy --lib --tests 2>&1 | tail -5
cargo fmt --check
```
Expected: clean (no warnings).

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/tests/crdt_doc_list_sheets_of_test.rs
git commit -m "feat(crdt_doc_tools): add crdt_doc_list_sheets_of (F-T1)

Peek-at-another-artifact tool. Returns the list of sheets in any
artifact in the registry with name + n_rows + n_cols (computed
on-the-fly from each sheet's cells Y.Map). NO session ownership
check — agent must have legitimately obtained the artifact_id
(via list_my_artifacts, prompt, or future workspace listing).

3 unit tests cover the happy path + the two error cases
(artifact_not_found, invalid_artifact_id)."
```

---

## Task 2 — F-T2: `crdt_doc_import_sheet` tool (the core)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` (add to `build_all_crdt_doc_tools`)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Test: `src/libs/colmena/tests/crdt_doc_import_sheet_test.rs`

- [ ] **Step 1: Write the failing test file**

Create `src/libs/colmena/tests/crdt_doc_import_sheet_test.rs`:

```rust
//! Unit tests for crdt_doc_import_sheet (F-T2).
//! Covers: happy clone, name collision, default name format, all 6 error paths,
//! audit event recording, dirty flag side-effect.

use colmena::crdt_documents::{
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId, CrdtDocumentsRuntime,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_import_sheet::{dispatch_crdt_doc_import_sheet, ImportSheetArgs, MAX_SHEETS_PER_ARTIFACT, MAX_IMPORT_BYTES},
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
    let result = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
    })).await;
    assert!(result["error"].is_null(), "got error: {:?}", result["error"]);
    assert_eq!(result["n_rows"], 2);
    assert_eq!(result["n_cols"], 2);
    assert_eq!(result["source"]["artifact_id"], aid_s.to_string());
    // Verify the principal now has the cloned sheet with same values.
    let entry_p = rt.registry.get(&aid_p).unwrap();
    let proj = colmena::crdt_documents::projection::project(&entry_p.doc);
    let sheets = proj["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0]["cells"]["A1"], json!("Region"));
    assert_eq!(sheets[0]["cells"]["B2"], json!(100));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_default_name_includes_short_source_id() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p, Some("s".to_string()));
    let result = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
    })).await;
    let aid_s_str = aid_s.to_string();
    // Default format: "<original> (from art_xxxx)" where xxxx = first 4 chars of ULID
    let expected_suffix = format!("(from art_{})", &aid_s_str[4..8]);
    let name = result["name"].as_str().expect("name string");
    assert!(name.starts_with("Inventory ("), "name was: {name}");
    assert!(name.contains(&expected_suffix), "name was: {name}, expected to contain {expected_suffix}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_auto_suffixes_on_name_collision() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    // First import — succeeds with "Mirror".
    let r1 = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
        "new_name": "Mirror",
    })).await;
    assert_eq!(r1["name"], "Mirror");
    // Second import with same name — should become "Mirror (2)".
    let r2 = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
        "new_name": "Mirror",
    })).await;
    assert_eq!(r2["name"], "Mirror (2)");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_source_not_found() {
    let (rt, aid_p, _aid_s, _sid, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt, aid_p, Some("s".to_string()));
    let missing = ArtifactId::new();
    let r = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": missing.to_string(),
        "source_sheet_id": "sh_anything",
    })).await;
    assert_eq!(r["error"], "source_artifact_not_found");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_sheet_not_found() {
    let (rt, aid_p, aid_s, _sid, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt, aid_p, Some("s".to_string()));
    let r = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": "sh_doesnotexist",
    })).await;
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
    let r = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_p.to_string(), // same as ctx → forbidden
        "source_sheet_id": own_sid,
    })).await;
    assert_eq!(r["error"], "self_import_forbidden");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_rejects_invalid_source_id() {
    let (rt, aid_p, _aid_s, _sid, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt, aid_p, Some("s".to_string()));
    let r = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": "not_a_ulid",
        "source_sheet_id": "sh_x",
    })).await;
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
    let r = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
    })).await;
    assert_eq!(r["error"], "max_sheets_in_artifact_exceeded");
    assert_eq!(r["current"], MAX_SHEETS_PER_ARTIFACT);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_records_audit_event_with_source() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s_audit".to_string()));
    let _ = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
    })).await;
    // Audit log of the principal should have one event mentioning the source.
    let events = ctx.backend()
        .list_events(&aid_p, None, Some(10), None)
        .await
        .expect("list_events");
    assert!(!events.is_empty());
    let summary = &events[0].summary;
    assert!(summary.contains("imported sheet"), "summary: {summary}");
    let aid_s_str = aid_s.to_string();
    // Should include some recognizable prefix of the source artifact id (first 4 chars of ULID)
    assert!(summary.contains(&aid_s_str[4..8]) || summary.contains(&aid_s_str), "summary: {summary}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_sheet_marks_dirty_for_snapshot_writer() {
    let (rt, aid_p, aid_s, sid_src, tmp) = make_two_artifacts().await;
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_p.clone(), Some("s".to_string()));
    let entry_p = rt.registry.get(&aid_p).unwrap();
    // Reset dirty flag to false to detect that import sets it.
    entry_p.dirty.store(false, std::sync::atomic::Ordering::Release);
    let _ = dispatch_crdt_doc_import_sheet(&ctx, json!({
        "source_artifact_id": aid_s.to_string(),
        "source_sheet_id": sid_src,
    })).await;
    assert!(entry_p.dirty.load(std::sync::atomic::Ordering::Acquire));
    let _ = std::fs::remove_dir_all(&tmp);
}

// Note: MAX_IMPORT_BYTES test (load_size_exceeded) is deferred to integration
// test because building a >100MB Y.Doc in-process is expensive. The constant
// is exported so the integration test can verify behaviour at the boundary
// without inflating unit tests.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test crdt_doc_import_sheet_test 2>&1 | tail -5`
Expected: FAIL with "unresolved import `crdt_doc_import_sheet`".

- [ ] **Step 3: Create the module file**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs`:

```rust
//! LLM tool `crdt_doc_import_sheet` — clone a sheet from any artifact into
//! the current ctx artifact. The core of subsystem F (cross-sheet analysis).
//! See: docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md §3.2

use crate::crdt_documents::{df_writer::resolve_unique_sheet_name, ArtifactId};
use crate::llm::domain::tools::ToolDefinition;
use schemars::JsonSchema;
use serde::Deserialize;
use yrs::{Array, ArrayPrelim, Map, MapPrelim, ReadTxn, Transact};

pub use super::crdt_doc_context::CrdtDocsContext;

pub const TOOL_IMPORT_SHEET: &str = "crdt_doc_import_sheet";

/// Cap matching `crdt_doc_run_python` (100 MB combined load). A single
/// import that would push the destination past this is rejected.
pub const MAX_IMPORT_BYTES: usize = 100 * 1024 * 1024;

/// Hard cap on sheets per artifact (defensive against agents that
/// import in a loop). 100 covers any plausible workflow; bump in v1.1
/// if real usage shows demand.
pub const MAX_SHEETS_PER_ARTIFACT: usize = 100;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportSheetArgs {
    /// Source artifact — where the sheet currently lives. Must be a different
    /// artifact than the current ctx (self-import is rejected).
    pub source_artifact_id: String,
    /// Sheet within the source artifact to clone.
    pub source_sheet_id: String,
    /// Optional new name for the cloned sheet in the destination. Default:
    /// `"<source_name> (from art_xxxx)"` where xxxx are the first 4 chars
    /// of the source ULID (after the "art_" prefix). Name collision auto-
    /// suffixes ` (2)`, ` (3)`, … (delegates to df_writer::resolve_unique_sheet_name).
    #[serde(default)]
    pub new_name: Option<String>,
}

pub fn tool_import_sheet() -> ToolDefinition {
    super::build_synthetic_tool::<ImportSheetArgs>(
        TOOL_IMPORT_SHEET,
        "Clone a sheet from another artifact into the CURRENT artifact \
         (the one your ctx is pinned to). The cloned sheet becomes a normal \
         sheet in the current workbook — you can read it, modify it, or pass \
         it together with original sheets to crdt_doc_run_python for \
         multi-sheet analysis (merge, compare, enrich, etc.). \n\
         IMPORTANT: snapshot only — later changes to the source do NOT \
         propagate. Re-import if you need fresh data. \n\
         Use AFTER crdt_doc_list_sheets_of (to discover the source_sheet_id). \
         See the crdt-doc-cross-sheet-analysis skill for the 6 canonical \
         pandas patterns this enables.",
    )
}

pub async fn execute_import_sheet(
    ctx: &CrdtDocsContext,
    args: ImportSheetArgs,
) -> serde_json::Value {
    // 1. Parse + validate source ID
    let src_aid: ArtifactId = match args.source_artifact_id.parse() {
        Ok(a) => a,
        Err(_) => return serde_json::json!({
            "error": "invalid_artifact_id",
            "value": args.source_artifact_id,
        }),
    };

    // 2. Forbid self-import (catches loops + makes intent explicit)
    if &src_aid == ctx.artifact_id() {
        return serde_json::json!({
            "error": "self_import_forbidden",
            "artifact_id": src_aid.to_string(),
        });
    }

    // 3. Resolve source artifact
    let Some(src_entry) = ctx.runtime().registry.get(&src_aid) else {
        return serde_json::json!({
            "error": "source_artifact_not_found",
            "artifact_id": src_aid.to_string(),
        });
    };

    // 4. Extract the source sheet's cells + name into owned data we can
    //    later write atomically into the destination. Done in a READ
    //    transaction over the source doc.
    let (source_name, cells_to_write, bytes_estimate) = {
        let txn = src_entry.doc.transact();
        let Some(workbook) = txn.get_map("workbook") else {
            return serde_json::json!({
                "error": "source_sheet_not_found",
                "artifact_id": src_aid.to_string(),
                "sheet_id": args.source_sheet_id,
            });
        };
        let Some(yrs::Out::YArray(sheets_arr)) = workbook.get(&txn, "sheets") else {
            return serde_json::json!({
                "error": "source_sheet_not_found",
                "artifact_id": src_aid.to_string(),
                "sheet_id": args.source_sheet_id,
            });
        };
        let mut found_name: Option<String> = None;
        let mut cells = Vec::<(String, serde_json::Value, String)>::new();
        let mut bytes: usize = 0;
        for i in 0..sheets_arr.len(&txn) {
            let Some(yrs::Out::YMap(sheet_map)) = sheets_arr.get(&txn, i) else { continue };
            let sid = match sheet_map.get(&txn, "id") {
                Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
                _ => continue,
            };
            if sid != args.source_sheet_id { continue; }
            let name = match sheet_map.get(&txn, "name") {
                Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
                _ => String::new(),
            };
            found_name = Some(name);
            if let Some(yrs::Out::YMap(cells_map)) = sheet_map.get(&txn, "cells") {
                for (addr, cell_out) in cells_map.iter(&txn) {
                    if let yrs::Out::YMap(cell_map) = cell_out {
                        let v_json = match cell_map.get(&txn, "v") {
                            Some(yrs::Out::Any(any)) => any_to_json(&any),
                            _ => serde_json::Value::Null,
                        };
                        let t = match cell_map.get(&txn, "t") {
                            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
                            _ => "s".to_string(),
                        };
                        bytes += addr.len() + v_json.to_string().len() + t.len() + 8;
                        cells.push((addr.to_string(), v_json, t));
                    }
                }
            }
            break;
        }
        let Some(name) = found_name else {
            return serde_json::json!({
                "error": "source_sheet_not_found",
                "artifact_id": src_aid.to_string(),
                "sheet_id": args.source_sheet_id,
            });
        };
        (name, cells, bytes)
    };

    // 5. Enforce size cap
    if bytes_estimate > MAX_IMPORT_BYTES {
        return serde_json::json!({
            "error": "load_size_exceeded",
            "actual_bytes": bytes_estimate,
            "limit_bytes": MAX_IMPORT_BYTES,
        });
    }

    // 6. Resolve destination
    let Some(dest_doc) = ctx.doc() else {
        return serde_json::json!({"error": "artifact_not_found"});
    };

    // 7. Enforce max-sheets-per-artifact on destination
    let n_existing = {
        let txn = dest_doc.transact();
        match txn.get_map("workbook").and_then(|wb| wb.get(&txn, "sheets")) {
            Some(yrs::Out::YArray(arr)) => arr.len(&txn) as usize,
            _ => 0,
        }
    };
    if n_existing >= MAX_SHEETS_PER_ARTIFACT {
        return serde_json::json!({
            "error": "max_sheets_in_artifact_exceeded",
            "current": n_existing,
            "limit": MAX_SHEETS_PER_ARTIFACT,
        });
    }

    // 8. Compute the destination sheet name with collision resolution
    let src_aid_str = src_aid.to_string();
    let proposed_name = args.new_name.unwrap_or_else(|| {
        format!("{} (from art_{})", source_name, &src_aid_str[4..8])
    });
    let final_name = resolve_unique_sheet_name(&dest_doc, &proposed_name);

    // 9. Write atomically — ONE transact_mut, all cells in one go
    let new_sheet_id = format!("sh_{}", ulid::Ulid::new());
    let (n_rows, n_cols) = {
        let mut txn = dest_doc.transact_mut();
        let workbook = match txn.get_map("workbook") {
            Some(wb) => wb,
            None => txn.insert("workbook", MapPrelim::default()),
        };
        let sheets_arr = match workbook.get(&txn, "sheets") {
            Some(yrs::Out::YArray(a)) => a,
            _ => workbook.insert(&mut txn, "sheets", ArrayPrelim::default()),
        };
        let new_sheet = sheets_arr.push_back(&mut txn, MapPrelim::default());
        new_sheet.insert(&mut txn, "id", new_sheet_id.clone());
        new_sheet.insert(&mut txn, "name", final_name.clone());
        let cells_map = new_sheet.insert(&mut txn, "cells", MapPrelim::default());
        let mut max_row = 0u32;
        let mut max_col = 0u32;
        for (addr, v_json, t) in &cells_to_write {
            if let Some((r, c)) = super::crdt_doc_tools::parse_a1_to_rc(addr) {
                if r > max_row { max_row = r; }
                if c > max_col { max_col = c; }
            }
            let cell_map = cells_map.insert(&mut txn, addr.as_str(), MapPrelim::default());
            cell_map.insert(&mut txn, "v", json_to_any(v_json));
            cell_map.insert(&mut txn, "t", t.clone());
        }
        let n_rows = if cells_to_write.is_empty() { 0 } else { max_row + 1 };
        let n_cols = if cells_to_write.is_empty() { 0 } else { max_col + 1 };
        (n_rows, n_cols)
    };

    // 10. Side-effects: dirty flag + audit event
    ctx.mark_dirty();
    let origin = ctx.session_id()
        .map(|s| format!("agent:{s}"))
        .unwrap_or_else(|| "agent:llm".to_string());
    let event_id = ctx.backend()
        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: ctx.artifact_id().clone(),
            sheet_id: Some(new_sheet_id.clone()),
            origin,
            summary: format!(
                "imported sheet '{}' ({} rows × {} cols) from artifact art_{}",
                source_name, n_rows, n_cols, &src_aid_str[4..8]
            ),
        })
        .await
        .unwrap_or(0);
    ctx.record_event_id(event_id);

    serde_json::json!({
        "sheet_id": new_sheet_id,
        "name": final_name,
        "n_rows": n_rows,
        "n_cols": n_cols,
        "source": {
            "artifact_id": src_aid.to_string(),
            "sheet_id": args.source_sheet_id,
            "name": source_name,
        },
    })
}

pub async fn dispatch_crdt_doc_import_sheet(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ImportSheetArgs>(args) {
        Ok(a) => execute_import_sheet(ctx, a).await,
        Err(e) => serde_json::json!({"error": format!("invalid_args: {e}")}),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn any_to_json(a: &yrs::Any) -> serde_json::Value {
    match a {
        yrs::Any::Null | yrs::Any::Undefined => serde_json::Value::Null,
        yrs::Any::Bool(b) => serde_json::Value::Bool(*b),
        yrs::Any::Number(n) => serde_json::json!(n),
        yrs::Any::BigInt(i) => serde_json::json!(i),
        yrs::Any::String(s) => serde_json::Value::String(s.to_string()),
        _ => serde_json::Value::Null,
    }
}

fn json_to_any(v: &serde_json::Value) -> yrs::Any {
    match v {
        serde_json::Value::Null => yrs::Any::Null,
        serde_json::Value::Bool(b) => yrs::Any::Bool(*b),
        serde_json::Value::Number(n) => n.as_f64()
            .map(yrs::Any::Number)
            .unwrap_or(yrs::Any::Null),
        serde_json::Value::String(s) => yrs::Any::String(s.clone().into()),
        _ => yrs::Any::Null,
    }
}
```

NOTE: `parse_a1_to_rc` must be made `pub(super)` in `crdt_doc_tools.rs` so this module can use it. Update the function signature in crdt_doc_tools.rs from `fn parse_a1_to_rc` to `pub(super) fn parse_a1_to_rc`.

- [ ] **Step 4: Register the module in mod.rs**

Append to `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` after the existing `pub mod crdt_doc_run_python;` line (around line 4-12):

```rust
pub mod crdt_doc_import_sheet;
```

And add to the re-exports (after the existing `pub use crdt_doc_run_python::{...}` block around line 188):

```rust
pub use crdt_doc_import_sheet::{
    dispatch_crdt_doc_import_sheet, execute_import_sheet, tool_import_sheet, ImportSheetArgs,
    MAX_IMPORT_BYTES, MAX_SHEETS_PER_ARTIFACT, TOOL_IMPORT_SHEET as CRDT_DOC_IMPORT_SHEET_TOOL,
};
```

- [ ] **Step 5: Register tool in build_all_crdt_doc_tools**

In `crdt_doc_tools.rs` function `build_all_crdt_doc_tools()` (around line 579), add `super::crdt_doc_import_sheet::tool_import_sheet()` to the returned `vec![]`.

- [ ] **Step 6: Wire dispatch in dag_tool_executor.rs**

(a) Add to the use block (line ~695):
```rust
dispatch_crdt_doc_import_sheet, CRDT_DOC_IMPORT_SHEET_TOOL,
```
(b) Add to the boolean filter (line ~709-718):
```rust
|| n == CRDT_DOC_IMPORT_SHEET_TOOL
```
(c) Add the match arm (after the list_sheets_of arm):
```rust
n if n == CRDT_DOC_IMPORT_SHEET_TOOL => {
    dispatch_crdt_doc_import_sheet(ctx, args).await
}
```

- [ ] **Step 7: Run tests to verify they pass**

```bash
cargo test --test crdt_doc_import_sheet_test 2>&1 | tail -15
```
Expected: PASS (10 tests).

- [ ] **Step 8: Verify clippy + fmt**

```bash
cargo clippy --lib --tests 2>&1 | tail -5
cargo fmt --check
```
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/tests/crdt_doc_import_sheet_test.rs
git commit -m "feat(crdt_doc_import_sheet): clone sheets from any artifact (F-T2)

Core tool of subsystem F. Reads the source sheet (any artifact in the
registry, no session enforcement), writes its cells into a new sheet in
the current ctx artifact in a single transact_mut. Snapshot only — no
live link.

10 unit tests cover happy path, name collision (\" (2)\" suffix),
default name format (\"<orig> (from art_xxxx)\"), 6 error paths
(invalid_artifact_id, source_artifact_not_found, source_sheet_not_found,
self_import_forbidden, max_sheets_in_artifact_exceeded), audit event
recording with source visible, and dirty flag side-effect.

Caps: MAX_IMPORT_BYTES = 100 MB (matches run_python), MAX_SHEETS_PER_ARTIFACT
= 100 (defensive vs runaway import loops). Both exported as pub const for
integration tests."
```

---

## Task 3 — F-T3: Extend `crdt_doc_get_recent_changes` with `artifact_id?`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` (extend `GetRecentChangesArgs` + dispatcher)
- Modify: `src/libs/colmena/tests/crdt_doc_recent_changes_test.rs` (add 2 tests)

- [ ] **Step 1: Write the failing tests**

Append to `src/libs/colmena/tests/crdt_doc_recent_changes_test.rs`:

```rust
#[tokio::test]
async fn get_recent_changes_artifact_filter_works() {
    // Cross-artifact audit: ctx is on A, but agent queries events of B.
    use colmena::crdt_documents::change_tracker_store::NewEvent;
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let aid_b = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_a, "A");
    let _ = rt.registry.get_or_create(&aid_b, "B");
    rt.change_tracker_store
        .record_event(NewEvent {
            artifact_id: aid_b.clone(),
            sheet_id: None,
            origin: "agent:other_session".to_string(),
            summary: "did something in B".to_string(),
        })
        .await
        .unwrap();
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a, Some("s".to_string()));
    let result = dispatch_crdt_doc_get_recent_changes(&ctx, json!({
        "artifact_id": aid_b.to_string(),
    })).await;
    let events = result["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["artifact_id"], aid_b.to_string());
    assert_eq!(events[0]["summary"], "did something in B");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn get_recent_changes_backward_compat_no_arg() {
    // No artifact_id → audits the ctx artifact (B behaviour unchanged).
    use colmena::crdt_documents::change_tracker_store::NewEvent;
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_a, "A");
    rt.change_tracker_store
        .record_event(NewEvent {
            artifact_id: aid_a.clone(),
            sheet_id: None,
            origin: "agent:s".to_string(),
            summary: "did something in A".to_string(),
        })
        .await
        .unwrap();
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a.clone(), Some("s".to_string()));
    // Call with empty args object (no artifact_id) — should default to ctx
    let result = dispatch_crdt_doc_get_recent_changes(&ctx, json!({})).await;
    let events = result["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["artifact_id"], aid_a.to_string());
    let _ = std::fs::remove_dir_all(&tmp);
}
```

NOTE: this file may need to add `make_runtime()` helper if not present. Check existing test file structure first.

- [ ] **Step 2: Run tests to verify failures**

```bash
cargo test --test crdt_doc_recent_changes_test get_recent_changes_artifact 2>&1 | tail -10
cargo test --test crdt_doc_recent_changes_test get_recent_changes_backward 2>&1 | tail -10
```
Expected: FAIL or panic (the `artifact_id` field doesn't exist yet — Serde will silently ignore in v0; the cross-artifact test will return events of A instead of B).

- [ ] **Step 3: Extend the args struct + dispatcher**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`, find `GetRecentChangesArgs` (around line 360) and add the new field:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetRecentChangesArgs {
    /// Filter events to a specific sheet within the artifact.
    #[serde(default)]
    pub sheet_id: Option<String>,
    /// How many events to return. Default 20, max 100.
    #[serde(default)]
    pub limit: Option<usize>,
    /// NEW in F (subsystem F): if provided, audits THIS artifact instead of
    /// the ctx's pinned artifact. Enables cross-artifact inspection without
    /// rebinding ctx. Default: ctx.artifact_id() (B behaviour unchanged).
    #[serde(default)]
    pub artifact_id: Option<String>,
}
```

Update the `tool_get_recent_changes()` description to mention the new field.

In `execute_get_recent_changes` (around line 387), where it currently uses `ctx.artifact_id()`, change to honour the override:

```rust
pub async fn execute_get_recent_changes(
    ctx: &CrdtDocsContext,
    args: GetRecentChangesArgs,
) -> serde_json::Value {
    use crate::crdt_documents::ArtifactId;
    let target_aid: ArtifactId = match args.artifact_id {
        Some(s) => match s.parse() {
            Ok(a) => a,
            Err(_) => return serde_json::json!({
                "error": "invalid_artifact_id",
                "value": s,
            }),
        },
        None => ctx.artifact_id().clone(),
    };
    // … rest of the existing logic, using target_aid in place of ctx.artifact_id()
}
```

- [ ] **Step 4: Run the 2 new tests + the existing ones**

```bash
cargo test --test crdt_doc_recent_changes_test 2>&1 | tail -15
```
Expected: all PASS (existing + 2 new).

- [ ] **Step 5: Verify clippy**

```bash
cargo clippy --lib --tests 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/tests/crdt_doc_recent_changes_test.rs
git commit -m "feat(crdt_doc_get_recent_changes): optional artifact_id arg (F-T3)

Backward-compatible extension: if artifact_id is provided, the dispatcher
audits that artifact instead of the ctx's pinned one. Enables
cross-artifact 'who touched X?' inspection without rebinding ctx.

Default (no arg) keeps subsystem B's behaviour bit-for-bit.

2 new unit tests: cross-artifact filter works, no-arg backward compat."
```

---

## Task 4 — F-T4: Skill `crdt-doc-cross-sheet-analysis`

**Files:**
- Create: `src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md`

- [ ] **Step 1: Create the skill file**

Create `src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md` with the full body (verbatim — engineers must not paraphrase the patterns):

```markdown
---
name: crdt-doc-cross-sheet-analysis
description: Use when comparing two sheets, joining/enriching data from one sheet into another, or transforming rows based on conditions from another sheet. Activates the workflow list_my_artifacts → list_sheets_of → import_sheet → run_python. Documents 6 canonical pandas patterns with verbatim code snippets. Load this BEFORE writing any compare/join/enrich code.
---

# crdt-doc-cross-sheet-analysis — Operator Manual

The CRDT documents toolkit lets you compare, join, enrich and transform data
across sheets that may live in **different artifacts**. Source sheets are
**cloned** into your current artifact (snapshot, no live link); from that
point on it is standard pandas multi-sheet work.

## The canonical flow

1. `crdt_doc_list_my_artifacts` — discover artifacts in your session.
2. `crdt_doc_list_sheets_of({artifact_id})` — peek at the other artifact's sheets without cloning.
3. `crdt_doc_import_sheet({source_artifact_id, source_sheet_id})` — clone the sheet you need into the current artifact. Returns a new local `sheet_id`.
4. `crdt_doc_run_python({sheet_ids: [original, cloned], code: "...", write_to_sheet: "..."})` — do the analysis.

**Tip:** before importing, check `crdt_doc_list_sheets` of the current artifact — the sheet may already be cloned from a previous turn, in which case you can skip step 3.

## The 6 canonical patterns

All examples assume both DataFrames need the standard header promotion if the
source xlsx had a title row (see the `crdt-doc-run-python` skill). The
boilerplate at the top of every snippet is:

```python
import pandas as pd
a, b = dfs[sid_a], dfs[sid_b]
a.columns = a.iloc[0].tolist(); a = a.iloc[1:].reset_index(drop=True)
b.columns = b.iloc[0].tolist(); b = b.iloc[1:].reset_index(drop=True)
```

### Pattern A — Cell-by-cell diff (same shape, what value changed)

When: two versions of the same report that should have identical layout.

```python
# (boilerplate)
common_cols = [c for c in a.columns if c in b.columns]
diff = a[common_cols].compare(b[common_cols].reindex(a.index), align_axis=1)
diff.columns = [f"{c}_{side}" for c, side in diff.columns]
output_sheet = diff.reset_index(names='row_index')
output = f"{len(diff)} cells changed across {len(common_cols)} columns"
```

### Pattern B — Row diff by key column (the most common case)

When: lists with unique identifiers (SKU, ID, email). Tags each row as
`only_in_A`, `only_in_B`, `changed`, or `unchanged`.

```python
# (boilerplate)
merged = a.merge(b, on='SKU', how='outer', suffixes=('_a','_b'), indicator=True)
merged['_status'] = merged['_merge'].map({
    'left_only':  'only_in_A',
    'right_only': 'only_in_B',
    'both':       'present_in_both',
})
shared = [c.removesuffix('_a') for c in merged.columns
          if c.endswith('_a') and f"{c.removesuffix('_a')}_b" in merged.columns]
def diff_mask(r):
    return any(
        r[f"{c}_a"] != r[f"{c}_b"]
        for c in shared
        if pd.notna(r[f"{c}_a"]) and pd.notna(r[f"{c}_b"])
    )
merged.loc[merged['_status'] == 'present_in_both', '_status'] = merged.apply(
    lambda r: 'changed' if diff_mask(r) else 'unchanged', axis=1)
output_sheet = merged.drop(columns='_merge')
output = merged['_status'].value_counts().to_dict()
```

### Pattern C — Schema diff (which columns exist, with which dtype)

When: quick structural check between two reports.

```python
# (boilerplate)
cols_a, cols_b = set(a.columns), set(b.columns)
all_cols = sorted(cols_a | cols_b)
output_sheet = pd.DataFrame([{
    'column':  c,
    'in_A':    c in cols_a,
    'in_B':    c in cols_b,
    'dtype_A': str(a[c].dtype) if c in cols_a else None,
    'dtype_B': str(b[c].dtype) if c in cols_b else None,
} for c in all_cols])
output = {
    'only_in_A': sorted(cols_a - cols_b),
    'only_in_B': sorted(cols_b - cols_a),
    'in_both':   sorted(cols_a & cols_b),
}
```

### Pattern D — Statistical comparison (numeric drift)

When: you want to know if column distributions changed significantly between two snapshots.

```python
# (boilerplate)
from scipy import stats
numeric_cols = [
    c for c in a.columns
    if c in b.columns and pd.api.types.is_numeric_dtype(a[c])
]
rows = []
for c in numeric_cols:
    sa, sb = a[c].dropna(), b[c].dropna()
    if len(sa) < 2 or len(sb) < 2: continue
    t, p = stats.ttest_ind(sa, sb, equal_var=False)
    rows.append({
        'column':   c,
        'mean_A':   sa.mean(),   'mean_B':   sb.mean(),
        'std_A':    sa.std(),    'std_B':    sb.std(),
        'median_A': sa.median(), 'median_B': sb.median(),
        't_stat':   t,           'p_value':  p,
        'sig':      bool(p < 0.05),
    })
output_sheet = pd.DataFrame(rows)
output = f"{sum(r['sig'] for r in rows)} columns with p<0.05 (significant drift)"
```

### Pattern E — Join / enrich (bring info from another table)

When: you have a primary table (e.g. sales) and want to add columns from a
lookup table (e.g. catalog).

```python
ventas, catalog = dfs[sid_ventas], dfs[sid_catalog]
# (boilerplate for both)
enriched = ventas.merge(catalog[['SKU', 'Category', 'Description']], on='SKU', how='left')
unmatched = enriched[enriched['Category'].isna()]
output_sheet = enriched
output = {
    'rows_enriched':    len(enriched) - len(unmatched),
    'unmatched_count':  len(unmatched),
    'unmatched_sample': unmatched['SKU'].head(5).tolist(),
}
```

### Pattern F — Conditional transform (rules from another table)

When: you have a rules table (e.g. discounts by Region with min Qty) and want
to apply it row-by-row to your primary table.

```python
ventas, reglas = dfs[sid_ventas], dfs[sid_reglas]
# (boilerplate for both)
ventas = ventas.merge(reglas, on='Region', how='left')
mask = ventas['Cantidad'] >= ventas['MinQty']
ventas['Descuento'] = 0.0
ventas.loc[mask, 'Descuento'] = ventas.loc[mask, 'Precio'] * ventas.loc[mask, 'DiscountPct'] / 100
ventas['PrecioFinal'] = ventas['Precio'] - ventas['Descuento']
output_sheet = ventas.drop(columns=['MinQty', 'DiscountPct'])
output = f"Applied discounts to {int(mask.sum())}/{len(ventas)} rows"
```

## Anti-patterns

- ❌ Importing a sheet that is already cloned in this artifact. Always call `crdt_doc_list_sheets` first; the previous turn may have done the import already.
- ❌ Importing the principal back into itself (the tool rejects this with `self_import_forbidden`, but it's a sign you've lost the mental model).
- ❌ Forcing a merge with mixed-type key columns without `pd.to_numeric` / `astype(str)` on both sides. Always cast the join key on both DataFrames to the same dtype.
- ❌ Loading 4 sheets when you only need 2. The 100 MB combined cap applies — be intentional.

## Cleanup

Cloned sheets persist in the current artifact. v1 has no delete-sheet tool;
if you need to free space, point the user at the BACKLOG entry (or rerun in
a fresh artifact). The 100-sheets-per-artifact cap prevents accidental
runaway accumulation.
```

- [ ] **Step 2: Build to verify the skill is picked up by include_dir**

```bash
cargo build --lib 2>&1 | tail -3
```
Expected: success.

- [ ] **Step 3: Run the builtin_skill_repository tests to confirm no regression**

```bash
cargo test --lib builtin_skill 2>&1 | tail -10
```
Expected: all 8 (or more) skill tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/
git commit -m "feat(skills): add crdt-doc-cross-sheet-analysis (F-T4)

Builtin skill that documents the cross-sheet/cross-artifact analysis
workflow: list_my_artifacts → list_sheets_of → import_sheet → run_python.
Includes 6 verbatim pandas patterns:
 - A: cell-by-cell diff (DataFrame.compare)
 - B: row diff by key column (merge + indicator)
 - C: schema diff (set ops on columns)
 - D: statistical comparison (scipy.stats.ttest_ind)
 - E: join/enrich (merge how='left' + unmatched report)
 - F: conditional transform (merge + mask + per-row rule)

Activates via config.skills.builtin: [\"crdt-doc-cross-sheet-analysis\"]."
```

---

## Task 5 — F-T5: Integration test (clone + run_python, requires Python)

**Files:**
- Create: `src/libs/colmena/tests/crdt_doc_cross_sheet_e2e_test.rs`

- [ ] **Step 1: Write the test**

Create `src/libs/colmena/tests/crdt_doc_cross_sheet_e2e_test.rs`:

```rust
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
    for (i, (sku, price)) in [("A", 100), ("B", 200), ("C", 300)].iter().enumerate() {
        apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, &format!("A{}", i + 2), &json!(sku));
        apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, &format!("B{}", i + 2), &json!(price));
    }
    apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, "A1", &json!("SKU"));
    apply_set_cell_in_proc(&entry_q3.doc, &sid_q3, "B1", &json!("Price"));

    // Q4: B/250 (changed), C/300 (same), D/400 (new). A is gone.
    let sid_q4 = apply_add_sheet(&entry_q4.doc, "Inventory");
    apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, "A1", &json!("SKU"));
    apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, "B1", &json!("Price"));
    for (i, (sku, price)) in [("B", 250), ("C", 300), ("D", 400)].iter().enumerate() {
        apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, &format!("A{}", i + 2), &json!(sku));
        apply_set_cell_in_proc(&entry_q4.doc, &sid_q4, &format!("B{}", i + 2), &json!(price));
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
    //    Use string literal at runtime to embed the two sheet_ids.
    let code = format!(
        r#"
import pandas as pd
a, b = dfs["{sid_q3}"], dfs["{cloned}"]
# headers already in row 1 (no title row in our seed) — just rename cols via iloc[0]
a.columns = a.iloc[0].tolist(); a = a.iloc[1:].reset_index(drop=True)
b.columns = b.iloc[0].tolist(); b = b.iloc[1:].reset_index(drop=True)
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

    // 6. Two events recorded for Q3: import + write_to_sheet.
    let events = ctx
        .backend()
        .list_events(&aid_q3, None, Some(10), None)
        .await
        .unwrap();
    assert!(events.len() >= 2);
    assert!(events.iter().any(|e| e.summary.contains("imported sheet")));
    assert!(events.iter().any(|e| e.summary.contains("Diff Q3 vs Q4")));

    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo test --test crdt_doc_cross_sheet_e2e_test --no-run 2>&1 | tail -3
```
Expected: compiles.

- [ ] **Step 3: Run the integration test (requires .venv with pandas)**

```bash
source .env
SITE_PKGS=$(.venv/bin/python -c "import site; print(site.getsitepackages()[0])")
PYTHONPATH="$SITE_PKGS" cargo test --test crdt_doc_cross_sheet_e2e_test -- --ignored 2>&1 | tail -10
```
Expected: 1 test PASSES.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/crdt_doc_cross_sheet_e2e_test.rs
git commit -m "test(crdt_documents): end-to-end clone+pandas row-diff (F-T5)

Integration test ignoring by default (requires pandas/numpy in the
embedded Python). Builds 2 artifacts (Q3 with SKUs A,B,C and Q4 with
B(changed price),C,D), imports Q4's sheet into Q3, then runs Pattern B
(row diff by SKU) via run_python. Asserts the produced count
distribution + the 3 sheets in the principal + 2 audit events."
```

---

## Task 6 — F-T6: Browser smoke graph + fixture generator

**Files:**
- Create: `tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py`
- Create: `tests/graphs/crdt_documents/f_cross_artifact_smoke.json`

- [ ] **Step 1: Create the fixture generator**

Create `tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py`:

```python
#!/usr/bin/env python3
"""Generate q3.xlsx and q4.xlsx for the F browser smoke.

Schema: Producto | Cantidad | Precio | Total (Total left empty so the
import path matches the C smoke's behaviour, with a title row in A1).

Overlap design (deterministic for reproducible smokes):
- 10 SKUs in common (SKU-0001 .. SKU-0010)
- 3 SKUs only in Q3 (SKU-Q3-only-1..3)
- 3 SKUs only in Q4 (SKU-Q4-only-1..3)
- 2 of the common SKUs have a different Precio in Q4 (drift)
"""
import random
import pandas as pd
from pathlib import Path

random.seed(2026)
OUT = Path('/tmp/colmena_e2e')
OUT.mkdir(parents=True, exist_ok=True)


def make(period: str, only_skus: list[str], price_overrides: dict[str, float]) -> None:
    rows = [
        ['Reporte ' + period + ' 2026', '', '', ''],
        ['Producto', 'Cantidad', 'Precio', 'Total'],
    ]
    common = [f'SKU-{i:04d}' for i in range(1, 11)]
    all_skus = common + only_skus
    for sku in all_skus:
        qty = random.choice([1, 2, 3, 5, 8, 12])
        if sku in price_overrides:
            price = price_overrides[sku]
        else:
            price = round(random.uniform(5.0, 200.0), 2)
        rows.append([sku, qty, price, ''])
    pd.DataFrame(rows).to_excel(OUT / f'{period.lower()}.xlsx', index=False, header=False)
    print(f'wrote {OUT / (period.lower() + ".xlsx")} ({len(rows)} rows)')


make('Q3', only_skus=['SKU-Q3-ONLY-1', 'SKU-Q3-ONLY-2', 'SKU-Q3-ONLY-3'], price_overrides={})
make('Q4', only_skus=['SKU-Q4-ONLY-1', 'SKU-Q4-ONLY-2', 'SKU-Q4-ONLY-3'],
     price_overrides={'SKU-0003': 999.99, 'SKU-0007': 0.99})
```

- [ ] **Step 2: Generate the fixtures locally to verify**

```bash
.venv/bin/python tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py
ls -la /tmp/colmena_e2e/q3.xlsx /tmp/colmena_e2e/q4.xlsx
```
Expected: both files written, each ~5-6 KB.

- [ ] **Step 3: Create the smoke graph**

Create `tests/graphs/crdt_documents/f_cross_artifact_smoke.json`:

```json
{
  "_comment": "Subsistema F browser smoke. Setup script crea 2 artifacts (Q3 + Q4) e importa los xlsx via REST; este grafo ejecuta el agente que (1) lista sheets de Q4, (2) clona la sheet al principal Q3, (3) ejecuta los 3 outputs: row diff por SKU, schema diff, y join/enrich (Q4_Price en Q3). Verificás en el browser que aparezcan 3 hojas nuevas en el artifact principal.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/f-cross-smoke",
        "method": "POST",
        "test_payload": {
          "prompt": "Tengo dos artifacts de ventas: el actual es Q3 y quiero compararlo con Q4 ($DYNAMIC:art_q4). Hacé tres hojas en el actual:\n(1) Row diff por SKU mostrando qué se agregó/borró/cambió. write_to_sheet='Row Diff Q3 vs Q4'.\n(2) Schema diff entre las dos sheets de Inventory. write_to_sheet='Schema Diff'.\n(3) Enrichment: agregar a Q3 una columna nueva 'Q4_Price' con el precio de cada SKU que también existe en Q4 (left join). write_to_sheet='Q3 Enriched with Q4 Price'.\n\nProcedimiento canónico:\n- list_sheets_of(art_q4) para descubrir el sheet_id de Q4.\n- import_sheet para traer la sheet de Q4 al principal.\n- load_skill('crdt-doc-cross-sheet-analysis') para ver los 6 patrones canónicos.\n- run_python una vez por cada output (patrón B, C, E).\nReportá brevemente al final."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "stream": false,
        "system_message": "You are a spreadsheet data analyst. You have CRDT document tools and a Python sandbox via crdt_doc_run_python. The current artifact is pinned via ctx — other artifacts are reachable via list_sheets_of + import_sheet. Load the crdt-doc-run-python and crdt-doc-cross-sheet-analysis skills before writing any pandas code: they explain the DataFrame shape contract (row 1 → columns) and the 6 canonical analysis patterns.",
        "skills": {
          "builtin": ["crdt-doc-run-python", "crdt-doc-cross-sheet-analysis"]
        },
        "crdt_documents": {
          "artifact_id": "$DYNAMIC:art_q3",
          "ws_url": "ws://127.0.0.1:8090/yjs"
        }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent",   "to": "log" }
  ]
}
```

- [ ] **Step 4: Document the atomic smoke runner script (inline — operator runs manually)**

Add a comment block to `f_cross_artifact_smoke.json` is not possible (JSON), so document the runner inline. Append to the top `_comment`:

The atomic runner the operator types in their terminal:

```bash
cd /Users/danielgarcia/startti/colmena
# Generate fixtures
.venv/bin/python tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py

# Make sure server is up
curl -sf http://127.0.0.1:8090/documents > /dev/null || {
  echo "Start server in Terminal A:"
  echo "  cargo run --bin dag_engine -- crdt-yws --port 8090 --dump-dir \$(pwd)/.colmena/crdt_documents"
  exit 1
}

# Create 2 artifacts, import the xlsx into each
ART_Q3=$(curl -s -X POST http://127.0.0.1:8090/documents -H 'content-type: application/json' \
  -d '{"name":"Q3 (F smoke)","agent_session_id":"agent_f_smoke"}' | jq -r .artifact_id)
ART_Q4=$(curl -s -X POST http://127.0.0.1:8090/documents -H 'content-type: application/json' \
  -d '{"name":"Q4 (F smoke)","agent_session_id":"agent_f_smoke"}' | jq -r .artifact_id)
curl -s -X POST -H "Content-Type: application/octet-stream" \
  --data-binary @/tmp/colmena_e2e/q3.xlsx \
  "http://127.0.0.1:8090/documents/$ART_Q3/import"
curl -s -X POST -H "Content-Type: application/octet-stream" \
  --data-binary @/tmp/colmena_e2e/q4.xlsx \
  "http://127.0.0.1:8090/documents/$ART_Q4/import"
echo
echo "PRINCIPAL (Q3) = $ART_Q3"
echo "SECONDARY (Q4) = $ART_Q4"
open "http://127.0.0.1:8090/?artifact=$ART_Q3"

# Materialize the graph with the two dynamic IDs
TMP=/tmp/colmena_e2e/f_cross.json
sed -e "s|\$DYNAMIC:art_q3|$ART_Q3|" -e "s|\$DYNAMIC:art_q4|$ART_Q4|" \
  tests/graphs/crdt_documents/f_cross_artifact_smoke.json > $TMP

# Run
SITE_PKGS=$(.venv/bin/python -c "import site; print(site.getsitepackages()[0])")
set -a; source .env; set +a
PYTHONPATH="$SITE_PKGS" cargo run --bin dag_engine -- run $TMP \
  --agent-session-id agent_f_smoke --include-extra-info 2>&1 | \
  tee /tmp/colmena_e2e/f_smoke.sse | tail -50
```

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/crdt_documents/f_cross_artifact_smoke.json \
        tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py
git commit -m "test(crdt_documents): F browser smoke graph + fixtures (F-T6)

JSON DAG and Python fixture generator for the F end-to-end browser
smoke. The graph instructs Gemini to produce three outputs from Q3
(principal) + Q4 (secondary): row diff by SKU, schema diff, and join
enrichment. The two artifact IDs are passed via \$DYNAMIC:art_q3 /
\$DYNAMIC:art_q4 substitution. The runner shell sequence is documented
in the JSON's _comment for operators."
```

---

## Task 7 — F-T7: Documentation (dev guide §5.7 + node_configurations.json)

**Files:**
- Modify: `docs/developer_guide/38_crdt_documents.md`
- Modify: `docs/node_configurations.json`

- [ ] **Step 1: Add §5.7 to dev guide**

In `docs/developer_guide/38_crdt_documents.md`, after §5.6 (existing — the C section), insert:

```markdown
### 5.7 Cross-sheet & cross-artifact analysis (subsistema F)

**Por qué existe.** Los workflows reales con xlsx tienen dos formatos: (a) un workbook con varias hojas que se comparan entre sí, (b) dos+ workbooks separados que se quieren cruzar. F unifica ambos casos vía clonado: cualquier sheet de cualquier artifact se puede traer al artifact actual y a partir de ahí todo es multi-sheet pandas (que ya funcionaba desde C).

**Tools nuevos:**

- `crdt_doc_list_sheets_of({artifact_id})` — peek a otro artifact sin clonar. Devuelve `{artifact_id, name, sheets:[{sheet_id, name, n_rows, n_cols}]}`.
- `crdt_doc_import_sheet({source_artifact_id, source_sheet_id, new_name?})` — clona la sheet completa al artifact actual. Snapshot (no live link). Resuelve collisions con sufijo ` (2)`, ` (3)`, …
- `crdt_doc_get_recent_changes` extendido — ahora acepta `artifact_id?` opcional para auditar otros artifacts.

**Caps:** `MAX_IMPORT_BYTES = 100 MB` (mismo que `run_python`), `MAX_SHEETS_PER_ARTIFACT = 100` (defensivo).

**Skill builtin:** `crdt-doc-cross-sheet-analysis` documenta 6 patrones canónicos pandas (cell-diff, row-diff por key, schema-diff, statistical, join/enrich, conditional transform) con snippets verbatim. Activación: `config.skills.builtin: ["crdt-doc-cross-sheet-analysis"]`.

**Auditoría cross-session.** El evento del import incluye el artifact origen en el summary (`"imported sheet 'X' (N rows × M cols) from artifact art_xxxx"`), entonces el log de cambios recientes muestra qué entró desde dónde sin importar quién creó el origen.

**Limitaciones v1:**
- Snapshot only — cambios posteriores en el source NO se propagan al clone (live linking es BACKLOG).
- No hay `crdt_doc_delete_sheet` para limpiar sheets clonadas (BACKLOG).
- Permisos por artifact: cualquier agente con el `artifact_id` puede leer e importar; modelo de permisos es BACKLOG (bloqueante para subsistema A).
- Cross-session discovery sigue scoped a `list_my_artifacts` (session-only); cuando shippeemos workspace concept los tools de F siguen funcionando sin cambios.

Spec completa: [`docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md`](../superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md).
```

- [ ] **Step 2: Add tool entries to node_configurations.json**

In `docs/node_configurations.json`, find the section for `llm_call` synthetic tools (the `crdt_doc_*` entries from B/C) and append:

```json
"crdt_doc_list_sheets_of": {
  "description": "Peek at the sheets of any artifact in the registry (cross-artifact, no session ownership check). Use to discover a source_sheet_id before crdt_doc_import_sheet.",
  "fields": {
    "artifact_id": { "type": "string", "required": true, "description": "ULID of the artifact to inspect." }
  }
},
"crdt_doc_import_sheet": {
  "description": "Clone a sheet from another artifact into the current artifact (snapshot only — no live link). After import, use crdt_doc_run_python with multi-sheet to analyze.",
  "fields": {
    "source_artifact_id": { "type": "string", "required": true, "description": "Source artifact ULID." },
    "source_sheet_id":    { "type": "string", "required": true, "description": "Source sheet id within that artifact." },
    "new_name":           { "type": "string", "required": false, "description": "Optional override; default \"<source_name> (from art_xxxx)\"." }
  }
}
```

And in the existing `crdt_doc_get_recent_changes` entry, add the new field:

```json
"artifact_id": { "type": "string", "required": false, "description": "NEW in F: if set, audits this artifact instead of the ctx's. Default: ctx artifact (subsystem B behaviour)." }
```

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/38_crdt_documents.md docs/node_configurations.json
git commit -m "docs(crdt_documents): document subsystem F (cross-sheet analysis) (F-T7)

New §5.7 in dev guide explains the two new tools, the extended
get_recent_changes, the skill, caps, and limitations. node_configurations.json
gets entries for the two new tools and the new artifact_id field on
get_recent_changes."
```

---

## Task 8 — F-T8: BACKLOG entries (v1.1 follow-ups)

**Files:**
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Append 3 entries before "Items resueltos recientemente"**

```markdown
---

## CRDT Documents v1.1 — Multi-session workspace visibility

- **Origen:** restricción explícita en F (subsistema 3, 2026-06-04): hoy `crdt_doc_list_my_artifacts` filtra por session_id, así que un agente solo descubre artifacts creados en su misma sesión. El owner pidió específicamente que "diferentes agentes en diferentes turnos incluso con diferentes agent session id puedan crear artefactos que otros agentes modifiquen lean o comparen".
- **Problema:** sin esto, F funciona dentro de una sesión pero no entre sesiones. El usuario tiene que pasar el `artifact_id` explícito en el prompt para cruzar sesiones, lo cual no escala a flujos colaborativos reales.
- **Fix propuesto:**
  1. Introducir concepto de "workspace" (= organización, team, project) en `crdt_doc_session_artifacts`: relación N:N en vez de "owned by one session".
  2. Nuevo tool `crdt_doc_list_workspace_artifacts({workspace_id?})` que devuelve los artifacts del workspace del caller. Default workspace = el del session id.
  3. Modelo de permisos opcional por artifact (`read | read_write | owner`) gateado por workspace membership.
  4. Mecanismo de share/link entre artifacts con auditoría (quién compartió con quién cuándo).
- **Acceptance criteria:**
  - Agente A (session_id=s1) crea artifact art_X. Agente B (session_id=s2, mismo workspace) lo descubre vía `list_workspace_artifacts` y lo importa vía `import_sheet`.
  - Agente C (otro workspace) NO ve art_X.
  - Owner puede revocar acceso de un workspace a un artifact.
- **Estimación:** ~2-3 días dev (incluyendo migrations + tools + tests).
- **Cuándo retomar:** bloqueante para subsistema A (microservice deploy multi-tenant). Antes de subir a prod multi-usuario.

---

## CRDT Documents v1.1 — Live linking de sheets clonadas

- **Origen:** decisión explícita en F: el clonado de `crdt_doc_import_sheet` es snapshot only — cambios posteriores en el source no se propagan al clone.
- **Problema:** para análisis "vivos" (ej: dashboard que compara Q3 con Q4 en tiempo real mientras Q4 se actualiza), el agente o el usuario tienen que re-importar manualmente.
- **Fix propuesto:**
  1. Nuevo flag `live: true` en `import_sheet` que registra una subscripción del clone al source.
  2. Cuando el source cambia (vía `cells_map.observe`), aplicar el delta al clone con conflict resolution (last-write-wins por celda).
  3. Manejo de borrado del source: el clone se "freezes" en el último estado y se marca con flag visible.
  4. Cleanup automático cuando el artifact destino se borra.
- **Acceptance criteria:**
  - Edito una celda en el source → se refleja en el clone dentro de 1s.
  - Borro la sheet source → el clone queda en el estado final con flag "source deleted".
  - Borro el clone → no afecta el source.
- **Estimación:** ~2 días (subscription management + conflict resolution + cleanup paths + tests).
- **Cuándo retomar:** cuando aparezca un caso de uso real de "dashboard cross-artifact" (compare/enrich que necesita seguir cambios upstream).

---

## CRDT Documents v1.1 — Eliminar sheets

- **Origen:** F clona sheets y no provee mecanismo para limpiar. El cap de 100 sheets/artifact protege contra runaway pero no permite mantener el workbook ordenado.
- **Problema:** después de un análisis, el agente o el usuario quieren eliminar las sheets clonadas temporales. Hoy no hay tool ni acción en el canvas.
- **Fix propuesto:**
  1. Nuevo tool `crdt_doc_delete_sheet({sheet_id, confirm?})` que elimina la sheet del Y.Doc en una transacción. Requiere `confirm: true` explícito para prevenir borrado accidental por el LLM.
  2. UI button en Univer para que el usuario también pueda borrar (probablemente ya existe en Univer — solo wirearlo al delta del Y.Doc).
  3. Audit event con el nombre y resumen del contenido borrado (para soft-undo manual si fuera necesario).
- **Acceptance criteria:**
  - Borrar una sheet decrementa `MAX_SHEETS_PER_ARTIFACT` counter; siguiente import vuelve a entrar.
  - El borrado se propaga vía WS a todos los peers.
  - Event log conserva el resumen para auditoría.
- **Estimación:** ~4-6 horas dev.
- **Cuándo retomar:** post-merge de F, cuando el feedback real de usuarios muestre que el clutter de sheets clonadas es molesto.
```

- [ ] **Step 2: Commit**

```bash
git add docs/BACKLOG.md
git commit -m "docs(backlog): 3 v1.1 items unlocked by subsystem F (F-T8)

- Multi-session workspace visibility (bloqueante para subsistema A).
- Live linking de sheets clonadas (depende de feedback real de uso).
- Eliminar sheets (tool + UI button, sin riesgo)."
```

---

## Task 9 — F-T9: CHANGELOG entry

**Files:**
- Modify: `docs/CHANGELOG_2026-06.md`

- [ ] **Step 1: Append §4 to changelog**

Append to `docs/CHANGELOG_2026-06.md`:

```markdown

---

## 4. CRDT Documents — Cross-sheet & cross-artifact analysis (subsistema F)

**Qué cambió.** El agente puede comparar, unir, enriquecer o transformar datos entre sheets — ya sea dentro del mismo artifact o trayéndolas desde otros artifacts. Dos tools nuevos: `crdt_doc_list_sheets_of({artifact_id})` (peek cross-artifact sin clonar) y `crdt_doc_import_sheet({source_artifact_id, source_sheet_id, new_name?})` (clonado snapshot al artifact actual). Una skill builtin nueva: `crdt-doc-cross-sheet-analysis` con 6 patrones pandas canónicos (cell-diff, row-diff por key, schema-diff, statistical, join/enrich, conditional transform). Extensión backward-compatible a `crdt_doc_get_recent_changes` con `artifact_id?` opcional para auditar otros artifacts. Cero cambios a `crdt_doc_run_python` (subsistema C) — la sheet clonada vive en el mismo artifact que el principal, multi-sheet ya funcionaba.

**Por qué importa.** Los workflows reales con xlsx exigen cruzar varias hojas / varios workbooks (versionado Q3 vs Q4, enrichment con catálogo, reglas externas). Sin F la única forma era pasar todo el contenido vía prompt o usar set_range manualmente — ambos prohibitivos en tokens y propensos a error. F unifica ambos casos en un solo flujo (`list_sheets_of → import_sheet → run_python`) reusando 100% de la infra de B (audit) y C (pandas).

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md`](superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md)
- Plan: [`docs/superpowers/plans/2026-06-04-crdt-cross-sheet-analysis.md`](superpowers/plans/2026-06-04-crdt-cross-sheet-analysis.md)
- Dev guide §5.7: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Items deferidos: [`docs/BACKLOG.md`](BACKLOG.md) — multi-session workspace (bloqueante para A), live linking, delete sheet.

**Commits (F-T1 a F-T10).** Ver `git log feature/docs --grep="F-T"`.

**Estado.** done.

**Limitaciones conocidas v1.**
- Snapshot only — sin live linking (BACKLOG v1.1).
- Sin tool de delete_sheet — el cap de 100 sheets/artifact protege pero no limpia (BACKLOG).
- Discovery sigue session-scoped (`list_my_artifacts`); cross-session via workspace es v1.1 bloqueante para subsistema A.
- Sin permission model — cualquier agente con `artifact_id` puede leer/importar.

**Forward compatibility.** Los tools de F no enforcean session ownership a nivel de tool — cuando workspace concept aterrize en v1.1, solo el discovery cambia; los tools de F siguen funcionando sin modificación.
```

- [ ] **Step 2: Commit**

```bash
git add docs/CHANGELOG_2026-06.md
git commit -m "docs(changelog): June 2026 — add subsystem F entry (F-T9)"
```

---

## Task 10 — F-T10: Final sweep (test/clippy/fmt + browser smoke)

**Files:** (verify only)

- [ ] **Step 1: Full test suite**

```bash
cargo test --lib 2>&1 | tail -5
```
Expected: previous 1227+ tests still pass, plus the 13+ new F unit tests (3 list_sheets_of + 10 import_sheet + 2 recent_changes filter). Total should be 1240+ passed / 0 failed.

- [ ] **Step 2: Integration test (with pandas)**

```bash
source .env
SITE_PKGS=$(.venv/bin/python -c "import site; print(site.getsitepackages()[0])")
PYTHONPATH="$SITE_PKGS" cargo test --test crdt_doc_cross_sheet_e2e_test -- --ignored 2>&1 | tail -5
```
Expected: 1 PASS.

- [ ] **Step 3: Clippy + fmt**

```bash
cargo clippy --lib --bins --tests 2>&1 | tail -3
cargo fmt --check
```
Expected: both clean.

- [ ] **Step 4: Run check_python_env (from C debt sweep)**

```bash
./scripts/check_python_env.sh
```
Expected: ✅ match + pandas/numpy/scipy import ok.

- [ ] **Step 5: Browser smoke (manual, but with verification)**

Start the server:
```bash
cargo run --bin dag_engine -- crdt-yws --port 8090 --dump-dir $(pwd)/.colmena/crdt_documents
```
In another terminal, run the atomic smoke script (the one documented in F-T6 step 4). Then verify:

- SSE shows tool calls in order: `list_sheets_of` → `import_sheet` → `load_skill` → `run_python` ×3.
- All three `run_python` calls have `wrote_sheet.n_rows > 0`.
- Browser tab opens to the Q3 artifact and shows 5 sheets total (original Q3 Inventory + cloned Q4_Inventory + Row Diff + Schema Diff + Q3 Enriched).
- The audit log for Q3 contains 1 import event + 3 write events.

Run a quick REST verification:
```bash
ART_Q3=$(grep -oE 'art_[0-9A-Z]+' /tmp/colmena_e2e/f_smoke.sse | head -1)
N_SHEETS=$(curl -s "http://127.0.0.1:8090/projection/$ART_Q3" | jq '.sheets | length')
echo "n_sheets in principal: $N_SHEETS (expected 5)"
```

- [ ] **Step 6: Commit (chore — nothing to add, but log the verification)**

```bash
# Confirm working tree clean
git status --short
# If anything sneaked in (e.g. .DS_Store), reset and skip the commit
```

If genuinely nothing to commit, skip the commit and just record the sweep in the conversation.

If sweep finds issues: fix them with targeted commits, then loop back through steps 1-5 until clean.

---

## Self-Review Notes

**Spec coverage check** (against `2026-06-04-crdt-cross-sheet-analysis-design.md`):

- §1 Objetivo → covered by all tasks
- §2 Decisiones de diseño → §2.1 principal+secundarios = F-T2; §2.2 forward-compat = preserved by design (no session enforcement in tools); §2.3 sin cambios a run_python = preserved (no run_python modification in plan); §2.4 snapshot no live link = explicit in F-T2 spec; §2.5 audit cross-session-friendly = F-T2 step 3 includes source in summary
- §3.1 list_sheets_of → F-T1
- §3.2 import_sheet → F-T2
- §3.3 get_recent_changes extension → F-T3
- §4 skill → F-T4
- §5 sin cambios fuera de tools → preserved (no migration in plan, run_python untouched, df_records untouched)
- §6.1 unit tests → F-T1, F-T2, F-T3 all include test files
- §6.2 integration test → F-T5
- §6.3 browser smoke → F-T6 + verification in F-T10
- §6.4 NO testeamos → reflected in BACKLOG entries (F-T8)
- §7 plan F-T1..F-T10 → all present
- §8 riesgos → mitigations are wired (caps in F-T2, self_import check, skill clarifies shape)
- §9 BACKLOG → F-T8 covers all 3
- §10 composición con resto del MVP → documented in §5.7 (F-T7)

**Placeholder scan:** all task steps contain complete code, exact commands, and exact file paths. No TBD/TODO.

**Type consistency:** verified `dispatch_crdt_doc_list_sheets_of`, `ImportSheetArgs`, `ListSheetsOfArgs`, `MAX_SHEETS_PER_ARTIFACT`, `MAX_IMPORT_BYTES`, `CRDT_DOC_LIST_SHEETS_OF_TOOL`, `CRDT_DOC_IMPORT_SHEET_TOOL`, `parse_a1_to_rc` (made pub(super) for cross-module use) — all referenced consistently across tasks.
