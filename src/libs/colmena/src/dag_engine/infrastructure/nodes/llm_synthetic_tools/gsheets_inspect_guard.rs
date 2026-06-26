//! Pure helpers for the gsheets "inspect-before-python" guard.
//!
//! The guard forces an agent to see a sheet's real columns before its
//! `gsheets_run_python` code runs. These functions are the pure, I/O-free
//! pieces: deciding which sheet bindings are still unread, and rendering the
//! bounded markdown preview the agent is shown. The stateful wiring (the
//! per-turn seen-set + the actual interception) lives in `DagToolExecutor`.
//!
//! Spec: `docs/superpowers/specs/2026-06-15-gsheets-inspect-guard-design.md`.

use serde_json::Value;
use std::collections::HashSet;

/// Key for the per-turn "seen sheets" set: `"<spreadsheet_id>::<sheet>"`.
///
/// The sheet name is NORMALIZED (trim → strip one layer of A1 quoting →
/// case-fold) so the same tab referred to with different casing/quoting across
/// calls within a turn maps to ONE key. Without this, a model that writes
/// `hoja 16` on its first `gsheets_run_python` and `Hoja 16` on the next would
/// re-trigger the inspect guard on the second call even though the sheet was
/// already inspected — Google resolves tab names case-insensitively, so these
/// are the same sheet. The spreadsheet_id is trimmed but NOT case-folded (Drive
/// file IDs are case-sensitive).
pub fn sheet_key(spreadsheet_id: &str, sheet: &str) -> String {
    format!("{}::{}", spreadsheet_id.trim(), normalize_sheet_name(sheet))
}

/// Normalize a sheet/tab name for the seen-set key: trim surrounding
/// whitespace, strip ONE layer of A1-style wrapping quotes (`'My Sheet'` or
/// `"My Sheet"`), then case-fold. Used only for de-duplication of the seen
/// set — never for the actual Sheets API call.
fn normalize_sheet_name(sheet: &str) -> String {
    let s = sheet.trim();
    let s = s
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .or_else(|| {
            s.strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'))
        })
        .unwrap_or(s);
    s.trim().to_lowercase()
}

/// A sheet binding that must be read before `gsheets_run_python` can use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetBindingRef {
    pub var: String,
    pub spreadsheet_id: String,
    pub sheet: String,
    pub range: Option<String>,
}

/// From `gsheets_run_python` args, return the SHEET bindings whose
/// `(spreadsheet_id, sheet)` is NOT in `seen`. Inline bindings (those carrying
/// `data`, or missing `spreadsheet_id`/`sheet`) are skipped — they need no read.
/// If `args` has no parseable `bindings` array, returns empty (caller executes
/// normally and lets the dispatcher surface any real error).
pub fn unseen_sheet_bindings(args: &Value, seen: &HashSet<String>) -> Vec<SheetBindingRef> {
    let Some(bindings) = args.get("bindings").and_then(|b| b.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for b in bindings {
        // Inline binding (has non-null `data`) → no read needed.
        if b.get("data").map(|d| !d.is_null()).unwrap_or(false) {
            continue;
        }
        let (Some(ss), Some(sheet)) = (
            b.get("spreadsheet_id").and_then(|v| v.as_str()),
            b.get("sheet").and_then(|v| v.as_str()),
        ) else {
            continue; // not a complete sheet binding
        };
        if seen.contains(&sheet_key(ss, sheet)) {
            continue;
        }
        out.push(SheetBindingRef {
            var: b
                .get("var")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            spreadsheet_id: ss.to_string(),
            sheet: sheet.to_string(),
            range: b
                .get("range")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        });
    }
    out
}

/// Truncate a markdown table to header + separator + the first `max_data_rows`
/// data rows. Tables with only a header (or empty) are returned unchanged.
pub fn truncate_markdown_preview(md: &str, max_data_rows: usize) -> String {
    let lines: Vec<&str> = md.lines().collect();
    if lines.len() <= 2 {
        return md.to_string();
    }
    let keep = 2 + max_data_rows; // header + separator + data rows
    lines.into_iter().take(keep).collect::<Vec<_>>().join("\n")
}

/// Parse column names from a markdown table's header line
/// (`| A | B | C |` → `["A","B","C"]`). Empty if there is no header.
pub fn columns_from_markdown_header(md: &str) -> Vec<String> {
    let Some(header) = md.lines().next() else {
        return Vec::new();
    };
    header
        .split('|')
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| c.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seen_with(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn sheet_key_joins_id_and_normalized_sheet() {
        // Sheet name is case-folded; spreadsheet_id is preserved (case-sensitive).
        assert_eq!(sheet_key("abc", "Ventas"), "abc::ventas");
        // All these spellings collapse to the same key.
        let k = sheet_key("abc", "ventas");
        assert_eq!(sheet_key("abc", "Ventas"), k);
        assert_eq!(sheet_key("abc", "'ventas'"), k);
        assert_eq!(sheet_key("abc", "  VENTAS "), k);
    }

    #[test]
    fn unseen_recognizes_case_and_quote_variants_as_seen() {
        // Marked seen via one spelling; a later call within the same turn refers
        // to the SAME tab with different case / A1-quoting / surrounding space.
        // Google resolves tab names case-insensitively, so these are the same
        // sheet — the guard must NOT re-inspect (the within-turn double-inspect bug).
        let seen: HashSet<String> = [sheet_key("ssid", "hoja 16")].into_iter().collect();
        for variant in ["Hoja 16", "HOJA 16", "'hoja 16'", " hoja 16 "] {
            let args = json!({
                "code": "x",
                "bindings": [{"var": "v", "spreadsheet_id": "ssid", "sheet": variant}]
            });
            assert!(
                unseen_sheet_bindings(&args, &seen).is_empty(),
                "variant {variant:?} should be recognized as already seen"
            );
        }
    }

    #[test]
    fn unseen_returns_sheet_binding_when_not_seen() {
        let args = json!({
            "code": "x",
            "bindings": [{"var": "v", "spreadsheet_id": "abc", "sheet": "Ventas"}]
        });
        let got = unseen_sheet_bindings(&args, &seen_with(&[]));
        assert_eq!(
            got,
            vec![SheetBindingRef {
                var: "v".into(),
                spreadsheet_id: "abc".into(),
                sheet: "Ventas".into(),
                range: None,
            }]
        );
    }

    #[test]
    fn unseen_skips_already_seen_sheet() {
        let args = json!({
            "code": "x",
            "bindings": [{"var": "v", "spreadsheet_id": "abc", "sheet": "Ventas"}]
        });
        let seen: HashSet<String> = [sheet_key("abc", "Ventas")].into_iter().collect();
        let got = unseen_sheet_bindings(&args, &seen);
        assert!(got.is_empty());
    }

    #[test]
    fn unseen_skips_inline_data_bindings() {
        let args = json!({
            "code": "x",
            "bindings": [{"var": "img", "data": [{"a": 1}]}]
        });
        let got = unseen_sheet_bindings(&args, &seen_with(&[]));
        assert!(got.is_empty());
    }

    #[test]
    fn unseen_mixed_returns_only_the_unseen_sheet() {
        let args = json!({
            "code": "x",
            "bindings": [
                {"var": "a", "spreadsheet_id": "ss", "sheet": "Seen"},
                {"var": "b", "spreadsheet_id": "ss", "sheet": "Fresh", "range": "A1:C9"},
                {"var": "img", "data": [{"k": 1}]}
            ]
        });
        let seen: HashSet<String> = [sheet_key("ss", "Seen")].into_iter().collect();
        let got = unseen_sheet_bindings(&args, &seen);
        assert_eq!(
            got,
            vec![SheetBindingRef {
                var: "b".into(),
                spreadsheet_id: "ss".into(),
                sheet: "Fresh".into(),
                range: Some("A1:C9".into()),
            }]
        );
    }

    #[test]
    fn unseen_empty_when_no_bindings_key() {
        let args = json!({"code": "x"});
        assert!(unseen_sheet_bindings(&args, &seen_with(&[])).is_empty());
    }

    #[test]
    fn truncate_keeps_header_separator_and_n_rows() {
        let md = "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |";
        let got = truncate_markdown_preview(md, 2);
        assert_eq!(got, "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |");
    }

    #[test]
    fn truncate_returns_short_table_unchanged() {
        let md = "| A | B |\n| --- | --- |";
        assert_eq!(truncate_markdown_preview(md, 5), md);
    }

    #[test]
    fn columns_parsed_from_header() {
        let md =
            "| Categoria | Producto | Monto |\n| --- | --- | --- |\n| Frutas | Manzana | 100 |";
        assert_eq!(
            columns_from_markdown_header(md),
            vec!["Categoria", "Producto", "Monto"]
        );
    }

    #[test]
    fn columns_empty_for_empty_markdown() {
        assert!(columns_from_markdown_header("").is_empty());
    }
}
