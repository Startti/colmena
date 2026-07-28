# src/libs/colmena/src/documents/application/apply_word_ops.rs

**Layer:** application  **Purpose:** Applies patch operations (`PatchOp`) to Word document IRs (`WordIR`), handling insert/delete/replace/move operations on blocks, runs, list items, and table rows.

## Symbols

- `WordOpApplier<'a>` (struct, pub) — Generic applier holding a reference to an `IdGenerator` for assigning new IDs to document elements.
- `ids` (field, pub) — Reference to the ID generator used by the applier.
- `WordOpApplier::apply` (fn, pub) — Applies a single patch operation to the Word IR; dispatches by `PatchOp` variant and returns `OpOutcome` with assigned IDs or a `DocumentError`.
- `PatchOp::InsertBlock` handler — Inserts a new block at a position determined by `before`/`after`/append; deserializes JSON block and assigns all nested IDs.
- `PatchOp::DeleteBlock` handler — Removes a block by ID.
- `PatchOp::ReplaceBlock` handler — Replaces a block's content while preserving its ID; deserializes new block and overwrites.
- `PatchOp::MoveBlock` handler — Moves a block to a new position after a target block.
- `PatchOp::SetHeadingLevel` handler — Updates heading level; errors if block is not a `Heading`.
- `PatchOp::ReplaceRunText` handler — Updates run text content within a block's runs.
- `PatchOp::SetRunStyle` handler — Applies style patch (bold, italic, underline, size, color) to a run; reads JSON object fields with optional values.
- `PatchOp::InsertRun` handler — Inserts a new run at index in a Paragraph or Heading; deserializes, assigns ID, tracks in assigned IDs.
- `PatchOp::DeleteRun` handler — Removes a run by ID from a block's runs.
- `PatchOp::InsertListItem` handler — Adds a list item with runs to a List block at index; assigns IDs to item and all runs.
- `PatchOp::ReplaceListItem` handler — Replaces a list item's runs by ID; rebuilds run vector with new IDs.
- `PatchOp::DeleteListItem` handler — Removes a list item by ID.
- `PatchOp::InsertTableRow` handler — Inserts a table row with cells at position (before/after/append); assigns IDs to all runs in cells.
- `PatchOp::DeleteTableRow` handler — Removes a table row by ID.
- `PatchOp::UpdateTableCell` handler — Updates cell content (runs) at row/column; assigns IDs to new runs.
- Unsupported ops catch-all (lines 299–336) — Returns `InvalidPatchOp` error for Excel, HTML, Slide, and other non-Word operations with explicit reason.
- `invalid` (fn, private) — Creates a `DocumentError::InvalidPatchOp` from a patch op and reason string; serializes op to JSON for error context.
- `assign_block_ids` (fn, private) — Recursively assigns new IDs to all structural elements in a block (heading/paragraph runs, list items and runs, table rows/cells/runs); populates `AssignedIds` output.
- `set_block_id` (fn, private) — Sets a block's ID field directly; handles all block variants.
- `find_run_mut<'a>` (fn, private) — Searches for a run by ID within a block's runs; returns `None` for List and Table blocks (only Heading and Paragraph have direct runs).
- `tests` (mod, cfg(test)) — Test module.
- `base_ir` (fn, private within tests) — Constructs a minimal `WordIR` with one paragraph and one run for test setup.
- `replace_run_text_updates` (test) — Verifies `ReplaceRunText` modifies run text correctly.
- `insert_block_assigns_server_id` (test) — Verifies `InsertBlock` assigns new IDs to block and runs.
- `insert_list_item_reports_ids` (test) — Verifies `InsertListItem` assigns and tracks list item and run IDs.

## File-level notes

- **Pattern duplication (improvement)**: Run-building pattern repeats in three places (lines 157–164, 191–198, 288–295): deserialize, assign ID, track in `assigned.runs`. Could extract to a helper function like `build_runs(runs: &[serde_json::Value], ids: &dyn IdGenerator, assigned: &mut AssignedIds) -> Result<Vec<Run>>`.
- **Position-finding duplication (improvement)**: "Find position before/after/append" logic duplicated (block insertion lines 22–32 vs. table row insertion lines 241–253). Both use identical if-let chains. Could extract to `find_insert_position()` helper.
- **Repetitive null-safety checks (improvement)**: Multiple error paths check block/list/table existence with `ir.block_mut()` + `ok_or_else()`. Seven+ instances inflate the match arm. Not incorrect, but could benefit from a custom `expect_block_type()` helper to reduce verbosity.
- **Asymmetric run search (observation)**: `find_run_mut()` only searches Heading and Paragraph blocks (returns `None` for List and Table), which is correct since those blocks don't have direct runs. However, the pattern is not uniform — other operations (InsertRun, DeleteRun) explicitly pattern-match to verify block type before accessing runs. This asymmetry is intentional and safe.
- **Error JSON serialization (observation)**: `invalid()` function calls `serde_json::to_value(op).unwrap_or(...)`, which will succeed for any `PatchOp` since it derives `Serialize`. Safe fallback to `Null` if ever broken.
- **Tests coverage**: Unit tests cover three key paths: text replacement (no new IDs), block insertion (ID assignment), and list insertion (ID tracking). Happy-path focused; error paths not directly tested.
