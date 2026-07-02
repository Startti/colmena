# Built-in tools index

Every Rust-native LLM tool colmena ships with. Each row links to a detailed
doc. The "Summary" column comes from `text/tools/*.yaml`; a CI test
(`index_doc_covers_all_registered_tools`) guarantees this index lists every
registered synthetic tool.

For the YAML registry that backs this index, see
[`src/libs/colmena/text/tools/`](../../src/libs/colmena/text/tools/).

For toolkit packages (`enabled_tools: ["gsheets"]` shortcut), see
[40_toolkit_packages.md](40_toolkit_packages.md).

## gsheets (15 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `gsheets_create_spreadsheet` | Create a new Google Sheets workbook and return its URL | [§39](39_gsheets.md) |
| `gsheets_create_from_xlsx` | Upload a local .xlsx attachment and convert it into a new Google Sheet | [§39](39_gsheets.md) |
| `gsheets_export_xlsx` | Download an existing Google Sheet as .xlsx bytes attachment | [§39](39_gsheets.md) |
| `gsheets_list_sheets` | List every tab (sheet) inside a spreadsheet by ID | [§39](39_gsheets.md) |
| `gsheets_list_spreadsheets` | Discover Google Spreadsheets visible to the OAuth user — optionally filtered by name, folder, or modified-after date | [§39](39_gsheets.md) |
| `gsheets_add_sheet` | Create a new tab inside an existing spreadsheet | [§39](39_gsheets.md) |
| `gsheets_delete_sheet` | Permanently delete a tab from a spreadsheet | [§39](39_gsheets.md) |
| `gsheets_read` | Read a cell range from a tab; supports formatted, unformatted, and formula render modes | [§39](39_gsheets.md) |
| `gsheets_set_cell` | Write one value or formula into a single cell | [§39](39_gsheets.md) |
| `gsheets_set_range` | Write a 2-D values array starting at a given address | [§39](39_gsheets.md) |
| `gsheets_format_range` | Apply cell formatting (text style, background, borders, alignment, number format, column/row size) to one or more ranges in one atomic batchUpdate. | [§39](39_gsheets.md) |
| `gsheets_share` | Grant a Google account access to a spreadsheet (reader / commenter / writer) | [§39](39_gsheets.md) |
| `gsheets_list_permissions` | Show every Drive permission on a spreadsheet (who has access and at what role) | [§39](39_gsheets.md) |
| `gsheets_unshare` | Revoke a previously-granted permission on a spreadsheet | [§39](39_gsheets.md) |
| `gsheets_run_python` | **DEPRECATED (2026-07-02) → use `data_run_python`.** Still works for back-compat. Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM) | [§39](39_gsheets.md) |

## crdt_doc (11 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `crdt_doc_list_sheets` | List every tab (sheet) inside the current CRDT document artifact | [§38](38_crdt_documents.md) |
| `crdt_doc_list_sheets_of` | List tabs inside a specific CRDT artifact by ID | [§38](38_crdt_documents.md) |
| `crdt_doc_read` | Read a cell range from the current CRDT document; supports include_formulas for round-trip | [§38](38_crdt_documents.md) |
| `crdt_doc_set_cell` | Write a value or formula into one cell of the current CRDT document | [§38](38_crdt_documents.md) |
| `crdt_doc_set_range` | Write a 2-D values array into the current CRDT document starting at an address | [§38](38_crdt_documents.md) |
| `crdt_doc_add_sheet` | Create a new tab inside the current CRDT document | [§38](38_crdt_documents.md) |
| `crdt_doc_get_recent_changes` | List recent CRDT change events since the agent's cursor | [§38](38_crdt_documents.md) |
| `crdt_doc_list_my_artifacts` | List CRDT artifacts owned by or shared with the current session | [§38](38_crdt_documents.md) |
| `crdt_doc_create_artifact` | Create a new empty CRDT spreadsheet artifact | [§38](38_crdt_documents.md) |
| `crdt_doc_run_python` | Run sandboxed pandas analysis over CRDT sheets without loading rows through the LLM | [§38](38_crdt_documents.md) |
| `crdt_doc_import_sheet` | Clone a sheet from another CRDT artifact into the current one | [§38](38_crdt_documents.md) |

## documents (7 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `document_create` | Create a new document artifact (Excel or Word); returns artifact_id and initial version | [§38](38_crdt_documents.md) |
| `document_read` | Read the IR of a document at a given version (or current HEAD) with optional slicing | [§38](38_crdt_documents.md) |
| `document_apply_patch` | Apply a patch (list of ops) to an existing document atomically with auto-rebase on non-conflicting changes | [§38](38_crdt_documents.md) |
| `document_get_head` | Get the current HEAD of an artifact, optionally with a human-readable summary of edits since a baseline version | [§38](38_crdt_documents.md) |
| `document_list_versions` | List the versions retained for an artifact with timestamps, source and per-version summary | [§38](38_crdt_documents.md) |
| `document_rollback` | Roll back an artifact to a previous version; full history is preserved | [§38](38_crdt_documents.md) |
| `document_list_my_artifacts` | List every document artifact that belongs to the current session with metadata | [§38](38_crdt_documents.md) |

## helpers (3 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `load_skill` | Load a markdown skill bundle into the conversation; reveals built-in or user-provided guidance on demand | [§24](24_skills.md) |
| `load_attachment` | Materialize a registered attachment's content (with auto-summary for large files) into the conversation | [§31](31_load_attachment.md) |
| `recall_history` | Re-read the FULL original content of one past message by its turn index (the `[Tn]` shown in the conversation summary); paginated (`offset`/`next_offset`) for large artifacts | [§15](15_memory_guide.md) |

## sql (3 tools — synthetic SQL/pandas tools for tabular attachments)

Opt-in via `tool_configurations`. Bypass the LLM context for large CSV/XLSX
attachments — the LLM sees only a sample (catalog auto-summary) or runs
computation server-side (run_python) instead of dumping every row.

| Tool | Summary | Detailed docs |
|---|---|---|
| `attachment_run_python` | **DEPRECATED (2026-07-02) → use `data_run_python`.** Still works for back-compat. Run pandas/numpy code against a registered CSV/XLSX attachment, get back stdout + a structured result — without dumping every row into your context | [§23](23_sql_node.md#attachment_run_python-shipped-2026-06-10) |
| `sql_inspect_attachment` | Open a registered CSV/XLSX attachment and report its header, sample rows, total row count, and inferred column types — without forcing the LLM to read every row | [§23](23_sql_node.md#bulk-insert-from-attachment-shipped-2026-06-09) |
| `sql_bulk_insert_from_attachment` | Stream a CSV attachment into a Postgres table via COPY FROM STDIN — bypasses the LLM context for the rows and runs orders of magnitude faster than per-row INSERT | [§23](23_sql_node.md#bulk-insert-from-attachment-shipped-2026-06-09) |
| `data_run_python` | **Primary tabular tool (2026-07-02).** Run pandas code across multiple data sources (attachments, Google Sheets, SQL, inline) in one call — bind each source where it lives, write back to SQL/Sheets/downloadable files, get back only the result. Replaces `gsheets_run_python` + `attachment_run_python`. | [§48](48_data_run_python.md) |

## gdocs (35 tools)

Content-addressed surgical editing — the LLM specifies WHAT to change
(text, heading, named range) and never computes UTF-16 indices.
Multi-tab. Markdown ↔ Docs conversion with structured `lossy_conversions`.
Co-edit safety via Drive Revisions + postgres `gdocs_session_state`.

Detailed docs: [§45 — Google Docs integration](45_gdocs.md). Design spec:
[`docs/superpowers/specs/2026-06-08-google-docs-design.md`](../superpowers/specs/2026-06-08-google-docs-design.md).

| Tool | Summary | Detailed docs |
|---|---|---|
| `gdocs_create` | Create an empty Google Doc and place it in a shared folder | [§45](45_gdocs.md) |
| `gdocs_create_from_markdown` | Create a new Google Doc from a markdown string (Drive does the conversion) | [§45](45_gdocs.md) |
| `gdocs_create_from_docx` | Upload a .docx attachment and convert it into a new Google Doc (DEFERRED in v1) | [§45](45_gdocs.md) |
| `gdocs_share` | Grant a user reader / commenter / writer access to a Google Doc | [§45](45_gdocs.md) |
| `gdocs_export` | Export a Google Doc as docx / pdf / markdown / txt / rtf / epub / odt / html | [§45](45_gdocs.md) |
| `gdocs_list_tabs` | List every tab inside a multi-tab Google Doc | [§45](45_gdocs.md) |
| `gdocs_add_tab` | Create a new tab inside a Google Doc | [§45](45_gdocs.md) |
| `gdocs_read_as_markdown` | Export a Google Doc (or one tab) as markdown | [§45](45_gdocs.md) |
| `gdocs_read_outline` | Read the heading + paragraph-preview outline of a Google Doc | [§45](45_gdocs.md) |
| `gdocs_list_named_ranges` | List every named range declared in the Google Doc | [§45](45_gdocs.md) |
| `gdocs_replace_text` | Find and replace text (content-addressed, optionally scoped) | [§45](45_gdocs.md) |
| `gdocs_insert_after_text` | Insert markdown after a content-addressed anchor | [§45](45_gdocs.md) |
| `gdocs_insert_before_text` | Insert markdown before a content-addressed anchor | [§45](45_gdocs.md) |
| `gdocs_insert_between` | Insert markdown between two headings (or after a heading until EOF) | [§45](45_gdocs.md) |
| `gdocs_insert_image_after_text` | Insert an inline image (by public URL) after a content-addressed anchor | [§45](45_gdocs.md) |
| `gdocs_delete_text` | Delete one or more occurrences of text (scoped, content-addressed) | [§45](45_gdocs.md) |
| `gdocs_replace_section` | Replace everything between a heading and the next same-or-higher heading | [§45](45_gdocs.md) |
| `gdocs_append_markdown` | Append markdown at the end of a Google Doc (or a specific tab) | [§45](45_gdocs.md) |
| `gdocs_apply_edits` | Apply N sub-edits atomically (single batchUpdate, all-or-nothing) | [§45](45_gdocs.md) |
| `gdocs_style_text` | Apply a style patch (bold / italic / link / heading) to a found span | [§45](45_gdocs.md) |
| `gdocs_create_named_range` | Declare a named range over a paragraph for stable addressing | [§45](45_gdocs.md) |
| `gdocs_replace_named_range` | Overwrite the contents of a named range | [§45](45_gdocs.md) |
| `gdocs_acknowledge_human_changes` | Acknowledge concurrent human edits and reset the co-edit cursor | [§45](45_gdocs.md) |
| `gdocs_list_documents` | Discover Google Docs visible to the OAuth user — optionally filtered by name, folder, or modified-after date | [§45](45_gdocs.md) |
| `gdocs_list_permissions` | Show every Drive permission on a document (who has access and at what role) | [§45](45_gdocs.md) |
| `gdocs_unshare` | Revoke a previously-granted permission on a document | [§45](45_gdocs.md) |
| `gdocs_add_comment` | Post a Drive comment on a document (human ↔ agent messaging) | [§45](45_gdocs.md) |
| `gdocs_list_comments` | List Drive comments on a document | [§45](45_gdocs.md) |
| `gdocs_resolve_comment` | Mark a Drive comment resolved (close the thread) | [§45](45_gdocs.md) |
| `gdocs_read_tables` | List every table in a Google Doc with a per-cell text preview | [§45](45_gdocs.md) |
| `gdocs_set_table_cell` | Replace the plain text of a single table cell | [§45](45_gdocs.md) |
| `gdocs_insert_table_row` | Insert a blank row into a table, above or below an existing row | [§45](45_gdocs.md) |
| `gdocs_delete_table_row` | Delete a row from a table by 0-based index | [§45](45_gdocs.md) |
| `gdocs_insert_table_column` | Insert a column into a table, left or right of an existing column | [§45](45_gdocs.md) |
| `gdocs_delete_table_column` | Delete a column from a table by 0-based index | [§45](45_gdocs.md) |
| `gdocs_format_table` | Apply formatting (background, borders, text style, alignment) to cell ranges in a table | [§45](45_gdocs.md) |

## Toolkit packages

See [40_toolkit_packages.md](40_toolkit_packages.md). Registered packages:

- `gsheets` — expands to all 10 `gsheets_*` tools.
- `gdocs` — expands to all 35 `gdocs_*` tools.
- `gdocsread` — read-only subset of gdocs (10 tools: `list_tabs`,
  `read_as_markdown`, `read_outline`, `list_named_ranges`, `read_tables`,
  `export`, `acknowledge_human_changes`, `list_documents`,
  `list_permissions`, `list_comments`).

## describe_tool

Dynamically constructed per turn by `lazy_tools_catalog::build_describe_tool_definition`
when `lazy_tool_loading: true`. See [§29](29_lazy_tool_loading.md). Not part of
the static `text/tools/*.yaml` registry by design.
