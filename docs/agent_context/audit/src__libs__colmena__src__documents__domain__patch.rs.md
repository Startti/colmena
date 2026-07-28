# src/libs/colmena/src/documents/domain/patch.rs

**Layer:** domain  **Purpose:** Defines patch operations domain model for document editing (spreadsheets, word docs, presentations). Supports cell/range operations, sheet/table management, block/run text manipulation, and slide management via a tagged-union enum.

## Symbols

- `Patch` (struct, pub) — Value object representing a versioned set of atomic operations on a document artifact.
- `artifact_id` (field, pub) — Artifact ID this patch targets (e.g., "art_abc123").
- `base_version` (field, pub) — Version the caller based this patch on; server rebases automatically if newer.
- `source` (field, pub) — Discriminator field indicating patch author (Agent or User).
- `ops` (field, pub) — Ordered list of PatchOp operations applied atomically.
- `default_source()` (fn, private) — Returns `PatchSource::Agent` as the default serialization value.
- `PatchSource` (enum, pub) — Two-variant enum: Agent, User. Indicates patch author for narration eligibility.
- `PatchOp` (enum, pub) — Tagged-union enum with 50+ variants covering all document patch operations (serde tag: "op").
- `SetCell` (PatchOp variant) — Set single cell value with optional type/format/style overrides.
- `SetRange` (PatchOp variant) — Bulk write a rectangular region (row-major 2D array).
- `ClearRange` (PatchOp variant) — Clear all cells in a range without deleting rows/columns.
- `InsertRow` (PatchOp variant) — Insert a row with optional initial values.
- `DeleteRow` (PatchOp variant) — Delete a row.
- `InsertColumn` (PatchOp variant) — Insert a column with optional initial values.
- `DeleteColumn` (PatchOp variant) — Delete a column.
- `AddSheet` (PatchOp variant) — Create a new sheet with optional position.
- `RenameSheet` (PatchOp variant) — Rename an existing sheet.
- `DeleteSheet` (PatchOp variant) — Delete a sheet and all its cells/tables.
- `ReorderSheets` (PatchOp variant) — Reorder sheets by full list of IDs.
- `CreateTable` (PatchOp variant) — Define a named table over a range with optional style preset.
- `ResizeTable` (PatchOp variant) — Change a table's extent.
- `DeleteTable` (PatchOp variant) — Delete a named table (cells persist).
- `SetColumnWidth` (PatchOp variant) — Set column width in points.
- `DefineStyle` (PatchOp variant) — Create or update a named style referenced by cells.
- `InsertBlock` (PatchOp variant) — Insert a block (word doc) with optional before/after positioning.
- `DeleteBlock` (PatchOp variant) — Delete a block by ID.
- `ReplaceBlock` (PatchOp variant) — Replace a block's entire content preserving ID.
- `MoveBlock` (PatchOp variant) — Move a block to appear after another.
- `SetHeadingLevel` (PatchOp variant) — Change heading level (1-6).
- `ReplaceRunText` (PatchOp variant) — Replace text of a specific run.
- `SetRunStyle` (PatchOp variant) — Update style properties of a run (partial patch).
- `InsertRun` (PatchOp variant) — Insert a run into a paragraph/heading.
- `DeleteRun` (PatchOp variant) — Delete a run.
- `InsertListItem` (PatchOp variant) — Insert an item into a list block.
- `ReplaceListItem` (PatchOp variant) — Replace all runs of a list item.
- `DeleteListItem` (PatchOp variant) — Delete a list item.
- `InsertTableRow` (PatchOp variant) — Insert a row in a table block with optional before/after positioning.
- `DeleteTableRow` (PatchOp variant) — Delete a table row.
- `UpdateTableCell` (PatchOp variant) — Replace a table cell's runs.
- `AddSlide` (PatchOp variant) — Create a new slide with optional position and title/subtitle.
- `DeleteSlide` (PatchOp variant) — Delete a slide.
- `ReorderSlides` (PatchOp variant) — Reorder slides by full list of IDs.
- `SetSlideLayout` (PatchOp variant) — Change a slide's layout.
- `SetSlideTitle` (PatchOp variant) — Set/clear a slide's title and optional subtitle.
- `SetSlideNotes` (PatchOp variant) — Set/clear speaker notes on a slide.
- `InsertHtmlBlock` (PatchOp variant) — Insert a block on a slide with optional before/after positioning.
- `DeleteHtmlBlock` (PatchOp variant) — Delete a block from a slide.
- `ReplaceHtmlBlock` (PatchOp variant) — Replace a block's contents preserving ID.
- `MoveHtmlBlock` (PatchOp variant) — Move a block within a slide to appear after another.
- `InsertHtmlTableRow` (PatchOp variant) — Insert a row in an HTML table block with optional before/after positioning.
- `DeleteHtmlTableRow` (PatchOp variant) — Delete an HTML table row.
- `UpdateHtmlTableCell` (PatchOp variant) — Update a table cell's content.
- `InsertHtmlListItem` (PatchOp variant) — Insert an item into an HTML list block.
- `DeleteHtmlListItem` (PatchOp variant) — Delete an HTML list item.
- `UpdateHtmlListItem` (PatchOp variant) — Update an HTML list item's runs.
- `SetTheme` (PatchOp variant) — Set document theme (enum-based).
- `SetDocProps` (PatchOp variant) — Set document properties (title, author, date, locale).
- `SetFooter` (PatchOp variant) — Set footer configuration.
- `tests` (mod, private) — 14 unit tests covering serialization, schema generation, and roundtrip of all major operation types.

## File-level notes

- **Clean domain model**: Pure value objects with no infrastructure dependencies, I/O, or external effects.
- **Exhaustive variant coverage**: 50+ enum variants systematically cover all three document types (spreadsheets, word docs, presentations) and their operations.
- **Documented runtime constraints**: Comments note that `InsertBlock`, `InsertTableRow`, and `InsertHtmlBlock` require exactly one of before/after to be provided, but this is not validated at the type level (executor must enforce).
- **Comprehensive test suite**: 14 tests verify serialization tags, schema generation, and round-trip fidelity.
- **No unused exports**: All symbols are either part of the public API (Patch, PatchSource, PatchOp) or internal helpers (default_source).
