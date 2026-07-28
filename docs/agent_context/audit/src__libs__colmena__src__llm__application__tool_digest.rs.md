# src/libs/colmena/src/llm/application/tool_digest.rs

**Layer:** application  
**Purpose:** Deterministic structured digests of JSON tool results for conversation history compaction (v1.1). Preserves schema shape (columns, field types, ranges) instead of lossy natural-language prose; full result recoverable via lossless recall_history (v1).

## Symbols

### Public API
- `digest_tool_result(content: &str) -> Option<String>` (pub fn) — entry point: returns structured one-line digest for recognizably structured JSON (object, array-of-objects, scalar array), or None to fall back to NL summary

### Constants
- `MAX_COLUMNS` (const) — cap on column names shown in tabular digest (12)
- `SAMPLE_ROWS` (const) — number of rows to sample for display in array digest (2)
- `SCAN_ROWS_FOR_COLUMNS` (const) — row scan depth to collect union of keys (50)
- `MAX_INLINE_FIELDS` (const) — scalar fields shown inline in object digest (6)
- `FIELD_VALUE_CHARS` (const) — char truncation per scalar field value (40)
- `DIGEST_CEILING_CHARS` (const) — total digest char cap after all composition (400)
- `IDENTITY_KEYS_NAME` (const) — priority key list for row names/identities (name, title, label)
- `IDENTITY_KEYS_TYPE` (const) — priority key list for row types/kinds (type, kind)
- `IDENTITY_SEARCH_DEPTH` (const) — nesting depth budget for identifier search (1 level)
- `IDENTITY_SAMPLE_ROWS` (const) — rows to sample for nominal row labels in drill-down (3)
- `MAX_AGG_COLS` (const) — max numeric columns to compute min/max aggregates (3)

### Private Functions
- `digest_array(arr: &[Value]) -> Option<String>` (fn) — array-of-objects → tabular digest with cols/sample/aggregates; array-of-scalars → count+sample
- `digest_object(v: &Value) -> Option<String>` (fn) — object digest: field markers, inline scalars, and drill-down into dominant nested array
- `consider_drill<'a>(k: &str, v: &'a Value, drill: &mut Option<...>)` (fn) — selects largest array-of-objects from nested structures for drilling
- `collect_columns(arr: &[Value]) -> Vec<String>` (fn) — union of object keys from first N rows, preserving first-seen order
- `join_capped(cols: &[String]) -> String` (fn) — renders column list with `+N más` overflow marker if >MAX_COLUMNS
- `sample_rows(arr: &[Value], cols: &[String]) -> Vec<String>` (fn) — extracts first 2 rows as field tuples `{col:value, col:value}`; object columns show identifier instead of opaque marker
- `numeric_aggregates(arr: &[Value], cols: &[String]) -> Vec<String>` (fn) — min/max for first MAX_AGG_COLS numeric columns across all rows
- `fmt_num(n: f64) -> String` (fn) — renders numbers as i64 if integral and <1e15, else 2-decimal float (note: precision loss >2^53 acceptable, digest is lossy)
- `scalar_str(v: &Value) -> String` (fn) — renders JSON value as string: null/bool/number/string direct; array/object as length markers; capped at FIELD_VALUE_CHARS
- `find_identifier(map: &serde_json::Map<String, Value>, keys: &[&str], depth: usize) -> Option<String>` (fn) — shallow priority search for first scalar value matching key list; recurses into objects up to depth budget
- `row_label(row: &Value, cols: &[String]) -> Option<String>` (fn) — compact row label: `<type> "<name>"` if both found, else whichever available, else fallback to `col:value` for first scalar
- `nominal_sample(arr: &[Value], cols: &[String]) -> Option<String>` (fn) — nominal preview of first 3 rows as `[label1, label2, label3, … +N]`; None if no labels
- `cap(s: &str, max: usize) -> String` (fn) — char-safe truncation with `…` ellipsis

### Tests (14 tests)
- `array_of_objects_becomes_tabular_digest` — verifies row count, column list, sample row formatting
- `array_of_scalars_becomes_count_and_sample` — verifies element count and sample for scalar arrays
- `object_lists_fields_inline_scalars_and_nested_markers` — checks field markers, inline scalars, array/object length notation, drill-down
- `non_json_returns_none` — bare text returns None
- `bare_scalar_returns_none` — single number/string returns None (not structured)
- `empty_collections_return_none` — `[]` and `{}` return None
- `many_columns_are_capped_with_overflow_marker` — column overflow with `+N más` marker
- `tabular_digest_includes_numeric_min_max` — min/max aggregates for numeric columns
- `aggregates_skip_non_numeric_columns_and_handle_partial` — skips non-numeric; handles sparse values
- `drilled_array_in_object_includes_aggregates` — wrapped payload drill-down with aggregates
- `drill_down_shows_nominal_row_labels` — row labels using type+name heuristics
- `tabular_sample_renders_object_column_identifier` — nested object columns show identifier instead of `{N}`
- `row_label_falls_back_to_first_scalar_when_no_identifier` — fallback to first scalar when no name/type
- `identifier_search_respects_depth_budget` — respects IDENTITY_SEARCH_DEPTH limit
- `nominal_label_caps_long_name` — long values truncated, digest stays within ceiling

## File-level notes
- Module is pure (no I/O, no LLM calls). Returns deterministic digests keyed on JSON content shape.
- All constants are well-named and documented with inline comments explaining their purpose (search depth budget at line 19–22).
- All functions handle edge cases (empty arrays, sparse columns, non-numeric fields, truncation) gracefully.
- Comprehensive test coverage: 14 tests verify array/object paths, edge cases, capping, aggregates, and drill-down logic.
- Comments document design intent (e.g., "recall_history is exact" at line 6, precision loss acceptable at line 218, drill-down rationale at line 87–88).
- Spanish UI strings throughout (filas, muestra, cols, más, objeto, elementos, etc.) consistent with regional audience.
