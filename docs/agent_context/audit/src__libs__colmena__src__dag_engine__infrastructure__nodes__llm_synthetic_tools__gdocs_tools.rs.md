# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs

**Layer:** infrastructure | **Purpose:** Provides 36 Google Docs synthetic LLM tools (Subsystem G) with tool factories, argument parsers, and async dispatchers that delegate to `gdocs::application` use cases for content-addressed surgical edits.

## Symbols

### Tool Name Constants (35 pub const &str)
- `TOOL_CREATE`, `TOOL_CREATE_FROM_MARKDOWN`, `TOOL_CREATE_FROM_DOCX`, `TOOL_SHARE`, `TOOL_EXPORT` — Lifecycle tool names (doc creation, sharing, export)
- `TOOL_LIST_TABS`, `TOOL_ADD_TAB`, `TOOL_READ_AS_MARKDOWN`, `TOOL_READ_OUTLINE` — Tab and read operations
- `TOOL_LIST_NAMED_RANGES`, `TOOL_REPLACE_TEXT`, `TOOL_INSERT_AFTER_TEXT`, `TOOL_INSERT_BEFORE_TEXT`, `TOOL_INSERT_BETWEEN` — Text editing operations
- `TOOL_INSERT_IMAGE_AFTER_TEXT`, `TOOL_DELETE_TEXT`, `TOOL_REPLACE_SECTION`, `TOOL_APPEND_MARKDOWN` — More text operations with markdown/image support
- `TOOL_APPLY_EDITS`, `TOOL_STYLE_TEXT`, `TOOL_CREATE_NAMED_RANGE`, `TOOL_REPLACE_NAMED_RANGE`, `TOOL_ACKNOWLEDGE_HUMAN_CHANGES` — Advanced edits and co-edit guard
- `TOOL_LIST_DOCUMENTS`, `TOOL_LIST_PERMISSIONS`, `TOOL_UNSHARE` — Bundle 2A/2B: Drive discovery and permission management
- `TOOL_ADD_COMMENT`, `TOOL_LIST_COMMENTS`, `TOOL_RESOLVE_COMMENT` — Bundle 4A: Drive comments messaging
- `TOOL_READ_TABLES`, `TOOL_SET_TABLE_CELL`, `TOOL_INSERT_TABLE_ROW`, `TOOL_DELETE_TABLE_ROW`, `TOOL_INSERT_TABLE_COLUMN`, `TOOL_DELETE_TABLE_COLUMN`, `TOOL_FORMAT_TABLE` — Subsystem G v1.1: surgical table edits

### Process-Wide Singletons (3 static OnceCell)
- `CLIENT` (static OnceCell<Arc<GoogleDocsHttpClient>>) — Lazy-initialized shared HTTP client for Drive API
- `CACHE` (static OnceCell<Arc<OutlineCache>>) — Lazy-initialized shared outline cache for document structure
- `REVS` (static OnceCell<Arc<dyn RevisionStore>>) — Lazy-initialized shared Postgres revision store for co-edit tracking

### Initialization Functions (async)
- `shared_client()` → Result<Arc<GoogleDocsHttpClient>, JSON> — Initializes GoogleDocsHttpClient from env config once, reuses thereafter
- `shared_cache()` → Arc<OutlineCache> — Initializes OutlineCache with revision TTL from config once
- `shared_revs()` → Result<Arc<dyn RevisionStore>, JSON> — Initializes PostgresRevisionStore from DATABASE_URL once, fails if env missing

### Error Mapping
- `error_to_json()` (pub fn) — Maps all 22 `DocsError` variants to stable JSON envelopes with LLM-actionable `error`, `message`, `hint`, `valid_next_moves` fields for pattern-matching; handles permission_denied in full and degraded mode

### Helper Parsers (private sync fns)
- `parse_scope()` — Deserializes optional Scope from JSON, defaults to empty Scope
- `parse_style_patch()` — Deserializes StylePatch from JSON, defaults to empty
- `parse_share_role()` — Validates string (`"reader"|"commenter"|"writer"`) to ShareRole enum
- `parse_export_format()` — Validates string (8 formats: docx|pdf|markdown|txt|rtf|epub|odt|html) to ExportFormat enum
- `named_range_meta_to_json()` — Converts NamedRangeMeta struct to JSON object with named_range_id, name, paragraph_start/end
- `invalid_args()` — Creates `{"error": "invalid_args", "message": <e>}` JSON
- `edit_result_to_json()` — Encodes EditResult into `{"ok": true, "changes": ..., "revision_id_after": ..., "outline_snapshot": ..., "lossy_conversions": ..., "pending_human_changes_outside_scope": ...}`
- `insert_image_source()` (pub fn) — Validates XOR of `image_url`/`attachment_id`, returns ImageSource::Url or ImageSource::Attachment; fails if both or neither present
- `default_true()` — Returns true, used as serde default for boolean fields

### Argument Structs (32 pub struct, all Deserialize + JsonSchema)
- `CreateArgs`, `CreateFromMarkdownArgs`, `CreateFromDocxArgs` — Doc creation with optional parent folder
- `ShareArgs`, `ExportArgs`, `ListTabsArgs`, `AddTabArgs` — Sharing and tab operations
- `ReadAsMarkdownArgs`, `ReadOutlineArgs`, `ListNamedRangesArgs` — Read operations with optional tab_id
- `ReplaceTextArgs` — Rich text replacement with scope, case/whole_word, dry_run, confirm_many, occurrence, anchor
- `InsertAfterTextArgs`, `InsertBeforeTextArgs`, `InsertImageAfterTextArgs`, `InsertBetweenArgs` — Text/image insertion with anchor/heading matching
- `DeleteTextArgs`, `ReplaceSectionArgs`, `AppendMarkdownArgs`, `StyleTextArgs` — Text deletion, section replacement, appending, styling
- `ApplyEditsArgs` — Batch edits: array of {tool, find, replace, scope, anchor} structs
- `CreateNamedRangeArgs`, `ReplaceNamedRangeArgs`, `AcknowledgeHumanChangesArgs` — Named ranges and co-edit tracking
- `ListDocumentsArgs` — Drive discovery: query, parent_folder_id, modified_after, limit, page_token (all optional)
- `ListPermissionsArgs`, `UnshareArgs` — Permission management
- `AddCommentArgs`, `ListCommentsArgs`, `ResolveCommentArgs` — Drive comments with pagination support
- `ReadTablesArgs`, `SetTableCellArgs`, `FormatTableArgs`, `InsertTableRowArgs`, `DeleteTableRowArgs`, `InsertTableColumnArgs`, `DeleteTableColumnArgs` — Table operations with table_index, row, col, tab_id

### Enums
- `ImageSource` (pub enum) — Url(String) | Attachment(String); represents the two sources for image insertion

### Tool Factories (32 pub fn each → ToolDefinition)
- `tool_create()`, `tool_create_from_markdown()`, `tool_create_from_docx()` — Lifecycle tools
- `tool_share()`, `tool_export()` — Sharing and export
- `tool_list_tabs()`, `tool_add_tab()`, `tool_read_as_markdown()`, `tool_read_outline()` — Tab and markdown operations
- `tool_list_named_ranges()` — Named range discovery
- `tool_replace_text()`, `tool_insert_after_text()`, `tool_insert_image_after_text()`, `tool_insert_before_text()`, `tool_insert_between()` — Text/image insertion
- `tool_delete_text()`, `tool_replace_section()`, `tool_append_markdown()` — Text removal and appending
- `tool_apply_edits()`, `tool_style_text()` — Batch and style operations
- `tool_create_named_range()`, `tool_replace_named_range()`, `tool_acknowledge_human_changes()` — Named ranges and co-edit tracking
- `tool_list_documents()`, `tool_list_permissions()`, `tool_unshare()` — Drive operations (Bundle 2A/2B)
- `tool_add_comment()`, `tool_list_comments()`, `tool_resolve_comment()` — Comments (Bundle 4A)
- `tool_read_tables()`, `tool_set_table_cell()`, `tool_format_table()`, `tool_insert_table_row()`, `tool_delete_table_row()`, `tool_insert_table_column()`, `tool_delete_table_column()` — Table tools (G v1.1)

All factories delegate to `build_synthetic_tool_with_summary::<ArgsType>(TOOL_*, text::tool_description(TOOL_*), text::tool_summary(TOOL_*))`

### Dispatcher Functions (async fn(args: serde_json::Value, session_id: &str) → serde_json::Value)
- `dispatch_create()`, `dispatch_create_from_markdown()` — Create docs, return {ok, doc_id, url, title, revision_id, tabs}
- `dispatch_create_from_docx()` — **Legacy stub** returning not_yet_wired error; directs to via_executor variant
- `dispatch_create_from_docx_via_executor()` (with executor param) — Fetches .docx attachment bytes, creates native Google Doc, returns doc metadata
- `dispatch_share()`, `dispatch_export()` — Share docs (returns {ok}), export formats (returns {ok, format, byte_len, note})
- `dispatch_export_via_executor()` (with executor param) — Exports bytes, registers as attachment, returns attachment_id + metadata
- `dispatch_list_permissions()`, `dispatch_unshare()` — List/revoke permissions
- `dispatch_add_comment()`, `dispatch_list_comments()`, `dispatch_resolve_comment()` — Drive comments
- `dispatch_list_documents()` — List Drive docs with pagination
- `dispatch_list_tabs()`, `dispatch_add_tab()` — Tab operations; add_tab includes optional markdown seeding
- `dispatch_read_as_markdown()`, `dispatch_read_outline()`, `dispatch_list_named_ranges()` — Read operations
- `dispatch_replace_text()`, `dispatch_insert_after_text()`, `dispatch_insert_before_text()`, `dispatch_insert_between()` — Text operations with co-edit guard context
- `dispatch_insert_image_after_text_via_executor()` (with executor param) — Branches on image_url (direct) vs attachment_id (fetch → Drive upload → insert); returns {ok} + soft_warnings on cleanup failure
- `dispatch_delete_text()`, `dispatch_replace_section()`, `dispatch_append_markdown()` — More text operations with co-edit guard
- `dispatch_style_text()`, `dispatch_apply_edits()` — Styling and batch operations
- `dispatch_create_named_range()`, `dispatch_replace_named_range()` — Named range management
- `dispatch_acknowledge_human_changes()` — Acknowledges pending human changes, updates revision store
- `dispatch_read_tables()`, `dispatch_set_table_cell()`, `dispatch_format_table()`, `dispatch_insert_table_row()`, `dispatch_delete_table_row()`, `dispatch_insert_table_column()`, `dispatch_delete_table_column()` — Table operations via `table::run_*` helpers

Common pattern: parse args → get shared_client/cache/revs → build GuardContext → call application layer → return JSON; errors return error_to_json()

### Bulk Tool Assembly
- `build_all_gdocs_tools()` (pub fn) → Vec<ToolDefinition> — Collects all 36 tool factory outputs; mirrors `build_all_*` in neighboring modules; used by `every_registered_tool_has_text_entry` test

### Tests (cfg(test) module, 10 tests)
- `tool_constants_unique()` — Asserts v1 22-tool name constants are unique (legacy assertion; newer tools not included)
- `build_all_returns_36_tools()` — Asserts build_all_gdocs_tools() returns 36 items (22 v1 + 1 Bundle 2A + 2 Bundle 2B + 3 Bundle 4A + 1 image + 6 table edits + 1 table format)
- `error_to_json_includes_kind_for_each_variant()` — Spot-checks error_to_json outputs for DocumentNotFound, TabExists, NoParentFolder
- `parse_share_role_maps_strings()` — Tests parse_share_role("reader"|"writer") and invalid input
- `add_comment_args_deserialize_with_optional_anchor()` — Tests AddCommentArgs with/without anchor field
- `list_comments_args_default_include_resolved_is_false()` — Tests ListCommentsArgs defaults include_resolved to false
- `resolve_comment_args_deserialize_with_optional_content()` — Tests ResolveCommentArgs with/without content field
- `parse_export_format_maps_strings()` — Tests parse_export_format("docx"|"pdf") and invalid input
- `permission_denied_payload_emits_share_email_and_directive_hint()` — Tests error_to_json(PermissionDenied("agents@startti.co")) includes share_email and Spanish hint with Editor keyword
- `insert_image_args_require_exactly_one_source()` — Tests insert_image_source XOR validation: both/neither/url-only/attachment-only
- `permission_denied_payload_in_degraded_mode_directs_to_operator()` — Tests error_to_json(PermissionDenied("")) with null share_email and hint pointing to operator

## File-level notes

- **Stub dispatchers**: `dispatch_create_from_docx()` (line 1113) and `dispatch_export()` (line 1359) are marked as legacy entry points that return errors directing callers to `*_via_executor` variants. These are intentional stubs for backward compatibility but could be confusing to future maintainers — consider documenting the migration path more explicitly in code comments or archiving them.

- **Misaligned doc comment**: Lines 2621–2623 contain the doc comment "Degraded mode mirror — empty share_email surfaces an actionable hint pointing to the operator" but this comment is incorrectly placed as the doc comment for `insert_image_args_require_exactly_one_source()` (line 2624). The comment actually belongs to the `permission_denied_payload_in_degraded_mode_directs_to_operator()` test below (line 2657). The test at line 2624 tests XOR validation, not degraded mode. This should be moved or corrected.

- **Test assertion drift**: `tool_constants_unique()` (line 2477) only checks the original 22 v1 tool constants but ignores the 14 newer tools (Bundles 2A/2B/4A + image + tables). While this may be intentional (only validating v1 for backward compat), it leaves newer tools unchecked for duplicates. The comment and assertion could clarify scope.

- **High dispatcher duplication**: Nearly all text-edit dispatchers (lines 1619–2471) follow the same pattern: parse args → fetch shared resources → build GuardContext → call application fn → return JSON. This repetitive boilerplate could benefit from a macro or helper to reduce line count, though the current explicit approach is more readable for debugging.

- **Session ID handling**: Most dispatchers accept `session_id: &str` but some read-only operations (e.g., `dispatch_export`, `dispatch_read_outline`) ignore it (marked `_session_id`). This is correct but could be documented in a module-level comment explaining when session_id is used (co-edit guard context vs read-only).

- **Process-wide singletons**: The three `OnceCell` statics (CLIENT, CACHE, REVS) are shared across all concurrent tool invocations in a single process. This is intentional for performance but means state is implicitly global. Thread-safety is guaranteed by OnceCell + Arc, but the implicit sharing should be documented to prevent misuse in tests or forked processes.
