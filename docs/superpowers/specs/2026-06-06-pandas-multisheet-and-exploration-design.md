# Pandas multi-sheet write-back + table exploration skills — design

**Status**: Approved (2026-06-06)
**Author**: Daniel García + colmena agent
**Tracks**: E-T20 (multi-sheet write-back) + E-T21 (table-exploration skills)
**Related**:
[2026-06-06 skills navigation spec](2026-06-06-skills-navigation-design.md),
existing `crdt_doc_run_python` (subsystem F, single-sheet `output_sheet`
+ `write_to_sheet`),
existing `gsheets_run_python` (subsystem E-T14, no write-back),
existing skills `crdt-doc-cross-sheet-analysis` and
`gsheets-cross-sheet-analysis` (cross-sheet patterns).

---

## 1. Goals

Two interlocked deliverables that let an agent answer table questions
without flooding its context — both grounded in the principle that
pandas is already the right hammer:

1. **Multi-sheet write-back from one `run_python` call.** Today the
   pattern `agent → gsheets_run_python(report_2d) → set_range(report_2d)`
   forces the report contents to flow through the LLM as a JSON 2-D
   array. Goal: let the python script declare `output_sheets = {<name>:
   DataFrame, ...}` and have the dispatcher create N new tabs directly,
   returning only metadata (`name`, `n_rows`, `n_cols`) to the LLM.
   Applies to both `gsheets_run_python` and `crdt_doc_run_python`.

2. **Table exploration skills.** Two new skill bundles
   (`gsheets-table-exploration`, `crdt-doc-table-exploration`) teach the
   agent how to inspect a single table efficiently — schema first
   (`df.head(3)`, `df.dtypes`), top-N via `nlargest`/`nsmallest` (not
   `sort_values().head()`), type coercion (`pd.to_numeric`), filtering
   (`df.query()`), grouping/aggregation. Designed to prevent the agent
   from inventing broken code or loading all rows when only the first
   handful are needed.

These two deliverables are paired because the skills' "Output shaping"
reference points users at the new multi-sheet write-back path for the
case where results belong in the spreadsheet, not in the conversation.

## 2. Non-goals

- New synthetic tools like `gsheets_query` or `gsheets_explore`. The
  user explicitly rejected adding API surface for table queries — the
  existing `run_python` machinery is sufficient when paired with a good
  skill.
- A `max_rows_to_load` or `limit` parameter on bindings. The user
  rejected this because the semantic ambiguity (raw cap vs ordered
  top-N) is a footgun; the right place to slice is in pandas after the
  load (`df.head()`, `df.tail()`, `df.nlargest()`).
- DuckDB or SQL-string parsing. Pandas already covers every query
  pattern we need.
- Changing how the agent loads data (`bindings`, sheet ranges, etc.). The
  write-back is additive.

## 3. Open-source rule

The new skills live in `src/libs/colmena/skills/` alongside the
existing ones — generic patterns, no ADP-specific business logic.
Examples use synthetic products/sales tables, never ADP entity names.

## 4. Components

### 4.1 New global `output_sheets` in the Python sandbox

Both `crdt_doc_run_python` and `gsheets_run_python` already wrap the
user's code with an auto-prelude + auto-postlude. The postlude extracts
`output` and (in CRDT today) `output_sheet`. New extension: also extract
`output_sheets` (a dict from name to DataFrame).

```python
# in postlude (new section, appended)
__col_output_sheets = None
if 'output_sheets' in dir() and output_sheets is not None:
    import pandas as _pd
    if isinstance(output_sheets, dict):
        __col_output_sheets = {}
        for k, v in output_sheets.items():
            if isinstance(v, _pd.DataFrame):
                __col_output_sheets[str(k)] = {
                    'records': v.to_dict('records'),
                    'cols': list(v.columns),
                }
            # else: silently skipped, surfaced in the dispatcher response

output = {
    'user_output': __col_user_output,
    'sheet_records': __col_sheet_records,    # existing single-sheet path (CRDT)
    'sheet_cols': __col_sheet_cols,
    'output_sheets': __col_output_sheets,    # NEW
}
```

The postlude lives under `text/prompts/python_sandbox/`. Both
`crdt_doc_run_python_postlude.md` and `gsheets_run_python_postlude.md`
get this addition.

### 4.2 Dispatcher: write the N sheets

#### crdt_doc_run_python

After the sandbox returns, if `wrapped_output.output_sheets` is a non-empty
dict, the dispatcher iterates each `(name, {records, cols})` entry and
calls `write_records_as_new_sheet(&doc, name, &cols, &records)` —
already available from subsystem F.

Conflict handling: same as today's single-sheet path. The
`write_records_as_new_sheet` helper appends `" (2)"`, `" (3)"` etc. when
a sheet name collides.

The single-sheet `output_sheet` + `write_to_sheet` path stays — kept for
back-compat. The dispatcher prefers `output_sheets` (multi) when both are
set.

#### gsheets_run_python

Today this tool has no write-back. The new args:

```rust
pub struct GsheetsRunPythonArgs {
    pub bindings: Vec<GsheetsBinding>,
    pub code: String,
    /// Optional target spreadsheet for `output_sheets` from the script.
    /// When omitted, output_sheets is ignored even if the script sets it.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,
}
```

If both `write_to_spreadsheet` is set AND the script's
`output_sheets` is non-empty, the dispatcher:

1. For each `(name, df)` entry, call `SheetsClient::add_sheet(spreadsheet_id, name)`.
2. On `SheetsError::NameAlreadyExists` (or any 400 "already exists"), retry with `" (2)"`, `" (3)"`, etc., capped at 10 attempts.
3. Build a 2-D matrix: header row from `df.cols` + body rows from `df.records`.
4. Call `SheetsClient::set_range(spreadsheet_id, resolved_name, "A1", matrix)`.
5. Collect a `WroteSheet` struct: `{name, sheet_id, n_rows, n_cols, resolved_name}`.

The output to the LLM caps each `wrote_sheets` entry — no row data.

### 4.3 LLM-visible response shape

Today:
```json
{
  "output": "...",          // user code's output dict (cap 10 KB)
  "stdout": "...",
  "error": null,
  "wrote_sheet": null       // OR the CRDT single-sheet metadata
}
```

After:
```json
{
  "output": "...",                       // unchanged
  "stdout": "...",
  "error": null,
  "wrote_sheet": null,                   // legacy single-sheet (CRDT only)
  "wrote_sheets": [                      // NEW
    {
      "name": "Top 10 Productos",
      "resolved_name": "Top 10 Productos",
      "sheet_id": 1234,
      "n_rows": 10,
      "n_cols": 4
    },
    { ... }
  ]
}
```

If `output_sheets` was empty or unset, `wrote_sheets` is `null` (not an empty array — distinguishes "not used" from "used but produced 0 sheets").

### 4.4 Two new skills (`text/skills/` — wait, in `src/libs/colmena/skills/`)

```
src/libs/colmena/skills/
├── gsheets-table-exploration/
│   ├── SKILL.md
│   └── references/
│       ├── 01-inspect-schema-first.md
│       ├── 02-top-n-patterns.md
│       ├── 03-filter-and-query.md
│       ├── 04-group-and-aggregate.md
│       ├── 05-type-coercion.md
│       └── 06-output-shaping.md
└── crdt-doc-table-exploration/
    ├── SKILL.md
    └── references/ (same 6 files, CRDT-flavored)
```

Each `SKILL.md` has frontmatter:

```yaml
---
name: gsheets-table-exploration
description: Patterns for exploring a single Google Sheets table — schema inspection, top-N, filters, aggregations, type coercion. Use BEFORE writing analysis code.
when_to_load: When the agent needs to answer a question about one sheet (top-N, filters, groupings) without joining other sheets.
---
```

Reference content highlights (each ≤ 80 lines):

- **`01-inspect-schema-first.md`**: ALWAYS start with `df.head(3)` and `df.dtypes`. Show one example each. Explains: column names matter, types matter, pandas inference is lossy.
- **`02-top-n-patterns.md`**: Use `df.nlargest(N, 'col')` for top-N by a column; `df.nsmallest(N, 'col')` for bottom-N. Do NOT use a binding-level `limit` parameter (does not exist); slice in pandas.
- **`03-filter-and-query.md`**: `df[df['col'] == X]`, `df.query('col == @x and other > 0')`. Mention `.isin()`, `.between()`, NaN handling.
- **`04-group-and-aggregate.md`**: `df.groupby('col')['num'].sum()`. Multi-agg dict, `as_index=False`, `reset_index()`.
- **`05-type-coercion.md`**: `pd.to_numeric(df['col'], errors='coerce')`, `pd.to_datetime`, the leading-apostrophe artifact from Google Sheets imports.
- **`06-output-shaping.md`**: Decision tree — small result for the agent → return via `output`; tabular result that belongs in the spreadsheet → use `output_sheets = {...}` (linked to §4.1).

The CRDT-flavored variant differs only in:
- Tool names referenced (`crdt_doc_run_python` instead of `gsheets_run_python`)
- The mention of `output_sheets` writing to the current artifact (no `write_to_spreadsheet` arg)

### 4.5 Tool-description updates

After the dispatcher change ships, the YAMLs at `text/tools/gsheets.yaml`
and `text/tools/crdt_doc.yaml` get their `gsheets_run_python` /
`crdt_doc_run_python` entries updated to mention `output_sheets` and link
to the new skill name in the description.

## 5. Decisions / edge cases

| Case | Decision |
|---|---|
| `output_sheets` set to non-dict | Skipped (postlude `isinstance(..., dict)` guard). Dispatcher returns `wrote_sheets: null` but `output` unaffected. |
| `output_sheets` value not a DataFrame | That entry is silently skipped (postlude guard). Other entries still write. |
| Sheet name collision on write | Auto-suffix `" (2)"`, `" (3)"` etc., capped at 10 attempts. Use the `resolved_name` field to tell the agent which name was actually used. |
| Both `output_sheet` (legacy) and `output_sheets` (new) set in CRDT | Prefer `output_sheets`; `output_sheet` ignored. Dispatcher includes a `_warning` in the response. |
| `write_to_spreadsheet` set but `output_sheets` unset (gsheets) | No write; normal output flow. Dispatcher includes a `_warning` so the agent learns that the arg without the code-side dict is a no-op. |
| `output_sheets` set but `write_to_spreadsheet` unset (gsheets) | No write; same warning. |
| DataFrame too large to write (e.g. > 100 cols × 1000 rows) | Truncate at the dispatcher level (matching the existing `crdt_doc_run_python` size cap of 100 MB), surface `truncated_at` in the per-sheet metadata. |
| Empty DataFrame | Header-only sheet is written (no rows). |

## 6. Tests

### B1 (multi-sheet write-back)

- **`gsheets_run_python.rs`** wiremock test: dispatcher with a 3-entry `output_sheets` produces 3 `add_sheet` + 3 `set_range` calls and returns 3 `wrote_sheets` metadata entries.
- **`gsheets_run_python.rs`** wiremock test: collision handling — `add_sheet` returns 400 once, the dispatcher retries with `" (2)"` and succeeds.
- **`crdt_doc_run_python.rs`** integration test (in-memory): a 3-entry `output_sheets` produces 3 new Y.Doc sheets, each with the expected header + rows.
- **`crdt_doc_run_python.rs`** back-compat test: old `output_sheet` + `write_to_sheet` still works alone, same response shape as before.
- **Postlude unit test**: the Python wrap-and-execute flow returns a `output_sheets` dict shape that matches the dispatcher's expectations (records + cols per entry).

### B2 (skills)

- **`skill_loads_cleanly`** (existing pattern from earlier skills): `load_skill("gsheets-table-exploration")` resolves and parses frontmatter without error.
- **`every_skill_has_frontmatter_description`** — already covered by the existing skill-loading tests; verify the new skills don't break it.
- **Index test from Alpha (`index_doc_covers_all_registered_skills`)**: the new skills must appear in `42_builtin_skills_index.md` once Alpha ships. The order Alpha → B2 in §7 ensures the index already exists when these skills land.

## 7. Task breakdown

| ID | Title | Estimate | Depends on |
|---|---|---:|---|
| **E-T20a** | Postlude update — both Python sandbox `postlude.md` files extract `output_sheets` | 30 min | — |
| **E-T20b** | `gsheets_run_python` dispatcher — parse `output_sheets`, call `add_sheet` + `set_range` N times, return `wrote_sheets` array. Includes the collision-retry logic. | 1.5 h | E-T20a |
| **E-T20c** | `crdt_doc_run_python` dispatcher — extend to `output_sheets`, keep `output_sheet` back-compat path | 1 h | E-T20a |
| **E-T20d** | Wiremock tests for gsheets (write success + collision) | 45 min | E-T20b |
| **E-T20e** | Integration test for CRDT multi-sheet | 30 min | E-T20c |
| **E-T20f** | Tool YAML updates (`text/tools/gsheets.yaml`, `text/tools/crdt_doc.yaml`) — description references `output_sheets` and the new skill | 15 min | E-T20b, E-T20c |
| **E-T21a** | New skill `gsheets-table-exploration` — SKILL.md + 6 references | 1 h | — |
| **E-T21b** | New skill `crdt-doc-table-exploration` — SKILL.md + 6 references (mirror E-T21a) | 30 min | E-T21a |
| **E-T21c** | Docs sweep — CHANGELOG entries, BACKLOG follow-ups, optional smoke graph showing multi-sheet write | 30 min | all of above |

Total: **~6 h** via subagent-driven.

**Suggested order**:
1. E-T20a (postlude — no dispatcher yet, just the markdown)
2. E-T20b + E-T20d (gsheets dispatcher + its tests)
3. E-T20c + E-T20e (CRDT dispatcher + its test)
4. E-T20f (YAML updates)
5. E-T21a → E-T21b (skills)
6. E-T21c (docs)

E-T21a/b can run in parallel with E-T20b/c if multiple subagents are dispatched, but the user has chosen serial execution.

## 8. Back-compat

| Existing usage | After change | Status |
|---|---|---|
| `crdt_doc_run_python({code, write_to_sheet, sheet_ids})` with `output_sheet = df` in the script | Still writes ONE sheet to the configured `write_to_sheet`. `wrote_sheet` response field unchanged. | ✅ Unchanged |
| `gsheets_run_python({bindings, code})` returning a JSON `output` | Still works; `write_to_spreadsheet` is optional and unset by default. | ✅ Unchanged |
| Existing `cross-sheet-analysis` skills | Untouched; the new `table-exploration` skills are siblings. | ✅ Unchanged |
| Existing CRDT integration tests | Continue to pass — back-compat path is preserved. | ✅ Verified by E-T20c |

Zero break. No downstream consumer needs to change.

## 9. Future BACKLOG

- **Format options per output sheet** — opt-in `header_style`, `column_widths`, `freeze_top_row` arguments accompanying each entry in `output_sheets`.
- **Auto-naming when name missing** — if the script puts a DataFrame in
  `output_sheets[None]` (or under a numeric key), the dispatcher names it
  `Untitled (1)`, `Untitled (2)`. Helps the LLM ship results without
  inventing names.
- **Diff-aware write** — if a sheet by that name already exists, OPTION
  to overwrite rather than create-new-with-suffix. Today's behavior
  (suffix) is the safer default; an opt-in arg makes destructive writes
  explicit.
- **Browser-side preview** — when the agent writes a new sheet, surface a
  hyperlink in the response so the user can open it directly.

## 10. Self-review

- ✅ Placeholders: none.
- ✅ Internal consistency: §4 components match §7 tasks 1:1. §6 tests
  reference shapes defined in §4.
- ✅ Scope: focused — two related deliverables sharing the same
  output-shaping contract.
- ✅ Ambiguity: §5 disambiguates every edge case (8 cases explicitly).
- ✅ Open-source rule: §3 explicit.
- ✅ Back-compat: §8 enumerated; tests verify.
- ✅ Boundary clarity: `output_sheet` (singular, CRDT-only legacy) vs
  `output_sheets` (dict, new, both tools).
- ✅ User-requested non-goals enforced: no `gsheets_query` tool, no
  `max_rows_to_load` parameter. Pandas does the work.
