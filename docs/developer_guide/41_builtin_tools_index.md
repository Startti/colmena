# Built-in tools index

Every Rust-native LLM tool colmena ships with. Each row links to a detailed
doc. The "Summary" column comes from `text/tools/*.yaml`; a CI test
(`index_doc_covers_all_registered_tools`) guarantees this index lists every
registered synthetic tool.

For the YAML registry that backs this index, see
[`src/libs/colmena/text/tools/`](../../src/libs/colmena/text/tools/).

For toolkit packages (`enabled_tools: ["gsheets"]` shortcut), see
[40_toolkit_packages.md](40_toolkit_packages.md).

## gsheets (10 tools)

| Tool | Summary | Detailed docs |
|---|---|---|
| `gsheets_create_spreadsheet` | Create a new Google Sheets workbook and return its URL | [§39](39_gsheets.md) |
| `gsheets_create_from_xlsx` | Upload a local .xlsx attachment and convert it into a new Google Sheet | [§39](39_gsheets.md) |
| `gsheets_export_xlsx` | Download an existing Google Sheet as .xlsx bytes attachment | [§39](39_gsheets.md) |
| `gsheets_list_sheets` | List every tab (sheet) inside a spreadsheet by ID | [§39](39_gsheets.md) |
| `gsheets_add_sheet` | Create a new tab inside an existing spreadsheet | [§39](39_gsheets.md) |
| `gsheets_delete_sheet` | Permanently delete a tab from a spreadsheet | [§39](39_gsheets.md) |
| `gsheets_read` | Read a cell range from a tab; supports formatted, unformatted, and formula render modes | [§39](39_gsheets.md) |
| `gsheets_set_cell` | Write one value or formula into a single cell | [§39](39_gsheets.md) |
| `gsheets_set_range` | Write a 2-D values array starting at a given address | [§39](39_gsheets.md) |
| `gsheets_run_python` | Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM) | [§39](39_gsheets.md) |

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
| `recall_history` | Re-read the original content of one past message by its turn index | [§29](29_lazy_tool_loading.md) |

## gdocs (22 tools)

Detailed docs land in T23 (see [`docs/superpowers/specs/2026-06-08-google-docs-design.md`](../superpowers/specs/2026-06-08-google-docs-design.md) for the design spec).

| Tool | Summary | Detailed docs |
|---|---|---|
| `gdocs_create` | Create an empty Google Doc and place it in a shared folder | (T23) |
| `gdocs_create_from_markdown` | Create a new Google Doc from a markdown string (Drive does the conversion) | (T23) |
| `gdocs_create_from_docx` | Upload a .docx attachment and convert it into a new Google Doc | (T23) |
| `gdocs_share` | Grant a user reader / commenter / writer access to a Google Doc | (T23) |
| `gdocs_export` | Export a Google Doc as docx / pdf / markdown / txt / rtf / epub / odt / html | (T23) |
| `gdocs_list_tabs` | List every tab inside a multi-tab Google Doc | (T23) |
| `gdocs_add_tab` | Create a new tab inside a Google Doc | (T23) |
| `gdocs_read_as_markdown` | Export a Google Doc (or one tab) as markdown | (T23) |
| `gdocs_read_outline` | Read the heading + paragraph-preview outline of a Google Doc | (T23) |
| `gdocs_list_named_ranges` | List every named range declared in the Google Doc | (T23) |
| `gdocs_replace_text` | Find and replace text (content-addressed, optionally scoped) | (T23) |
| `gdocs_insert_after_text` | Insert markdown after a content-addressed anchor | (T23) |
| `gdocs_insert_before_text` | Insert markdown before a content-addressed anchor | (T23) |
| `gdocs_insert_between` | Insert markdown between two headings (or after a heading until EOF) | (T23) |
| `gdocs_delete_text` | Delete one or more occurrences of text (scoped, content-addressed) | (T23) |
| `gdocs_replace_section` | Replace everything between a heading and the next same-or-higher heading | (T23) |
| `gdocs_append_markdown` | Append markdown at the end of a Google Doc (or a specific tab) | (T23) |
| `gdocs_apply_edits` | Apply N sub-edits atomically (single batchUpdate, all-or-nothing) | (T23) |
| `gdocs_style_text` | Apply a style patch (bold / italic / link / heading) to a found span | (T23) |
| `gdocs_create_named_range` | Declare a named range over a paragraph for stable addressing | (T23) |
| `gdocs_replace_named_range` | Overwrite the contents of a named range | (T23) |
| `gdocs_acknowledge_human_changes` | Acknowledge concurrent human edits and reset the co-edit cursor | (T23) |

## Toolkit packages

See [40_toolkit_packages.md](40_toolkit_packages.md). Today the only registered
package is `gsheets`, which expands to all 10 gsheets_* tools listed above.

## describe_tool

Dynamically constructed per turn by `lazy_tools_catalog::build_describe_tool_definition`
when `lazy_tool_loading: true`. See [§29](29_lazy_tool_loading.md). Not part of
the static `text/tools/*.yaml` registry by design.
