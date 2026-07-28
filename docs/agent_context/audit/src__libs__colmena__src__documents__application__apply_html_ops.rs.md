# src/libs/colmena/src/documents/application/apply_html_ops.rs

**Layer:** application  
**Purpose:** Dispatches and applies PatchOp operations (slide-level and block-level) to mutate HtmlIR, using IdGenerator for assigning new element IDs.

## Symbols

### Public

- `HtmlOpApplier<'a>` (struct, pub) — Generic-lifetime wrapper holding reference to IdGenerator for mutation operations
- `HtmlOpApplier::ids` (field, pub) — Reference to IdGenerator port for creating new element IDs
- `HtmlOpApplier::apply(&self, ir: &mut HtmlIR, op: &PatchOp)` (fn, pub) — Dispatches a single PatchOp to appropriate handler method; returns OpOutcome with assigned IDs or DocumentError
- `recompute_assets_referenced(ir: &mut HtmlIR)` (fn, pub(crate)) — Rebuilds ir.assets_referenced set by walking all blocks and collecting asset IDs from Image/Video sources

### Private (impl methods)

- `add_slide` (fn, priv) — Creates new Slide with generated ID at specified index; validates Title/SectionDivider layouts require non-empty title
- `delete_slide` (fn, priv) — Removes slide by ID; prevents deletion of last slide; recomputes assets after removal
- `reorder_slides` (fn, priv) — Reorders slides by consuming and reinserting in provided order; validates order length matches slide count
- `set_slide_layout` (fn, priv) — Updates slide layout; validates Title/SectionDivider layouts require non-empty title
- `set_slide_title` (fn, priv) — Updates slide title and subtitle fields
- `set_slide_notes` (fn, priv) — Updates slide notes field
- `insert_html_block` (fn, priv) — Parses block JSON, assigns new ID and nested IDs, inserts before/after target block; recomputes assets
- `delete_html_block` (fn, priv) — Removes block by ID from slide; recomputes assets
- `replace_html_block` (fn, priv) — Replaces block by ID with parsed JSON (retaining block_id); recomputes assets
- `move_html_block` (fn, priv) — Moves block to position after another block; restores if target not found
- `insert_html_table_row` (fn, priv) — Creates TableRow with new ID and parsed cells; inserts before/after row ID in table
- `delete_html_table_row` (fn, priv) — Removes row by ID from table block; validates block type
- `update_html_table_cell` (fn, priv) — Updates cell in table row by column index; validates index in range
- `insert_html_list_item` (fn, priv) — Creates ListItem with new ID and parsed runs; inserts at index with clamping
- `delete_html_list_item` (fn, priv) — Removes list item by ID from list block; validates block type
- `update_html_list_item` (fn, priv) — Updates list item runs (assigning new IDs to runs without id field)
- `assign_nested_ids` (fn, priv) — Recursively assigns IDs to run, list item, and table row arrays in JSON objects; skips if id already present

### Private (module-level helpers)

- `block_id_of(b: &Block) -> &str` (fn, priv) — Extracts id field from any Block variant via exhaustive pattern match
- `find_slide_mut<'b>(ir: &'b mut HtmlIR, id: &str)` (fn, priv) — Finds mutable slide by ID or returns IRValidationFailed error
- `walk_block_assets(blocks: &[Block], out: &mut HashSet<String>)` (fn, priv) — Recursively collects asset_id references from Image/Video blocks and nested TwoColumns/ThreeColumns

### Test module

- `tests::fresh()` (fn, test-local) — Fixture creating minimal HtmlIR with one blank slide
- `tests::add_slide_appends_and_returns_id` (test) — Verifies AddSlide appends slide and returns assigned ID
- `tests::add_title_slide_without_title_rejected` (test) — Verifies Title layout requires non-empty title
- `tests::delete_last_slide_rejected` (test) — Verifies single-slide IR prevents deletion
- `tests::reorder_validates_length_and_ids` (test) — Verifies ReorderSlides validates list length and unknown IDs
- `tests::insert_html_block_assigns_id_and_appends` (test) — Verifies InsertHtmlBlock parses, assigns ID, appends block
- `tests::delete_html_block_removes_by_id` (test) — Verifies DeleteHtmlBlock removes by ID
- `tests::set_theme_changes_ir_theme` (test) — Verifies SetTheme updates theme field
- `tests::insert_image_block_with_asset_updates_assets_referenced` (test) — Verifies asset ID is collected after insertion

## File-level notes

- **Comprehensive error handling**: All operations validate state (slide existence, block type, index bounds) before mutation; errors include path context for debugging
- **Lexical structure clarity**: Helper methods grouped (slide ops, block ops, table ops, list ops); test module at end
- **ID generation completeness**: All operations that create new elements call IdGenerator (new_slide_id, new_block_id, new_row_id, new_list_item_id, new_run_id); no risk of ID collisions
- **Asset tracking correctness**: `recompute_assets_referenced` called after deletions/replacements; `walk_block_assets` recursively handles nested structures (TwoColumns, ThreeColumns)
- **JSON mutation pattern**: Block/row/item JSON parsed after ID injection; runs assigned new IDs only if missing (idempotent on re-apply)
- **Test coverage**: 8 unit tests cover slide CRUD, layout validation, block insertion/deletion, asset tracking
- **No dead code or stubs**: All private methods called from apply dispatch or tests; no unimplemented!() / todo!() / unreachable!()
