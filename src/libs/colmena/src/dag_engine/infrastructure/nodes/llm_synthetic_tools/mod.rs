//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.

pub mod crdt_doc_context;
pub mod crdt_doc_import_sheet;
pub mod crdt_doc_run_python;
pub mod crdt_doc_tools;
pub mod crdt_summary;
pub mod describe_tool;
pub mod diff_writer;
pub mod document_tools;
pub mod gdocs_tools;
pub mod gsheets_run_python;
pub mod gsheets_tools;
pub mod lazy_tools_catalog;
pub mod load_attachment_tool;
pub mod load_skill_tool;
pub mod markdown_to_docs_ops;
pub mod recall_history;
pub mod sheet_collision;
pub mod sql_bulk_tools;
pub mod toolkit_packages;

// ── Shared schema helpers ────────────────────────────────────────────────────
//
// Both `document_tools` (v0, patches-based) and `crdt_doc_tools` (v1, CRDT)
// build their LLM tool definitions the same way: derive a JSON Schema via
// `schemars`, sanitize it for provider quirks, and carry it in
// `input_schema_override`. These two helpers are `pub(super)` so every
// sub-module in this directory can call them without duplication.

use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
use schemars::JsonSchema;

/// Build a [`ToolDefinition`] whose input schema is derived from `T` via
/// `schemars`. The schema is sanitized before being placed in
/// `input_schema_override`; `parameters` is left empty (providers use the
/// override verbatim when it is present).
pub(super) fn build_synthetic_tool<T: JsonSchema>(name: &str, description: &str) -> ToolDefinition {
    let schema = schemars::schema_for!(T);
    let mut schema_json =
        serde_json::to_value(schema).expect("schemars schema must serialize to JSON Value");
    sanitize_schema_for_llm_providers(&mut schema_json);
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        summary: None,
        parameters: ToolParameters::new(),
        input_schema_override: Some(schema_json),
    }
}

/// Like [`build_synthetic_tool`], but additionally attaches a one-line
/// `summary` (≤ 200 chars) used by `lazy_tool_loading` catalogs. Every
/// synthetic tool registered in colmena MUST go through this builder so
/// the `every_synthetic_tool_has_summary` test passes at CI time.
#[allow(dead_code)]
pub(super) fn build_synthetic_tool_with_summary<T: JsonSchema>(
    name: &str,
    description: &str,
    summary: &str,
) -> ToolDefinition {
    build_synthetic_tool::<T>(name, description).with_summary(summary.to_string())
}

/// Walk a JSON Schema and normalize shapes that some LLM providers reject:
///
/// 1. Boolean schemas at `items` / `additionalProperties` positions are
///    replaced with `{}` (schemars emits `true` for opaque types like
///    `serde_json::Value`, but OpenAI requires an object schema there).
/// 2. Any `{"type": "object"}` without a `properties` field gets an empty
///    `properties: {}` injected (OpenAI requires `properties` to be present
///    even on schemas that take no parameters).
/// 3. `$schema` keys are stripped at every level — Gemini's proto-based
///    Schema type rejects unknown fields and fails with
///    `Unknown name "$schema"`.
/// 4. `"type": ["string", "null"]` array form (emitted by `schemars` for
///    `Option<T>`) is collapsed to `"type": "string"`. Gemini's proto schema
///    requires a singular type string; it cannot start a list at the `type`
///    position. We drop `"null"` and, if exactly one non-null type remains,
///    write it as a scalar. The "is this field optional" signal is carried
///    by the parent object's `required` array, which schemars already omits
///    for `Option<T>` fields — so dropping the `null` member loses nothing.
/// 5. `$ref` references to `#/definitions/X` or `#/$defs/X` are inlined
///    (the referenced schema body is substituted at the ref site), and the
///    top-level `definitions` / `$defs` maps are then dropped. Gemini's proto
///    schema rejects both `$ref` and `definitions`. This runs ONCE at the
///    top before the recursive per-node normalization, so authors can freely
///    use nested struct/enum types in Args without thinking about it.
pub(super) fn sanitize_schema_for_llm_providers(value: &mut serde_json::Value) {
    inline_refs_top_level(value);
    normalize_recursive(value);
}

/// Extract `definitions` / `$defs` from the top-level object, then walk the
/// tree replacing any `{"$ref": "#/definitions/X"}` with a clone of `X`'s
/// schema body. Nested refs in resolved bodies are resolved transitively.
fn inline_refs_top_level(value: &mut serde_json::Value) {
    use serde_json::Value;
    let mut defs: serde_json::Map<String, Value> = serde_json::Map::new();
    if let Value::Object(map) = value {
        if let Some(Value::Object(d)) = map.remove("definitions") {
            for (k, v) in d {
                defs.insert(k, v);
            }
        }
        if let Some(Value::Object(d)) = map.remove("$defs") {
            for (k, v) in d {
                defs.insert(k, v);
            }
        }
    }
    if defs.is_empty() {
        return;
    }
    resolve_refs(value, &defs, 0);
}

fn resolve_refs(
    value: &mut serde_json::Value,
    defs: &serde_json::Map<String, serde_json::Value>,
    depth: usize,
) {
    use serde_json::Value;
    // Defensive: bail on pathological recursion. Real schemars output for
    // our Args structs is shallow; 32 is generous and catches cycles before
    // they stack-overflow.
    if depth > 32 {
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ref_path)) = map.get("$ref") {
                let name = ref_path
                    .strip_prefix("#/definitions/")
                    .or_else(|| ref_path.strip_prefix("#/$defs/"));
                if let Some(n) = name {
                    if let Some(resolved) = defs.get(n) {
                        let mut new_value = resolved.clone();
                        resolve_refs(&mut new_value, defs, depth + 1);
                        *value = new_value;
                        return;
                    }
                }
            }
            for (_, v) in map.iter_mut() {
                resolve_refs(v, defs, depth + 1);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                resolve_refs(v, defs, depth + 1);
            }
        }
        _ => {}
    }
}

fn normalize_recursive(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            // (3) Strip `$schema` at every level.
            map.remove("$schema");

            // (4) Collapse `"type": [...]` → singular type, dropping `"null"`.
            if let Some(Value::Array(arr)) = map.get("type") {
                let kept: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| *s != "null")
                    .map(|s| s.to_string())
                    .collect();
                if kept.len() == 1 {
                    map.insert(
                        "type".to_string(),
                        Value::String(kept.into_iter().next().unwrap()),
                    );
                } else if kept.is_empty() {
                    // Pathological: only "null" was in the list. Default
                    // to "string" so the schema stays well-formed.
                    map.insert("type".to_string(), Value::String("string".to_string()));
                }
                // Else: multi-type union (e.g. ["string","number"]) — leave
                // as-is. Not produced by our v0/v1 Args structs today.
            }

            // `items`: a boolean schema there is rejected by OpenAI. Convert
            // to an empty object schema (accept anything).
            if let Some(v) = map.get_mut("items") {
                if v.is_boolean() {
                    *v = Value::Object(serde_json::Map::new());
                }
            }
            // `additionalProperties`: Gemini's proto Schema rejects this key
            // entirely (treats it as "Unknown name"). schemars emits it for
            // `Map<String, V>` field types (e.g. `column_mapping` in
            // `sql_bulk_insert_from_attachment`'s `BulkInsertArgs`).
            //
            // Strategy: strip it at every nesting level. The resulting schema
            // is semantically equivalent for our needs — OpenAI / Anthropic
            // default to allowing extra properties in non-strict mode, and
            // we are not in strict mode. The LLM can still pass arbitrary
            // string-keyed maps for the column_mapping field.
            map.remove("additionalProperties");

            // Schema-position keys whose children are themselves schemas —
            // a raw `true` there (schemars emits this for opaque types like
            // `serde_json::Value` used as a field type) is rejected by
            // Gemini's proto Schema. Replace with `{}` so the field accepts
            // any value.
            for key in ["properties", "patternProperties", "definitions", "$defs"] {
                if let Some(Value::Object(child_map)) = map.get_mut(key) {
                    for (_, v) in child_map.iter_mut() {
                        if v.is_boolean() {
                            *v = Value::Object(serde_json::Map::new());
                        }
                    }
                }
            }
            for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
                if let Some(Value::Array(arr)) = map.get_mut(key) {
                    for v in arr.iter_mut() {
                        if v.is_boolean() {
                            *v = Value::Object(serde_json::Map::new());
                        }
                    }
                }
            }
            let is_object_schema = map
                .get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "object")
                .unwrap_or(false);
            if is_object_schema && !map.contains_key("properties") {
                map.insert(
                    "properties".to_string(),
                    Value::Object(serde_json::Map::new()),
                );
            }
            for (_, v) in map.iter_mut() {
                normalize_recursive(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                normalize_recursive(v);
            }
        }
        _ => {}
    }
}

pub use describe_tool::{
    dispatch_describe_tool, into_tool_result as describe_tool_into_tool_result,
    DescribeToolDispatchResult, DESCRIBE_TOOL_NAME,
};

pub use document_tools::{
    build_all_document_tools, build_document_apply_patch_tool, build_document_create_tool,
    build_document_get_head_tool, build_document_list_my_artifacts_tool,
    build_document_list_versions_tool, build_document_read_tool, build_document_rollback_tool,
    dispatch_document_apply_patch, dispatch_document_create, dispatch_document_get_head,
    dispatch_document_list_my_artifacts, dispatch_document_list_versions, dispatch_document_read,
    dispatch_document_rollback, DocumentToolsContext, DOCUMENTS_SYSTEM_PRELUDE,
    DOCUMENT_APPLY_PATCH_TOOL, DOCUMENT_CREATE_TOOL, DOCUMENT_GET_HEAD_TOOL,
    DOCUMENT_LIST_MY_ARTIFACTS_TOOL, DOCUMENT_LIST_VERSIONS_TOOL, DOCUMENT_READ_TOOL,
    DOCUMENT_ROLLBACK_TOOL,
};

pub use lazy_tools_catalog::{
    build_describe_tool_definition, reconstruct_discovered_set, summary_for_catalog, CatalogEntry,
};

pub use toolkit_packages::{find_package, ToolkitPackage, TOOLKIT_PACKAGES};

pub use load_attachment_tool::{
    build_load_attachment_tool_definition, dispatch_load_attachment, ATTACHMENTS_SYSTEM_PRELUDE,
    LOAD_ATTACHMENT_TOOL_NAME,
};

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, into_tool_result,
    LoadSkillDispatchResult, LOAD_SKILL_TOOL_NAME,
};

pub use crdt_summary::{build_recent_changes_block, CRDT_SPREADSHEET_PROTOCOL_PRELUDE};

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
    TOOL_LIST_SHEETS_OF as CRDT_DOC_LIST_SHEETS_OF_TOOL, TOOL_READ as CRDT_DOC_READ_TOOL,
    TOOL_SET_CELL as CRDT_DOC_SET_CELL_TOOL, TOOL_SET_RANGE as CRDT_DOC_SET_RANGE_TOOL,
};

pub use crdt_doc_run_python::{
    dispatch_crdt_doc_run_python, execute_run_python, tool_run_python, RunPythonArgs,
    TOOL_RUN_PYTHON as CRDT_DOC_RUN_PYTHON_TOOL,
};

pub use crdt_doc_import_sheet::{
    dispatch_crdt_doc_import_sheet, execute_import_sheet, tool_import_sheet, ImportSheetArgs,
    MAX_IMPORT_BYTES, MAX_SHEETS_PER_ARTIFACT, TOOL_IMPORT_SHEET as CRDT_DOC_IMPORT_SHEET_TOOL,
};

pub use recall_history::{
    dispatch_recall_history, tool_recall_history, RecallHistoryArgs,
    TOOL_RECALL_HISTORY as RECALL_HISTORY_TOOL,
};

pub use gsheets_run_python::{
    dispatch_gsheets_run_python, dispatch_gsheets_run_python_with_client,
    tool_gsheets_run_python as gsheets_tool_run_python, GsheetsBinding, GsheetsRunPythonArgs,
    TOOL_GSHEETS_RUN_PYTHON,
};

pub use gsheets_tools::{
    dispatch_add_sheet as dispatch_gsheets_add_sheet,
    dispatch_create_spreadsheet as dispatch_gsheets_create_spreadsheet,
    dispatch_delete_sheet as dispatch_gsheets_delete_sheet,
    dispatch_list_sheets as dispatch_gsheets_list_sheets, dispatch_read as dispatch_gsheets_read,
    dispatch_set_cell as dispatch_gsheets_set_cell,
    dispatch_set_range as dispatch_gsheets_set_range, tool_add_sheet as gsheets_tool_add_sheet,
    tool_create_from_xlsx as gsheets_tool_create_from_xlsx,
    tool_create_spreadsheet as gsheets_tool_create_spreadsheet,
    tool_delete_sheet as gsheets_tool_delete_sheet, tool_export_xlsx as gsheets_tool_export_xlsx,
    tool_list_sheets as gsheets_tool_list_sheets, tool_read as gsheets_tool_read,
    tool_set_cell as gsheets_tool_set_cell, tool_set_range as gsheets_tool_set_range,
    TOOL_ADD_SHEET as GSHEETS_ADD_SHEET_TOOL,
    TOOL_CREATE_FROM_XLSX as GSHEETS_CREATE_FROM_XLSX_TOOL,
    TOOL_CREATE_SPREADSHEET as GSHEETS_CREATE_SPREADSHEET_TOOL,
    TOOL_DELETE_SHEET as GSHEETS_DELETE_SHEET_TOOL, TOOL_EXPORT_XLSX as GSHEETS_EXPORT_XLSX_TOOL,
    TOOL_LIST_SHEETS as GSHEETS_LIST_SHEETS_TOOL, TOOL_READ as GSHEETS_READ_TOOL,
    TOOL_SET_CELL as GSHEETS_SET_CELL_TOOL, TOOL_SET_RANGE as GSHEETS_SET_RANGE_TOOL,
};

pub use gdocs_tools::{
    build_all_gdocs_tools,
    dispatch_acknowledge_human_changes as dispatch_gdocs_acknowledge_human_changes,
    dispatch_add_tab as dispatch_gdocs_add_tab,
    dispatch_append_markdown as dispatch_gdocs_append_markdown,
    dispatch_apply_edits as dispatch_gdocs_apply_edits, dispatch_create as dispatch_gdocs_create,
    dispatch_create_from_docx as dispatch_gdocs_create_from_docx,
    dispatch_create_from_markdown as dispatch_gdocs_create_from_markdown,
    dispatch_create_named_range as dispatch_gdocs_create_named_range,
    dispatch_delete_text as dispatch_gdocs_delete_text, dispatch_export as dispatch_gdocs_export,
    dispatch_insert_after_text as dispatch_gdocs_insert_after_text,
    dispatch_insert_before_text as dispatch_gdocs_insert_before_text,
    dispatch_insert_between as dispatch_gdocs_insert_between,
    dispatch_list_named_ranges as dispatch_gdocs_list_named_ranges,
    dispatch_list_tabs as dispatch_gdocs_list_tabs,
    dispatch_read_as_markdown as dispatch_gdocs_read_as_markdown,
    dispatch_read_outline as dispatch_gdocs_read_outline,
    dispatch_replace_named_range as dispatch_gdocs_replace_named_range,
    dispatch_replace_section as dispatch_gdocs_replace_section,
    dispatch_replace_text as dispatch_gdocs_replace_text, dispatch_share as dispatch_gdocs_share,
    dispatch_style_text as dispatch_gdocs_style_text,
    tool_acknowledge_human_changes as gdocs_tool_acknowledge_human_changes,
    tool_add_tab as gdocs_tool_add_tab, tool_append_markdown as gdocs_tool_append_markdown,
    tool_apply_edits as gdocs_tool_apply_edits, tool_create as gdocs_tool_create,
    tool_create_from_docx as gdocs_tool_create_from_docx,
    tool_create_from_markdown as gdocs_tool_create_from_markdown,
    tool_create_named_range as gdocs_tool_create_named_range,
    tool_delete_text as gdocs_tool_delete_text, tool_export as gdocs_tool_export,
    tool_insert_after_text as gdocs_tool_insert_after_text,
    tool_insert_before_text as gdocs_tool_insert_before_text,
    tool_insert_between as gdocs_tool_insert_between,
    tool_list_named_ranges as gdocs_tool_list_named_ranges, tool_list_tabs as gdocs_tool_list_tabs,
    tool_read_as_markdown as gdocs_tool_read_as_markdown,
    tool_read_outline as gdocs_tool_read_outline,
    tool_replace_named_range as gdocs_tool_replace_named_range,
    tool_replace_section as gdocs_tool_replace_section,
    tool_replace_text as gdocs_tool_replace_text, tool_share as gdocs_tool_share,
    tool_style_text as gdocs_tool_style_text,
    TOOL_ACKNOWLEDGE_HUMAN_CHANGES as GDOCS_ACKNOWLEDGE_HUMAN_CHANGES_TOOL,
    TOOL_ADD_TAB as GDOCS_ADD_TAB_TOOL, TOOL_APPEND_MARKDOWN as GDOCS_APPEND_MARKDOWN_TOOL,
    TOOL_APPLY_EDITS as GDOCS_APPLY_EDITS_TOOL, TOOL_CREATE as GDOCS_CREATE_TOOL,
    TOOL_CREATE_FROM_DOCX as GDOCS_CREATE_FROM_DOCX_TOOL,
    TOOL_CREATE_FROM_MARKDOWN as GDOCS_CREATE_FROM_MARKDOWN_TOOL,
    TOOL_CREATE_NAMED_RANGE as GDOCS_CREATE_NAMED_RANGE_TOOL,
    TOOL_DELETE_TEXT as GDOCS_DELETE_TEXT_TOOL, TOOL_EXPORT as GDOCS_EXPORT_TOOL,
    TOOL_INSERT_AFTER_TEXT as GDOCS_INSERT_AFTER_TEXT_TOOL,
    TOOL_INSERT_BEFORE_TEXT as GDOCS_INSERT_BEFORE_TEXT_TOOL,
    TOOL_INSERT_BETWEEN as GDOCS_INSERT_BETWEEN_TOOL,
    TOOL_LIST_NAMED_RANGES as GDOCS_LIST_NAMED_RANGES_TOOL, TOOL_LIST_TABS as GDOCS_LIST_TABS_TOOL,
    TOOL_READ_AS_MARKDOWN as GDOCS_READ_AS_MARKDOWN_TOOL,
    TOOL_READ_OUTLINE as GDOCS_READ_OUTLINE_TOOL,
    TOOL_REPLACE_NAMED_RANGE as GDOCS_REPLACE_NAMED_RANGE_TOOL,
    TOOL_REPLACE_SECTION as GDOCS_REPLACE_SECTION_TOOL,
    TOOL_REPLACE_TEXT as GDOCS_REPLACE_TEXT_TOOL, TOOL_SHARE as GDOCS_SHARE_TOOL,
    TOOL_STYLE_TEXT as GDOCS_STYLE_TEXT_TOOL,
};

#[cfg(test)]
mod text_coverage_tests {
    //! Enforces that EVERY synthetic tool registered in colmena has an
    //! entry in text/tools/*.yaml and that no YAML entry is orphaned. The
    //! build refuses to ship if either invariant breaks.
    //!
    //! `describe_tool` is exempt — it is constructed dynamically per turn
    //! by `lazy_tools_catalog::build_describe_tool_definition` and does
    //! not go through the synthetic-tool builders covered here.

    use crate::llm::domain::tools::ToolDefinition;
    use crate::skills::domain::{Skill, SkillCatalogEntry, SkillError, SkillReference};
    use crate::text;
    use async_trait::async_trait;
    use std::sync::Arc;

    /// Minimal in-test SkillRepository that returns an empty catalog.
    struct EmptySkillRepo;

    #[async_trait]
    impl crate::skills::domain::SkillRepository for EmptySkillRepo {
        fn list_available(&self) -> Vec<SkillCatalogEntry> {
            vec![]
        }
        async fn load_skill(&self, _name: &str) -> Result<Skill, SkillError> {
            Err(SkillError::SkillNotFound("test".to_string()))
        }
        async fn load_reference(
            &self,
            _skill_name: &str,
            _reference_name: &str,
        ) -> Result<SkillReference, SkillError> {
            Err(SkillError::SkillNotFound("test".to_string()))
        }
    }

    /// Returns every synthetic ToolDefinition the colmena library registers,
    /// excluding describe_tool (dynamic per-turn construction).
    fn all_synthetic_tools() -> Vec<ToolDefinition> {
        // gsheets — 9 tools + gsheets_run_python
        let mut tools = vec![
            super::gsheets_tools::tool_create_spreadsheet(),
            super::gsheets_tools::tool_create_from_xlsx(),
            super::gsheets_tools::tool_export_xlsx(),
            super::gsheets_tools::tool_list_sheets(),
            super::gsheets_tools::tool_add_sheet(),
            super::gsheets_tools::tool_delete_sheet(),
            super::gsheets_tools::tool_read(),
            super::gsheets_tools::tool_set_cell(),
            super::gsheets_tools::tool_set_range(),
            super::gsheets_run_python::tool_gsheets_run_python(),
        ];

        // gdocs — 22 tools (via collector)
        tools.extend(super::gdocs_tools::build_all_gdocs_tools());

        // crdt_doc — 9 tools (via collector)
        tools.extend(super::crdt_doc_tools::build_all_crdt_doc_tools());

        // crdt_doc_run_python + crdt_doc_import_sheet
        tools.push(super::crdt_doc_run_python::tool_run_python());
        tools.push(super::crdt_doc_import_sheet::tool_import_sheet());

        // document_tools — 7 tools (via collector)
        tools.extend(super::document_tools::build_all_document_tools());

        // recall_history — 1 tool
        tools.push(super::recall_history::tool_recall_history());

        // load_attachment — 1 tool; pass empty catalog (valid defensive path per docs)
        tools.push(super::load_attachment_tool::build_load_attachment_tool_definition(&[]));

        // load_skill — 1 tool; pass an empty-catalog repository
        let repo: Arc<dyn crate::skills::domain::SkillRepository> = Arc::new(EmptySkillRepo);
        tools.push(super::load_skill_tool::build_load_skill_tool_definition(
            &repo,
        ));

        // sql_bulk_tools — 2 tools (item 13, 2026-06-09)
        tools.push(super::sql_bulk_tools::build_sql_inspect_attachment_tool_definition());
        tools.push(super::sql_bulk_tools::build_sql_bulk_insert_tool_definition());

        tools
    }

    #[test]
    fn every_registered_tool_has_text_entry() {
        let tools = all_synthetic_tools();
        for td in &tools {
            let s = text::tool_summary(&td.name);
            let d = text::tool_description(&td.name);
            let len = s.chars().count();
            assert!(
                (10..=200).contains(&len),
                "summary for '{}' out of bounds (len={})",
                td.name,
                len,
            );
            assert!(!d.is_empty(), "description for '{}' is empty", td.name);
        }
        assert!(
            !tools.is_empty(),
            "all_synthetic_tools() returned 0 entries — wiring bug",
        );
    }

    #[test]
    fn no_orphan_yaml_entries() {
        let registered: std::collections::HashSet<String> = all_synthetic_tools()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let orphans: Vec<&'static str> = text::all_tool_names()
            .into_iter()
            .filter(|name| !registered.contains(*name))
            .collect();
        assert!(
            orphans.is_empty(),
            "Orphan YAML entries (no matching registered builder): {:?}",
            orphans,
        );
    }

    #[test]
    fn tool_def_summary_matches_yaml() {
        // ToolDefinition.summary is set by the builder, which (after E-T17c-f)
        // reads from the YAML. This test verifies they stay in sync, catching
        // a regression where someone reverts a builder to an inline literal.
        for td in all_synthetic_tools() {
            let yaml_summary = text::tool_summary(&td.name);
            assert_eq!(
                td.summary.as_deref(),
                Some(yaml_summary),
                "ToolDefinition.summary for '{}' diverges from text/tools/*.yaml — \
                 likely a builder was hand-edited",
                td.name,
            );
        }
    }

    #[test]
    fn index_doc_covers_all_registered_tools() {
        // Embed the doc at compile time so the test is portable.
        // Path: from mod.rs → up to repo root → docs/developer_guide/41_builtin_tools_index.md
        // mod.rs is at src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
        //   = 8 directories under repo root
        const INDEX_DOC: &str =
            include_str!("../../../../../../../../docs/developer_guide/41_builtin_tools_index.md");

        let registered: Vec<String> = all_synthetic_tools()
            .iter()
            .map(|t| t.name.clone())
            .collect();

        let mut missing: Vec<String> = Vec::new();
        for name in &registered {
            // Each tool name should appear at least once as a backtick-wrapped
            // token in the doc (the table convention `| \`tool_name\` | ...`).
            let needle = format!("`{}`", name);
            if !INDEX_DOC.contains(&needle) {
                missing.push(name.clone());
            }
        }

        assert!(
            missing.is_empty(),
            "These registered tools are missing from \
             docs/developer_guide/41_builtin_tools_index.md: {:?}",
            missing,
        );
    }

    #[test]
    fn index_doc_covers_all_registered_skills() {
        // Embed the skills-index doc via include_str! so the test is portable.
        // Path: from mod.rs at
        //   src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
        // go up 8 levels to repo root, then into docs/developer_guide/.
        const INDEX_DOC: &str =
            include_str!("../../../../../../../../docs/developer_guide/42_builtin_skills_index.md");

        let skills_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");

        let mut missing: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&skills_dir).expect("can read skills dir") {
            let entry = entry.expect("can read dir entry");
            if !entry.file_type().expect("file_type").is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // _placeholder exists only for include_dir!'s empty-folder contract.
            if name == "_placeholder" {
                continue;
            }
            // Only folders that actually contain a SKILL.md count as registered skills.
            if !entry.path().join("SKILL.md").exists() {
                continue;
            }
            let needle = format!("`{}`", name);
            if !INDEX_DOC.contains(&needle) {
                missing.push(name);
            }
        }

        assert!(
            missing.is_empty(),
            "These registered skills are missing from \
             docs/developer_guide/42_builtin_skills_index.md: {:?}",
            missing,
        );
    }
}

#[cfg(test)]
mod synthetic_builder_tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct FakeArgs {
        pub x: String,
    }

    #[test]
    fn build_synthetic_tool_with_summary_sets_summary() {
        let td = build_synthetic_tool_with_summary::<FakeArgs>(
            "fake_tool",
            "A fake tool used only in tests",
            "Run a fake operation",
        );
        assert_eq!(td.name, "fake_tool");
        assert_eq!(td.summary.as_deref(), Some("Run a fake operation"));
        assert!(td.input_schema_override.is_some());
    }

    #[test]
    fn build_synthetic_tool_without_summary_is_none() {
        let td = build_synthetic_tool::<FakeArgs>("fake_tool", "A fake tool");
        assert!(td.summary.is_none());
    }
}

#[cfg(test)]
mod sanitize_tests {
    use super::sanitize_schema_for_llm_providers;
    use serde_json::json;

    #[test]
    fn strips_dollar_schema() {
        let mut v = json!({"$schema": "http://...", "type": "object"});
        sanitize_schema_for_llm_providers(&mut v);
        assert!(v.get("$schema").is_none());
    }

    #[test]
    fn collapses_optional_string_type() {
        let mut v = json!({
            "type": "object",
            "properties": {
                "range": {"type": ["string", "null"]}
            }
        });
        sanitize_schema_for_llm_providers(&mut v);
        assert_eq!(v["properties"]["range"]["type"], json!("string"));
    }

    #[test]
    fn collapses_optional_number_type() {
        let mut v = json!({"type": ["null", "number"]});
        sanitize_schema_for_llm_providers(&mut v);
        assert_eq!(v["type"], json!("number"));
    }

    #[test]
    fn preserves_multi_non_null_union() {
        let mut v = json!({"type": ["string", "number"]});
        sanitize_schema_for_llm_providers(&mut v);
        assert_eq!(v["type"], json!(["string", "number"]));
    }

    #[test]
    fn replaces_boolean_schema_in_properties() {
        // schemars emits `true` for `serde_json::Value` fields — the
        // SetCellArgs `value` field reproduces this.
        let mut v = json!({
            "type": "object",
            "properties": {
                "value": true
            }
        });
        sanitize_schema_for_llm_providers(&mut v);
        assert_eq!(v["properties"]["value"], json!({}));
    }

    #[test]
    fn replaces_boolean_schema_in_anyof() {
        let mut v = json!({
            "anyOf": [{"type": "string"}, true]
        });
        sanitize_schema_for_llm_providers(&mut v);
        assert_eq!(v["anyOf"][1], json!({}));
    }

    #[test]
    fn inlines_dollar_ref_from_definitions() {
        // schemars emits `$ref` + `definitions` for any nested struct/enum.
        // Gemini rejects both — sanitizer must inline the ref body and drop
        // the definitions map. Reproduces what `Vec<GsheetsBinding>` produces.
        let mut v = json!({
            "type": "object",
            "properties": {
                "bindings": {
                    "type": "array",
                    "items": { "$ref": "#/definitions/Binding" }
                }
            },
            "definitions": {
                "Binding": {
                    "type": "object",
                    "properties": {
                        "var": { "type": "string" }
                    }
                }
            }
        });
        sanitize_schema_for_llm_providers(&mut v);
        assert!(
            v.get("definitions").is_none(),
            "definitions must be dropped"
        );
        assert!(
            v["properties"]["bindings"]["items"].get("$ref").is_none(),
            "$ref must be inlined"
        );
        assert_eq!(
            v["properties"]["bindings"]["items"]["properties"]["var"]["type"],
            json!("string")
        );
    }

    #[test]
    fn inlines_dollar_ref_from_dollar_defs() {
        // schemars 0.8 emits `$defs` instead of `definitions` in some modes —
        // handle both for forward-compat.
        let mut v = json!({
            "type": "object",
            "properties": {
                "x": { "$ref": "#/$defs/Foo" }
            },
            "$defs": {
                "Foo": { "type": "string" }
            }
        });
        sanitize_schema_for_llm_providers(&mut v);
        assert!(v.get("$defs").is_none());
        assert_eq!(v["properties"]["x"]["type"], json!("string"));
    }
}
