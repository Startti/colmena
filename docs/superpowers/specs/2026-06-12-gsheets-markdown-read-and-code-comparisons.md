# Design — gsheets: markdown reads + reliable code-based comparisons

**Date:** 2026-06-12
**Status:** Approved (brainstorm) — pending implementation plan
**Scope:** Colmena gsheets synthetic tools (`gsheets_read`, `gsheets_run_python`) + their LLM-facing descriptions. No ADP change (the LLM consumes these tool outputs).

---

## 1. Problem & goal

Two related needs around reading and comparing Google Sheets data:

1. **Reading is not comparison-friendly.** `gsheets_read` returns `values` as a
   JSON 2-D array (`[[...]]`) — noisy with brackets/quotes, hard for the LLM to
   read or eyeball. We want a **markdown table** so a human/LLM can *see* a table
   clearly, and an easy way to read a **whole sheet** without computing ranges.

2. **Comparisons must be reliable.** Comparing tables by having the LLM eyeball
   values is unreliable. The user's concrete goal: compare a **nutritional table
   extracted from an image** against a **table in a Google Sheet**, and also
   compare **two sheets**. The solution must be **general**: compare any table
   (from an image, a sheet, or anywhere) against any other table, with the
   *comparison itself done in code* (deterministic), not by the LLM.

**Key realization:** comparison = code. `gsheets_run_python` already runs pandas
over multiple sheet ranges with the rows loaded directly (never through the LLM).
The only missing piece for the image case is that an image-extracted table is
*inline data the LLM produces*, not a sheet — and run_python today only loads
operands from sheets.

---

## 2. Architecture: separate extraction from comparison

- **Extraction** (source → structured records): inherently the LLM's job for
  images (vision reads the nutritional table into JSON). Sheets are already
  structured. We do NOT try to make extraction deterministic — it is not a
  comparison.
- **Comparison** (table vs table): always **code** (pandas in
  `gsheets_run_python`). Deterministic. The LLM writes the comparison logic
  (name/unit normalization, numeric tolerances), which is exactly the messy part
  that belongs in code, not in a rigid fixed-purpose tool.

This yields two independent deliverables (Part A = comparison core, Part B =
markdown reads), shipped together.

---

## Part A — Polymorphic bindings in `gsheets_run_python` (the comparison core)

### A.1 Change

A `gsheets_run_python` binding may be **either** a sheet source (current) **or**
an inline-data source (new):

```jsonc
"bindings": [
  // sheet source (unchanged)
  { "var": "sheet_table", "spreadsheet_id": "<id>", "sheet": "Nutrition", "range": "A1:H50" },
  // inline source (NEW) — e.g. the table the LLM extracted from an image
  { "var": "image_table", "data": [
      { "nutrient": "Protein", "per_100g": 12.0 },
      { "nutrient": "Fat",     "per_100g": 3.5 }
  ] }
]
```

- **Dispatcher:** for each binding, if `data` is present → use it directly as the
  records (skip the Google fetch); else fetch from the sheet (current path).
- **Validation:** each binding must be **exactly one** of (sheet source: has
  `spreadsheet_id` + `sheet`) or (inline: has `data`). Reject a binding with
  both or neither, with a clear message. `var` must be a unique non-empty
  identifier (current rule). `data` must be a JSON array (of objects, or of
  arrays — see A.2).
- **In-sandbox shape:** an inline binding becomes the Python global `<var>` with
  the *same shape* as a sheet binding — a list of `{col: val}` dicts — so user
  pandas code is **identical regardless of source** (`pd.DataFrame(var)` works
  either way).
- **`_gsheets_loaded_columns`** includes inline bindings (columns from the first
  record's keys), so the existing KeyError self-correction works for them too.
- **Parallelism:** only sheet bindings hit the network; inline bindings are
  ready immediately. The existing `join_all` fetch path handles a mix (inline
  bindings resolve instantly).
- Everything else unchanged: pandas/numpy/scipy, 30 s timeout, 10 KB `output`
  cap, `output_sheets` write-back (`replace`/`update_in_place`/`overwrite`),
  collision policy.

### A.2 Inline `data` accepted shapes

To match what the LLM naturally produces and what sheet bindings yield:
- **Array of objects** (records): `[{ "nutrient": "Protein", "per_100g": 12 }, …]`
  — used as-is.
- **2-D array** (`[[header...], [row...], …]`): converted to records via the
  existing `rectangle_to_records` (first row = keys), so it matches the
  `as_records` shape sheet bindings use.

Empty `data` (`[]`) → an empty list global (valid; pandas yields an empty frame).

### A.3 Tool description rewrite (`text/tools/gsheets.yaml`)

Make the comparison use-case explicit so the LLM reaches for this tool:
- "Use this for comparing / cross-referencing / deduping / diffing tables. Load
  each table as a binding (from a sheet, or inline via `data`) and compare with
  pandas. Do NOT compare values by eye."
- Document the two binding kinds (sheet vs `data`).
- Two worked recipes:
  - **Image vs sheet:** "Extract the table from the image yourself, pass it as an
    inline `data` binding, pass the sheet as a sheet binding, then compare with
    pandas — normalize nutrient names/units and compare numbers with a tolerance
    (e.g. abs diff ≤ rounding, or ±5 %)."
  - **Sheet vs sheet:** two sheet bindings → `df_a.merge(df_b, on=key)` /
    `df_a.compare(df_b)`.
- Note: write large diffs back to a tab via `output_sheets` (rows never return
  through the LLM); keep `output` to a small summary.

---

## Part B — Markdown reads for `gsheets_read`

### B.1 Change

- New parameter **`format`**: `"markdown"` (**new default**) | `"json"` (opt-out
  = current behaviour, including `as_records`).
- **`format: "markdown"`** → `{ ok, sheet, range, dimensions, markdown }`.
- **`format: "json"`** → `{ ok, sheet, range, dimensions, values }` (current
  `values`, now with `dimensions` added).
- New **`dimensions: { rows, columns }`** — the *actual data extent* (rows =
  number of value rows, columns = max row width), so the LLM knows the shape
  without parsing the A1 `range`.
- **Omit `range` = read the whole sheet** (already works: `read_range` uses the
  sheet name alone and Google returns the used range). Documented as the easy
  path; **no new boolean**.

### B.2 Markdown rendering rules

Pure helper `values_to_markdown(&Value) -> String` (operates on the 2-D values):
- **First row = header**; second markdown row is the `---` separator.
- **Ragged rows** padded to the max column count (missing cells = empty).
- **Cell rendering:** `null` → empty · bool → `true`/`false` · number → its JSON
  repr · string → escape `|` as `\|` and newlines as `<br>`.
- **Empty range** → `markdown: ""` plus a `(empty range)` note in the value.
- `as_records` is irrelevant in markdown mode (markdown is already a table); it
  only applies to `format: "json"`.

### B.3 Isolation

Markdown + dimensions are assembled in the **tool dispatcher**
(`dispatch_read_with_client`, `gsheets_tools.rs`). The client
`http_client.read_range` keeps returning structured `values` unchanged, so
`gsheets_run_python` and any structured consumer are **unaffected**.

### B.4 Tool description rewrite (`text/tools/gsheets.yaml`)

- Default output is a markdown table; `format: "json"` returns the structured
  array for programmatic use.
- To read/compare a full table: **omit `range`** to read the whole sheet; the
  result includes `dimensions` and `range`; pass an A1 range only for a subset.
- `gsheets_list_sheets` gives a rough grid size (allocated `row_count`/
  `col_count`, NOT the data extent) if you want to gauge a sheet before reading
  something huge.
- Cross-pointer: "**To compare/cross data, don't read-and-eyeball — use
  `gsheets_run_python` (code).**"

---

## 3. Testing

**Unit (Rust):**
- `values_to_markdown`: happy path, ragged rows, empty, pipe/newline escaping,
  null/bool/number/string cells, single-row.
- `dimensions` computation (rows × max width).
- `gsheets_read` dispatcher: markdown default shape; `format: "json"` preserves
  current `values` (+ dimensions); `as_records` still works under json.
- `gsheets_run_python` binding parse + dispatch: inline `data` binding (array of
  objects and 2-D array forms) becomes the right global; mixed inline+sheet;
  validation errors (both/neither source; non-array `data`); `_gsheets_loaded_columns`
  for inline.
- **Migrate** existing `gsheets_read` tests that assert `values` by default (now
  markdown default → assert markdown, or pass `format: "json"`).

**E2E (real, against Google Sheets + a real LLM):** per the project rule, save
SSE to `/tmp/colmena_e2e/` and report.
- Markdown read of a whole sheet (omit range) → markdown table + dimensions.
- **Sheet vs sheet** comparison via `gsheets_run_python` (two sheet bindings) →
  deterministic diff in `output`.
- **Inline vs sheet** comparison (simulating image-vs-sheet): one inline `data`
  binding + one sheet binding → pandas compares with a tolerance; verify the
  diff.

---

## 4. Breaking changes & compatibility

- **`gsheets_read` default output flips to markdown** (tool-output change). Sweep
  in-repo gsheets read tests. ADP unaffected (the LLM consumes the output; ADP
  does not parse it). `gsheets_run_python` and the client `read_range` are
  untouched, so structured consumers are safe.
- **`gsheets_run_python` inline bindings are additive** — existing sheet-only
  configs keep working unchanged.

---

## 5. Files (anticipated)

- `src/.../llm_synthetic_tools/gsheets_tools.rs` — `gsheets_read`: `format`
  param, `dimensions`, `values_to_markdown` helper, dispatcher assembly.
- `src/.../llm_synthetic_tools/gsheets_run_python.rs` — polymorphic binding
  parse/validation + inline-data path in the dispatcher.
- `text/tools/gsheets.yaml` — descriptions for `gsheets_read` and
  `gsheets_run_python` (formats, whole-sheet, comparison recipes, cross-pointer).
- `text/prompts/python_sandbox/gsheets_run_python_prelude.md` — note that a
  binding may come from inline `data` (same shape).
- `docs/developer_guide/39_gsheets.md` — document both features.
- `tests/graphs/...` — E2E graphs for markdown read + the two comparison flows.

---

## 6. Out of scope (explicitly)

- A dedicated fixed-purpose "compare tables" tool — pandas via `run_python` is
  more flexible (tolerances, fuzzy name/unit matching) and is the chosen path.
- Pre-binding each operand as a pandas `DataFrame` instead of a list of dicts —
  considered, deferred (would change the existing binding contract). The recipe
  documents the one-line `pd.DataFrame(var)`.
- Image extraction tooling — the LLM's vision does the extraction natively; this
  spec only consumes the extracted records as an inline binding.
- A `max_rows` truncation cap on whole-sheet reads — the LLM controls size via
  ranges + `dimensions`; revisit only if oversized reads become a problem.

---

## 7. Open items for the plan

- Exact validation messages for malformed bindings (both/neither source).
- Whether `dimensions.columns` counts the header row's width or the max across
  all rows (decision: **max across all rows**, so ragged data isn't under-counted).
- Number formatting in markdown (e.g. `42.0` vs `42`): render the JSON repr
  as-is (decision), revisit if it reads poorly.
