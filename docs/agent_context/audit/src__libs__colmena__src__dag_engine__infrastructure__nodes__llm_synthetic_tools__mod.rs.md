# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs

**Layer:** infrastructure  **Purpose:** Module hub and shared schema utilities for synthetic LLM tools — provides builders for schemars-backed JSON schemas, sanitization normalizations for OpenAI/Gemini/Anthropic quirks, and re-exports all ~120 tool definitions and dispatchers from 30+ submodules (gsheets, gdocs, crdt_doc, document, sql, attachment, skills, etc.).

## Symbols

### Module declarations
- `attachment_run_python` (mod, pub) — Python execution tool over attachments
- `attachment_writer` (mod, pub) — Attachment result writer
- `crdt_doc_context` (mod, pub) — CRDT document context utilities
- `crdt_doc_import_sheet` (mod, pub) — Sheet import into CRDT documents
- `crdt_doc_run_python` (mod, pub) — Python execution on CRDT documents
- `crdt_doc_tools` (mod, pub) — CRDT document manipulation tools
- `crdt_summary` (mod, pub) — CRDT document change summaries
- `data_run_python` (mod, pub) — Unified CSV/XLSX/SQL/Sheets Python execution
- `describe_tool` (mod, pub) — Tool schema discovery and lazy catalog
- `diff_writer` (mod, pub) — Diff writing utilities
- `document_tools` (mod, pub) — Document (v0 patches-based) tools
- `gdocs_tools` (mod, pub) — Google Docs manipulation tools (25 tools)
- `google_workspace_prelude` (mod, pub) — Google Workspace system prelude
- `gsheets_inspect_guard` (mod, pub) — Sheet inspection safety guards
- `gsheets_run_python` (mod, pub) — Python execution on Google Sheets
- `gsheets_tools` (mod, pub) — Google Sheets manipulation tools (14 tools)
- `lazy_tools_catalog` (mod, pub) — Lazy tool loading catalog and discovery
- `load_attachment_tool` (mod, pub) — Attachment loading tool
- `load_skill_tool` (mod, pub) — Skill loading tool
- `markdown_to_docs_ops` (mod, pub) — Markdown-to-docs conversion utilities
- `recall_history` (mod, pub) — Conversation history recall tool
- `sheet_collision` (mod, pub) — Sheet collision detection and policies
- `sheet_writer` (mod, pub) — Sheet data writing utilities
- `sql_bulk_tools` (mod, pub) — SQL bulk insert and inspection tools
- `table_writer` (mod, pub) — Table writing utilities
- `tabular_bindings` (mod, pub) — Tabular data binding types
- `toolkit_packages` (mod, pub) — Toolkit package registry

### Functions
- `build_synthetic_tool<T: JsonSchema>` (pub(super) fn) — Builds a ToolDefinition from `T` via schemars, sanitizes for providers, stores in `input_schema_override`
- `build_synthetic_tool_with_summary<T: JsonSchema>` (pub(super) fn) — Like `build_synthetic_tool` but also sets a summary field for lazy catalog indexing  [FLAG: improvement — docstring (lines 60–63) references nonexistent test `every_synthetic_tool_has_summary`; actual test is `tool_def_summary_matches_yaml`]
- `sanitize_schema_for_llm_providers` (pub(super) fn) — Normalizes JSON schema for OpenAI/Gemini/Anthropic provider compatibility
- `inline_refs_top_level` (fn, private) — Extracts top-level `definitions`/`$defs` and resolves all `$ref` references to inline schema bodies
- `resolve_refs` (fn, private) — Recursively inlines `$ref` with depth limit of 32; walks object and array structures
- `normalize_recursive` (fn, private) — Recursively normalizes schema: strips `$schema`, collapses `["type", "null"]` to scalar, replaces boolean schemas with `{}`, removes `additionalProperties`

### Re-exports from submodules (126 total)
- `describe_tool`: `dispatch_describe_tool`, `describe_tool_into_tool_result`, `DescribeToolDispatchResult`, `DESCRIBE_TOOL_NAME`
- `document_tools`: `build_all_document_tools`, 5x `build_document_*`, 7x `dispatch_document_*`, `DocumentToolsContext`, `DOCUMENTS_SYSTEM_PRELUDE`, 7 tool name constants
- `lazy_tools_catalog`: `build_describe_tool_definition`, `current_turn_slice`, `reconstruct_discovered_set`, `summary_for_catalog`, `CatalogEntry`
- `toolkit_packages`: `find_package`, `ToolkitPackage`, `TOOLKIT_PACKAGES`
- `load_attachment_tool`: `build_load_attachment_tool_definition`, `dispatch_load_attachment`, `ATTACHMENTS_SYSTEM_PRELUDE`, `LOAD_ATTACHMENT_TOOL_NAME`
- `load_skill_tool`: `build_load_skill_tool_definition`, `dispatch_load_skill`, `LoadSkillDispatchResult`, `LOAD_SKILL_TOOL_NAME`
- `crdt_summary`: `build_recent_changes_block`, `CRDT_SPREADSHEET_PROTOCOL_PRELUDE`
- `google_workspace_prelude`: `build_google_workspace_prelude`, `has_google_workspace_tools`, `resolve_share_email`
- `crdt_doc_tools`: `build_all_crdt_doc_tools`, 8x `dispatch_crdt_doc_*`, `CrdtDocsContext`, `ListSheetsOfArgs`, 8 tool name constants
- `crdt_doc_run_python`: `dispatch_crdt_doc_run_python`, `execute_run_python`, `tool_run_python`, `RunPythonArgs`, tool name constant
- `crdt_doc_import_sheet`: `dispatch_crdt_doc_import_sheet`, `execute_import_sheet`, `tool_import_sheet`, `ImportSheetArgs`, `MAX_IMPORT_BYTES`, `MAX_SHEETS_PER_ARTIFACT`, tool name constant
- `recall_history`: `dispatch_recall_history`, `tool_recall_history`, `RecallHistoryArgs`, tool name constant
- `gsheets_run_python`: `dispatch_gsheets_run_python`, `dispatch_gsheets_run_python_with_client`, `gsheets_tool_run_python`, `GsheetsBinding`, `GsheetsRunPythonArgs`, `TOOL_GSHEETS_RUN_PYTHON`
- `gsheets_tools`: 14 tool builders (create_spreadsheet, create_from_xlsx, export_xlsx, list_sheets, list_spreadsheets, add_sheet, delete_sheet, read, set_cell, set_range, format_range, share, list_permissions, unshare) with corresponding dispatchers and name constants
- `gdocs_tools`: 25 tool builders (acknowledge_human_changes, add_comment, add_tab, append_markdown, apply_edits, create, create_from_docx, create_from_markdown, create_named_range, delete_table_column, delete_table_row, delete_text, export, format_table, insert_after_text, insert_before_text, insert_between, insert_image_after_text, insert_table_column, insert_table_row, list_comments, list_documents, list_named_ranges, list_permissions, list_tabs, read_as_markdown, read_outline, read_tables, replace_named_range, replace_section, replace_text, resolve_comment, set_table_cell, share, style_text, unshare) with corresponding dispatchers and name constants

### Tests (all `#[cfg(test)]`)
- `text_coverage_tests` (mod) — Enforces YAML text registry coverage for all synthetic tools
  - `EmptySkillRepo` (struct) — Dummy SkillRepository for test isolation
  - `all_synthetic_tools()` (fn) — Collects all registered tool definitions for coverage checks
  - `every_registered_tool_has_text_entry()` (test) — Validates each tool has text/tools YAML entry with 10–200 char summary
  - `no_orphan_yaml_entries()` (test) — Validates no YAML entries exist without registered tool builder
  - `tool_def_summary_matches_yaml()` (test) — Validates ToolDefinition.summary matches YAML
  - `index_doc_covers_all_registered_tools()` (test) — Validates docs/developer_guide/41_builtin_tools_index.md lists all tools
  - `index_doc_covers_all_registered_skills()` (test) — Validates docs/developer_guide/42_builtin_skills_index.md lists all skills
- `synthetic_builder_tests` (mod) — Tests schema builder functions
  - `FakeArgs` (struct) — Dummy serde/JsonSchema struct for testing
  - `build_synthetic_tool_with_summary_sets_summary()` (test) — Verifies summary field is populated
  - `build_synthetic_tool_without_summary_is_none()` (test) — Verifies summary is None when not set
- `sanitize_tests` (mod) — Tests JSON schema normalization
  - `strips_dollar_schema()` (test) — Validates `$schema` removal
  - `collapses_optional_string_type()` (test) — Validates `["string", "null"]` → `"string"`
  - `collapses_optional_number_type()` (test) — Validates `["null", "number"]` → `"number"`
  - `preserves_multi_non_null_union()` (test) — Validates `["string", "number"]` preserved
  - `replaces_boolean_schema_in_properties()` (test) — Validates boolean schema in properties becomes `{}`
  - `replaces_boolean_schema_in_anyof()` (test) — Validates boolean schema in anyOf becomes `{}`
  - `inlines_dollar_ref_from_definitions()` (test) — Validates `$ref` + `definitions` inlining
  - `inlines_dollar_ref_from_dollar_defs()` (test) — Validates `$ref` + `$defs` inlining (schemars 0.8 compat)

## File-level notes

- **Massive re-export wall (lines 261–458):** This is intentional as the module's primary purpose is to serve as the public facade for all synthetic tools. No simplification without breaking downstream imports.
- **`all_synthetic_tools()` maintenance hazard (lines 498–559):** New tools must be manually added to this list to pass CI text-coverage tests. This is acceptable; the tests will catch omissions.
- **Docstring stale reference (line 62):** Comment references test name `every_synthetic_tool_has_summary` which does not exist; the actual summary-validation test is `tool_def_summary_matches_yaml` (line 600).
- **Defensive error handling:** Line 49 uses `.expect()` on schemars JSON serialization. Acceptable because schemars producing invalid JSON would be a library bug; error message is clear.
- **Ref resolution depth limit:** Line 135 uses 32-level depth cap to prevent pathological recursion. Safe for real schemars output (actual graphs are shallow).
- **Comprehensive schema normalization:** The `sanitize_schema_for_llm_providers` function handles 5 major provider quirks (Gemini proto schema, OpenAI strict mode, schemars output shapes). Well-tested with 8 parametric tests.
