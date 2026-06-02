//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.

pub mod crdt_doc_tools;
pub mod describe_tool;
pub mod document_tools;
pub mod lazy_tools_catalog;
pub mod load_attachment_tool;
pub mod load_skill_tool;

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
        parameters: ToolParameters::new(),
        input_schema_override: Some(schema_json),
    }
}

/// Walk a JSON Schema and normalize shapes that some LLM providers (notably
/// OpenAI) reject:
///
/// 1. Boolean schemas at `items` / `additionalProperties` positions are
///    replaced with `{}` (schemars emits `true` for opaque types like
///    `serde_json::Value`, but OpenAI requires an object schema there).
/// 2. Any `{"type": "object"}` without a `properties` field gets an empty
///    `properties: {}` injected (OpenAI requires `properties` to be present
///    even on schemas that take no parameters).
pub(super) fn sanitize_schema_for_llm_providers(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for key in ["items", "additionalProperties"] {
                if let Some(v) = map.get_mut(key) {
                    if v.is_boolean() {
                        *v = Value::Object(serde_json::Map::new());
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
                sanitize_schema_for_llm_providers(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                sanitize_schema_for_llm_providers(v);
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

pub use load_attachment_tool::{
    build_load_attachment_tool_definition, dispatch_load_attachment, ATTACHMENTS_SYSTEM_PRELUDE,
    LOAD_ATTACHMENT_TOOL_NAME,
};

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, into_tool_result,
    LoadSkillDispatchResult, LOAD_SKILL_TOOL_NAME,
};

pub use crdt_doc_tools::{
    build_all_crdt_doc_tools, dispatch_crdt_doc_add_sheet, dispatch_crdt_doc_list_sheets,
    dispatch_crdt_doc_read, dispatch_crdt_doc_set_cell, dispatch_crdt_doc_set_range,
    CrdtDocsContext, TOOL_ADD_SHEET as CRDT_DOC_ADD_SHEET_TOOL,
    TOOL_LIST_SHEETS as CRDT_DOC_LIST_SHEETS_TOOL, TOOL_READ as CRDT_DOC_READ_TOOL,
    TOOL_SET_CELL as CRDT_DOC_SET_CELL_TOOL, TOOL_SET_RANGE as CRDT_DOC_SET_RANGE_TOOL,
};
