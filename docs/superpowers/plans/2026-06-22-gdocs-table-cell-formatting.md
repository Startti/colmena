# gdocs_format_table — Table-Cell Formatting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `gdocs_format_table` synthetic tool that applies presentable formatting (cell background, borders, vertical alignment, text style, horizontal alignment) to rectangular cell ranges inside Google Docs tables, in one atomic multi-op `batchUpdate`.

**Architecture:** A new pure builder + use case in `gdocs/application/table_format.rs` reuses the table addressing (`find_table`/`find_cell`/`cell_location` from `table.rs`, promoted to `pub(crate)`), the non-blocking co-edit guard, `apply_and_finalize`, and the already-parsed `CellSnapshot.content_start_index/end_index`. Three Docs API request types are emitted per op: `updateTableCellStyle` (cell-level, via `tableRange`), `updateTextStyle` (per cell), `updateParagraphStyle` (per cell). The tool mirrors `gsheets_format_range`'s multi-op `ops:[{...}]` shape. An always-on "presentable by default" nudge lives in the tool description (no skill in v1).

**Tech Stack:** Rust (`colmena_dag_engine`), `serde_json` request building, `schemars`/`serde` for tool Args, the gdocs hexagonal layers (domain `RgbColor`/`CellSnapshot`, application use cases, infrastructure synthetic tools), YAML text registry.

**Spec:** `docs/superpowers/specs/2026-06-22-gdocs-table-cell-formatting-design.md`

---

## File Structure

- `src/libs/colmena/src/gdocs/application/table_format.rs` — **new**. Format input types (`TableFormatOp`, `CellFormat`, `CellTextFormat`, `CellBorders`, `CellBorderSide`, `CellRange`), the pure builder `build_format_table_requests`, the `hex_to_rgb`/`rgb_to_color`/`validate_range` helpers, the `run_format_table` use case, and unit tests.
- `src/libs/colmena/src/gdocs/application/table.rs` — **modify**. Promote `find_table`, `find_cell`, `cell_location` from private to `pub(crate)` so `table_format.rs` can reuse them.
- `src/libs/colmena/src/gdocs/application/mod.rs` — **modify**. Add `pub mod table_format;`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs` — **modify**. `TOOL_FORMAT_TABLE` const, `FormatTableArgs`/`FormatTableOp` Args, `tool_format_table()` ToolDefinition, `dispatch_format_table()`, and add `tool_format_table()` to `build_all_gdocs_tools()`.
- `src/libs/colmena/text/tools/gdocs.yaml` — **modify**. `gdocs_format_table` description (with presentable nudge) + summary.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — **modify**. Re-export the dispatcher + tool builder + TOOL const aliases.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — **modify**. Import, the "is gdocs tool" predicate, and the dispatch match arm.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — **modify**. `all_gdocs` array + `gdocs_entries` array, count `35 → 36`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs` — **modify**. Add `"gdocs_format_table"` to the `gdocs` and `gdocsread`? (NO — formatting is write; add to `gdocs` full only) package list(s).
- `docs/developer_guide/41_builtin_tools_index.md`, `docs/developer_guide/46_gdocs_tables.md` (or §45), `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md` — **modify**. Docs.
- `tests/graphs/agents/gdocs_format_table_e2e.json` — **new**. Live E2E graph.

**Note on the wiring trap (from §47/§46):** a tool can be routed in `dag_tool_executor.rs` yet invisible because it is missing from `llm.rs`'s `all_gdocs`/`gdocs_entries` arrays — those are the actual exposure path. Task 4 covers every sync point explicitly, and a count test (`build_all_gdocs_tools().len()`) catches drift.

---

## Task 1: `table_format.rs` — types + pure builder + unit tests

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/table_format.rs`
- Modify: `src/libs/colmena/src/gdocs/application/table.rs` (promote 3 helpers to `pub(crate)`)
- Modify: `src/libs/colmena/src/gdocs/application/mod.rs` (add `pub mod table_format;`)

- [ ] **Step 1: Promote the table helpers.** In `table.rs`, change the three helper signatures from private to `pub(crate)`:
  - `fn cell_location(` → `pub(crate) fn cell_location(` (line ~43)
  - `fn find_table<'a>(` → `pub(crate) fn find_table<'a>(` (line ~57)
  - `fn find_cell(` → `pub(crate) fn find_cell(` (line ~87)

  Run: `cargo build --lib 2>&1 | tail -5` — Expected: clean (no other change needed; promoting visibility is non-breaking).

- [ ] **Step 2: Register the module.** In `src/libs/colmena/src/gdocs/application/mod.rs`, add `pub mod table_format;` alongside the existing `pub mod table;` line. Run: `cargo build --lib 2>&1 | tail -5` — Expected: clean (empty module compiles; if Rust complains the file doesn't exist yet, create an empty `table_format.rs` first with just `//! Table cell formatting.`).

- [ ] **Step 3: Write the failing builder + helper tests.** Create `src/libs/colmena/src/gdocs/application/table_format.rs` with the types and a `#[cfg(test)]` module FIRST (TDD). Paste this whole file — the `build_format_table_requests` / helper bodies are stubs that `todo!()` so tests fail to link/compile meaningfully; you will fill them in Step 5:

```rust
//! Table-cell formatting use case: apply presentable formatting (cell
//! background, borders, vertical alignment, text style, horizontal alignment)
//! to rectangular cell ranges inside a Google Docs table, in one atomic
//! multi-op `batchUpdate`. Mirrors `gsheets_format_range`'s `ops:[...]` shape.
//! See spec `docs/superpowers/specs/2026-06-22-gdocs-table-cell-formatting-design.md`.

use crate::gdocs::application::co_edit_guard::{run_guard_non_blocking, GuardContext};
use crate::gdocs::application::insert::apply_and_finalize;
use crate::gdocs::application::table::{cell_location, find_cell, find_table};
use crate::gdocs::application::util;
use crate::gdocs::domain::{
    ChangeKind, DocsError, DocumentId, EditResult, RgbColor, TabId, TableSnapshot,
};
use schemars::JsonSchema;
use serde::Deserialize;

/// A 0-based, end-EXCLUSIVE rectangle of cells within one table.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CellRange {
    pub row_start: u32,
    pub row_end: u32,
    pub col_start: u32,
    pub col_end: u32,
}

/// One formatting op: which table, which cell rectangle, what format.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TableFormatOp {
    pub table_index: u32,
    pub cell_range: CellRange,
    pub format: CellFormat,
}

/// All formatting attributes are optional; only set ones are emitted.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellFormat {
    /// Cell background, hex `#RRGGBB`.
    pub background_color: Option<String>,
    /// Vertical alignment of cell content: TOP | MIDDLE | BOTTOM.
    pub vertical_alignment: Option<String>,
    pub borders: Option<CellBorders>,
    pub text: Option<CellTextFormat>,
    /// Paragraph alignment: LEFT | CENTER | RIGHT | JUSTIFIED.
    pub horizontal_alignment: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellTextFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub font_size: Option<f32>,
    /// Text color, hex `#RRGGBB`.
    pub color: Option<String>,
}

/// Each side optional; applied to every cell in the range (Docs has no
/// inner/outer border concept — every cell carries its own 4 borders).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellBorders {
    pub top: Option<CellBorderSide>,
    pub bottom: Option<CellBorderSide>,
    pub left: Option<CellBorderSide>,
    pub right: Option<CellBorderSide>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellBorderSide {
    /// SOLID | DOTTED | DASHED. Default SOLID when the side is present.
    pub style: Option<String>,
    /// Border color, hex `#RRGGBB`. Default black.
    pub color: Option<String>,
    /// Border width in points. Default 1.
    pub width_pt: Option<f32>,
}

/// Parse `#RRGGBB` to a normalized `RgbColor` (0.0..=1.0). Rejects anything
/// that is not exactly 6 hex digits after an optional `#`.
pub(crate) fn hex_to_rgb(hex: &str) -> Result<RgbColor, DocsError> {
    let h = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DocsError::InvalidArgs(format!(
            "invalid hex color '{hex}' — expected #RRGGBB"
        )));
    }
    let parse = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
    Ok(RgbColor {
        r: parse(0) as f32 / 255.0,
        g: parse(2) as f32 / 255.0,
        b: parse(4) as f32 / 255.0,
    })
}

/// Docs `OptionalColor` shape: `{ "color": { "rgbColor": { r, g, b } } }`.
fn rgb_to_color(c: &RgbColor) -> serde_json::Value {
    serde_json::json!({ "color": { "rgbColor": { "red": c.r, "green": c.g, "blue": c.b } } })
}

/// Validate the range against the table dims (end-exclusive, non-empty).
/// Checked arithmetic — never panics on adversarial LLM input.
pub(crate) fn validate_range(table: &TableSnapshot, r: &CellRange) -> Result<(), DocsError> {
    let bad = |m: String| DocsError::InvalidArgs(m);
    if r.row_start >= r.row_end || r.col_start >= r.col_end {
        return Err(bad(format!(
            "empty cell_range (rows {}..{}, cols {}..{}) — end must be > start",
            r.row_start, r.row_end, r.col_start, r.col_end
        )));
    }
    if r.row_end > table.rows || r.col_end > table.columns {
        return Err(bad(format!(
            "cell_range rows {}..{} cols {}..{} out of bounds for table {} ({}x{})",
            r.row_start, r.row_end, r.col_start, r.col_end,
            table.table_index, table.rows, table.columns
        )));
    }
    Ok(())
}

/// Build the `batchUpdate` requests for ONE op against an already-located
/// table. Emits (at most): 1 `updateTableCellStyle` over the cell range,
/// N `updateTextStyle` + N `updateParagraphStyle` (one each per cell).
pub(crate) fn build_format_table_requests(
    table: &TableSnapshot,
    range: &CellRange,
    format: &CellFormat,
    tab_id: Option<&TabId>,
) -> Result<Vec<serde_json::Value>, DocsError> {
    todo!("step 5")
}

/// Apply formatting ops to a doc's tables in one atomic batchUpdate.
pub async fn run_format_table(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    ops: &[TableFormatOp],
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    todo!("task 2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{CellSnapshot, TableSnapshot};

    fn cell(row: u32, col: u32, start: u32, end: u32) -> CellSnapshot {
        CellSnapshot {
            row,
            col,
            text: "x".into(),
            content_start_index: start,
            content_end_index: end,
            row_span: 1,
            col_span: 1,
        }
    }

    /// A 2x2 table: cells at content indices 10/20/30/40, table starts at 5.
    fn table_2x2() -> TableSnapshot {
        TableSnapshot {
            table_index: 0,
            start_index: 5,
            rows: 2,
            columns: 2,
            cells: vec![
                vec![cell(0, 0, 10, 12), cell(0, 1, 20, 22)],
                vec![cell(1, 0, 30, 32), cell(1, 1, 40, 42)],
            ],
        }
    }

    #[test]
    fn hex_parses_and_rejects() {
        let c = hex_to_rgb("#FF8000").unwrap();
        assert!((c.r - 1.0).abs() < 1e-6);
        assert!((c.g - 128.0 / 255.0).abs() < 1e-6);
        assert!(hex_to_rgb("#FFF").is_err());
        assert!(hex_to_rgb("nothex!").is_err());
    }

    #[test]
    fn validate_range_rejects_empty_and_oob() {
        let t = table_2x2();
        assert!(validate_range(&t, &CellRange { row_start: 0, row_end: 0, col_start: 0, col_end: 1 }).is_err());
        assert!(validate_range(&t, &CellRange { row_start: 0, row_end: 3, col_start: 0, col_end: 1 }).is_err());
        assert!(validate_range(&t, &CellRange { row_start: 0, row_end: 1, col_start: 0, col_end: 2 }).is_ok());
    }

    #[test]
    fn cell_level_only_emits_one_table_cell_style() {
        let t = table_2x2();
        let fmt = CellFormat { background_color: Some("#1F4E78".into()), ..Default::default() };
        let reqs = build_format_table_requests(&t, &CellRange { row_start: 0, row_end: 1, col_start: 0, col_end: 2 }, &fmt, None).unwrap();
        assert_eq!(reqs.len(), 1, "background-only → single updateTableCellStyle");
        let r = &reqs[0]["updateTableCellStyle"];
        assert!(r.is_object());
        assert_eq!(r["fields"].as_str().unwrap(), "backgroundColor");
        // tableRange covers 1 row x 2 cols starting at (0,0)
        let tr = &r["tableRange"];
        assert_eq!(tr["rowSpan"].as_u64().unwrap(), 1);
        assert_eq!(tr["columnSpan"].as_u64().unwrap(), 2);
    }

    #[test]
    fn text_only_emits_text_and_no_cell_style() {
        let t = table_2x2();
        let fmt = CellFormat { text: Some(CellTextFormat { bold: Some(true), ..Default::default() }), ..Default::default() };
        // header row: (0,0)..(1,2) = 2 cells
        let reqs = build_format_table_requests(&t, &CellRange { row_start: 0, row_end: 1, col_start: 0, col_end: 2 }, &fmt, None).unwrap();
        assert!(reqs.iter().all(|r| r.get("updateTableCellStyle").is_none()), "no cell-level attr → no updateTableCellStyle");
        let text_reqs: Vec<_> = reqs.iter().filter(|r| r.get("updateTextStyle").is_some()).collect();
        assert_eq!(text_reqs.len(), 2, "one updateTextStyle per cell in range");
        assert_eq!(text_reqs[0]["updateTextStyle"]["fields"].as_str().unwrap(), "bold");
        // its range is the first cell's content range 10..12
        assert_eq!(text_reqs[0]["updateTextStyle"]["range"]["startIndex"].as_u64().unwrap(), 10);
        assert_eq!(text_reqs[0]["updateTextStyle"]["range"]["endIndex"].as_u64().unwrap(), 12);
    }

    #[test]
    fn horizontal_alignment_emits_paragraph_style_per_cell() {
        let t = table_2x2();
        let fmt = CellFormat { horizontal_alignment: Some("CENTER".into()), ..Default::default() };
        let reqs = build_format_table_requests(&t, &CellRange { row_start: 0, row_end: 2, col_start: 0, col_end: 1 }, &fmt, None).unwrap();
        let para: Vec<_> = reqs.iter().filter(|r| r.get("updateParagraphStyle").is_some()).collect();
        assert_eq!(para.len(), 2, "one updateParagraphStyle per cell in the column range");
        assert_eq!(para[0]["updateParagraphStyle"]["paragraphStyle"]["alignment"].as_str().unwrap(), "CENTER");
        assert_eq!(para[0]["updateParagraphStyle"]["fields"].as_str().unwrap(), "alignment");
    }

    #[test]
    fn borders_set_all_requested_sides_on_cell_style() {
        let t = table_2x2();
        let fmt = CellFormat {
            borders: Some(CellBorders {
                top: Some(CellBorderSide { style: Some("SOLID".into()), ..Default::default() }),
                bottom: Some(CellBorderSide::default()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let reqs = build_format_table_requests(&t, &CellRange { row_start: 0, row_end: 1, col_start: 0, col_end: 2 }, &fmt, None).unwrap();
        let cs = &reqs[0]["updateTableCellStyle"];
        let fields = cs["fields"].as_str().unwrap();
        assert!(fields.contains("borderTop"));
        assert!(fields.contains("borderBottom"));
        assert!(!fields.contains("borderLeft"));
        assert!(cs["tableCellStyle"]["borderTop"]["width"]["magnitude"].as_f64().unwrap() == 1.0, "default width 1pt");
    }
}
```

  IMPORTANT — confirm the real `TableSnapshot` field names before running. Read `src/libs/colmena/src/gdocs/domain/types.rs` for `TableSnapshot` (the test uses `table_index`, `start_index`, `rows`, `columns`, `cells`). If a field has a different name (e.g. `start_index` may be `content_start_index` or `start`), fix the test helper accordingly. Likewise confirm `CellSnapshot` fields (`row`, `col`, `text`, `content_start_index`, `content_end_index`, `row_span`, `col_span`) — these were verified present, but match reality if any differ.

- [ ] **Step 4: Run the tests to verify they fail.**

  Run: `cargo test --lib gdocs::application::table_format 2>&1 | tail -20`
  Expected: the `hex_parses_and_rejects` and `validate_range_*` tests PASS (those helpers are implemented), but the four `build_format_table_requests` tests FAIL/panic with `todo!("step 5")`.

- [ ] **Step 5: Implement `build_format_table_requests`.** Replace the `todo!("step 5")` body with:

```rust
pub(crate) fn build_format_table_requests(
    table: &TableSnapshot,
    range: &CellRange,
    format: &CellFormat,
    tab_id: Option<&TabId>,
) -> Result<Vec<serde_json::Value>, DocsError> {
    let mut reqs = Vec::new();

    // ── cell-level: one updateTableCellStyle over the rectangle ──
    let mut cell_style = serde_json::Map::new();
    let mut fields: Vec<&str> = Vec::new();
    if let Some(bg) = &format.background_color {
        cell_style.insert("backgroundColor".into(), rgb_to_color(&hex_to_rgb(bg)?));
        fields.push("backgroundColor");
    }
    if let Some(va) = &format.vertical_alignment {
        cell_style.insert("contentAlignment".into(), serde_json::json!(va));
        fields.push("contentAlignment");
    }
    if let Some(b) = &format.borders {
        for (side_opt, key) in [
            (&b.top, "borderTop"),
            (&b.bottom, "borderBottom"),
            (&b.left, "borderLeft"),
            (&b.right, "borderRight"),
        ] {
            if let Some(side) = side_opt {
                let color = match &side.color {
                    Some(c) => hex_to_rgb(c)?,
                    None => RgbColor { r: 0.0, g: 0.0, b: 0.0 },
                };
                cell_style.insert(
                    key.to_string(),
                    serde_json::json!({
                        "color": rgb_to_color(&color),
                        "width": { "magnitude": side.width_pt.unwrap_or(1.0), "unit": "PT" },
                        "dashStyle": side.style.clone().unwrap_or_else(|| "SOLID".into()),
                    }),
                );
                fields.push(key);
            }
        }
    }
    if !fields.is_empty() {
        reqs.push(serde_json::json!({
            "updateTableCellStyle": {
                "tableRange": {
                    "tableCellLocation": cell_location(table.start_index, range.row_start, range.col_start, tab_id),
                    "rowSpan": range.row_end - range.row_start,
                    "columnSpan": range.col_end - range.col_start,
                },
                "tableCellStyle": serde_json::Value::Object(cell_style),
                "fields": fields.join(","),
            }
        }));
    }

    // ── text-level + paragraph-level: per cell in the range ──
    let (text_map, text_fields) = build_text_style(format.text.as_ref());
    let want_para = format.horizontal_alignment.is_some();
    for row in range.row_start..range.row_end {
        for col in range.col_start..range.col_end {
            let cell = find_cell(table, row, col)?;
            let content = util::range(cell.content_start_index, cell.content_end_index, &tab_id.cloned());
            if !text_fields.is_empty() {
                reqs.push(serde_json::json!({
                    "updateTextStyle": {
                        "range": content.clone(),
                        "textStyle": serde_json::Value::Object(text_map.clone()),
                        "fields": text_fields.join(","),
                    }
                }));
            }
            if let Some(align) = &format.horizontal_alignment {
                reqs.push(serde_json::json!({
                    "updateParagraphStyle": {
                        "range": content,
                        "paragraphStyle": { "alignment": align },
                        "fields": "alignment",
                    }
                }));
            }
        }
    }
    let _ = want_para;
    Ok(reqs)
}

/// Build the textStyle map + fields list from the optional text format.
fn build_text_style(t: Option<&CellTextFormat>) -> (serde_json::Map<String, serde_json::Value>, Vec<&'static str>) {
    let mut m = serde_json::Map::new();
    let mut fields: Vec<&'static str> = Vec::new();
    let Some(t) = t else { return (m, fields) };
    if let Some(b) = t.bold { m.insert("bold".into(), b.into()); fields.push("bold"); }
    if let Some(i) = t.italic { m.insert("italic".into(), i.into()); fields.push("italic"); }
    if let Some(u) = t.underline { m.insert("underline".into(), u.into()); fields.push("underline"); }
    if let Some(s) = t.strikethrough { m.insert("strikethrough".into(), s.into()); fields.push("strikethrough"); }
    if let Some(sz) = t.font_size { m.insert("fontSize".into(), serde_json::json!({ "magnitude": sz, "unit": "PT" })); fields.push("fontSize"); }
    if let Some(c) = &t.color {
        if let Ok(rgb) = hex_to_rgb(c) {
            m.insert("foregroundColor".into(), serde_json::json!({ "color": { "rgbColor": { "red": rgb.r, "green": rgb.g, "blue": rgb.b } } }));
            fields.push("foregroundColor");
        }
    }
    (m, fields)
}
```

  NOTE: `build_text_style` swallows a bad text `color` hex silently (the `if let Ok`). This is intentional asymmetry to avoid the borrow/`?` issue inside the helper that returns no `Result`; the cell-level `background_color`/`border.color` paths above DO validate via `?`. If you prefer strict validation for text color too, change `build_text_style` to return `Result` and propagate — but keep it simple for v1. Document the choice in a comment.

- [ ] **Step 6: Run the builder tests to verify they pass.**

  Run: `cargo test --lib gdocs::application::table_format 2>&1 | tail -20`
  Expected: all tests PASS (7 tests: 2 helper + 1 range + 4 builder).

- [ ] **Step 7: Clippy + fmt (CI gates).**

  Run: `cargo fmt -p colmena_dag_engine && cargo clippy --all-targets 2>&1 | grep -E "^error|^warning:" | head`
  Expected: no output (clean). Fix any lint (e.g. remove the `let _ = want_para;` line if clippy flags it — it exists only to avoid an unused warning; delete `want_para` entirely and the `let _` if so).

- [ ] **Step 8: Commit.**

```bash
git add src/libs/colmena/src/gdocs/application/table_format.rs src/libs/colmena/src/gdocs/application/table.rs src/libs/colmena/src/gdocs/application/mod.rs
git commit -m "feat(gdocs): table_format.rs builder for gdocs_format_table (cell/text/paragraph requests)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: `run_format_table` use case + integration test

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/table_format.rs` (implement `run_format_table` + add an integration-style test with a fake `DocsClient`)

- [ ] **Step 1: Implement `run_format_table`.** Replace the `todo!("task 2")` body:

```rust
pub async fn run_format_table(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    ops: &[TableFormatOp],
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    if ops.is_empty() {
        return Err(DocsError::InvalidArgs("ops is empty — nothing to format".into()));
    }
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let snap = &guard.snapshot;

    let mut requests = Vec::new();
    let mut summary_parts = Vec::new();
    for op in ops {
        let table = find_table(snap, op.table_index, tab_id.as_ref())?;
        validate_range(table, &op.cell_range)?;
        let mut reqs = build_format_table_requests(table, &op.cell_range, &op.format, tab_id.as_ref())?;
        requests.append(&mut reqs);
        summary_parts.push(format!(
            "table[{}] cells({}..{},{}..{})",
            op.table_index, op.cell_range.row_start, op.cell_range.row_end,
            op.cell_range.col_start, op.cell_range.col_end
        ));
    }
    if requests.is_empty() {
        return Err(DocsError::InvalidArgs(
            "ops produced no formatting requests — every op's format was empty".into(),
        ));
    }
    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        requests,
        ChangeKind::Style,
        0,
        &format!("format {}", summary_parts.join("; ")),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}
```

  CONFIRM against `table.rs`'s `run_set_table_cell` (lines 170-197) that the `apply_and_finalize` argument order and `guard.snapshot`/`guard.soft_warnings` field names match exactly. The `run_guard_non_blocking` return type is whatever `run_set_table_cell` binds — mirror it (it binds `guard.snapshot` and `guard.soft_warnings`). If `EditResult`/`ChangeKind::Style` need importing, they are already in the `use` block from Task 1.

- [ ] **Step 2: Add the integration test** (fake `DocsClient` capturing the `batchUpdate` body). FIRST read an existing gdocs use-case test that constructs a fake `DocsClient` + `GuardContext` (search `table.rs` tests or `insert.rs` tests for a `struct Fake`/`MockDocsClient` + how `GuardContext` is built in tests — reuse that exact harness). Then add, adapting names to the real harness:

```rust
    #[tokio::test]
    async fn run_format_table_emits_one_batch_update() {
        // Build a fake DocsClient whose get_document returns a snapshot with a
        // 2x2 table, and whose batch_update records the requests it received.
        // (Reuse the harness pattern from table.rs / insert.rs tests.)
        // ... harness setup ...
        let ops = vec![TableFormatOp {
            table_index: 0,
            cell_range: CellRange { row_start: 0, row_end: 1, col_start: 0, col_end: 2 },
            format: CellFormat {
                background_color: Some("#1F4E78".into()),
                text: Some(CellTextFormat { bold: Some(true), color: Some("#FFFFFF".into()), ..Default::default() }),
                horizontal_alignment: Some("CENTER".into()),
                ..Default::default()
            },
        }];
        let res = run_format_table(&ctx, &DocumentId("doc1".into()), &ops, None).await.unwrap();
        // Assert exactly one batchUpdate happened and it contained a cell-style
        // request + per-cell text/paragraph requests.
        let captured = fake.captured_requests();
        assert!(captured.iter().any(|r| r.get("updateTableCellStyle").is_some()));
        assert_eq!(captured.iter().filter(|r| r.get("updateTextStyle").is_some()).count(), 2);
        let _ = res;
    }
```

  This test is a SKELETON: you MUST wire it to the real fake-client harness used elsewhere in the gdocs application tests. If the gdocs tests use `mockall`'s `MockDocsClient`, set expectations on `get_document`/`batch_update` and capture via a closure. If they use a hand-written fake, instantiate it. The behavioral assertions (one cell-style request + two text-style requests for a 1x2 header range) are the point — keep them.

- [ ] **Step 3: Run the use-case test.**

  Run: `cargo test --lib gdocs::application::table_format 2>&1 | tail -20`
  Expected: all PASS (builder tests + the new `run_format_table_emits_one_batch_update`).

- [ ] **Step 4: Clippy + fmt.**

  Run: `cargo fmt -p colmena_dag_engine && cargo clippy --all-targets 2>&1 | grep -E "^error|^warning:" | head`
  Expected: clean.

- [ ] **Step 5: Commit.**

```bash
git add src/libs/colmena/src/gdocs/application/table_format.rs
git commit -m "feat(gdocs): run_format_table use case (guard + validate + atomic batchUpdate)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Synthetic tool `gdocs_format_table` (Args + dispatcher + tool def + YAML)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Modify: `src/libs/colmena/text/tools/gdocs.yaml`

- [ ] **Step 1: Add the tool-name const.** In `gdocs_tools.rs`, next to the other table consts (line ~70-75), add:

```rust
pub const TOOL_FORMAT_TABLE: &str = "gdocs_format_table";
```

- [ ] **Step 2: Add the Args structs.** Near `SetTableCellArgs` (line ~516), add. Reuse the application-layer format types so the schema is defined once:

```rust
/// Args for `gdocs_format_table` — apply formatting to cell ranges in tables.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormatTableArgs {
    /// Drive id of the target document.
    pub doc_id: String,
    /// One or more formatting operations, applied in one atomic batchUpdate.
    pub ops: Vec<crate::gdocs::application::table_format::TableFormatOp>,
    /// Optional tab id when the tables live in a specific tab.
    pub tab_id: Option<String>,
}
```

  CONFIRM `TableFormatOp` derives `JsonSchema` (it does — Task 1). If `schemars` rejects the nested `Option<...>` types, ensure every nested struct (`CellFormat`, `CellTextFormat`, `CellBorders`, `CellBorderSide`, `CellRange`) derives `JsonSchema` (they do in Task 1).

- [ ] **Step 3: Add the ToolDefinition builder.** Near `tool_set_table_cell()` (line ~774), add:

```rust
pub fn tool_format_table() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<FormatTableArgs>(
        TOOL_FORMAT_TABLE,
        text::tool_description(TOOL_FORMAT_TABLE),
        text::tool_summary(TOOL_FORMAT_TABLE),
    )
}
```

- [ ] **Step 4: Add the dispatcher.** Near `dispatch_set_table_cell` (line ~2210), add (mirror its client/cache/revisions/ctx setup exactly):

```rust
pub async fn dispatch_format_table(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    use crate::gdocs::application::table_format;
    let parsed: FormatTableArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    match table_format::run_format_table(
        &ctx,
        &DocumentId(parsed.doc_id),
        &parsed.ops,
        parsed.tab_id.map(TabId),
    )
    .await
    {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}
```

  CONFIRM the helper names `shared_client`/`shared_cache`/`shared_revs`/`invalid_args`/`edit_result_to_json`/`error_to_json`/`co_edit_guard`/`TabId`/`DocumentId` are all already imported/used in `dispatch_set_table_cell` — they are; reuse verbatim.

- [ ] **Step 5: Add to `build_all_gdocs_tools()`.** Find the `vec![ ... ]` in `build_all_gdocs_tools` (line ~969-1004), and add `tool_format_table(),` right after `tool_set_table_cell(),` (line ~1004).

- [ ] **Step 6: Add the YAML description + summary.** In `src/libs/colmena/text/tools/gdocs.yaml`, after the `gdocs_set_table_cell:` entry (line ~434), add a `gdocs_format_table:` entry. Use a `summary` + a `description: |` block. The description documents the args AND carries the presentable nudge:

```yaml
gdocs_format_table:
  summary: Apply formatting (background, borders, text style, alignment) to cell ranges in a Google Docs table
  description: |
    Aplica FORMATO a rangos rectangulares de celdas dentro de una tabla de un
    Google Doc, en un solo batchUpdate atómico. Pasá `ops: [{table_index,
    cell_range, format}]`. `table_index` 0-based (de gdocs_read_tables).
    `cell_range` = { row_start, row_end, col_start, col_end } 0-based y
    END-EXCLUSIVE (row_end/col_end NO se incluyen). `format` (todos opcionales):
      - background_color: "#RRGGBB"   (fondo de celda)
      - vertical_alignment: TOP|MIDDLE|BOTTOM
      - borders: { top|bottom|left|right: { style: SOLID|DOTTED|DASHED, color?: "#RRGGBB", width_pt?: 1 } }
      - text: { bold, italic, underline, strikethrough, font_size, color: "#RRGGBB" }
      - horizontal_alignment: LEFT|CENTER|RIGHT|JUSTIFIED
    A diferencia de Sheets, Docs NO tiene bordes internos/externos — cada lado
    se aplica a TODAS las celdas del rango. Para escribir el TEXTO de una celda
    usá gdocs_set_table_cell (este tool solo da estilo, no cambia el contenido).
    Por DEFAULT, cuando armes una tabla para que la vea una persona, dejala
    presentable en UNA llamada multi-op (sin esperar a que te lo pidan):
    encabezado en negrita con fondo y texto contrastante + centrado, bordes en
    la tabla, y fila de totales destacada (negrita + fondo). Una tabla sin
    formato no es una entrega cuidada.
    Ejemplo (header azul + bordes en toda la tabla 5x3):
    ops: [
      { table_index:0, cell_range:{row_start:0,row_end:1,col_start:0,col_end:3}, format:{ text:{bold:true,color:"#FFFFFF"}, background_color:"#1F4E78", horizontal_alignment:"CENTER" } },
      { table_index:0, cell_range:{row_start:0,row_end:5,col_start:0,col_end:3}, format:{ borders:{ top:{style:"SOLID"}, bottom:{style:"SOLID"}, left:{style:"SOLID"}, right:{style:"SOLID"} } } }
    ]
```

  CONFIRM the YAML structure matches the existing `gdocs_set_table_cell:` entry (does it use `summary:` + `description: |`? top-level key at column 0?). Match it exactly so `text::tool_description(TOOL_FORMAT_TABLE)` and `text::tool_summary(...)` resolve. If the registry requires both keys, include both.

- [ ] **Step 7: Verify it builds + YAML loads.**

  Run: `cargo build --lib 2>&1 | tail -8 && cargo test --lib text 2>&1 | tail -8`
  Expected: builds clean; text-registry tests PASS (the new description/summary load). Note: the tool is NOT yet routed/exposed — that's Task 4. `build_all_gdocs_tools()` now returns 36 entries; if a test asserts its length here it may fail until Task 4 updates the `llm.rs` arrays — if so, note it and proceed (Task 4 fixes the count cross-checks together).

- [ ] **Step 8: Commit.**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs src/libs/colmena/text/tools/gdocs.yaml
git commit -m "feat(gdocs): gdocs_format_table synthetic tool (args + dispatcher + YAML nudge)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Wiring / exposure (the trap) — mod.rs, executor, llm.rs, toolkit

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`

- [ ] **Step 1: Re-export in `mod.rs`.** In `llm_synthetic_tools/mod.rs`:
  - Add to the dispatcher re-export block (next to `dispatch_set_table_cell as dispatch_gdocs_set_table_cell,` line ~392): `dispatch_format_table as dispatch_gdocs_format_table,`
  - Add to the tool-builder re-export block (next to `tool_set_table_cell as gdocs_tool_set_table_cell,` line ~420): `tool_format_table as gdocs_tool_format_table,`
  - Add to the TOOL-const re-export block (next to `TOOL_SET_TABLE_CELL as GDOCS_SET_TABLE_CELL_TOOL,` line ~447): `TOOL_FORMAT_TABLE as GDOCS_FORMAT_TABLE_TOOL,`

- [ ] **Step 2: Route in `dag_tool_executor.rs`.** Three edits (mirror `GDOCS_SET_TABLE_CELL_TOOL`):
  - Import the dispatcher (line ~1361 block): add `dispatch_gdocs_format_table,`
  - Import the const (line ~1374 block): add `GDOCS_FORMAT_TABLE_TOOL,`
  - "is gdocs tool" predicate (line ~1411): add `|| n == GDOCS_FORMAT_TABLE_TOOL`
  - Dispatch match arm (line ~1537): add
    ```rust
                    n if n == GDOCS_FORMAT_TABLE_TOOL => {
                        dispatch_gdocs_format_table(args, session_id).await
                    }
    ```

- [ ] **Step 3: Expose in `llm.rs` (THE CRITICAL PATH).** Three edits:
  - `all_gdocs` array (line ~2689): change `[&str; 35]` → `[&str; 36]` and add `GDOCS_FORMAT_TABLE_TOOL,` to the list (group it near `GDOCS_SET_TABLE_CELL_TOOL` at line ~2722).
  - The const import for `llm.rs` — confirm `GDOCS_FORMAT_TABLE_TOOL` is imported in this file (it imports the others via the `mod.rs` re-export aliases; add it to that `use` group if needed).
  - `gdocs_entries` array (line ~2736): change `[(&str, fn() -> ...); 35]` → `; 36]` and add `(GDOCS_FORMAT_TABLE_TOOL, gdocs_tool_format_table),` near the `(GDOCS_SET_TABLE_CELL_TOOL, gdocs_tool_set_table_cell),` line (~2781).

- [ ] **Step 4: Add to the toolkit package.** In `toolkit_packages.rs`, add `"gdocs_format_table",` to the `gdocs` full toolkit tool list (there are two arrays at lines ~78 and ~202 — read both; one is the `gdocs` full alias, one may be `gdocsread` read-only. Add ONLY to the full `gdocs` write toolkit, NOT to `gdocsread` — formatting is a write op). Confirm which is which by reading the `alias:` field of each `ToolkitPackage`.

- [ ] **Step 5: Build + run the count/coverage cross-checks.**

  Run: `cargo build --lib 2>&1 | tail -8`
  Expected: clean.

  Run: `cargo test --lib gdocs 2>&1 | tail -20`
  Expected: PASS — in particular any test asserting `build_all_gdocs_tools().len()` equals the `all_gdocs`/`gdocs_entries` array length (now all 36), and any toolkit-package count test for the `gdocs` alias. If a hardcoded count (e.g. "35") appears in a test, update it to 36.

- [ ] **Step 6: Clippy + fmt.**

  Run: `cargo fmt -p colmena_dag_engine && cargo clippy --all-targets 2>&1 | grep -E "^error|^warning:" | head`
  Expected: clean.

- [ ] **Step 7: Commit.**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs
git commit -m "feat(gdocs): wire gdocs_format_table into executor, llm.rs exposure arrays, and gdocs toolkit (35→36)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Docs + index + sweep + live E2E (success criterion) + PR — controller-run

**Files:**
- Modify: `docs/developer_guide/41_builtin_tools_index.md`, `docs/developer_guide/46_gdocs_tables.md` (or the gdocs tables dev guide section), `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md`
- Create: `tests/graphs/agents/gdocs_format_table_e2e.json`

- [ ] **Step 1: Update the builtin-tools index.** In `docs/developer_guide/41_builtin_tools_index.md`, add a `gdocs_format_table` row in the gdocs section (mirror the `gdocs_set_table_cell` row's columns). There is a coverage test `index_doc_covers_all_registered_tools` that will FAIL if a registered tool is missing from this doc — so this is mandatory. Run after: `cargo test --lib index_doc_covers_all_registered_tools 2>&1 | tail -8` — Expected: PASS.

- [ ] **Step 2: Dev guide.** In the gdocs tables dev guide (find the section documenting `gdocs_set_table_cell` / table tools — likely `docs/developer_guide/46_*.md` or §45; grep `gdocs_set_table_cell` under `docs/developer_guide/`), add a short subsection for `gdocs_format_table`: what it does, the multi-op `ops` shape, the end-exclusive ranges, the 3 request types it emits, the "borders apply per-cell (no inner/outer)" caveat, and that it's non-destructive (style only). Note the presentable-by-default nudge in its description.

- [ ] **Step 3: CHANGELOG §49.** In `docs/CHANGELOG_2026-06.md`, add `## 49. Formato de celdas de tabla en Google Docs (gdocs_format_table) — Subsystem G v1.1 — 2026-06-22` after §48 (match §47/§48 style). Cover: the tool, multi-op ranges (end-exclusive), the 3 Docs API request types (updateTableCellStyle via tableRange + per-cell updateTextStyle + updateParagraphStyle), reuse of the #121 addressing + non-blocking guard + already-parsed CellSnapshot ranges (no trait/parser change), the per-cell borders caveat vs Sheets, the presentable nudge (no skill in v1), gdocs 35→36, additive/no ADP impact. Reference spec + plan + E2E graph.

- [ ] **Step 4: BACKLOG.** Mark the "Formato de celdas de tabla en gdocs" item `[x]` SHIPPED 2026-06-22 with the §49 pointer (match nearby shipped-item format).

- [ ] **Step 5: Write the E2E graph** `tests/graphs/agents/gdocs_format_table_e2e.json`. Open prompt: create a doc with a small table and make it presentable. Use the default LLM stack + `enabled_tools: ["gdocs"]`. Read a sibling gdocs agents graph (e.g. the §46 table-edits E2E or any `tests/graphs/agents/gdocs_*` graph) to match the exact node/config shape (trigger type, `llm_call` keys, `connection_url`, etc.). Skeleton (ADAPT keys to the real sibling):

```json
{
  "_comment": "E2E (manual, live): OPEN prompt — agent creates a Doc with a small budget table and should format it presentably BY DEFAULT (header bg+bold, borders) via gdocs_format_table. Needs GEMINI_API_KEY + DATABASE_URL + OAuth creds in-memory + COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID + --agent-session-id.",
  "nodes": {
    "trigger": { "type": "trigger_webhook", "config": { "path": "/gdocs-format-table", "method": "POST",
      "test_payload": { "prompt": "Creá un Google Doc titulado 'Presupuesto Q3' con una tabla de 4 filas x 3 columnas: encabezados Concepto, Estimado, Real; y 3 filas de datos (Marketing 5000 4800; Ventas 8000 8200; Operaciones 3000 2900). Dejala lista para mostrar." } } },
    "agent": { "type": "llm_call", "config": { "provider": "google", "api_key": "${GEMINI_API_KEY}", "model": "gemini-2.5-flash", "stream": false, "max_iterations": 20, "connection_url": "${DATABASE_URL}", "system_message": "Sos un asistente que arma documentos en Google Docs. Creá el doc, llená la tabla y dejala presentable. Reportá la URL.", "enabled_tools": ["gdocs"] } },
    "log": { "type": "log" }
  },
  "edges": [ { "from": "trigger", "to": "agent" }, { "from": "agent", "to": "log" } ]
}
```

  Validate JSON: `python3 -c "import json;json.load(open('tests/graphs/agents/gdocs_format_table_e2e.json'));print('ok')"`.

- [ ] **Step 6: Commit docs + graph.**

```bash
git add docs/developer_guide/41_builtin_tools_index.md docs/developer_guide/ docs/CHANGELOG_2026-06.md docs/BACKLOG.md tests/graphs/agents/gdocs_format_table_e2e.json
git commit -m "docs(gdocs): gdocs_format_table — index, dev guide, CHANGELOG §49, BACKLOG, E2E graph

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Step 7: Full sweep (CI parity — clippy is a SEPARATE gate).**

```bash
cargo fmt --check && echo FMT_OK
cargo clippy --all-targets 2>&1 | grep -E "^error|^warning:" ; echo CLIPPY_DONE
cargo test --verbose 2>&1 | tail -25
```
  Expected: fmt clean, clippy 0 errors/warnings, all tests exit 0 (`--verbose` runs doctests + integration tests `--lib` skips).

- [ ] **Step 8: ADP additive sweep.** `git diff develop...HEAD --stat`. Confirm the change touches only: `gdocs/application/table_format.rs` (+ `table.rs` visibility + `mod.rs`), `gdocs_tools.rs`, `gdocs.yaml`, `mod.rs`/`dag_tool_executor.rs`/`llm.rs`/`toolkit_packages.rs` wiring, docs, and the E2E graph. NO change to `EngineConfig`/`ColmenaEngine`/exported trait signatures (no `DocsClient` trait change). Confirm in the report.

- [ ] **Step 9: Live E2E — THE SUCCESS CRITERION (controller, secrets in-memory).** Per memory `ref_local_gsheets_e2e_runbook` + `feedback_always_real_e2e`: rebuild `cargo build --release --bin dag_engine`; inject OAuth creds from Secret Manager in-memory (no echo/file/commit); ensure `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID` is set; run `tests/graphs/agents/gdocs_format_table_e2e.json` with `--agent-session-id`; save SSE to `/tmp/colmena_e2e/gdocs_format_table.sse`. Then **read back the table style** via Docs API `documents.get` (fields for `table` → `tableRows` → `tableCells` → `tableCellStyle.backgroundColor`/`borderTop` and the cell paragraph `textRun.textStyle.bold`) and assert the header got background + bold WITHOUT the prompt specifying how. Present a friendly report. If formatting is minimal, iterate the nudge wording (part of E2E acceptance, not a new task). Delete the demo doc afterward.

- [ ] **Step 10: Push + open PR + watch CI.**

```bash
git push -u origin feat/gdocs-table-formatting
gh pr create --base develop --title "feat(gdocs): table-cell formatting (gdocs_format_table)" --body "..."
gh pr checks <n> --watch
```

---

## Self-Review Notes (author)

- **Spec coverage:** Tool shape (multi-op, end-exclusive ranges) → Task 3 Args + Task 1 `CellRange`/`validate_range`. Attributes v1 (bg/text/H+V align/borders) → Task 1 `CellFormat` + builder. 3 request types (updateTableCellStyle via tableRange / per-cell updateTextStyle / updateParagraphStyle) → Task 1 builder + tests. Per-cell borders (no inner/outer) → Task 1 `borders_set_all_requested_sides_on_cell_style` test + YAML caveat. Reuse of find_table/find_cell/cell_location/guard/apply_and_finalize/CellSnapshot ranges (no trait/parser change) → Tasks 1-2. Presentable nudge, no skill → Task 3 YAML. New file `table_format.rs` → Task 1. All sync points (llm.rs arrays 35→36, executor, mod.rs, toolkit, index doc, count tests) → Task 4 + Task 5 step 1. Live E2E success criterion → Task 5 step 9. Cross-repo additive → Task 5 step 8.
- **Verify-during-impl flags:** confirm `TableSnapshot`/`CellSnapshot` field names (Task 1 step 3); confirm `apply_and_finalize` arg order + guard field names against `run_set_table_cell` (Task 2 step 1); confirm the fake-`DocsClient` test harness used by gdocs application tests (Task 2 step 2); confirm `gdocs.yaml` entry shape (Task 3 step 6); confirm which `toolkit_packages.rs` array is the `gdocs` full alias vs `gdocsread` (Task 4 step 4); confirm latest CHANGELOG number is §48 before writing §49 (Task 5 step 3); confirm the real sibling gdocs E2E graph config keys (Task 5 step 5).
- **Type/name consistency:** `TableFormatOp`/`CellFormat`/`CellTextFormat`/`CellBorders`/`CellBorderSide`/`CellRange` defined in Task 1, reused verbatim in Task 3 Args. `build_format_table_requests`/`run_format_table`/`validate_range`/`hex_to_rgb` names consistent across Tasks 1-2. `TOOL_FORMAT_TABLE` = `"gdocs_format_table"` and alias `GDOCS_FORMAT_TABLE_TOOL` consistent across Tasks 3-4. Count 35→36 consistent in Task 4 (all_gdocs, gdocs_entries) + Task 3 build_all_gdocs_tools.
- **Behavioral success criterion (Task 5 step 9):** "done" requires the open-prompt live E2E to actually produce header bg+bold via gdocs_format_table; wording iteration allowed within that step.
