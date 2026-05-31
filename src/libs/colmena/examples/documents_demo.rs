//! Documents feature — narrative walkthrough.
//!
//! Walks through the full lifecycle of an Excel and a Word artifact so you can
//! see exactly what happens on each turn:
//!   1. Create an artifact with an initial IR.
//!   2. Apply a patch and inspect the response (natural_language + structured).
//!   3. Use a server-minted id (from step 2) in a follow-up patch — this is
//!      what the LLM does on its next turn without re-reading the full IR.
//!   4. Rollback and list versions.
//!
//! Output is persisted to `/tmp/colmena_docs_demo/`; open the rendered
//! `.xlsx` / `.docx` files in LibreOffice (or Excel / Word) to verify visually.
//!
//! Run: `cargo run --example documents_demo`

use colmena::documents::application::apply_patch::{
    ApplyPatchInput, ApplyPatchOutput, ApplyPatchUseCase,
};
use colmena::documents::application::create_document::{
    CreateDocumentInput, CreateDocumentUseCase,
};
use colmena::documents::application::list_versions::ListVersionsUseCase;
use colmena::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use colmena::documents::application::rollback::{RollbackInput, RollbackUseCase};
use colmena::documents::domain::artifact::PatchSummary;
use colmena::documents::domain::ids::{ArtifactKind, SessionId, VersionId};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use colmena::documents::domain::{ArtifactStore, IRRenderer, IRValidator, RenderError};
use colmena::documents::infrastructure::ids::UlidIdGenerator;
use colmena::documents::infrastructure::render::{ExcelRenderer, WordRenderer};
use colmena::documents::infrastructure::storage::LocalFsStore;
use colmena::documents::infrastructure::validation::{ExcelValidator, WordValidator};
use std::path::PathBuf;
use std::sync::Arc;

struct NoopR;
#[async_trait::async_trait]
impl IRRenderer for NoopR {
    async fn render(&self, _ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        Ok(vec![])
    }
    fn target_extension(&self) -> &'static str {
        "html"
    }
    fn target_mime(&self) -> &'static str {
        "text/html"
    }
}

struct NoopV;
impl IRValidator for NoopV {
    fn validate(
        &self,
        _ir: &serde_json::Value,
    ) -> Result<(), colmena::documents::domain::DocumentError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from("/tmp/colmena_docs_demo");
    if root.exists() {
        std::fs::remove_dir_all(&root).ok();
    }
    std::fs::create_dir_all(&root).unwrap();

    let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(&root));
    let ids = Arc::new(UlidIdGenerator);

    let create = CreateDocumentUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(WordRenderer),
        word_validator: Arc::new(WordValidator),
        html_renderer: Arc::new(NoopR),
        html_validator: Arc::new(NoopV),
        ids: ids.clone(),
        default_retention: 20,
    };
    let apply = ApplyPatchUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(WordRenderer),
        word_validator: Arc::new(WordValidator),
        html_renderer: Arc::new(NoopR),
        html_validator: Arc::new(NoopV),
        ids: ids.clone(),
    };
    let read = ReadDocumentUseCase {
        store: store.clone(),
    };
    let list = ListVersionsUseCase {
        store: store.clone(),
    };
    let rollback = RollbackUseCase {
        store: store.clone(),
    };

    banner("Documents Feature Demo");
    println!("Storage root: {}", root.display());
    println!(
        "Every version writes: ir.json (source of truth), render.{{xlsx|docx}} (binary),\n\
         and patch_applied.json (the request + the summary the LLM sees).\n"
    );

    // ==========================================================
    //                      EXCEL ARTIFACT
    // ==========================================================
    section("EXCEL ARTIFACT");

    // ---- Step 1: create ----
    step(1, 6, "Create Excel artifact");
    println!(
        "  Sending an initial IR with two sheets (Summary, Detail).\n\
         The client doesn't mint ids — the backend does.\n"
    );
    let excel_out = create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Excel,
            session_id: SessionId::new("demo_session"),
            label: Some("Quarterly Report".into()),
            retention_limit: None,
            initial_ir: Some(serde_json::json!({
                "kind": "excel",
                "artifact_id": "placeholder",
                "version_id": "v1",
                "schema_version": "1.0.0",
                "workbook": {
                    "sheets": [
                        {
                            "id": "sheet_summary",
                            "name": "Summary",
                            "order": 0,
                            "columns": [{"index": 0, "width": 24.0}, {"index": 1, "width": 14.0}],
                            "cells": {
                                "A1": {"value": "Region",  "value_type": "string", "style_ref": "header"},
                                "B1": {"value": "Revenue", "value_type": "string", "style_ref": "header"},
                                "A2": {"value": "North"},
                                "B2": {"value": 1200},
                                "A3": {"value": "South"},
                                "B3": {"value": 980}
                            },
                            "tables": []
                        },
                        {
                            "id": "sheet_detail",
                            "name": "Detail",
                            "order": 1,
                            "columns": [],
                            "cells": {"A1": {"value": "TBD"}},
                            "tables": []
                        }
                    ],
                    "named_styles": {
                        "header": {
                            "font": {"bold": true, "size": 12.0, "color": "FFFFFF"},
                            "fill": "2F5496",
                            "alignment": "center"
                        }
                    }
                }
            })),
            source: PatchSource::Agent,
        })
        .await
        .expect("create excel");

    println!(
        "  ← artifact_id: {}\n  ← version:     v1\n  ← label:       \"{}\"\n",
        excel_out.artifact_id.0, excel_out.label
    );

    // ---- Step 2: patch — SetCell x2 (deterministic addresses, no new ids) ----
    step(2, 6, "Patch — add a Total row (SetCell × 2)");
    println!(
        "  Ops target deterministic cell addresses (A4, B4), so nothing new needs an id.\n\
         Expect natural_language with 2 lines and structured: [] (no assigned ids).\n"
    );
    let out = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: excel_out.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::SetCell {
                        sheet_id: "sheet_summary".into(),
                        address: "A4".into(),
                        value: serde_json::json!("Total"),
                        value_type: None,
                        format: None,
                        style_ref: Some("header".into()),
                    },
                    PatchOp::SetCell {
                        sheet_id: "sheet_summary".into(),
                        address: "B4".into(),
                        value: serde_json::json!("=SUM(B2:B3)"),
                        value_type: Some("formula".into()),
                        format: Some("#,##0".into()),
                        style_ref: None,
                    },
                ],
            },
        })
        .await
        .expect("excel set-cell patch");
    print_response(&out);

    // ---- Step 3: patch — CreateTable (mints table id) ----
    step(3, 6, "Patch — CreateTable (server mints `table` id)");
    println!(
        "  CreateTable needs an id the client doesn't know yet. The server mints one,\n\
         returns it inside summary.structured[].assigned_ids.table, and the LLM captures\n\
         it from the response so the NEXT turn can reference it directly.\n"
    );
    let out = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: excel_out.artifact_id.0.clone(),
                base_version: "v2".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::CreateTable {
                    sheet_id: "sheet_summary".into(),
                    range: "A1:B3".into(),
                    name: "Revenue".into(),
                    header_row: true,
                    style_preset: None,
                }],
            },
        })
        .await
        .expect("excel create-table patch");
    print_response(&out);
    let minted_table_id =
        assigned_id(&out.summary, 0, "table").expect("CreateTable must mint a table id");
    println!("  ↳ Captured minted table id for next turn: {minted_table_id}\n");

    // ---- Step 4: LLM follow-up turn — ResizeTable using minted id ----
    step(
        4,
        6,
        "Follow-up turn — ResizeTable using the minted id (no full re-read)",
    );
    println!(
        "  This is the payoff: on its next turn the LLM just uses the id it captured.\n\
         No need to re-read the IR to look up the table id.\n"
    );
    let out = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: excel_out.artifact_id.0.clone(),
                base_version: "v3".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::ResizeTable {
                    table_id: minted_table_id.clone(),
                    new_range: "A1:B4".into(),
                }],
            },
        })
        .await
        .expect("excel resize-table patch");
    print_response(&out);

    // ---- Step 5: rollback to v2 ----
    step(
        5,
        6,
        "Rollback to v2 (past versions are never mutated; a new v5 is written)",
    );
    let excel_rb = rollback
        .execute(RollbackInput {
            artifact_id: excel_out.artifact_id.clone(),
            to_version: VersionId::new("v2"),
        })
        .await
        .expect("rollback excel to v2");
    println!(
        "  ← rollback target:  v2\n  ← new version:     {}\n  (IR + render copied from v2; patch_applied.json records the rollback.)\n",
        excel_rb.new_version_id.0
    );

    // ---- Step 6: list + read current ----
    step(6, 6, "List versions + read current");
    let excel_versions = list
        .execute(&excel_out.artifact_id, None)
        .await
        .expect("list excel versions");
    println!("  Versions ({}):", excel_versions.len());
    for v in &excel_versions {
        println!("    {}  ({})", v.version_id.0, v.source);
    }
    let excel_current = read
        .execute(ReadDocumentInput {
            artifact_id: excel_out.artifact_id.clone(),
            version: None,
        })
        .await
        .expect("read excel current");
    println!("  Current: {}\n", excel_current.version.0);

    // ==========================================================
    //                      WORD ARTIFACT
    // ==========================================================
    section("WORD ARTIFACT");

    // ---- Step 1: create ----
    step(1, 5, "Create Word artifact");
    println!("  Initial IR with a heading, intro paragraph, bullet list, and a roster table.\n");
    let word_out = create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Word,
            session_id: SessionId::new("demo_session"),
            label: Some("Kickoff Memo".into()),
            retention_limit: None,
            initial_ir: Some(serde_json::json!({
                "kind": "word",
                "artifact_id": "placeholder",
                "version_id": "v1",
                "schema_version": "1.0.0",
                "document": {
                    "blocks": [
                        {
                            "type": "heading", "id": "blk_title", "level": 1,
                            "runs": [
                                {"id": "run_t1", "text": "Project Kickoff", "bold": true, "size": 18.0}
                            ]
                        },
                        {
                            "type": "paragraph", "id": "blk_intro",
                            "runs": [
                                {"id": "run_i1", "text": "This memo summarizes the "},
                                {"id": "run_i2", "text": "initial", "italic": true},
                                {"id": "run_i3", "text": " scope of the project."}
                            ]
                        },
                        {
                            "type": "list", "id": "blk_goals", "style": "bullet",
                            "items": [
                                {"id": "li_g1", "runs": [{"id": "run_g1", "text": "Ship MVP by Q3"}]},
                                {"id": "li_g2", "runs": [{"id": "run_g2", "text": "Migrate legacy users"}]}
                            ]
                        },
                        {
                            "type": "table", "id": "blk_roster",
                            "rows": [
                                {"id": "row_hdr", "cells": [
                                    {"runs": [{"id": "run_h1", "text": "Name",  "bold": true}]},
                                    {"runs": [{"id": "run_h2", "text": "Role",  "bold": true}]}
                                ]},
                                {"id": "row_1", "cells": [
                                    {"runs": [{"id": "run_1a", "text": "Ana"}]},
                                    {"runs": [{"id": "run_1b", "text": "PM"}]}
                                ]}
                            ]
                        }
                    ],
                    "named_styles": {}
                }
            })),
            source: PatchSource::Agent,
        })
        .await
        .expect("create word");
    println!(
        "  ← artifact_id: {}\n  ← version:     v1\n  ← label:       \"{}\"\n",
        word_out.artifact_id.0, word_out.label
    );

    // ---- Step 2: multi-op patch — some mint ids, some don't ----
    step(
        2,
        5,
        "Patch — ReplaceRunText (no mint) + InsertListItem (mints) + InsertTableRow (mints)",
    );
    println!(
        "  Three ops in one patch. Only the INSERTs need new ids (list item, row, and\n\
         the runs inside the new cells). ReplaceRunText edits existing ids, no mint needed.\n\
         The structured response shows one entry per op that minted ids.\n"
    );
    let out = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: word_out.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![
                    PatchOp::ReplaceRunText {
                        block_id: "blk_title".into(),
                        run_id: "run_t1".into(),
                        new_text: "Project Kickoff — Revised".into(),
                    },
                    PatchOp::InsertListItem {
                        list_block_id: "blk_goals".into(),
                        at_index: 2,
                        runs: vec![serde_json::json!({"text": "Publish changelog weekly"})],
                    },
                    PatchOp::InsertTableRow {
                        table_block_id: "blk_roster".into(),
                        before: None,
                        after: Some("row_1".into()),
                        cells: vec![
                            serde_json::json!({"runs": [{"text": "Beto"}]}),
                            serde_json::json!({"runs": [{"text": "Engineer"}]}),
                        ],
                    },
                ],
            },
        })
        .await
        .expect("word multi-op patch");
    print_response(&out);

    // ---- Step 3: InsertBlock — mints block id + runs ----
    step(
        3,
        5,
        "Patch — InsertBlock (mints `block` id and all run ids inside it)",
    );
    println!("  Adds a 'Next steps' paragraph after the roster table.\n");
    let out = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: word_out.artifact_id.0.clone(),
                base_version: "v2".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::InsertBlock {
                    before: None,
                    after: Some("blk_roster".into()),
                    block: serde_json::json!({
                        "type": "paragraph",
                        "runs": [
                            {"text": "Next steps: ", "bold": true},
                            {"text": "schedule design review on Monday."}
                        ]
                    }),
                }],
            },
        })
        .await
        .expect("word insert-block patch");
    print_response(&out);

    let minted_block_id =
        assigned_id(&out.summary, 0, "block").expect("InsertBlock must mint a block id");
    let minted_runs = assigned_ids_array(&out.summary, 0, "runs");
    let first_run_id = minted_runs
        .first()
        .cloned()
        .expect("InsertBlock must mint run ids for each run");
    println!(
        "  ↳ Captured for next turn:\n      block  = {minted_block_id}\n      run[0] = {first_run_id}\n"
    );

    // ---- Step 4: LLM follow-up turn — edit a run inside the just-minted block ----
    step(
        4,
        5,
        "Follow-up turn — ReplaceRunText inside the block we just created",
    );
    println!(
        "  This simulates the LLM's next turn: it uses the block + run ids it captured in step 3\n\
         to tweak the text it just wrote, without re-reading the IR.\n"
    );
    let out = apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: word_out.artifact_id.0.clone(),
                base_version: "v3".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::ReplaceRunText {
                    block_id: minted_block_id.clone(),
                    run_id: first_run_id.clone(),
                    new_text: "Next steps (updated): ".into(),
                }],
            },
        })
        .await
        .expect("word follow-up patch");
    print_response(&out);

    // ---- Step 5: list + read current ----
    step(5, 5, "List versions + read current");
    let word_versions = list
        .execute(&word_out.artifact_id, None)
        .await
        .expect("list word versions");
    println!("  Versions ({}):", word_versions.len());
    for v in &word_versions {
        println!("    {}  ({})", v.version_id.0, v.source);
    }
    let word_current = read
        .execute(ReadDocumentInput {
            artifact_id: word_out.artifact_id.clone(),
            version: None,
        })
        .await
        .expect("read word current");
    println!("  Current: {}\n", word_current.version.0);

    // ==========================================================
    //                          SUMMARY
    // ==========================================================
    section("FILES ON DISK");
    println!(
        "  Excel (current render):\n    {}/artifacts/{}/versions/{}/render.xlsx",
        root.display(),
        excel_out.artifact_id.0,
        excel_current.version.0
    );
    println!(
        "  Word  (current render):\n    {}/artifacts/{}/versions/{}/render.docx",
        root.display(),
        word_out.artifact_id.0,
        word_current.version.0
    );
    println!(
        "\n  Per-version layout (inspect any vN):\n    \
             ir.json            — canonical IR (source of truth)\n    \
             render.xlsx/.docx  — rendered binary\n    \
             patch_applied.json — the patch request + the summary returned to the LLM\n"
    );
    println!("=== Done ===");
}

// -------------------- presentation helpers --------------------

fn banner(title: &str) {
    let bar = "=".repeat(70);
    println!("\n{bar}\n  {title}\n{bar}\n");
}

fn section(title: &str) {
    let bar = "━".repeat(70);
    println!("\n{bar}\n{title:^70}\n{bar}\n");
}

fn step(n: usize, total: usize, title: &str) {
    println!("[STEP {n}/{total}] {title}");
}

fn print_response(out: &ApplyPatchOutput) {
    println!("  Response:");
    println!("    version_id: {}", out.version_id.0);
    println!("    natural_language:");
    if out.summary.natural_language.is_empty() {
        println!("      (empty)");
    } else {
        for line in &out.summary.natural_language {
            println!("      • {line}");
        }
    }
    println!("    structured:");
    if out.summary.structured.is_empty() {
        println!("      [] (no ids minted by these ops)");
    } else {
        for entry in &out.summary.structured {
            println!(
                "      • {}",
                serde_json::to_string(entry).unwrap_or_else(|_| "<unserializable>".into())
            );
        }
    }
    println!();
}

fn assigned_id(summary: &PatchSummary, op_index: usize, key: &str) -> Option<String> {
    summary
        .structured
        .iter()
        .find(|e| e.get("op_index").and_then(|v| v.as_u64()) == Some(op_index as u64))
        .and_then(|e| e.get("assigned_ids"))
        .and_then(|a| a.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn assigned_ids_array(summary: &PatchSummary, op_index: usize, key: &str) -> Vec<String> {
    summary
        .structured
        .iter()
        .find(|e| e.get("op_index").and_then(|v| v.as_u64()) == Some(op_index as u64))
        .and_then(|e| e.get("assigned_ids"))
        .and_then(|a| a.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}
