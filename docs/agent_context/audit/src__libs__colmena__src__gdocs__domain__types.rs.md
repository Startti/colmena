# src/libs/colmena/src/gdocs/domain/types.rs

**Layer:** domain  **Purpose:** Pure value types and domain models for Google Docs subsystem. All types serialize/deserialize for tool-result transport; zero infrastructure dependencies.

## Symbols

- `DocumentId` (struct, pub) — Wrapper for stable Google Doc identifier (document/d/<id>)
- `DocumentId::fmt` (impl Display, pub) — Display trait writes wrapped string
- `TabId` (struct, pub) — Wrapper for stable tab identifier within multi-tab Docs
- `RevisionId` (struct, pub) — Wrapper for opaque Google revision id used as optimistic concurrency token
- `PermissionEntry` (struct, pub) — Single Drive permission entry: permission_id, type, role, optional email/display_name
- `PermissionList` (struct, pub) — Result of list_permissions: vector of entries with no pagination today
- `DocumentListItem` (struct, pub) — Single entry in list_documents response: doc_id, name, url, modified_time, owners
- `DocumentListResult` (struct, pub) — Result of list_documents with optional pagination cursor
- `DocumentListFilter<'a>` (struct, pub) — Filter args for list_documents: query, parent_folder_id, modified_after, limit, page_token
- `Scope` (enum, pub) — Where to search in find operations: All, Tab, Paragraph, UnderHeading, BetweenHeadings
- `RgbColor` (struct, pub) — 0..=1 normalized RGB color (r, g, b floats)
- `HeadingLevel` (enum, pub) — Cascading heading levels: Normal, H1–H6, Title, Subtitle mapping to namedStyleType
- `HeadingLevel::as_api_str` (impl, pub) — Maps HeadingLevel to wire-format string (HEADING_1, NORMAL_TEXT, etc.)
- `StylePatch` (struct, pub) — Optional style attributes supplied by agent: bold, italic, underline, strikethrough, font_size_pt, colors, link, heading_level
- `ChangeKind` (enum, pub) — Edit type: Replace, Insert, Delete, Style
- `ChangeRecord` (struct, pub) — Observable edit applied to paragraph: kind, paragraph index, before/after text, optional tab_id
- `ParagraphKind` (enum, pub) — One of 11 paragraph shapes Docs supports: Heading1–6, Title, Subtitle, Paragraph, ListItem, TableRow
- `OutlineEntry` (struct, pub) — Single line in document outline returned after edits: paragraph index, tab_id, kind, text_preview
- `HumanChangeKind` (enum, pub) — Edit type by human collaborator: Insert, Modify, Delete
- `HumanChange` (struct, pub) — Human-authored change outside agent's scope: kind, paragraph, preview, modified_time, modifying_user, optional tab_id, before_text, after_text
- `LossyConversion` (struct, pub) — Markdown element that failed lossless conversion to Docs: element_type, original_markdown
- `MatchPreview` (struct, pub) — Single hit in find/replace preview: match number, paragraph index, preview text
- `EditResult` (struct, pub) — Complete result of successful edit: changes, revision_id_after, outline_snapshot, optional lossy_conversions, optional pending_human_changes_outside_scope
- `DocumentMeta` (struct, pub) — Top-level doc metadata: doc_id, url, title, revision_id, tabs vector
- `TabMeta` (struct, pub) — Metadata for single tab: tab_id, title, index, optional parent_tab_id
- `NamedRangeMeta` (struct, pub) — Metadata for named range: named_range_id, name, paragraph_start, paragraph_end
- `RevisionMeta` (struct, pub) — Metadata from Drive's revisions.list: revision_id, modified_time, optional modifying_user_email
- `CreateFromMarkdownResult` (struct, pub) — Result of create_from_markdown: meta, outline_snapshot, lossy_conversions
- `ShareRole` (enum, pub) — Drive sharing role: Reader, Commenter, Writer
- `ShareRole::as_api_str` (impl, pub) — Maps ShareRole to wire-format string (reader, commenter, writer)
- `ExportFormat` (enum, pub) — Export formats: Docx, Pdf, Markdown, Txt, Rtf, Epub, Odt, Html
- `ExportFormat::mime` (impl, pub) — Maps ExportFormat to MIME string; Html exports as zipped bundle with application/zip
- `DocumentSnapshot` (struct, pub) — In-memory doc snapshot for scope resolution and diffing: doc_id, revision_id, title, tabs
- `TabSnapshot` (struct, pub) — One tab's paragraphs within DocumentSnapshot: tab_id (None for single-tab), paragraphs, tables (additive field for backward compat)
- `ParagraphSnapshot` (struct, pub) — One paragraph within TabSnapshot: n (index), kind, text, start_index, end_index (Docs API offsets)
- `TableSnapshot` (struct, pub) — Table parsed from document body: table_index, tab_id, start_index, rows, columns, row-major cell grid
- `CellSnapshot` (struct, pub) — Single table cell: row, col, plain text, content_start_index, content_end_index, row_span, col_span
- `BatchUpdateResult` (struct, pub) — Raw result from documents.batchUpdate: revision_id_after, replies vector (kept verbatim for reply-specific fields)
- `tests::scope_roundtrips` (fn, private) — Verifies Scope enum JSON serialization round-trips all variants
- `tests::heading_level_api_strings` (fn, private) — Verifies HeadingLevel::as_api_str maps correctly (H1→HEADING_1, Normal→NORMAL_TEXT, etc.)
- `tests::export_format_mime_types` (fn, private) — Verifies ExportFormat::mime returns correct MIME strings
- `tests::human_change_serializes_new_fields` (fn, private) — Verifies HumanChange serializes tab_id, before_text, after_text when present
- `tests::human_change_omits_new_fields_when_none` (fn, private) — Verifies HumanChange omits optional fields when None via skip_serializing_if
- `tests::edit_result_omits_empty_optional_fields` (fn, private) — Verifies EditResult omits lossy_conversions and pending_human_changes_outside_scope when empty
- `tests::table_snapshot_round_trips_and_tab_default` (fn, private) — Verifies TabSnapshot::tables defaults to empty vec on deserialization; verifies TableSnapshot and CellSnapshot round-trip

## File-level notes

- Clean, focused domain types file with no infrastructure coupling (only serde + chrono for standard serialization and timestamps)
- Comprehensive doc comments on all public types explaining Bundle origins (2A, 4A, v1.1), API correspondence, and field semantics
- All test coverage is thorough: serde round-trips, API string mapping, optional field serialization, backward compatibility defaults
- No breaking changes or tech debt visible
