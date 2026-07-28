# src/libs/colmena/src/gdocs/application/apply_edits.rs

**Layer:** application  
**Purpose:** Atomic compound-edits use case that resolves N sub-edits in two phases (resolve all pre-emptively, then sort globally write-backwards), runs co-edit guard once, and POSTs a single batchUpdate to Google Docs.

## Symbols

- `APPLY_EDITS_MANY_HITS_THRESHOLD` (const, pub) — Hard cap (5) on hit count per sub-edit before rejecting; mirrors standalone `replace_text` tool threshold for safety.

- `ApplyEditOp` (enum, pub) — Variant enum for sub-edit types: `ReplaceText { find, replace, scope }`, `InsertAfterText { anchor, new_markdown }`, `DeleteText { find, scope }`.

- `ApplyEditsInput` (struct, pub) — Input wrapper carrying `edits: Vec<ApplyEditOp>`.

- `run()` (async fn, pub) — Main orchestrator: validates inputs, runs co-edit guard on Scope::All, resolves each sub-edit into ResolvedEmit records with byte offsets, checks for overlaps, sorts globally write-backwards (paragraph DESC, byte_off DESC), flattens into final request vector, POSTs batchUpdate, invalidates cache, refetches snapshot, returns EditResult with changes + lossy_conversions + soft_warnings.

- `ResolvedEmit` (struct, private) — Intermediate record carrying: paragraph number, byte range (off/len) for sorting, requests Vec, ChangeRecord; used to defer write-backwards sort until all sub-edits are resolved.

- `find_hits()` (fn, private) — Scope-aware substring search using regex::escape and case-insensitive matching; returns Vec<(paragraph, byte_off, byte_len)> or TextNotFound error.

- `enforce_many_hits_threshold()` (fn, private) — Guard that rejects hit count >= 5 with ConfirmManyMatches error (previews, find string, count); fails before any batchUpdate request.

- `build_previews()` (fn, private) — Builds Vec<MatchPreview> for ConfirmManyMatches error; extracts ~30 chars before, ~50 chars after each hit with UTF-8 safe slicing.

- `take_slice_around()` (fn, private) — UTF-8 safe string slicing between byte offsets, snapping inward to char boundaries to avoid mid-sequence slices.

- `check_no_overlaps_within_paragraph()` (fn, private) — Rejects any pair of ResolvedEmit records on the same paragraph whose half-open [byte_off, byte_off+byte_len) intervals intersect; special-cases two zero-length inserts at same offset; fires during Phase A before any write.

- `find_anchor_first()` (fn, private) — Linear search through tabs and paragraphs for anchor string; returns (paragraph_num, byte_off, anchor_len, tab_id) or TextNotFound error.

- `lookup_para()` (fn, private) — O(n) lookup of ParagraphSnapshot by paragraph number; panics (expect) if not found (invariant: only called when paragraph is guaranteed to exist).

- `lookup_tab_id()` (fn, private) — Finds the TabId for a paragraph number by iterating tabs; returns Option<TabId>.

- `utf16_len()` (fn, private) — Computes UTF-16 encoded length of a string (for Google Docs index calculations).

- `build_outline()` (fn, private) — Flattens snapshot tabs and paragraphs into Vec<OutlineEntry> for EditResult.

- `app_tests` (mod, cfg(test)) — Test module with 11 test cases covering empty edits, single/compound replaces, global write-backwards sort regression, overlap detection, many-hits threshold, scope narrowing bypass, delete threshold parity.

## File-level notes

- **Two-phase algorithm is sound and well-documented** (lines 84–96): Phase A resolves all edits with snapshot indices pre-emptively, Phase B sorts globally and flattens. Regression test `apply_edits_global_write_backwards_sort_across_sub_edits` pins the invariant.
- **Pre-checks before any write**: Empty list, table markdown, hit resolution, overlap detection, and many-hits threshold all fire BEFORE batchUpdate POST; document left untouched on error (lines 67–232).
- **UTF-16 awareness**: Correctly handles Google Docs index calculations via `utf16_len()` at lines 111–112, 147–148, 177.
- **Revision tracking discipline** (lines 238–246): Stores the snapshot's revision_id (documents.get format) not batch_update's writeControl id, so co-edit guard's next revisionId comparison succeeds; see replace_text.rs for rationale.
- **Invariant safety**: `lookup_para()` and `lookup_tab_id()` use expect/panics, but only called after hits are confirmed to exist; not unfinished but asserting preconditions guaranteed by prior checks.
- **Test coverage comprehensive**: 11 tests including boundary conditions (4 hits vs 5 hits), regression (global sort), and recovery paths (scope narrowing).
- **No outstanding issues**: No dead code, todos, or simplifications needed; production-quality application-layer module.
