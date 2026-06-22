# Google Sheets Cell Formatting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one LLM tool `gsheets_format_range` that applies cell formatting (text style, background, borders, alignment, number format, wrap, column width, row height) to one or more ranges of a Google Sheet in a single atomic `spreadsheets.batchUpdate`.

**Architecture:** A new `batch_update` method on the `SheetsClient` port (generic `spreadsheets.batchUpdate` POST) + a pure `gsheets/application/format.rs` module that maps a declarative `FormatSpec` to Sheets API requests (`repeatCell` with a precise `fields` mask, `updateBorders`, `updateDimensionProperties`) + a synthetic tool dispatcher that resolves sheet names to numeric ids and fans out. Purely additive — no public engine API change, no ADP impact.

**Tech Stack:** Rust (`colmena_dag_engine`), hexagonal layers, `async-trait`, `serde_json`, `schemars` (JsonSchema for tool args), `wiremock` for HTTP tests, hand-written fake `SheetsClient` impls for dispatcher tests (the repo mocks `SheetsClient` by hand, NOT via `automock`).

**Spec:** `docs/superpowers/specs/2026-06-22-gsheets-cell-formatting-design.md`

---

## File Structure

- `src/libs/colmena/src/gsheets/domain/traits.rs` — add `batch_update` to `SheetsClient`.
- `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` — impl `batch_update` (POST `{sheets_base}/{id}:batchUpdate`); wiremock test.
- `src/libs/colmena/src/gsheets/application/mod.rs` — **new** (`pub mod format;`).
- `src/libs/colmena/src/gsheets/application/format.rs` — **new**: `FormatSpec` + sub-types, `a1_to_grid_range`, `hex_to_rgb`, `build_format_requests`. Pure, unit-tested.
- `src/libs/colmena/src/gsheets/mod.rs` — add `pub mod application;`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` — `TOOL_FORMAT_RANGE`, `FormatRangeArgs`, `tool_format_range()`, `dispatch_format_range_with_client` + `dispatch_format_range`, add to `build_all_gsheets_tools`.
- `.../llm_synthetic_tools/mod.rs` — re-export dispatcher + const.
- `.../llm_synthetic_tools/toolkit_packages.rs` — add to `gsheets` package (10→11) + update count test.
- `.../infrastructure/dag_tool_executor.rs` — router: import + `is_gsheets_tool` clause + match arm.
- `src/libs/colmena/text/tools/gsheets.yaml` — LLM-facing description.
- `docs/developer_guide/41_builtin_tools_index.md` — index row.
- `src/libs/colmena/tests/gsheets_integration_test.rs` — `#[ignore]` live round-trip (create file if absent; otherwise append).
- `tests/graphs/agents/gsheets_format_range_e2e.json` — **new** E2E graph.
- `docs/developer_guide/39_gsheets.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md` — docs.

**Count contract:** the `gsheets` toolkit package goes 10 → **11** tools; the test `gsheets_package_has_all_ten_tools` in `toolkit_packages.rs` must be updated (rename + assert 11).

---

## Task 1: Domain + HTTP — `SheetsClient::batch_update`

**Files:**
- Modify: `src/libs/colmena/src/gsheets/domain/traits.rs` (end of trait)
- Modify: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` (`impl SheetsClient`, ~line 320; wiremock test in the `#[cfg(test)]` mod)
- Modify (compile-fix): the two test fakes in `gsheets_tools.rs` (`FakeClient` ~line 907, `ReadClient` ~line 1101)

- [ ] **Step 1: Add the trait method.** In `traits.rs`, append inside `trait SheetsClient` (after `batch_update_cells`):

```rust
    /// Apply N raw `spreadsheets.batchUpdate` requests in one round-trip.
    /// Used for formatting (`repeatCell` / `updateBorders` /
    /// `updateDimensionProperties`). Distinct from `batch_update_cells`,
    /// which is the values-only `values.batchUpdate`.
    async fn batch_update(
        &self,
        id: &SpreadsheetId,
        requests: Vec<serde_json::Value>,
    ) -> Result<(), SheetsError>;
```

- [ ] **Step 2: Write the failing wiremock test** in the `#[cfg(test)] mod tests` of `http_client.rs` (model on the existing `batch_update_cells_posts_value_ranges` test — find it ~line 1373 and copy its client-construction + `Mock` setup style):

```rust
#[tokio::test]
async fn batch_update_posts_requests_to_spreadsheets_batch_update() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/sheet123:batchUpdate"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "spreadsheetId": "sheet123", "replies": [{}]
        })))
        .mount(&server)
        .await;
    let client = test_client_pointing_at(&server); // same helper the sibling test uses
    let reqs = vec![serde_json::json!({"repeatCell": {"range": {"sheetId": 0}}})];
    let r = client.batch_update(&SpreadsheetId("sheet123".into()), reqs).await;
    assert!(r.is_ok(), "batch_update should succeed: {r:?}");
}
```

(Use whatever the sibling test uses to build a client whose `sheets_base` points at the wiremock server — reuse that exact helper/constructor.)

- [ ] **Step 3: Run — expect FAIL** (method not implemented yet for the HTTP client → won't compile, or test fails):

Run: `cargo test --lib gsheets::infrastructure::http_client::tests::batch_update_posts 2>&1 | tail -15`
Expected: compile error `not all trait items implemented` or test FAIL.

- [ ] **Step 4: Implement on `GoogleSheetsHttpClient`** (in `impl SheetsClient for GoogleSheetsHttpClient`, next to `batch_update_cells`):

```rust
    async fn batch_update(
        &self,
        id: &SpreadsheetId,
        requests: Vec<serde_json::Value>,
    ) -> Result<(), SheetsError> {
        if requests.is_empty() {
            return Ok(());
        }
        let body = serde_json::json!({ "requests": requests });
        let url = format!("{}/{}:batchUpdate", self.sheets_base, id.0);
        self.post_json(&url, body).await?;
        Ok(())
    }
```

(Confirm `self.sheets_base` and `self.post_json(&url, body)` are the exact field/method names — they are used by `batch_update_cells` right above. `post_json` returns `Result<Value, SheetsError>`; we discard the body.)

- [ ] **Step 5: Add the method to the two test fakes** so the crate's tests compile. In `gsheets_tools.rs`, both `impl SheetsClient for FakeClient` and `impl SheetsClient for ReadClient` need:

```rust
    async fn batch_update(
        &self,
        _id: &SpreadsheetId,
        _requests: Vec<serde_json::Value>,
    ) -> Result<(), crate::gsheets::domain::SheetsError> {
        Ok(())
    }
```

(If `FakeClient` records calls for assertions, you may instead store the requests in a field — but a no-op `Ok(())` is sufficient for existing tests. Task 3 adds a dedicated fake that records.)

- [ ] **Step 6: Run — expect PASS** + no regressions:

Run: `cargo test --lib gsheets 2>&1 | tail -8`
Expected: all PASS (including the new wiremock test).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/gsheets/domain/traits.rs src/libs/colmena/src/gsheets/infrastructure/http_client.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs
git commit -m "feat(gsheets): SheetsClient::batch_update (spreadsheets.batchUpdate)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Application — `format.rs` (FormatSpec + helpers + request builder)

**Files:**
- Create: `src/libs/colmena/src/gsheets/application/mod.rs`
- Create: `src/libs/colmena/src/gsheets/application/format.rs`
- Modify: `src/libs/colmena/src/gsheets/mod.rs` (add `pub mod application;`)
- Test: inline `#[cfg(test)]` in `format.rs`

- [ ] **Step 1: Register the module.** Create `gsheets/application/mod.rs`:

```rust
//! Application-layer use cases for Google Sheets (pure logic, no I/O).
pub mod format;
```

In `gsheets/mod.rs`, add (alphabetical with existing `pub mod domain; pub mod infrastructure;`):

```rust
pub mod application;
```

- [ ] **Step 2: Write the types + helper signatures** in `format.rs`:

```rust
//! Pure mapping from a declarative `FormatSpec` to Google Sheets
//! `spreadsheets.batchUpdate` requests. No I/O. See spec
//! `docs/superpowers/specs/2026-06-22-gsheets-cell-formatting-design.md`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// A 0-based, end-exclusive grid rectangle on one sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct GridRange {
    pub sheet_id: i64,
    pub start_row: u32,
    pub end_row: u32,
    pub start_col: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct FormatSpec {
    pub text: Option<TextFormat>,
    /// Hex `#RRGGBB` cell background.
    pub background_color: Option<String>,
    /// LEFT | CENTER | RIGHT
    pub horizontal_alignment: Option<String>,
    /// TOP | MIDDLE | BOTTOM
    pub vertical_alignment: Option<String>,
    pub number_format: Option<NumberFormat>,
    /// OVERFLOW | CLIP | WRAP
    pub wrap: Option<String>,
    pub borders: Option<Borders>,
    /// Pixel width applied to the columns spanned by the range.
    pub column_width_px: Option<u32>,
    /// Pixel height applied to the rows spanned by the range.
    pub row_height_px: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct TextFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub font_size: Option<u32>,
    pub font_family: Option<String>,
    /// Hex `#RRGGBB` text (foreground) color.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct NumberFormat {
    /// NUMBER | CURRENCY | PERCENT | DATE | TIME | DATE_TIME | TEXT | SCIENTIFIC
    pub r#type: String,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct Borders {
    pub top: Option<BorderSide>,
    pub bottom: Option<BorderSide>,
    pub left: Option<BorderSide>,
    pub right: Option<BorderSide>,
    pub inner_horizontal: Option<BorderSide>,
    pub inner_vertical: Option<BorderSide>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct BorderSide {
    /// SOLID | SOLID_MEDIUM | SOLID_THICK | DASHED | DOTTED | DOUBLE
    pub style: String,
    /// Hex `#RRGGBB`. Defaults to black when omitted.
    pub color: Option<String>,
}

/// Error returned by the pure mappers (mapped to InvalidArgs by the dispatcher).
#[derive(Debug, PartialEq)]
pub struct FormatError(pub String);
```

- [ ] **Step 3: Write failing tests for `hex_to_rgb` + `a1_to_grid_range`** (append to a `#[cfg(test)] mod tests` in `format.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_rgb_parses_with_and_without_hash() {
        assert_eq!(hex_to_rgb("#FFFFFF").unwrap(), json!({"red":1.0,"green":1.0,"blue":1.0}));
        let black = hex_to_rgb("000000").unwrap();
        assert_eq!(black, json!({"red":0.0,"green":0.0,"blue":0.0}));
        assert!(hex_to_rgb("#ZZZ").is_err());
        assert!(hex_to_rgb("#FFF").is_err()); // only 6-digit supported
    }

    #[test]
    fn a1_single_cell_and_range_and_full_column() {
        assert_eq!(a1_to_grid_range(7, "A1").unwrap(),
            GridRange { sheet_id: 7, start_row: 0, end_row: 1, start_col: 0, end_col: 1 });
        assert_eq!(a1_to_grid_range(7, "A1:D1").unwrap(),
            GridRange { sheet_id: 7, start_row: 0, end_row: 1, start_col: 0, end_col: 4 });
        // Full column "B:B" → columns bounded, rows unbounded (0..0 sentinel = no row bounds)
        let col = a1_to_grid_range(7, "B:B").unwrap();
        assert_eq!((col.start_col, col.end_col), (1, 2));
        assert!(a1_to_grid_range(7, "").is_err());
    }
}
```

- [ ] **Step 4: Run — expect FAIL** (functions undefined):

Run: `cargo test --lib gsheets::application::format 2>&1 | tail -15`
Expected: FAIL (`cannot find function`).

- [ ] **Step 5: Implement `hex_to_rgb` + `a1_to_grid_range`:**

```rust
/// Parse `#RRGGBB` (or `RRGGBB`) → Sheets `RgbColor` JSON with 0.0–1.0 floats.
pub fn hex_to_rgb(hex: &str) -> Result<Value, FormatError> {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(FormatError(format!("invalid hex color: {hex:?} (expected #RRGGBB)")));
    }
    let comp = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
    Ok(json!({ "red": comp(0), "green": comp(2), "blue": comp(4) }))
}

/// Parse an A1 range against `sheet_id` → 0-based, end-exclusive `GridRange`.
/// Supports "A1", "A1:D5", whole columns "B:D", and whole rows "2:5".
/// For whole-column ranges, row bounds are 0..0 (caller omits row indices);
/// for whole-row ranges, col bounds are 0..0.
pub fn a1_to_grid_range(sheet_id: i64, a1: &str) -> Result<GridRange, FormatError> {
    if a1.trim().is_empty() {
        return Err(FormatError("empty A1 range".into()));
    }
    let (start, end) = match a1.split_once(':') {
        Some((s, e)) => (s, e),
        None => (a1, a1),
    };
    let (sc, sr) = parse_a1_cell(start)?;
    let (ec, er) = parse_a1_cell(end)?;
    // Column index: present unless the token had no letters (pure-row ref).
    let (start_col, end_col) = match (sc, ec) {
        (Some(a), Some(b)) => (a.min(b), a.max(b) + 1),
        _ => (0, 0),
    };
    let (start_row, end_row) = match (sr, er) {
        (Some(a), Some(b)) => (a.min(b), a.max(b) + 1),
        _ => (0, 0),
    };
    Ok(GridRange { sheet_id, start_row, end_row, start_col, end_col })
}

/// Parse one A1 token like "A", "1", or "B12" → (col_index?, row_index?), 0-based.
fn parse_a1_cell(tok: &str) -> Result<(Option<u32>, Option<u32>), FormatError> {
    let letters: String = tok.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = tok.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    if letters.is_empty() && digits.is_empty() {
        return Err(FormatError(format!("invalid A1 token: {tok:?}")));
    }
    let col = if letters.is_empty() {
        None
    } else {
        let mut n: u32 = 0;
        for c in letters.chars() {
            n = n * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
        }
        Some(n - 1)
    };
    let row = if digits.is_empty() {
        None
    } else {
        Some(digits.parse::<u32>().map_err(|_| FormatError(format!("invalid row in {tok:?}")))? - 1)
    };
    Ok((col, row))
}
```

- [ ] **Step 6: Run — expect PASS:**

Run: `cargo test --lib gsheets::application::format 2>&1 | tail -10`
Expected: 2 tests PASS.

- [ ] **Step 7: Write failing tests for `build_format_requests`:**

```rust
    fn gr() -> GridRange { GridRange { sheet_id: 0, start_row: 0, end_row: 1, start_col: 0, end_col: 4 } }

    #[test]
    fn background_and_bold_produce_one_repeat_cell_with_composite_mask() {
        let spec = FormatSpec {
            background_color: Some("#000000".into()),
            text: Some(TextFormat { bold: Some(true), ..Default::default() }),
            ..Default::default()
        };
        let reqs = build_format_requests(&gr(), &spec).unwrap();
        assert_eq!(reqs.len(), 1);
        let rc = &reqs[0]["repeatCell"];
        assert_eq!(rc["cell"]["userEnteredFormat"]["textFormat"]["bold"], true);
        assert!(rc["cell"]["userEnteredFormat"]["backgroundColor"].is_object());
        let fields = rc["fields"].as_str().unwrap();
        assert!(fields.contains("backgroundColor"));
        assert!(fields.contains("textFormat.bold"));
    }

    #[test]
    fn borders_and_column_width_produce_separate_requests() {
        let spec = FormatSpec {
            borders: Some(Borders { top: Some(BorderSide { style: "SOLID".into(), color: None }), ..Default::default() }),
            column_width_px: Some(120),
            ..Default::default()
        };
        let reqs = build_format_requests(&gr(), &spec).unwrap();
        assert!(reqs.iter().any(|r| r.get("updateBorders").is_some()));
        let dim = reqs.iter().find(|r| r.get("updateDimensionProperties").is_some()).unwrap();
        assert_eq!(dim["updateDimensionProperties"]["range"]["dimension"], "COLUMNS");
        assert_eq!(dim["updateDimensionProperties"]["properties"]["pixelSize"], 120);
    }

    #[test]
    fn empty_spec_is_error() {
        assert!(build_format_requests(&gr(), &FormatSpec::default()).is_err());
    }
```

- [ ] **Step 8: Run — expect FAIL** (`build_format_requests` undefined).

Run: `cargo test --lib gsheets::application::format 2>&1 | tail -15`
Expected: FAIL.

- [ ] **Step 9: Implement `build_format_requests` + the GridRange→JSON helper:**

```rust
fn grid_json(g: &GridRange) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("sheetId".into(), json!(g.sheet_id));
    if g.end_row > g.start_row {
        m.insert("startRowIndex".into(), json!(g.start_row));
        m.insert("endRowIndex".into(), json!(g.end_row));
    }
    if g.end_col > g.start_col {
        m.insert("startColumnIndex".into(), json!(g.start_col));
        m.insert("endColumnIndex".into(), json!(g.end_col));
    }
    Value::Object(m)
}

fn border_json(side: &BorderSide) -> Result<Value, FormatError> {
    let color = match &side.color {
        Some(h) => hex_to_rgb(h)?,
        None => json!({ "red": 0.0, "green": 0.0, "blue": 0.0 }),
    };
    Ok(json!({ "style": side.style, "color": color }))
}

/// Map a `FormatSpec` over `grid` to Sheets batchUpdate requests.
/// Errors if the spec carries no attributes (fail loud, not silent no-op).
pub fn build_format_requests(grid: &GridRange, spec: &FormatSpec) -> Result<Vec<Value>, FormatError> {
    let mut out = Vec::new();

    // 1. repeatCell for text/background/alignment/number/wrap.
    let mut fmt = serde_json::Map::new();
    let mut fields: Vec<String> = Vec::new();

    if let Some(bg) = &spec.background_color {
        fmt.insert("backgroundColor".into(), hex_to_rgb(bg)?);
        fields.push("backgroundColor".into());
    }
    if let Some(t) = &spec.text {
        let mut tf = serde_json::Map::new();
        if let Some(b) = t.bold { tf.insert("bold".into(), json!(b)); fields.push("textFormat.bold".into()); }
        if let Some(b) = t.italic { tf.insert("italic".into(), json!(b)); fields.push("textFormat.italic".into()); }
        if let Some(b) = t.underline { tf.insert("underline".into(), json!(b)); fields.push("textFormat.underline".into()); }
        if let Some(b) = t.strikethrough { tf.insert("strikethrough".into(), json!(b)); fields.push("textFormat.strikethrough".into()); }
        if let Some(s) = t.font_size { tf.insert("fontSize".into(), json!(s)); fields.push("textFormat.fontSize".into()); }
        if let Some(f) = &t.font_family { tf.insert("fontFamily".into(), json!(f)); fields.push("textFormat.fontFamily".into()); }
        if let Some(c) = &t.color { tf.insert("foregroundColor".into(), hex_to_rgb(c)?); fields.push("textFormat.foregroundColor".into()); }
        if !tf.is_empty() { fmt.insert("textFormat".into(), Value::Object(tf)); }
    }
    if let Some(a) = &spec.horizontal_alignment {
        fmt.insert("horizontalAlignment".into(), json!(a));
        fields.push("horizontalAlignment".into());
    }
    if let Some(a) = &spec.vertical_alignment {
        fmt.insert("verticalAlignment".into(), json!(a));
        fields.push("verticalAlignment".into());
    }
    if let Some(nf) = &spec.number_format {
        let mut n = serde_json::Map::new();
        n.insert("type".into(), json!(nf.r#type));
        if let Some(p) = &nf.pattern { n.insert("pattern".into(), json!(p)); }
        fmt.insert("numberFormat".into(), Value::Object(n));
        fields.push("numberFormat".into());
    }
    if let Some(w) = &spec.wrap {
        fmt.insert("wrapStrategy".into(), json!(w));
        fields.push("wrapStrategy".into());
    }
    if !fmt.is_empty() {
        out.push(json!({ "repeatCell": {
            "range": grid_json(grid),
            "cell": { "userEnteredFormat": Value::Object(fmt) },
            "fields": format!("userEnteredFormat({})", fields.join(",")),
        }}));
    }

    // 2. updateBorders.
    if let Some(b) = &spec.borders {
        let mut ub = serde_json::Map::new();
        ub.insert("range".into(), grid_json(grid));
        for (key, side) in [
            ("top", &b.top), ("bottom", &b.bottom), ("left", &b.left), ("right", &b.right),
            ("innerHorizontal", &b.inner_horizontal), ("innerVertical", &b.inner_vertical),
        ] {
            if let Some(s) = side { ub.insert(key.into(), border_json(s)?); }
        }
        if ub.len() > 1 { // more than just "range"
            out.push(json!({ "updateBorders": Value::Object(ub) }));
        }
    }

    // 3. updateDimensionProperties (column width / row height).
    if let Some(px) = spec.column_width_px {
        out.push(json!({ "updateDimensionProperties": {
            "range": { "sheetId": grid.sheet_id, "dimension": "COLUMNS",
                       "startIndex": grid.start_col, "endIndex": grid.end_col.max(grid.start_col + 1) },
            "properties": { "pixelSize": px },
            "fields": "pixelSize",
        }}));
    }
    if let Some(px) = spec.row_height_px {
        out.push(json!({ "updateDimensionProperties": {
            "range": { "sheetId": grid.sheet_id, "dimension": "ROWS",
                       "startIndex": grid.start_row, "endIndex": grid.end_row.max(grid.start_row + 1) },
            "properties": { "pixelSize": px },
            "fields": "pixelSize",
        }}));
    }

    if out.is_empty() {
        return Err(FormatError("format has no attributes".into()));
    }
    Ok(out)
}
```

- [ ] **Step 10: Run — expect PASS** (all format tests):

Run: `cargo test --lib gsheets::application::format 2>&1 | tail -12`
Expected: all PASS.

- [ ] **Step 11: Commit**

```bash
git add src/libs/colmena/src/gsheets/application src/libs/colmena/src/gsheets/mod.rs
git commit -m "feat(gsheets): format.rs — FormatSpec → batchUpdate request builder

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Synthetic tool — `gsheets_format_range`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`

- [ ] **Step 1: Study the pattern.** READ in `gsheets_tools.rs`: the `TOOL_SET_RANGE` const (~line 45), `SetRangeArgs` (~line 129), `tool_set_range()` (~line 403), `dispatch_set_range_with_client` / `dispatch_set_range` (~line 760-797), `build_all_gsheets_tools()`, `build_client()`, and `error_to_json`. Match these exactly. Also note how a dispatcher calls `client.list_sheets(...)` (grep `list_sheets`) — you need it to resolve sheet name → numeric `sheet_id`.

- [ ] **Step 2: Add the const + Args.** Add near the other consts (~line 45):

```rust
pub const TOOL_FORMAT_RANGE: &str = "gsheets_format_range";
```

Add the Args struct near `SetRangeArgs` (~line 129). `FormatSpec` is reused from the application module:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormatRangeArgs {
    pub spreadsheet_id: String,
    pub ops: Vec<FormatOp>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormatOp {
    /// Sheet/tab name (or numeric sheetId as a string).
    pub sheet: String,
    /// A1 range, e.g. "A1:D1" (no sheet prefix).
    pub range: String,
    pub format: crate::gsheets::application::format::FormatSpec,
}
```

Add the import at the top of the file if needed: `use crate::gsheets::application::format::{build_format_requests, a1_to_grid_range};` (and `FormatSpec`/`FormatOp` are referenced via full path above — or import them too).

- [ ] **Step 3: Add the tool definition** near `tool_set_range()` (~line 403):

```rust
pub fn tool_format_range() -> ToolDefinition {
    build_synthetic_tool_with_summary::<FormatRangeArgs>(
        TOOL_FORMAT_RANGE,
        text::tool_description(TOOL_FORMAT_RANGE),
        text::tool_summary(TOOL_FORMAT_RANGE),
    )
}
```

(Match the EXACT builder signature used by `tool_set_range()` — copy its form verbatim; the helper name may differ, use what the file uses.)

Add `tool_format_range()` to the `build_all_gsheets_tools()` vec (next to `tool_set_range()`).

- [ ] **Step 4: Write the failing dispatcher test** (in the `#[cfg(test)] mod tests` of `gsheets_tools.rs`; add a dedicated fake that records the batch_update requests + answers list_sheets):

```rust
    struct FormatFake { sheets: Vec<SheetMeta>, captured: std::sync::Mutex<Vec<serde_json::Value>> }
    #[async_trait::async_trait]
    impl SheetsClient for FormatFake {
        async fn list_sheets(&self, _id: &SpreadsheetId) -> Result<Vec<SheetMeta>, crate::gsheets::domain::SheetsError> {
            Ok(self.sheets.clone())
        }
        async fn batch_update(&self, _id: &SpreadsheetId, requests: Vec<serde_json::Value>) -> Result<(), crate::gsheets::domain::SheetsError> {
            self.captured.lock().unwrap().extend(requests);
            Ok(())
        }
        // ... all other SheetsClient methods: unimplemented!() or minimal Ok(...) stubs ...
    }

    #[tokio::test]
    async fn format_range_resolves_sheet_and_builds_requests() {
        let fake = FormatFake {
            sheets: vec![SheetMeta { sheet_id: crate::gsheets::domain::SheetId(42), title: "Hoja1".into(), /* + other fields */ }],
            captured: std::sync::Mutex::new(vec![]),
        };
        let args = serde_json::json!({
            "spreadsheet_id": "S1",
            "ops": [{ "sheet": "Hoja1", "range": "A1:D1",
                      "format": { "text": {"bold": true}, "background_color": "#000000" } }]
        });
        let out = dispatch_format_range_with_client(args, &fake).await;
        assert_eq!(out["ok"], true);
        let captured = fake.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0]["repeatCell"]["range"]["sheetId"], 42);
    }
```

(Fill in `SheetMeta`'s other fields from its definition in `gsheets/domain/types.rs`. Stub the unused trait methods with `unimplemented!()`.)

- [ ] **Step 5: Run — expect FAIL** (`dispatch_format_range_with_client` undefined):

Run: `cargo test --lib gsheets_tools::tests::format_range 2>&1 | tail -15`
Expected: FAIL.

- [ ] **Step 6: Implement the dispatchers:**

```rust
pub async fn dispatch_format_range_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: FormatRangeArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    if parsed.ops.is_empty() {
        return serde_json::json!({"error": "invalid_args", "message": "ops must not be empty"});
    }
    let id = SpreadsheetId(parsed.spreadsheet_id);
    // Resolve sheet names → numeric ids once.
    let sheets = match client.list_sheets(&id).await {
        Ok(s) => s,
        Err(e) => return error_to_json(e),
    };
    let resolve = |name: &str| -> Option<i64> {
        // Accept numeric sheetId or a title.
        if let Ok(n) = name.parse::<i64>() {
            return Some(n);
        }
        sheets.iter().find(|s| s.title == name).map(|s| s.sheet_id.0)
    };

    let mut requests: Vec<serde_json::Value> = Vec::new();
    for (i, op) in parsed.ops.iter().enumerate() {
        let sheet_id = match resolve(&op.sheet) {
            Some(id) => id,
            None => return serde_json::json!({"error": "invalid_args",
                "message": format!("op {i}: sheet {:?} not found; available: {:?}",
                    op.sheet, sheets.iter().map(|s| &s.title).collect::<Vec<_>>())}),
        };
        let grid = match a1_to_grid_range(sheet_id, &op.range) {
            Ok(g) => g,
            Err(e) => return serde_json::json!({"error": "invalid_args", "message": format!("op {i}: {}", e.0)}),
        };
        match build_format_requests(&grid, &op.format) {
            Ok(mut r) => requests.append(&mut r),
            Err(e) => return serde_json::json!({"error": "invalid_args", "message": format!("op {i}: {}", e.0)}),
        }
    }
    let n_requests = requests.len();
    match client.batch_update(&id, requests).await {
        Ok(()) => serde_json::json!({ "ok": true, "ops_applied": parsed.ops.len(), "requests_sent": n_requests }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_format_range(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_format_range_with_client(args, &client).await
}
```

(`a1_to_grid_range` returns `GridRange` whose `sheet_id` I set here; confirm the import. `SheetId` is a tuple struct `SheetId(i64)` — `.0` gets the int; confirm from types.rs.)

- [ ] **Step 7: Run — expect PASS:**

Run: `cargo test --lib gsheets_tools::tests::format_range 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 8: Build the whole lib** (catch unused imports under deny-warnings):

Run: `cargo build --lib 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs
git commit -m "feat(gsheets): gsheets_format_range synthetic tool (def + dispatcher)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Wiring — re-export, toolkit, router, yaml, index

**Files:**
- Modify: `.../llm_synthetic_tools/mod.rs`
- Modify: `.../llm_synthetic_tools/toolkit_packages.rs`
- Modify: `.../infrastructure/dag_tool_executor.rs`
- Modify: `src/libs/colmena/text/tools/gsheets.yaml`
- Modify: `docs/developer_guide/41_builtin_tools_index.md`

- [ ] **Step 1: Re-export in `mod.rs`.** In the `gsheets_tools::{…}` re-export block, add (matching the existing `dispatch_X as dispatch_gsheets_X` / `TOOL_X as GSHEETS_X_TOOL` aliasing):

```rust
    dispatch_format_range as dispatch_gsheets_format_range,
    // ...
    TOOL_FORMAT_RANGE as GSHEETS_FORMAT_RANGE_TOOL,
```

(Grep `dispatch_gsheets_set_range` in mod.rs to find the exact block and aliasing convention; also check whether tool builders are re-exported as `gsheets_tool_*` and add `tool_format_range as gsheets_tool_format_range` if so.)

- [ ] **Step 2: Toolkit package.** In `toolkit_packages.rs`, add `"gsheets_format_range"` to the `gsheets` package `tools` array (after `gsheets_set_range`), bump the package description count if it states one, and update the count test `gsheets_package_has_all_ten_tools` (~line 130): rename to `..._eleven_tools` and assert the new length (read the test; it likely does `assert_eq!(pkg.tools.len(), 10)` → make it 11, and may list exact names — add `gsheets_format_range`).

- [ ] **Step 3: Router** in `dag_tool_executor.rs`:
  - Import `dispatch_gsheets_format_range` and `GSHEETS_FORMAT_RANGE_TOOL` in the gsheets `use …::{…}` block (~line 1218-1234).
  - Add `|| n == GSHEETS_FORMAT_RANGE_TOOL` to the `is_gsheets_tool` matches! (~line 1237).
  - Add the match arm (gsheets dispatchers take `(args)` only — no session_id):

```rust
                    n if n == GSHEETS_FORMAT_RANGE_TOOL => {
                        dispatch_gsheets_format_range(args).await
                    }
```

(Find the existing gsheets match block — model the arm on `dispatch_gsheets_set_range(args).await`.)

- [ ] **Step 4: YAML description** in `text/tools/gsheets.yaml` (match the entry shape of `gsheets_set_range`; grep it). Add:

```yaml
gsheets_format_range:
  summary: Apply cell formatting (style, background, borders, alignment, number format, column width) to one or more ranges
  description: |
    Aplica FORMATO (no valores) a una o más zonas de un Google Sheet en un solo
    batchUpdate atómico. Pasá `ops: [{sheet, range, format}]`. `sheet` = nombre
    del tab (o sheetId). `range` = A1 sin prefijo de tab (ej "A1:D1", "B:B").
    `format` (todos los campos opcionales):
      - text: { bold, italic, underline, strikethrough, font_size, font_family, color }  (color hex "#RRGGBB")
      - background_color: "#RRGGBB"
      - horizontal_alignment: LEFT|CENTER|RIGHT ; vertical_alignment: TOP|MIDDLE|BOTTOM
      - number_format: { type: NUMBER|CURRENCY|PERCENT|DATE|TIME|DATE_TIME|TEXT|SCIENTIFIC, pattern? }
      - wrap: OVERFLOW|CLIP|WRAP
      - borders: { top|bottom|left|right|inner_horizontal|inner_vertical: { style: SOLID|SOLID_MEDIUM|SOLID_THICK|DASHED|DOTTED|DOUBLE, color? } }
      - column_width_px / row_height_px (aplican a las columnas/filas del range)
    Es NO-DESTRUCTIVO: no toca valores ni fórmulas, y solo cambia los atributos
    que pasás (setear fondo no borra el bold previo). Para escribir VALORES o
    FÓRMULAS usá gsheets_set_cell / gsheets_set_range (no este tool).
    Ejemplo header: { sheet:"Hoja1", range:"A1:D1", format:{ text:{bold:true,color:"#FFFFFF"}, background_color:"#4472C4", horizontal_alignment:"CENTER" } }
```

- [ ] **Step 5: Index doc.** In `docs/developer_guide/41_builtin_tools_index.md`, find the gsheets section, bump its count (e.g. `## gsheets (N tools)` → N+1), and add a row:

```markdown
| `gsheets_format_range` | Apply cell formatting (style, background, borders, alignment, number format, column/row size) to one or more ranges in one atomic batchUpdate. |
```

(If there's a `gsheets` toolkit-count line like "expands to all N gsheets_* tools", bump it too.)

- [ ] **Step 6: Build + run the contract/text tests:**

Run: `cargo test --lib 2>&1 | tail -15`
Expected: all PASS — especially `toolkit` (count test) and `text` (yaml key present) and `gsheets`. If a test asserts the old count, update it.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/text/tools/gsheets.yaml \
        docs/developer_guide/41_builtin_tools_index.md
git commit -m "feat(gsheets): wire gsheets_format_range (mod, toolkit 10->11, router, yaml, index)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Integration `#[ignore]` live test + E2E graph

**Files:**
- Create or modify: `src/libs/colmena/tests/gsheets_integration_test.rs`
- Create: `tests/graphs/agents/gsheets_format_range_e2e.json`

- [ ] **Step 1: Find the live-test harness.** Check whether `src/libs/colmena/tests/gsheets_integration_test.rs` exists. If yes, READ it and reuse its client construction (`GoogleSheetsHttpClient::from_config(...)` or the helper it uses) + env guard + `#[ignore]` attribute style. If no such file, model it on `src/libs/colmena/tests/gdocs_integration_test.rs` (same auth env, OAuth from env). Use the SAME `#[ignore = "requires ... — run with \`cargo test -- --ignored\`"]` convention.

- [ ] **Step 2: Add the `#[ignore]` test** `format_range_live_roundtrip`:

```rust
#[tokio::test]
#[ignore = "requires Google OAuth creds — run with `cargo test -- --ignored`"]
async fn format_range_live_roundtrip() {
    // 1. build client (same as sibling live tests)
    // 2. create_spreadsheet("colmena fmt IT")
    // 3. set_range Hoja1!A1: [["Producto","Stock"],["Lapices","10"],["Cuadernos","5"]]
    // 4. resolve sheet_id via list_sheets
    // 5. build requests for: header A1:B1 (bold + background #4472C4 + center),
    //    block A1:B3 (borders all SOLID), column A width 160px — via
    //    gsheets::application::format::{a1_to_grid_range, build_format_requests}
    // 6. client.batch_update(&id, requests).await — assert Ok
    // 7. cleanup: (Drive delete via the helper the sibling tests use, or leave —
    //    match what gsheets live tests already do for cleanup)
}
```

(Fill in client construction + cleanup verbatim from the sibling harness; do not invent a new auth path.)

- [ ] **Step 3: Write the E2E graph** `tests/graphs/agents/gsheets_format_range_e2e.json` (nodes-as-map; default LLM stack; placeholder `${GEMINI_API_KEY}`/`${DATABASE_URL}`; `enabled_tools: ["gsheets"]`). Model the shape on an existing gsheets E2E graph (e.g. `tests/graphs/agents/gsheets_collision_envelope_e2e.json`):

```json
{
  "_comment": "E2E (manual, live): agent creates a spreadsheet, writes a small table, then formats the header (bold+background) and a totals row in one turn. Needs GEMINI_API_KEY + DATABASE_URL + OAuth creds in-memory + COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID (gsheets create may also need a folder) + --agent-session-id.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/gsheets-format",
        "method": "POST",
        "test_payload": {
          "prompt": "Create a spreadsheet titled 'Inventario demo'. Write this table starting at A1 on the first sheet: header [Producto, Stock], then rows [Lapices, 10] and [Cuadernos, 5]. Then format the header row A1:B1 in bold with a blue background (#4472C4) and white centered text. Report the spreadsheet URL and the verbatim result of gsheets_format_range."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "stream": false,
        "max_iterations": 12,
        "connection_url": "${DATABASE_URL}",
        "system_message": "You manage Google Sheets. Create the spreadsheet, write values with gsheets_set_range, then style with gsheets_format_range (formatting is separate from values). Report tool results verbatim including the URL.",
        "enabled_tools": ["gsheets"]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent", "to": "log" }
  ]
}
```

Validate: `python3 -c "import json;json.load(open('tests/graphs/agents/gsheets_format_range_e2e.json'));print('ok')"`.

- [ ] **Step 4: Compile-check (do NOT run live):**

Run: `cargo test --no-run 2>&1 | tail -10`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/tests/gsheets_integration_test.rs tests/graphs/agents/gsheets_format_range_e2e.json
git commit -m "test(gsheets): live #[ignore] format roundtrip + E2E LLM graph

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Docs + sweep + live verify + push (controller-run)

**Files:**
- Modify: `docs/developer_guide/39_gsheets.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Dev guide §39.** Add a "Cell formatting" subsection documenting `gsheets_format_range`: the `ops`/`format` shape, that it's non-destructive and separate from value writes, the attribute list, and an example. Bump any tool-count in the file header if it states one.

- [ ] **Step 2: CHANGELOG.** Add the next section number (check the last `## N.` in `docs/CHANGELOG_2026-06.md` and use N+1; as of this writing the last was §46 → use §47). Spanish. Cover: `gsheets_format_range` (1 tool, ops list → atomic batchUpdate), attributes, fields-mask partial updates, new `SheetsClient::batch_update`, gsheets toolkit 10→11, additive/no ADP impact. Reference spec + plan + E2E graph.

- [ ] **Step 3: BACKLOG.** Mark the `Cell formatting` item (Subsystem E v1.1, ~line 682) `[x]` SHIPPED with date + §47 pointer, and the prioritization-table row if present.

- [ ] **Step 4: Commit docs**

```bash
git add docs/developer_guide/39_gsheets.md docs/CHANGELOG_2026-06.md docs/BACKLOG.md
git commit -m "docs(gsheets): cell formatting — dev guide, CHANGELOG, BACKLOG shipped

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Step 5: Full sweep (CI parity — clippy is a SEPARATE gate, run it!):**

```bash
cargo fmt --check && cargo clippy --all-targets 2>&1 | grep -E "^error|^warning:" ; cargo test --verbose 2>&1 | tail -25
```
Expected: fmt clean, clippy 0 errors/warnings, tests exit 0. (Per memory: `cargo clippy --all-targets` catches `needless_lifetimes`/`type_complexity` that `cargo test` does not.)

- [ ] **Step 6: ADP additive sweep.** Confirm additive-only: `git diff <base>..HEAD -- src/libs/colmena/src/lib.rs` and grep for `EngineConfig`/`ColmenaEngine`/exported-trait signature changes — expected none (only a new `SheetsClient` method, which ADP doesn't implement). Nothing to change in ADP.

- [ ] **Step 7: Live E2E (CONTROLLER — secrets in-memory).** Per memory `feedback_always_real_e2e` + `ref_local_gsheets_e2e_runbook`: inject OAuth creds from Secret Manager in-memory (no echo/file/commit), build the release binary, run `tests/graphs/agents/gsheets_format_range_e2e.json` with `--agent-session-id`, save SSE to `/tmp/colmena_e2e/gsheets_format_range.sse`, present a friendly report. Also run `source .env && cargo test -- --ignored format_range_live_roundtrip`. Note the pandas `PYTHONPATH` gotcha only matters if a python tool runs; this graph doesn't use run_python.

- [ ] **Step 8: Push + open PR + watch CI**

```bash
git push -u origin <branch>
gh pr create --base develop --title "feat(gsheets): cell formatting (gsheets_format_range)" --body "..."
gh pr checks <n> --watch
```

---

## Self-Review Notes (author)

- **Spec coverage:** §1 tool shape → Task 3 (FormatRangeArgs/FormatOp) + Task 2 (FormatSpec). §2 fan-out (repeatCell+fields mask, updateBorders, updateDimensionProperties) → Task 2 `build_format_requests`. §3 new trait method → Task 1; pure builder module → Task 2; dispatcher w/ sheet-id resolution → Task 3; no co-edit guard → reflected (dispatcher takes no session). §4 wiring (toolkit 10→11, yaml, router, index) → Task 4. §5 edge cases (empty ops, sheet not found, bad A1, bad hex, empty format) → Task 3 dispatcher + Task 2 `build_format_requests`/`hex_to_rgb`/`a1_to_grid_range` (all tested). §6 testing (unit/integration/E2E) → Tasks 2,3 (unit), 5 (integration+E2E). §7 non-goals → not implemented (correct). Cross-repo additive → Task 6 step 6.
- **Type consistency:** `FormatSpec`/`TextFormat`/`NumberFormat`/`Borders`/`BorderSide`/`GridRange`/`FormatError` defined in Task 2, reused by name in Task 3. `build_format_requests(&GridRange, &FormatSpec) -> Result<Vec<Value>, FormatError>` and `a1_to_grid_range(i64, &str) -> Result<GridRange, FormatError>` and `hex_to_rgb(&str) -> Result<Value, FormatError>` signatures stable across tasks. `SheetsClient::batch_update(&self, &SpreadsheetId, Vec<Value>) -> Result<(), SheetsError>` identical in Task 1 (def), Task 3 (call), Task 1 fakes + Task 3 fake (impl). Toolkit 10→11 applied in Task 4 only.
- **Verify-during-impl flags (use existing names, not placeholders):** exact `build_synthetic_tool_with_summary` helper name (Task 3 step 3); `SheetMeta` full field set for the test fake (Task 3 step 4); `SheetId` tuple `.0` accessor; `build_client()` + `error_to_json` names; the mod.rs re-export aliasing convention. All are "match the sibling code" checks.
