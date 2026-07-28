# src/libs/colmena/src/crdt_documents/projection.rs

**Layer:** infrastructure  **Purpose:** Read-only projection of yrs::Doc (CRDT, Excel-shaped) into minimal IR JSON used by colmena's documents library. Provides multiple views: basic sheet/cells projection, formula-aware projection with cell metadata, and formula counting for cost optimization.

## Symbols

- `project` (pub fn) — Projects the current state of `doc` to minimal IR JSON structure with `{ "sheets": [...] }` envelope
- `project_sheet` (pub(crate) fn, generic over ReadTxn) — Projects a single yrs MapRef sheet to `{ "id", "name", "cells": { addr: value } }` object
- `project_sheet_cells_with_formulas` (pub fn) — Returns A1-keyed map of cells with formula metadata: each entry is `{v}` for literals or `{v, f, fs}` when formula/formula_status present
- `count_formulas_in_sheet` (pub fn) — Counts non-empty formula strings in a sheet, returns `0` if sheet not found; used by callers to decide cost of full formula projection
- `any_to_json` (fn, private) — Converts `yrs::Any` scalar to `serde_json::Value` (null/bool/number/bigint/string, unknown → null)
- `test_helpers` (mod, test, pub(crate)) — Test seeding utilities for populating doc structures
- `seed_simple` (fn in test_helpers, pub) — Populates doc with string cells at specified addresses (minimal fixture)
- `seed_n_cells` (fn in test_helpers, pub) — Populates doc with alternating string/number cells in sequential A1..Z1, A2..Z2 pattern (perf testing)
- `tests` (mod, test) — Unit tests: empty doc, single/multiple sheets, malformed cell handling, R2.1 projection latency benchmark
- `empty_doc_projects_to_empty_sheets` (test) — Verifies empty doc projects to `{ "sheets": [] }`
- `projects_single_sheet_with_string_cells` (test) — Verifies sheet with A1/B1 cells projects correctly
- `malformed_cell_without_v_is_skipped` (test) — Verifies cells missing required `v` field are silently omitted
- `projects_multiple_sheets` (test) — Verifies multiple sheets project in order, each with correct id/name/cells
- `r2_1_benchmark_1000_cells_p50_under_50ms` (test, #[ignore]) — Perf regression gate: projects 1000-cell doc 100 times, asserts p50 < 50ms
- `formula_projection_tests` (mod, test) — Formula-specific tests
- `project_with_formulas_emits_v_f_fs` (test) — Verifies `project_sheet_cells_with_formulas` emits formula/formula_status alongside value
- `project_with_formulas_empty_sheet_returns_empty` (test) — Verifies call on nonexistent sheet returns empty map
- `project_with_formulas_unknown_sheet_id_returns_empty` (test) — Verifies unknown sheet_id returns empty map despite other sheets existing

## File-level notes

- **Duplication between `project_sheet_cells_with_formulas` and `count_formulas_in_sheet`** (improvement): Both functions iterate sheets and match on sheet ID using identical `matches!(sheet.get(&txn, "id"), Some(yrs::Out::Any(yrs::Any::String(ref s))) if s.as_ref() == sheet_id)` pattern (lines 100–106 vs. 152–158). Could extract to private helper `find_sheet_by_id(&txn, &sheets, sheet_id) -> Option<yrs::MapRef>`.
- Line 36 comment mentions `tool_executor::apply_reorder_sheets` inlines a near-identical helper to be replaced post-Task 4 — indicates `project_sheet` is the canonical version for this operation; any future refactoring should consolidate there.
- Spec reference: "Read-only. Spec §4.3." (line 2) — see design docs for IR format contract.
- All functions handle malformed CRDT state gracefully: missing fields → empty defaults, malformed cells → silent skip. No panic on corrupt input.
- Projection functions use read-only transactions (`txn` / `&txn` passed, never mutated) — safe for concurrent access.
