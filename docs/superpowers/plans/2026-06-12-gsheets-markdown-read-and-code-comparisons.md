# gsheets: markdown reads + code-based comparisons — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `gsheets_read` return a markdown table by default (with data dimensions, whole-sheet via omitting range), and let `gsheets_run_python` accept inline-data bindings so any table — including one the LLM extracted from an image — can be compared with pandas (deterministic).

**Architecture:** Two independent, additive changes to Colmena gsheets synthetic tools. Part B (markdown read) lives entirely in the tool dispatcher, leaving the client `read_range` (used by run_python) untouched. Part A (polymorphic bindings) makes a binding's source either a sheet or inline `data`, reusing the existing pandas pipeline. Both follow TDD with inline `#[cfg(test)]` tests.

**Tech Stack:** Rust (`colmena_dag_engine`), serde/serde_json, the gsheets synthetic tools. Build/test from `src/libs/colmena/`. Spec: [`../specs/2026-06-12-gsheets-markdown-read-and-code-comparisons.md`](../specs/2026-06-12-gsheets-markdown-read-and-code-comparisons.md).

**Build commands:** `cargo test --lib <module>` for unit tests, `cargo clippy --lib`, `cargo fmt`. E2E: `set -a && source .env && set +a && cargo run --bin dag_engine -- run <graph>`.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` | Modify | `gsheets_read`: `format` arg, `dimensions`, `values_to_markdown` + `compute_dimensions` helpers, dispatcher branching |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` | Modify | Polymorphic `GsheetsBinding` (sheet \| inline `data`) + per-binding validation + inline path in dispatcher |
| `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` | Modify | Expose `rectangle_to_records` as `pub(crate)` (reused for inline 2-D data) |
| `src/libs/colmena/text/tools/gsheets.yaml` | Modify | LLM-facing descriptions for `gsheets_read` (formats, whole-sheet) and `gsheets_run_python` (inline bindings, comparison recipes) |
| `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_prelude.md` | Modify | Note a binding may come from inline `data` (same shape) |
| `docs/developer_guide/39_gsheets.md` | Modify | Document both features |
| `tests/graphs/agents/gsheets_markdown_read.json` | Create | E2E: whole-sheet markdown read |
| `tests/graphs/agents/gsheets_compare_inline_vs_sheet.json` | Create | E2E: inline-vs-sheet comparison (image-vs-sheet shape) |

---

## PART B — gsheets_read markdown (Phase 1)

### Task 1: `values_to_markdown` + `compute_dimensions` helpers

**Files:**
- Modify: `.../llm_synthetic_tools/gsheets_tools.rs` (add two private fns + their tests)

- [ ] **Step 1: Write failing tests** — append to the existing `#[cfg(test)] mod tests` in `gsheets_tools.rs` (find it near the end of the file):

```rust
    #[test]
    fn values_to_markdown_renders_table_with_header() {
        let v = serde_json::json!([["name", "qty"], ["apple", 3], ["pear", 10]]);
        let md = values_to_markdown(&v);
        assert_eq!(
            md,
            "| name | qty |\n| --- | --- |\n| apple | 3 |\n| pear | 10 |"
        );
    }

    #[test]
    fn values_to_markdown_pads_ragged_rows_and_renders_types() {
        // ragged (2nd row shorter), null cell, bool, pipe + newline escaping
        let v = serde_json::json!([
            ["a", "b", "c"],
            [true, serde_json::Value::Null],
            ["x|y", "line1\nline2", 1.5]
        ]);
        let md = values_to_markdown(&v);
        assert_eq!(
            md,
            "| a | b | c |\n| --- | --- | --- |\n| true |  |  |\n| x\\|y | line1<br>line2 | 1.5 |"
        );
    }

    #[test]
    fn values_to_markdown_empty_is_empty_string() {
        let v = serde_json::json!([]);
        assert_eq!(values_to_markdown(&v), "");
    }

    #[test]
    fn compute_dimensions_uses_max_width_including_header() {
        let v = serde_json::json!([["a", "b", "c"], [1, 2]]);
        assert_eq!(
            compute_dimensions(&v),
            serde_json::json!({ "rows": 2, "columns": 3 })
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib gsheets_tools::tests::values_to_markdown 2>&1 | tail -5`
Expected: FAIL — `values_to_markdown` / `compute_dimensions` not found.

- [ ] **Step 3: Implement the helpers** — add near the other private helpers in `gsheets_tools.rs` (e.g. just below `parse_value_render`):

```rust
/// Render a single cell value for a markdown table cell.
/// null → empty; bool/number → plain repr; string → escape `|` and newlines.
fn md_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.replace('|', "\\|").replace('\n', "<br>"),
        // Arrays/objects shouldn't appear in a flat sheet cell; stringify defensively.
        other => other.to_string().replace('|', "\\|").replace('\n', "<br>"),
    }
}

/// Convert a 2-D values array (`[[..],[..]]`) into a GitHub markdown table.
/// First row is the header. Ragged rows are padded to the max column count.
/// Returns an empty string for an empty range.
fn values_to_markdown(values: &serde_json::Value) -> String {
    let rows = match values.as_array() {
        Some(r) if !r.is_empty() => r,
        _ => return String::new(),
    };
    let width = rows
        .iter()
        .map(|row| row.as_array().map(|a| a.len()).unwrap_or(0))
        .max()
        .unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let render_row = |row: &serde_json::Value| -> String {
        let cells = row.as_array().cloned().unwrap_or_default();
        let mut out = String::from("|");
        for i in 0..width {
            let cell = cells.get(i).map(md_cell).unwrap_or_default();
            out.push_str(&format!(" {cell} |"));
        }
        out
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(render_row(&rows[0])); // header
    lines.push(format!("|{}", " --- |".repeat(width))); // separator
    for row in &rows[1..] {
        lines.push(render_row(row));
    }
    lines.join("\n")
}

/// Compute the data extent of a read result: rows = number of returned rows
/// (includes the header row in 2-D mode), columns = max row width. Works for
/// both 2-D arrays (cells) and record arrays (object keys).
fn compute_dimensions(values: &serde_json::Value) -> serde_json::Value {
    let arr = values.as_array();
    let rows = arr.map(|a| a.len()).unwrap_or(0);
    let columns = arr
        .map(|a| {
            a.iter()
                .map(|row| match row {
                    serde_json::Value::Array(r) => r.len(),
                    serde_json::Value::Object(o) => o.len(),
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    serde_json::json!({ "rows": rows, "columns": columns })
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --lib gsheets_tools::tests::values_to_markdown && cargo test --lib gsheets_tools::tests::compute_dimensions`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs
git commit -m "feat(gsheets): values_to_markdown + compute_dimensions helpers"
```

### Task 2: `format` arg + dispatcher branching for `gsheets_read`

**Files:**
- Modify: `gsheets_tools.rs` — `ReadArgs` (line ~104) and `dispatch_read_with_client` (line ~592)

- [ ] **Step 1: Write the failing test** — append to the tests module. Uses the existing `MockSheetsClient` pattern; mirror how other dispatch tests in this file build a mock that returns a fixed `ReadResponse`. (Find an existing `dispatch_read_*` test in the file and copy its mock setup.)

```rust
    #[tokio::test]
    async fn dispatch_read_defaults_to_markdown_with_dimensions() {
        // Mock returns the raw 2-D grid for as_records=false.
        let client = mock_client_returning(serde_json::json!([["name", "qty"], ["apple", 3]]));
        let args = serde_json::json!({ "spreadsheet_id": "id", "sheet": "Sheet1" });
        let out = dispatch_read_with_client(args, &client).await;
        assert_eq!(out["ok"], true);
        assert_eq!(out["markdown"], "| name | qty |\n| --- | --- |\n| apple | 3 |");
        assert_eq!(out["dimensions"], serde_json::json!({ "rows": 2, "columns": 2 }));
        assert!(out.get("values").is_none(), "markdown mode must not include raw values");
    }

    #[tokio::test]
    async fn dispatch_read_json_format_preserves_values() {
        let client = mock_client_returning(serde_json::json!([["name", "qty"], ["apple", 3]]));
        let args = serde_json::json!({ "spreadsheet_id": "id", "sheet": "Sheet1", "format": "json" });
        let out = dispatch_read_with_client(args, &client).await;
        assert_eq!(out["values"], serde_json::json!([["name", "qty"], ["apple", 3]]));
        assert_eq!(out["dimensions"], serde_json::json!({ "rows": 2, "columns": 2 }));
        assert!(out.get("markdown").is_none());
    }
```

> If the file has no reusable `mock_client_returning`, add a tiny helper in the
> tests module that builds a mock `SheetsClient` whose `read_range` returns a
> `ReadResponse { sheet, range, values }` with the given `values` (model it on
> the existing `read_range_returns_2d_array_for_unformatted` mock in
> `http_client.rs` and any mock already used by dispatch tests in this file).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib gsheets_tools::tests::dispatch_read_defaults_to_markdown 2>&1 | tail -8`
Expected: FAIL — `format` unknown field / `markdown` key missing.

- [ ] **Step 3: Add the `format` field to `ReadArgs`** (line ~104):

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub spreadsheet_id: String,
    pub sheet: String,
    pub range: Option<String>,
    /// "FORMULA" | "UNFORMATTED_VALUE" | "FORMATTED_VALUE". Default
    /// UNFORMATTED_VALUE (pandas-friendly scalars).
    pub value_render: Option<String>,
    #[serde(default)]
    pub as_records: bool,
    /// "markdown" (default) renders a markdown table; "json" returns the
    /// structured `values` array (honoring `as_records`).
    pub format: Option<String>,
}
```

- [ ] **Step 4: Rewrite the `Ok(r)` arm of `dispatch_read_with_client`** (line ~592). Replace the `opts`/`match` tail with:

```rust
    let want_markdown = parsed.format.as_deref().unwrap_or("markdown") != "json";
    let opts = ReadOptions {
        value_render: parse_value_render(parsed.value_render.as_deref()),
        // Markdown needs the 2-D grid (header row); json honors as_records.
        as_records: if want_markdown { false } else { parsed.as_records },
    };
    match client
        .read_range(
            &SpreadsheetId(parsed.spreadsheet_id),
            &parsed.sheet,
            range.as_deref(),
            opts,
        )
        .await
    {
        Ok(r) => {
            let dimensions = compute_dimensions(&r.values);
            if want_markdown {
                serde_json::json!({
                    "ok": true,
                    "sheet": r.sheet,
                    "range": r.range,
                    "dimensions": dimensions,
                    "markdown": values_to_markdown(&r.values),
                })
            } else {
                serde_json::json!({
                    "ok": true,
                    "sheet": r.sheet,
                    "range": r.range,
                    "dimensions": dimensions,
                    "values": r.values,
                })
            }
        }
        Err(e) => error_to_json(e),
    }
```

- [ ] **Step 5: Migrate existing read tests** — search the file (and `http_client.rs` integration tests if they assert dispatcher output) for any `dispatch_read*` test asserting `out["values"]` by default. Either (a) add `"format": "json"` to its args, or (b) update the assertion to markdown. Run:

```bash
grep -rn "dispatch_read" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs
```
Update each so it reflects the new default. (Client-level `read_range` tests in `http_client.rs` are unaffected — they test the client, not the tool.)

- [ ] **Step 6: Run to verify**

Run: `cargo test --lib gsheets_tools::tests`
Expected: PASS (all, including the 2 new dispatch tests).

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --lib 2>&1 | tail -3
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs
git commit -m "feat(gsheets): gsheets_read defaults to markdown + dimensions (format:json opt-out)"
```

### Task 3: `gsheets_read` tool description

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml` (the `gsheets_read` entry, ~lines 33-36)

- [ ] **Step 1: Rewrite the `gsheets_read` description** to (read the current entry first, then replace its `description`):

```
Read a sheet and return it as a MARKDOWN TABLE (default) — ideal for reading or
showing a table. To read a whole sheet, OMIT `range` (reads the entire used
area); pass an A1 `range` only for a subset. The result includes `dimensions`
{rows, columns} (the data extent) and the A1 `range`, so you can then read a
precise subrange. For structured data (further processing) pass
`format: "json"` to get the raw `values` array. NOTE: to COMPARE or cross-
reference tables, do NOT read-and-eyeball — use `gsheets_run_python` (code).
`gsheets_list_sheets` gives a rough allocated grid size (not the data extent)
if you want to gauge a sheet before reading something large.
```

- [ ] **Step 2: Verify the YAML still parses** (it's loaded at build via the text registry):

Run: `cargo test --lib text:: 2>&1 | tail -5` (or `cargo build --lib`)
Expected: builds/loads without error.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml
git commit -m "docs(gsheets): gsheets_read description — markdown default, whole-sheet, dimensions"
```

---

## PART A — gsheets_run_python polymorphic bindings (Phase 2)

### Task 4: polymorphic `GsheetsBinding` + per-binding validation

**Files:**
- Modify: `gsheets_run_python.rs` — `GsheetsBinding` (line ~85) + the validation block in the dispatcher (line ~236)

- [ ] **Step 1: Write failing tests** — append to the `#[cfg(test)] mod tests` in `gsheets_run_python.rs`:

```rust
    #[test]
    fn binding_inline_data_parses() {
        let v = serde_json::json!({ "var": "t", "data": [{"a": 1}] });
        let b: GsheetsBinding = serde_json::from_value(v).unwrap();
        assert_eq!(b.var, "t");
        assert!(b.data.is_some());
        assert!(b.spreadsheet_id.is_none());
    }

    #[tokio::test]
    async fn dispatch_rejects_binding_with_both_sources() {
        let client = test_client_empty(); // any mock; validation happens pre-fetch
        let args = serde_json::json!({
            "bindings": [{ "var": "t", "spreadsheet_id": "x", "sheet": "S", "data": [{"a":1}] }],
            "code": "output = 1"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        assert_eq!(out["error"], "invalid_args");
        assert!(out["message"].as_str().unwrap().contains("exactly one"));
    }

    #[tokio::test]
    async fn dispatch_rejects_binding_with_no_source() {
        let client = test_client_empty();
        let args = serde_json::json!({
            "bindings": [{ "var": "t" }],
            "code": "output = 1"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        assert_eq!(out["error"], "invalid_args");
        assert!(out["message"].as_str().unwrap().contains("exactly one"));
    }
```

> `test_client_empty()` = any `SheetsClient` mock already used by tests in this
> file (reuse the existing mock builder from `dispatch_runs_pandas_over_two_bindings_in_parallel`).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib gsheets_run_python::tests::binding_inline 2>&1 | tail -6`
Expected: FAIL — `data` unknown field / `spreadsheet_id` not Option.

- [ ] **Step 3: Make `GsheetsBinding` polymorphic** (line ~85):

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GsheetsBinding {
    /// Python variable name to bind the records to (e.g. "products").
    /// Accepts UX aliases `binding_name` and `name`.
    #[serde(alias = "binding_name", alias = "name")]
    pub var: String,
    /// Spreadsheet ID. Required for a SHEET binding; omit for an inline `data`
    /// binding.
    #[serde(default)]
    pub spreadsheet_id: Option<String>,
    /// Tab name. Required for a SHEET binding; omit for an inline `data` binding.
    #[serde(default, alias = "sheet_name")]
    pub sheet: Option<String>,
    /// Optional A1 range (sheet bindings only). Omit to read the entire tab.
    #[serde(default)]
    pub range: Option<String>,
    /// Inline table data (an array of `{col: val}` objects, OR a 2-D array
    /// whose first row is the header). When present, this binding is NOT
    /// fetched from Google — the data is used directly. Use this to pass a
    /// table the model produced (e.g. extracted from an image) as an operand.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}
```

- [ ] **Step 4: Add per-binding source validation** — in the dispatcher, right after the existing duplicate-`var` check block (line ~256), insert:

```rust
    // Each binding must have EXACTLY ONE source: a sheet (spreadsheet_id + sheet)
    // OR inline `data`. Reject both/neither so the LLM gets a clear signal.
    for b in &parsed.bindings {
        let has_sheet = b.spreadsheet_id.is_some() && b.sheet.is_some();
        let has_data = b.data.is_some();
        if has_sheet == has_data {
            return serde_json::json!({
                "error": "invalid_args",
                "message": format!(
                    "binding '{}' must have exactly one source: either \
                     `spreadsheet_id` + `sheet`, or inline `data`",
                    b.var
                ),
            });
        }
        if let Some(d) = &b.data {
            if !d.is_array() {
                return serde_json::json!({
                    "error": "invalid_args",
                    "message": format!("binding '{}' inline `data` must be a JSON array", b.var),
                });
            }
        }
    }
```

- [ ] **Step 5: Run the parse/validation tests**

Run: `cargo test --lib gsheets_run_python::tests::binding_inline && cargo test --lib gsheets_run_python::tests::dispatch_rejects`
Expected: the parse test PASSES; the two reject tests PASS. (The two-bindings + update-in-place tests will still compile but may need the fetch-loop change from Task 5 to keep passing — that's next.)

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(gsheets): polymorphic run_python bindings — sheet OR inline data (struct + validation)"
```

### Task 5: inline-data path in the dispatcher fetch loop

**Files:**
- Modify: `gsheets_run_python.rs` — the fetch loop (line ~261) + records-build loop (line ~281)
- Modify: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` — make `rectangle_to_records` `pub(crate)`

- [ ] **Step 1: Write the failing test** — inline binding flows into the sandbox as a `var`:

```rust
    #[tokio::test]
    async fn dispatch_inline_data_binding_reaches_sandbox() {
        // No sheet fetch needed; client can be any mock.
        let client = test_client_empty();
        let args = serde_json::json!({
            "bindings": [{ "var": "img", "data": [{"nutrient": "Protein", "g": 12}, {"nutrient": "Fat", "g": 3}] }],
            "code": "output = {'rows': len(img), 'first': img[0]['nutrient']}"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        assert_eq!(out["output"]["rows"], 2);
        assert_eq!(out["output"]["first"], "Protein");
        // columns surfaced for self-correction
        assert!(out["loaded_columns"]["img"].as_array().unwrap().contains(&serde_json::json!("nutrient")));
    }

    #[tokio::test]
    async fn dispatch_inline_2d_array_is_converted_to_records() {
        let client = test_client_empty();
        let args = serde_json::json!({
            "bindings": [{ "var": "t", "data": [["nutrient", "g"], ["Protein", 12]] }],
            "code": "output = {'rows': len(t), 'g': t[0]['g']}"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        assert_eq!(out["output"]["rows"], 1);
        assert_eq!(out["output"]["g"], 12);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib gsheets_run_python::tests::dispatch_inline 2>&1 | tail -8`
Expected: FAIL — inline bindings are currently fetched as sheets (empty/var missing).

- [ ] **Step 3: Make `rectangle_to_records` reusable** — in `http_client.rs`, change its signature visibility from `fn rectangle_to_records` to `pub(crate) fn rectangle_to_records` (search for `fn rectangle_to_records`), and add a re-export usable from the tool module. Confirm it's importable:

```rust
// in gsheets_run_python.rs imports
use crate::gsheets::infrastructure::http_client::rectangle_to_records;
```
(If `http_client`'s module path differs, use the actual path; verify with `grep -rn "fn rectangle_to_records" src/libs/colmena/src`.)

- [ ] **Step 4: Only fetch SHEET bindings; resolve inline bindings directly.** Replace the fetch loop (line ~261) so it filters to sheet bindings:

```rust
    // 2. Fetch only the SHEET bindings in parallel. Inline (`data`) bindings
    //    are resolved directly below — no network.
    let fetches = parsed
        .bindings
        .iter()
        .filter(|b| b.data.is_none())
        .map(|b| {
            let client = client.clone();
            let opts = ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: true,
            };
            // safe: validation guarantees sheet bindings have these
            let ss = SpreadsheetId(b.spreadsheet_id.clone().unwrap());
            let sheet = b.sheet.clone().unwrap();
            let range = b.range.clone();
            let var = b.var.clone();
            async move {
                let res = client.read_range(&ss, &sheet, range.as_deref(), opts).await;
                (var, res)
            }
        });
    let results: Vec<(String, Result<ReadResponse, _>)> = join_all(fetches).await;
```

- [ ] **Step 5: Add inline bindings into the inputs/loaded_columns maps.** In the records-build section (line ~281), AFTER the existing `for (var, res) in results { ... }` loop (which handles sheet bindings), insert a loop over inline bindings:

```rust
    // Inline `data` bindings: normalize to records and inject like sheet bindings.
    for b in parsed.bindings.iter().filter(|b| b.data.is_some()) {
        let raw = b.data.clone().unwrap(); // validated to be an array
        // Accept records (array of objects) as-is; convert a 2-D array
        // (first row = header) to records to match the sheet shape.
        let records = match raw.as_array().and_then(|a| a.first()) {
            Some(serde_json::Value::Array(_)) => rectangle_to_records(&raw),
            _ => raw, // array of objects, or empty array
        };
        let columns = extract_columns(&records);
        loaded_columns.insert(
            b.var.clone(),
            serde_json::Value::Array(columns.into_iter().map(serde_json::Value::String).collect()),
        );
        inputs.insert(b.var.clone(), records);
    }
```

> Note: `loaded_columns` is built as a `serde_json::Map` in the existing code
> BEFORE it's wrapped into a `Value::Object`. Insert this block BEFORE the
> `let loaded_columns = serde_json::Value::Object(loaded_columns);` line so both
> sheet and inline columns are included.

- [ ] **Step 6: Run to verify**

Run: `cargo test --lib gsheets_run_python::tests`
Expected: PASS — new inline tests pass; existing sheet/two-binding/update-in-place tests still pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --lib 2>&1 | tail -3
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs src/libs/colmena/src/gsheets/infrastructure/http_client.rs
git commit -m "feat(gsheets): run_python resolves inline-data bindings (records or 2-D) into the sandbox"
```

### Task 6: prelude note + run_python tool description (comparison recipes)

**Files:**
- Modify: `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_prelude.md`
- Modify: `src/libs/colmena/text/tools/gsheets.yaml` (the `gsheets_run_python` entry)

- [ ] **Step 1: Add a line to the prelude** (after the "Each binding is already bound" line):

```
# A binding may come from a sheet OR from inline `data` you passed — either way
# it is a list of {col: val} dicts. Build a DataFrame with pd.DataFrame(<var>).
```

- [ ] **Step 2: Rewrite the `gsheets_run_python` description** in `gsheets.yaml` (read the current entry first). Add the comparison framing + binding kinds + two recipes:

```
Run sandboxed pandas (pd), numpy (np), scipy.stats over one or more tables.
USE THIS TO COMPARE / cross-reference / dedupe / diff tables — load each table
as a binding and compare in code; do NOT compare values by eye.

A binding is either a SHEET source ({var, spreadsheet_id, sheet, range?}) or an
INLINE source ({var, data: [...]}). Inline `data` is an array of {col: val}
objects (or a 2-D array with a header row) — use it to pass a table you produced,
e.g. one you extracted from an image. Each binding becomes a Python list of dicts
under `var`; do `df = pd.DataFrame(var)`.

Recipe — image vs sheet: extract the table from the image yourself, pass it as an
inline `data` binding, pass the sheet as a sheet binding, then compare with
pandas — normalize names/units and compare numbers WITH A TOLERANCE (e.g.
abs(a-b) <= 0.5, or within ±5%). Recipe — sheet vs sheet: two sheet bindings →
df_a.merge(df_b, on=key) or df_a.compare(df_b). Define `output` as a small
summary; for large diffs write rows back via `output_sheets` (rows never return
through the model).
```

- [ ] **Step 3: Verify build/load**

Run: `cargo build --lib 2>&1 | tail -3`
Expected: builds (text registry loads).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_prelude.md src/libs/colmena/text/tools/gsheets.yaml
git commit -m "docs(gsheets): run_python description — inline bindings + comparison recipes"
```

---

## Phase 3 — full verification, docs, E2E

### Task 7: full suite + docs

**Files:**
- Modify: `docs/developer_guide/39_gsheets.md`

- [ ] **Step 1: Full unit suite + lint**

Run: `cargo test --lib 2>&1 | tail -5 && cargo clippy --lib 2>&1 | tail -3`
Expected: all pass, no clippy warnings.

- [ ] **Step 2: Document both features** in `docs/developer_guide/39_gsheets.md` — add a "Reading: markdown default + dimensions" subsection (format param, whole-sheet via omitting range) and a "Comparing tables (code)" subsection (polymorphic bindings, image-vs-sheet + sheet-vs-sheet recipes). Read the file's existing structure and match it.

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/39_gsheets.md
git commit -m "docs(gsheets): document markdown reads + code-based comparisons (dev guide §39)"
```

### Task 8: E2E graphs against real Google Sheets + Gemini

**Files:**
- Create: `tests/graphs/agents/gsheets_markdown_read.json`, `tests/graphs/agents/gsheets_compare_inline_vs_sheet.json`

> Requires real creds: per the local runbook, inject Google OAuth creds from
> Secret Manager (startti-dev) in-memory and `set -a && source .env && set +a`
> for `GEMINI_API_KEY`. Save SSE to `/tmp/colmena_e2e/` and report a friendly
> summary; do not paste full SSE in chat. Use a real spreadsheet the OAuth user
> can access (ask the user for an id, or reuse one from the gsheets E2E runbook).

- [ ] **Step 1: Markdown read graph** — `gsheets_markdown_read.json`: trigger_webhook + llm_call (gemini-2.5-flash, `enabled_tools: ["gsheets"]`) with a prompt: "Read the whole '<sheet>' tab of spreadsheet <id> and show it as a table." Expected: the agent calls `gsheets_read` (no range) and gets back a `markdown` table + `dimensions`.

- [ ] **Step 2: Inline-vs-sheet comparison graph** — `gsheets_compare_inline_vs_sheet.json`: prompt gives a small inline nutritional table in text and asks: "Compare this nutritional table: <Protein 12g, Fat 3g, Carbs 40g> against the '<sheet>' tab of <id> (columns nutrient, grams). Report any value that differs by more than 0.5g." Expected: the agent calls `gsheets_run_python` with one inline `data` binding (the values from the prompt) + one sheet binding, writes pandas to compare with tolerance, returns the mismatches.

- [ ] **Step 3: Run both, save SSE, verify**

```bash
mkdir -p /tmp/colmena_e2e && set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_markdown_read.json > /tmp/colmena_e2e/gsheets_md_read.sse 2>&1; echo "exit: $?"
cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_compare_inline_vs_sheet.json > /tmp/colmena_e2e/gsheets_compare.sse 2>&1; echo "exit: $?"
```
Verify in the SSE: read returns `markdown` + `dimensions` (no Gemini errors); the comparison run loads an inline binding + a sheet binding and `output` contains the deterministic mismatch result. Report a friendly summary.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/gsheets_markdown_read.json tests/graphs/agents/gsheets_compare_inline_vs_sheet.json
git commit -m "test(gsheets): E2E graphs for markdown read + inline-vs-sheet comparison"
```

### Task 9: pre-push verification

- [ ] **Step 1:** `cargo test --verbose 2>&1 | tail -15` — CI parity (unit + integration + doctests), per project convention before any push.
- [ ] **Step 2:** Confirm `gsheets_run_python` existing tests (`dispatch_runs_pandas_over_two_bindings_in_parallel`, the update-in-place test) still pass — i.e. sheet-only configs are unbroken.
- [ ] **Step 3:** No commit (verification). Fixes from failures go under their own task.

---

## Self-Review (against the spec)

- **Spec coverage:** Part A (polymorphic bindings) → Tasks 4–6; Part B (markdown read + dimensions + whole-sheet) → Tasks 1–3; inline data shapes (records + 2-D) → Task 5 Step 5; isolation (read_range untouched) → Task 2 (dispatcher-only) + Task 5 (inline bypasses fetch); descriptions → Tasks 3, 6; docs → Task 7; tests + E2E (markdown, inline-vs-sheet, sheet-vs-sheet) → Tasks 1/2/4/5 + Task 8; breaking-change sweep of read tests → Task 2 Step 5. Covered.
- **Placeholder scan:** all code blocks are concrete; the only deferred specifics are real-spreadsheet ids in Task 8 (intentionally supplied at run time per the creds runbook).
- **Type/name consistency:** `values_to_markdown`/`compute_dimensions`/`md_cell` (Task 1) used in Task 2; `format` field name consistent; `GsheetsBinding` optional `spreadsheet_id`/`sheet` + `data` (Task 4) used by the fetch filter + inline loop (Task 5); `rectangle_to_records` (made pub(crate)) reused in Task 5. Consistent.
- **Decisions locked (spec §7):** `dimensions.columns` = max width incl header; numbers render via JSON repr; inline data accepts records or 2-D.
