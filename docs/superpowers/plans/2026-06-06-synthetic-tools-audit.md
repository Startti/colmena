# Synthetic tools audit (2026-06-06)

Generated for E-T15a. Enumerates every Rust-side synthetic tool builder, current summary status, and proposed one-line summary for each. This audit drives the migration in E-T15b–15e.

---

## gsheets_tools.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_create_spreadsheet` | `gsheets_create_spreadsheet` | No | Create a new Google Sheets workbook and return its URL |
| `tool_create_from_xlsx` | `gsheets_create_from_xlsx` | No | Upload a local .xlsx attachment and convert it into a new Google Sheet |
| `tool_export_xlsx` | `gsheets_export_xlsx` | No | Download an existing Google Sheet as .xlsx bytes attachment |
| `tool_list_sheets` | `gsheets_list_sheets` | No | List every tab (sheet) inside a spreadsheet by ID |
| `tool_add_sheet` | `gsheets_add_sheet` | No | Create a new tab inside an existing spreadsheet |
| `tool_delete_sheet` | `gsheets_delete_sheet` | No | Permanently delete a tab from a spreadsheet |
| `tool_read` | `gsheets_read` | No | Read a cell range from a tab; supports formatted, unformatted, and formula render modes |
| `tool_set_cell` | `gsheets_set_cell` | No | Write one value or formula into a single cell |
| `tool_set_range` | `gsheets_set_range` | No | Write a 2-D values array starting at a given address |

---

## gsheets_run_python.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_gsheets_run_python` | `gsheets_run_python` | No | Run sandboxed Python (pandas/numpy/scipy.stats) over data loaded directly from Google Sheets; rows never pass through the LLM |

---

## crdt_doc_tools.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_list_sheets` | `crdt_doc_list_sheets` | No | List the sheets in the current CRDT document with metadata |
| `tool_list_sheets_of` | `crdt_doc_list_sheets_of` | No | Peek at another artifact's sheets (cross-artifact reference) |
| `tool_read` | `crdt_doc_read` | No | Read a cell range from a sheet; supports formatted, unformatted, and formula render modes |
| `tool_set_cell` | `crdt_doc_set_cell` | No | Write one value or formula into a single cell |
| `tool_set_range` | `crdt_doc_set_range` | No | Write a 2-D values array starting at a given address |
| `tool_add_sheet` | `crdt_doc_add_sheet` | No | Create a new tab inside the current document |
| `tool_get_recent_changes` | `crdt_doc_get_recent_changes` | No | Retrieve the last N changes made to the document with timestamps and authors |
| `tool_list_my_artifacts` | `crdt_doc_list_my_artifacts` | No | List every CRDT document artifact belonging to the current session |
| `tool_create_artifact` | `crdt_doc_create_artifact` | No | Create a new empty CRDT document artifact |
| `build_all_crdt_doc_tools` | (returns Vec, not individual tool) | — | (builder function, not enumerated) |

---

## crdt_doc_run_python.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_run_python` | `crdt_doc_run_python` | No | Run sandboxed Python (pandas/numpy/scipy.stats) over requested sheets; define output and/or output_sheet |

---

## crdt_doc_import_sheet.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_import_sheet` | `crdt_doc_import_sheet` | No | Clone a sheet from another artifact into the current one (snapshot, not a live link) |

---

## document_tools.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `build_document_create_tool` | `document_create` | No | Create a new document artifact (Excel or Word); returns artifact_id and initial version |
| `build_document_apply_patch_tool` | `document_apply_patch` | No | Apply a patch (list of ops) to an existing document atomically with auto-rebase on non-conflicting changes |
| `build_document_read_tool` | `document_read` | No | Read the IR of a document at a given version (or current HEAD) with optional slicing |
| `build_document_get_head_tool` | `document_get_head` | No | Get the current HEAD of an artifact; optionally receive a natural-language narration of user edits since a baseline version |
| `build_document_list_versions_tool` | `document_list_versions` | No | List the versions retained for an artifact with timestamps, source and per-version summary |
| `build_document_rollback_tool` | `document_rollback` | No | Roll back an artifact to a previous version; full history is preserved |
| `build_document_list_my_artifacts_tool` | `document_list_my_artifacts` | No | List every document artifact that belongs to the current session with metadata |
| `build_all_document_tools` | (returns Vec, not individual tool) | — | (builder function, not enumerated) |

---

## load_skill_tool.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `build_load_skill_tool_definition` | `load_skill` | No | Load a knowledge skill on demand; call before responding when relevant |

---

## load_attachment_tool.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `build_load_attachment_tool_definition` | `load_attachment` | No | Load a document that has been attached to this conversation; returns content or metadata |

---

## recall_history.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `tool_recall_history` | `recall_history` | No | Re-read the FULL original content of one past message by its turn index from the conversation summary |

---

## describe_tool.rs + lazy_tools_catalog.rs

| Builder | Tool name (constant) | Has summary today | Proposed summary |
|---|---|---|---|
| `build_describe_tool_definition` | `describe_tool` | No | Reveal the full parameter schema and usage notes for one of the available tools (lazy loading) |

**Note**: `describe_tool` is **exempt from E-T15c migration**. It is constructed dynamically by `lazy_tools_catalog::build_describe_tool_definition()` at each turn based on the pending undiscovered tools. The summary is synthesized from the catalog entries themselves and is not a fixed Rust constant.

---

## api_explorer toolkits

The `api_explorer` toolkits are **not built via synthetic-tool builders** in `llm_synthetic_tools/`. Instead, they are:

1. **Constructed dynamically** by the DAG executor when a tool_configurations entry with `node_type: "api_explorer"` is present.
2. **Registered by the `__` prefix-rule** in `llm.rs` where they match any tool name starting with `api_explorer__` (e.g., `api_explorer__load_spec`, `api_explorer__list_endpoints`).
3. **Not enumerable statically** — the five sub-tools (`load_spec`, `list_endpoints`, `search_endpoint`, `get_endpoint_details`, `build_http_request`) are synthesized at runtime per OpenAPI spec.

Per the toolkit-packages design (spec §4.8), `api_explorer` is an **existing prefix-rule** that will continue to work via the existing `__` mechanism. **No E-T15 changes needed** — the toolkit packages expansion in E-T16 preserves back-compat by continuing to support the prefix-rule alongside the new `gsheets` / future package aliases.

---

## Summary

### Tool count by module

| Module | Count | Notes |
|---|---:|---|
| gsheets_tools.rs | 9 | no summary field yet |
| gsheets_run_python.rs | 1 | no summary field yet |
| crdt_doc_tools.rs | 9 | no summary field yet |
| crdt_doc_run_python.rs | 1 | no summary field yet |
| crdt_doc_import_sheet.rs | 1 | no summary field yet |
| document_tools.rs | 7 | no summary field yet |
| load_skill_tool.rs | 1 | no summary field yet |
| load_attachment_tool.rs | 1 | no summary field yet |
| recall_history.rs | 1 | no summary field yet |
| describe_tool.rs | 1 | dynamically synthesized per turn (exempt from E-T15c) |
| **Total** | **32** | **excluding describe_tool's dynamic generation** |

**Actual deliverable builders: 32 unique tools**, all of which have well-written one-line proposed summaries (see "Proposed summary" column). However, `ToolDefinition` does not yet have a `summary` field. This confirms the spec's 35–40 estimate is accurate (32 synthetic + describe_tool + api_explorer's 5 sub-tools = ~38).

### Status

**No tools currently have a `summary` field in `ToolDefinition`** (see `src/libs/colmena/src/llm/domain/tools.rs:32–49`). All 32 enumerated tools have proposed summaries ready. The E-T15b–15e migration tasks will:

1. Add the `summary` field to `ToolDefinition`.
2. Populate it in each of the 32 builders via the proposed summaries.
3. Enforce via CI that every new synthetic tool registered in the future includes a summary.
4. Wire the summaries into the lazy-loading catalog for progressive reveal in the LLM system message.
5. Test end-to-end that `lazy_tool_loading: true` agents benefit from the summaries before paying for full schemas.

---

**Audit completed 2026-06-06. Ready for E-T15b implementation.**
