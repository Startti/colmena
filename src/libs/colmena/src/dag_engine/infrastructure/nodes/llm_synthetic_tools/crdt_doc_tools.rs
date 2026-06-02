//! Synthetic LLM tools for the v1 CRDT documents feature.
//!
//! Mirrors the `document_tools.rs` pattern: each tool is a thin adapter that
//! builds a JSON Schema (via `schemars`), parses LLM-provided args into a
//! typed struct, and calls a function on `crdt_documents::tool_executor::*`
//! against the registered `Doc`.
//!
//! Tools refuse to operate if `artifact_id` is not registered in the
//! runtime. The `artifact_id` itself is injected by the executor from the
//! `llm_call.config.crdt_documents` block — the LLM never sets it.

use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use crate::llm::domain::tools::ToolDefinition;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

pub const TOOL_LIST_SHEETS: &str = "crdt_doc_list_sheets";

/// Execution context bundled by the LLM-call executor and passed to every
/// CRDT tool dispatch. The `artifact_id` is resolved from the node config —
/// the LLM never provides it.
pub struct CrdtDocsContext {
    pub runtime: Arc<CrdtDocumentsRuntime>,
    pub artifact_id: ArtifactId,
}

// ── list_sheets ───────────────────────────────────────────────────────────────

/// `list_sheets` takes no parameters; the artifact is resolved server-side.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListSheetsArgs {}

/// Build the [`ToolDefinition`] for `crdt_doc_list_sheets`.
pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool::<ListSheetsArgs>(
        TOOL_LIST_SHEETS,
        "List the sheets in the current CRDT document. Returns id + name for each sheet.",
    )
}

/// Execute `list_sheets` against the runtime. Returns
/// `{ "sheets": [ { "sheet_id": "…", "name": "…" }, … ] }` or
/// `{ "error": "artifact_not_found" }` when the id is not registered.
pub fn execute_list_sheets(ctx: &CrdtDocsContext) -> serde_json::Value {
    let Some(entry) = ctx.runtime.registry.get(&ctx.artifact_id) else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let sheets: Vec<serde_json::Value> = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "sheet_id": s["id"],
                "name": s["name"],
            })
        })
        .collect();
    serde_json::json!({ "sheets": sheets })
}

// ── all tools ─────────────────────────────────────────────────────────────────

/// All CRDT document tool definitions (currently one; more added in Task 15).
pub fn build_all_crdt_doc_tools() -> Vec<ToolDefinition> {
    vec![tool_list_sheets()]
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::apply_add_sheet;
    use serde_json::json;

    async fn make_runtime() -> Arc<CrdtDocumentsRuntime> {
        let tmp = std::env::temp_dir().join(format!("crdt_ls_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap())
    }

    #[tokio::test]
    async fn lists_two_sheets() {
        let rt = make_runtime().await;
        let id = ArtifactId::new();
        let entry = rt.registry.get_or_create(&id, "workbook");
        apply_add_sheet(&entry.doc, "Sales");
        apply_add_sheet(&entry.doc, "Summary");
        let ctx = CrdtDocsContext {
            runtime: rt,
            artifact_id: id,
        };
        let v = execute_list_sheets(&ctx);
        let sheets = v["sheets"].as_array().unwrap();
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0]["name"], "Sales");
        assert_eq!(sheets[1]["name"], "Summary");
        // sheet_id must be a non-empty string (sh_<ULID>)
        assert!(sheets[0]["sheet_id"].as_str().unwrap().starts_with("sh_"));
    }

    #[tokio::test]
    async fn returns_error_for_unknown_artifact() {
        let rt = make_runtime().await;
        let unknown = ArtifactId::new(); // never registered
        let ctx = CrdtDocsContext {
            runtime: rt,
            artifact_id: unknown,
        };
        let v = execute_list_sheets(&ctx);
        assert_eq!(v["error"], "artifact_not_found");
    }

    #[test]
    fn list_sheets_tool_def_has_correct_name() {
        let def = tool_list_sheets();
        assert_eq!(def.name, TOOL_LIST_SHEETS);
        assert!(!def.description.is_empty());
    }

    #[test]
    fn list_sheets_schema_has_no_visible_params() {
        let def = tool_list_sheets();
        let schema_str = def
            .input_schema_override
            .as_ref()
            .expect("must have input_schema_override")
            .to_string();
        // Empty args struct → schema should NOT expose artifact_id to the LLM
        assert!(!schema_str.contains("artifact_id"));
    }
}
