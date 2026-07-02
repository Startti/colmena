//! Google Sheets write-back for `gsheets_run_python` (and future callers
//! like `data_run_python`): dispatches the `output_sheets` map the sandbox
//! returns to per-mode writers (`replace`, `overwrite`, `update_in_place`,
//! `update_by_position`), plus their shared helpers (A1 addressing, formula
//! `{{Column}}` placeholder resolution, tab-metadata fetch, new-column
//! planning). Extracted verbatim from `gsheets_run_python.rs` — no behavior
//! change.

use crate::gsheets::domain::{ReadOptions, SheetsClient, SpreadsheetId};
use std::sync::Arc;

use super::diff_writer::diff_records;
use super::sheet_collision::{build_sheet_exists_error, CollisionPolicy, TabMeta};

// ── Helpers ──────────────────────────────────────────────────────────

/// Dispatch each normalized `output_sheets` entry by mode. Returns one
/// metadata entry per attempted write.
/// Snapshot of a sheet binding loaded this run. `update_by_position` diffs the
/// returned df against `records` (what the code started from) and maps each
/// changed cell back to the sheet by position, so the agent never computes an
/// A1 address. `ambiguous` is set when more than one binding loaded the same
/// sheet (no single position mapping). `had_range` blocks the mode (a range
/// subset shifts the header/row mapping).
pub struct LoadedSnapshot {
    pub records: Vec<serde_json::Map<String, serde_json::Value>>,
    pub had_range: bool,
    pub ambiguous: bool,
}

pub async fn write_output_sheets(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    output_sheets: &serde_json::Value,
    policy: CollisionPolicy,
    loaded: &std::collections::HashMap<String, LoadedSnapshot>,
) -> Vec<serde_json::Value> {
    let Some(map) = output_sheets.as_object() else {
        return Vec::new();
    };
    let mut results: Vec<serde_json::Value> = Vec::new();
    for (raw_name, entry) in map {
        // Surface postlude-side errors as-is.
        if let Some(err) = entry.get("_postlude_error").and_then(|v| v.as_str()) {
            results.push(serde_json::json!({"name": raw_name, "error": err}));
            continue;
        }
        let mode = entry
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("replace");
        match mode {
            "update_in_place" => {
                results.push(do_update_in_place(client, spreadsheet_id, raw_name, entry).await);
            }
            "update_by_position" => {
                results.push(
                    do_update_by_position(client, spreadsheet_id, raw_name, entry, loaded).await,
                );
            }
            "overwrite" => {
                results.push(do_overwrite(client, spreadsheet_id, raw_name, entry).await);
            }
            "replace" => {
                results.push(do_replace(client, spreadsheet_id, raw_name, entry, policy).await);
            }
            other => {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "error": format!("unknown mode '{other}'; valid: replace, update_in_place, update_by_position, overwrite"),
                }));
            }
        }
    }
    results
}

/// Mode `replace`: create tab and write full DataFrame. Apply policy on collision.
async fn do_replace(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
    policy: CollisionPolicy,
) -> serde_json::Value {
    let exists = match fetch_tab_meta(client, spreadsheet_id, raw_name).await {
        Ok(opt) => opt,
        Err(e) => {
            return serde_json::json!({
                "name": raw_name,
                "error": format!("metadata fetch failed: {e}"),
            });
        }
    };
    if let Some(meta) = exists {
        match policy {
            CollisionPolicy::Fail => {
                return build_sheet_exists_error(raw_name, Some(&spreadsheet_id.0), &meta);
            }
            CollisionPolicy::Overwrite => {
                // Use the existing tab name as-is (Sheets API set_range on
                // existing tab replaces contents from A1 down).
                return write_full_df(client, spreadsheet_id, raw_name, raw_name, entry).await;
            }
            CollisionPolicy::AutoSuffix => {
                // Start at 2: raw_name itself is already known to collide (we just
                // hit `Some(meta)` above). First candidate is "raw_name (2)".
                for attempt in 2i32..=10 {
                    let candidate = format!("{raw_name} ({attempt})");
                    match client.add_sheet(spreadsheet_id, &candidate).await {
                        Ok(_) => {
                            return write_full_df(
                                client,
                                spreadsheet_id,
                                raw_name,
                                &candidate,
                                entry,
                            )
                            .await;
                        }
                        Err(crate::gsheets::domain::SheetsError::Http(msg))
                            if msg.to_lowercase().contains("already exists") =>
                        {
                            continue;
                        }
                        Err(e) => {
                            return serde_json::json!({
                                "name": raw_name,
                                "error": format!("add_sheet failed: {e}"),
                            });
                        }
                    }
                }
                return serde_json::json!({
                    "name": raw_name,
                    "error": "auto_suffix: all 10 name attempts already exist",
                });
            }
        }
    }
    // Doesn't exist — create + write.
    match client.add_sheet(spreadsheet_id, raw_name).await {
        Ok(_) => write_full_df(client, spreadsheet_id, raw_name, raw_name, entry).await,
        Err(e) => serde_json::json!({
            "name": raw_name,
            "error": format!("add_sheet failed: {e}"),
        }),
    }
}

/// Mode `overwrite`: replace contents of existing tab. Creates if absent.
async fn do_overwrite(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    // Schema-change guard: if the tab exists and its columns differ from
    // the input's columns, reject unless allow_schema_change=true.
    match fetch_tab_meta(client, spreadsheet_id, raw_name).await {
        Err(e) => {
            return serde_json::json!({
                "name": raw_name,
                "error": format!("metadata fetch failed: {e}"),
            });
        }
        Ok(None) => {
            // Tab doesn't exist — fall through to write_full_df below.
        }
        Ok(Some(meta)) => {
            let input_cols: Vec<String> = entry
                .get("df_cols")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let allow = entry
                .get("allow_schema_change")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mismatch = !columns_match(&meta.columns, &input_cols);
            if mismatch && !allow {
                return serde_json::json!({
                    "name": raw_name,
                    "error": "SchemaChange",
                    "current_columns": meta.columns,
                    "input_columns": input_cols,
                    "message": format!(
                        "Overwriting '{raw_name}' would change its schema: current {:?} → new {:?}. \
                         This is likely a mistake. RECOMMENDED: use a different tab name. To proceed \
                         anyway, add 'allow_schema_change: true' to the spec dict.",
                        meta.columns, input_cols
                    ),
                });
            }
        }
    }
    write_full_df(client, spreadsheet_id, raw_name, raw_name, entry).await
}

/// Mode `update_in_place`: diff and apply only changed cells.
async fn do_update_in_place(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    let Some(key) = entry.get("key").and_then(|v| v.as_str()) else {
        return serde_json::json!({
            "tab": raw_name,
            "error": "update_in_place requires `key` field in the spec dict",
        });
    };
    let restrict: Option<Vec<String>> = entry.get("columns").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|c| c.as_str().map(String::from))
            .collect()
    });
    let strict = entry
        .get("strict_match")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let new_records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();

    // Fetch current rows AND header column order from the tab.
    let read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            None,
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: true,
            },
        )
        .await
    {
        Ok(r) => r,
        Err(crate::gsheets::domain::SheetsError::SheetNotFound(_)) => {
            return serde_json::json!({
                "name": raw_name,
                "error": "UpdateRequiresExistingTab",
                "message": format!(
                    "update_in_place needs the tab '{raw_name}' to exist. Use mode=replace to create it."
                ),
            });
        }
        Err(e) => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("read failed: {e}"),
            });
        }
    };
    let current_records: Vec<serde_json::Map<String, serde_json::Value>> = match read.values {
        serde_json::Value::Array(a) => a
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::Object(o) => Some(o),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    // Header order (for A1 mapping) — re-read the WHOLE header row ("1:1", not a
    // capped "A1:Z1") so columns past Z map correctly. The cap silently dropped
    // edits to columns beyond Z; "1:1" returns the full row like update_by_position.
    let header_read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            Some("1:1"),
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("header read failed: {e}"),
            });
        }
    };
    let header_cols: Vec<String> = match header_read.values {
        serde_json::Value::Array(rows) => rows
            .first()
            .and_then(|r| r.as_array())
            .map(|cells| {
                cells
                    .iter()
                    .map(|c| c.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let diff = match diff_records(
        &current_records,
        &new_records,
        key,
        restrict.as_deref(),
        strict,
        raw_name,
    ) {
        Ok(d) => d,
        Err(e) => return e.to_json(),
    };

    // Map key_value → row index (1-based, row 1 is header → first data is row 2).
    let mut key_to_row: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, r) in current_records.iter().enumerate() {
        if let Some(k) = r.get(key).and_then(|v| match v {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }) {
            key_to_row.insert(k, i + 2);
        }
    }
    let mut col_to_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, c) in header_cols.iter().enumerate() {
        col_to_index.insert(c.clone(), i);
    }
    // Formula `{{Column}}` refs resolve only against UNIQUELY-named columns
    // (empty/duplicate names are ambiguous and excluded).
    let resolvable_cols = addressable_columns(&header_cols);

    let mut cell_updates: Vec<(String, crate::gsheets::domain::CellValue)> = Vec::new();
    let mut formula_log = FormulaCellLog::default();
    for chg in &diff.changes {
        let (Some(row), Some(col_idx)) = (
            key_to_row.get(&chg.key_value),
            col_to_index.get(&chg.column),
        ) else {
            continue;
        };
        let addr = a1_addr(*col_idx, *row);
        let resolved = match resolve_formula_placeholders(&chg.new_value, &resolvable_cols, *row) {
            Ok(v) => v,
            Err(e) => return e.to_json(raw_name),
        };
        formula_log.record(&addr, &resolved);
        cell_updates.push((
            addr,
            crate::gsheets::domain::CellValue::from_json(&resolved),
        ));
    }

    let cells_to_write = cell_updates.len();
    if cells_to_write == 0 {
        return serde_json::json!({
            "tab": raw_name,
            "mode": "update_in_place",
            "changes": {"rows": 0, "cells": 0, "columns": diff.columns_touched},
            "unchanged": {"rows": diff.rows_unchanged},
            "skipped": {
                "rows_not_in_target": diff.rows_skipped_not_in_target,
                "rows_null_key": diff.rows_skipped_null_key,
            },
        });
    }

    match client
        .batch_update_cells(spreadsheet_id, raw_name, cell_updates)
        .await
    {
        Ok(_) => {
            let mut resp = serde_json::json!({
            "tab": raw_name,
            "mode": "update_in_place",
            "changes": {
                "rows": diff.rows_changed,
                "cells": cells_to_write,
                "columns": diff.columns_touched,
            },
            "unchanged": {"rows": diff.rows_unchanged},
            "skipped": {
                "rows_not_in_target": diff.rows_skipped_not_in_target,
                "rows_null_key": diff.rows_skipped_null_key,
            },
            });
            formula_log.attach(&mut resp);
            resp
        }
        Err(e) => serde_json::json!({
            "tab": raw_name,
            "error": format!("batch_update_cells failed: {e}"),
        }),
    }
}

/// Validate that the returned df index is exactly the set `{0..n-1}` — i.e. the
/// model returned the WHOLE bound df (modified in place), not a filtered subset.
/// This is what makes positional write-back safe: a subset / `reset_index` /
/// `concat` fails here loudly instead of silently writing the wrong rows.
fn validate_full_index(df_index: &[serde_json::Value], n: usize) -> Result<(), String> {
    if df_index.len() != n {
        return Err(format!(
            "update_by_position needs the FULL df ({n} rows), got {}. Return the WHOLE df \
             modified in place — do NOT filter/subset the rows you return.",
            df_index.len()
        ));
    }
    let mut seen = vec![false; n];
    for v in df_index {
        let Some(idx) = v.as_i64().filter(|i| *i >= 0).map(|i| i as usize) else {
            return Err("the df index must be the original 0..N-1 integer row labels. Modify the \
                 bound df IN PLACE and return it whole — do NOT reset_index / sort+reset_index / concat."
                .to_string());
        };
        if idx >= n {
            return Err(format!(
                "row index {idx} is outside the loaded range 0..{n}. Return the whole bound df \
                 modified in place — do NOT add rows or reset_index."
            ));
        }
        if seen[idx] {
            return Err(
                "duplicate row index — the df index must be the original 0..N-1 labels \
                 (no concat/duplicates). Modify the bound df in place and return it whole."
                    .to_string(),
            );
        }
        seen[idx] = true;
    }
    Ok(())
}

/// Map each UNIQUELY-named, non-empty header column to its 0-based position.
/// Empty or duplicate header names are excluded — they can't be addressed by
/// name; the caller reports them as `skipped_columns`.
fn addressable_columns(header_cols: &[String]) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in header_cols {
        *counts.entry(c.as_str()).or_insert(0) += 1;
    }
    header_cols
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty() && counts.get(c.as_str()) == Some(&1))
        .map(|(i, c)| (c.clone(), i))
        .collect()
}

/// A single cell a new column needs: a header cell (row 1) or a body value.
/// `raw` is the value as it came from the df — formula resolution happens in the
/// caller, where the full column→index map (existing + new) is available.
#[derive(Debug, PartialEq)]
struct PlannedCell {
    col_idx: usize,
    /// 1-based; row 1 is the header.
    row: usize,
    raw: serde_json::Value,
}

/// What `update_by_position` should write for columns present in the returned df
/// but absent from the sheet header.
#[derive(Debug, Default, PartialEq)]
struct NewColumnPlan {
    /// (column name, 0-based column index) for each added column, in df order.
    added: Vec<(String, usize)>,
    /// Header cells (row 1) + body cells, in write order.
    cells: Vec<PlannedCell>,
}

/// For each df column whose name is NOT in `header_cols`, assign the next free
/// column index (appended after the last header column, in df-column order) and
/// emit its header cell plus one body cell per record with a non-null value.
/// A column whose body is entirely null is skipped (no orphan header). The sheet
/// row for record `i` is `df_index[i] + 2` (header is row 1).
fn plan_new_columns(
    header_cols: &[String],
    new_records: &[serde_json::Map<String, serde_json::Value>],
    df_index: &[serde_json::Value],
) -> NewColumnPlan {
    use std::collections::HashSet;
    let existing: HashSet<&str> = header_cols.iter().map(String::as_str).collect();

    // Discover new column names in first-seen df order, excluding the synthetic
    // index and any name already present in the header.
    let mut new_names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in new_records {
        for k in r.keys() {
            if k == "__index__" || existing.contains(k.as_str()) {
                continue;
            }
            if seen.insert(k.clone()) {
                new_names.push(k.clone());
            }
        }
    }

    let mut plan = NewColumnPlan::default();
    let mut next_idx = header_cols.len();
    for name in new_names {
        let mut body: Vec<PlannedCell> = Vec::new();
        for (i, r) in new_records.iter().enumerate() {
            let Some(v) = r.get(&name) else { continue };
            if v.is_null() {
                continue;
            }
            let Some(row_idx) = df_index.get(i).and_then(serde_json::Value::as_u64) else {
                continue;
            };
            body.push(PlannedCell {
                col_idx: next_idx,
                row: row_idx as usize + 2,
                raw: v.clone(),
            });
        }
        if body.is_empty() {
            continue; // entirely-null column → don't create an orphan header.
        }
        plan.cells.push(PlannedCell {
            col_idx: next_idx,
            row: 1,
            raw: serde_json::Value::String(name.clone()),
        });
        plan.cells.extend(body);
        plan.added.push((name, next_idx));
        next_idx += 1;
    }
    plan
}

/// Positional / index-based write-back. The model modifies the bound df IN
/// PLACE and returns the WHOLE df under `mode:'update_by_position'` (no key).
/// The dispatcher diffs it against the load snapshot by row index and writes
/// only the changed cells — no agent-computed A1 address, no unique key needed.
async fn do_update_by_position(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
    loaded: &std::collections::HashMap<String, LoadedSnapshot>,
) -> serde_json::Value {
    // 1. The tab must have been BOUND whole-sheet (unambiguously) this run.
    let snap = match loaded.get(raw_name) {
        None => {
            return serde_json::json!({
                "tab": raw_name,
                "error": "UpdateByPositionRequiresBinding",
                "message": format!(
                    "update_by_position needs the tab '{raw_name}' to be BOUND in this same \
                     gsheets_run_python call (whole sheet, no `range`)."
                ),
            })
        }
        Some(s) if s.ambiguous => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("the tab '{raw_name}' was bound more than once — can't pick a positional mapping."),
            })
        }
        Some(s) if s.had_range => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("update_by_position needs '{raw_name}' bound WITHOUT a `range` (whole sheet)."),
            })
        }
        Some(s) => s,
    };
    let n = snap.records.len();

    let new_records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();
    let df_index: Vec<serde_json::Value> = entry
        .get("df_index")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if df_index.len() != new_records.len() {
        return serde_json::json!({
            "tab": raw_name,
            "error": "internal: df_index/df_records length mismatch",
        });
    }

    // 2. Require the full {0..N-1} index (catches subset / reset_index / concat).
    if let Err(msg) = validate_full_index(&df_index, n) {
        return serde_json::json!({"tab": raw_name, "error": "InvalidIndex", "message": msg});
    }

    // 3. Positional header → addressable columns (skip empty/duplicate names).
    let header_read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            Some("1:1"),
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({"tab": raw_name, "error": format!("header read failed: {e}")})
        }
    };
    let header_cols: Vec<String> = match header_read.values {
        serde_json::Value::Array(rows) => rows
            .first()
            .and_then(|r| r.as_array())
            .map(|cells| {
                cells
                    .iter()
                    .map(|c| c.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let col_to_index = addressable_columns(&header_cols);

    // 4. Comparable columns = snapshot columns that are addressable.
    let snap_cols: Vec<String> = snap
        .records
        .first()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    let comparable: Vec<String> = snap_cols
        .iter()
        .filter(|c| col_to_index.contains_key(*c))
        .cloned()
        .collect();
    let skipped_columns: Vec<String> = snap_cols
        .iter()
        .filter(|c| !col_to_index.contains_key(*c))
        .cloned()
        .collect();

    // 5. Inject a synthetic `__index__` key into both sides (snapshot = position,
    //    new = df_index), projecting `new` to the comparable columns so a
    //    model-added column never trips the diff's column-mismatch check.
    const IDX: &str = "__index__";
    let cur: Vec<serde_json::Map<String, serde_json::Value>> = snap
        .records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut m = r.clone();
            m.insert(IDX.to_string(), serde_json::json!(i));
            m
        })
        .collect();
    let nw: Vec<serde_json::Map<String, serde_json::Value>> = new_records
        .iter()
        .zip(df_index.iter())
        .map(|(r, idx)| {
            let mut m = serde_json::Map::new();
            for c in &comparable {
                if let Some(v) = r.get(c) {
                    m.insert(c.clone(), v.clone());
                }
            }
            m.insert(IDX.to_string(), idx.clone());
            m
        })
        .collect();

    // 6. Reuse the existing cell-diff, keyed on the synthetic index.
    let diff = match diff_records(&cur, &nw, IDX, Some(&comparable), false, raw_name) {
        Ok(d) => d,
        Err(e) => return e.to_json(),
    };

    // 7. Map each change → A1 cell. The `__index__` value IS the original row
    //    position, so `sheet_row = index + 2` (header is row 1); the column
    //    comes from the positional header. No agent arithmetic anywhere.
    let mut cell_updates: Vec<(String, crate::gsheets::domain::CellValue)> = Vec::new();
    let mut formula_log = FormulaCellLog::default();
    for chg in &diff.changes {
        let (Ok(idx), Some(&col_idx)) = (
            chg.key_value.parse::<usize>(),
            col_to_index.get(&chg.column),
        ) else {
            continue;
        };
        let target_row = idx + 2;
        // Resolve any `{{Column}}` refs in a formula using the SAME positional
        // header + row we use to place the cell — so the model never computes A1.
        let resolved = match resolve_formula_placeholders(&chg.new_value, &col_to_index, target_row)
        {
            Ok(v) => v,
            Err(e) => return e.to_json(raw_name),
        };
        let addr = a1_addr(col_idx, target_row);
        formula_log.record(&addr, &resolved);
        cell_updates.push((
            addr,
            crate::gsheets::domain::CellValue::from_json(&resolved),
        ));
    }

    // Append columns present in the returned df but absent from the sheet header.
    // They get the next free column indices; their formulas may reference other
    // new columns, so resolve against existing + new positions.
    let new_plan = plan_new_columns(&header_cols, &new_records, &df_index);
    let added_columns: Vec<serde_json::Value> = new_plan
        .added
        .iter()
        .map(|(name, i)| serde_json::json!({ "name": name, "column": col_letter(*i) }))
        .collect();
    let new_column_names: Vec<String> = new_plan
        .added
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    if !new_plan.cells.is_empty() {
        let mut resolvable = col_to_index.clone();
        for (name, i) in &new_plan.added {
            resolvable.insert(name.clone(), *i);
        }
        for pc in &new_plan.cells {
            let resolved = match resolve_formula_placeholders(&pc.raw, &resolvable, pc.row) {
                Ok(v) => v,
                Err(e) => return e.to_json(raw_name),
            };
            let addr = a1_addr(pc.col_idx, pc.row);
            formula_log.record(&addr, &resolved);
            cell_updates.push((
                addr,
                crate::gsheets::domain::CellValue::from_json(&resolved),
            ));
        }
    }

    let cells = cell_updates.len();
    if cells == 0 {
        return serde_json::json!({
            "tab": raw_name, "mode": "update_by_position",
            "changes": {"rows": 0, "cells": 0},
            "skipped_columns": skipped_columns,
        });
    }
    match client
        .batch_update_cells(spreadsheet_id, raw_name, cell_updates)
        .await
    {
        Ok(_) => {
            let mut columns_touched = diff.columns_touched.clone();
            columns_touched.extend(new_column_names.iter().cloned());
            let mut resp = serde_json::json!({
                "tab": raw_name, "mode": "update_by_position",
                "changes": {"rows": diff.rows_changed, "cells": cells, "columns": columns_touched},
                "skipped_columns": skipped_columns,
            });
            if !added_columns.is_empty() {
                resp["added_columns"] = serde_json::Value::Array(added_columns.clone());
            }
            formula_log.attach(&mut resp);
            resp
        }
        Err(e) => serde_json::json!({
            "tab": raw_name, "error": format!("batch_update_cells failed: {e}"),
        }),
    }
}

/// Common DataFrame write — used by `replace`, `overwrite`, and the
/// auto_suffix paths. `name` is what gets written to. `raw_name` is what
/// the LLM asked for (for response labeling).
async fn write_full_df(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    use crate::gsheets::domain::CellValue;
    let records = entry.get("df_records").and_then(|v| v.as_array());
    let cols = entry.get("df_cols").and_then(|v| v.as_array());
    let (Some(records), Some(cols)) = (records, cols) else {
        return serde_json::json!({
            "name": raw_name,
            "error": "entry missing df_records or df_cols",
        });
    };
    let col_names: Vec<String> = cols
        .iter()
        .map(|c| c.as_str().unwrap_or("").to_string())
        .collect();
    // Resolve `{{Column}}` formula placeholders here too, so creating a tab WITH
    // a formula column (replace/overwrite) works like the diff-write modes.
    let resolvable = addressable_columns(&col_names);
    let header_row: Vec<CellValue> = col_names.iter().cloned().map(CellValue::String).collect();
    let mut matrix: Vec<Vec<CellValue>> = vec![header_row];
    let mut formula_log = FormulaCellLog::default();
    for rec in records {
        let Some(obj) = rec.as_object() else { continue };
        // The row about to be pushed lands at sheet row `matrix.len() + 1`
        // (matrix[0] is the header → sheet row 1).
        let target_row = matrix.len() + 1;
        let mut row: Vec<CellValue> = Vec::with_capacity(col_names.len());
        for (j, key) in col_names.iter().enumerate() {
            let raw = obj.get(key).unwrap_or(&serde_json::Value::Null);
            let resolved = match resolve_formula_placeholders(raw, &resolvable, target_row) {
                Ok(v) => v,
                Err(e) => return e.to_json(raw_name),
            };
            formula_log.record(&a1_addr(j, target_row), &resolved);
            row.push(CellValue::from_json(&resolved));
        }
        matrix.push(row);
    }
    let n_rows = matrix.len();
    let n_cols = cols.len();
    match client.set_range(spreadsheet_id, name, "A1", matrix).await {
        Ok(_) => {
            let mut resp = serde_json::json!({
                "name": raw_name,
                "resolved_name": name,
                "n_rows": n_rows,
                "n_cols": n_cols,
            });
            formula_log.attach(&mut resp);
            resp
        }
        Err(e) => serde_json::json!({
            "name": raw_name,
            "resolved_name": name,
            "error": format!("set_range failed: {e}"),
        }),
    }
}

/// Returns true if two column lists name the same set. Order-insensitive
/// (Sheets column order is not part of the schema contract) and
/// duplicate-insensitive (HashSet semantics). Used by the overwrite
/// schema-change guard.
fn columns_match(a: &[String], b: &[String]) -> bool {
    let sa: std::collections::HashSet<&String> = a.iter().collect();
    let sb: std::collections::HashSet<&String> = b.iter().collect();
    sa == sb
}

/// Fetch lightweight metadata for the target tab: row/col count from
/// list_sheets + column names from the header row.
async fn fetch_tab_meta(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    tab: &str,
) -> Result<Option<TabMeta>, crate::gsheets::domain::SheetsError> {
    let sheets = client.list_sheets(spreadsheet_id).await?;
    let Some(meta) = sheets.into_iter().find(|s| s.title == tab) else {
        return Ok(None);
    };
    // Header row — span the full width reported by `col_count` instead of a
    // hardcoded `A1:Z1`, so sheets with >26 columns surface every header
    // (otherwise `current_state.columns` truncated at column Z and the LLM
    // decided collisions with incomplete info).
    let last_col = meta.col_count.max(1) as usize - 1;
    let header_range = format!("A1:{}", a1_addr(last_col, 1));
    let read = client
        .read_range(
            spreadsheet_id,
            tab,
            Some(&header_range),
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await?;
    let columns: Vec<String> = match read.values {
        serde_json::Value::Array(rows) => rows
            .first()
            .and_then(|r| r.as_array())
            .map(|cells| {
                cells
                    .iter()
                    .map(|c| c.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    // Best-effort Drive `modifiedTime` — a collision envelope is more useful
    // when the LLM can tell fresh data from last year's. Never fail the whole
    // meta fetch over it.
    let last_modified = client
        .get_modified_time(spreadsheet_id)
        .await
        .ok()
        .flatten();
    Ok(Some(TabMeta {
        n_rows: meta.row_count as u64,
        n_cols: meta.col_count as u64,
        columns,
        last_modified,
    }))
}

/// Convert a 0-based column index to its A1 column letter(s):
/// `0 → "A"`, `25 → "Z"`, `26 → "AA"`, `27 → "AB"`. Shared by [`a1_addr`] and
/// the formula-placeholder resolver so a cell address and a `{{Column}}`
/// reference are computed by exactly the same rule.
fn col_letter(col_index: usize) -> String {
    let mut col = String::new();
    let mut n = col_index;
    loop {
        col.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    col
}

/// Convert column index + row index to an A1 string for `batch_update_cells`.
/// `row_index` is 1-based, where row 1 is the header.
///
/// `pub(super)` (rather than private): `gsheets_run_python.rs`'s own test
/// module exercises this directly (`header_range_spans_past_column_z`).
pub(super) fn a1_addr(col_index: usize, row_index: usize) -> String {
    format!("{}{}", col_letter(col_index), row_index)
}

/// A `{{Column}}` placeholder named a column that isn't addressable (unknown,
/// empty, or duplicate header name). Surfaced as a structured tool error so the
/// model can self-correct — far better than the silent `#VALUE!` you get when a
/// model hand-computes the wrong column letter inside a formula.
#[derive(Debug)]
struct FormulaResolveError {
    unknown: String,
    valid: Vec<String>,
}

impl FormulaResolveError {
    fn to_json(&self, tab: &str) -> serde_json::Value {
        serde_json::json!({
            "tab": tab,
            "error": "FormulaUnknownColumn",
            "unknown_column": self.unknown,
            "valid_columns": self.valid,
            "message": format!(
                "A formula references column {} which is not an addressable column in \
                 '{}'. Reference columns by their EXACT name (case-sensitive) using \
                 {{{{Name}}}} — it resolves to that column in the SAME row. Never compute \
                 column letters by hand. Valid columns: {:?}.",
                format!("{{{{{}}}}}", self.unknown),
                tab,
                self.valid,
            ),
        })
    }
}

/// Resolve `{{ColumnName}}` placeholders inside a formula string into real A1
/// refs for `target_row`, using `resolvable` (addressable header column name →
/// 0-based index). Only a String that starts with `=` is processed — numbers,
/// plain strings, and null are returned unchanged. Each `{{Name}}` resolves to
/// that column's cell in the SAME row (`<letter><target_row>`); a name that
/// isn't addressable returns `Err` so the caller aborts the write (no partial
/// writes) instead of producing a broken formula.
fn resolve_formula_placeholders(
    value: &serde_json::Value,
    resolvable: &std::collections::HashMap<String, usize>,
    target_row: usize,
) -> Result<serde_json::Value, FormulaResolveError> {
    let Some(s) = value.as_str() else {
        return Ok(value.clone());
    };
    // Only formulas carry column references; `{{` is meaningless elsewhere.
    if !s.starts_with('=') || !s.contains("{{") {
        return Ok(value.clone());
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        // A doubled `{{` opens a placeholder; a single `{` (Sheets array
        // literal, e.g. `={1,2;3,4}`) is copied through untouched.
        if bytes[i] == b'{' && i + 1 < s.len() && bytes[i + 1] == b'{' {
            if let Some(rel) = s[i + 2..].find("}}") {
                let name = s[i + 2..i + 2 + rel].trim();
                match resolvable.get(name) {
                    Some(&idx) => {
                        out.push_str(&col_letter(idx));
                        out.push_str(&target_row.to_string());
                        i = i + 2 + rel + 2;
                        continue;
                    }
                    None => {
                        let mut valid: Vec<String> = resolvable.keys().cloned().collect();
                        valid.sort();
                        return Err(FormulaResolveError {
                            unknown: name.to_string(),
                            valid,
                        });
                    }
                }
            }
        }
        // Copy one full char (keeps us on UTF-8 boundaries).
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(serde_json::Value::String(out))
}

/// Max resolved-formula cells echoed back in a tool result. The model uses these
/// to report what it wrote truthfully (instead of recomputing A1 by hand, which
/// it gets wrong); a small sample is enough for a confirmation message. When the
/// write exceeds this (e.g. a whole-column fill), the response also carries
/// `formula_cells_total` + `formula_cells_truncated` so the model reports the
/// real count instead of mistaking the 50-cell sample for everything.
const FORMULA_CELLS_SAMPLE_CAP: usize = 50;

/// Accumulates the formulas a diff-write lands: a bounded `sample` (real A1 →
/// formula text) the model can quote verbatim, plus the `total` count so it
/// knows when the sample is partial.
#[derive(Default)]
struct FormulaCellLog {
    sample: serde_json::Map<String, serde_json::Value>,
    total: usize,
}

impl FormulaCellLog {
    /// Record `resolved` if it is a formula (`=…`). Counts every formula; only
    /// the first [`FORMULA_CELLS_SAMPLE_CAP`] land in the echoed sample.
    fn record(&mut self, addr: &str, resolved: &serde_json::Value) {
        if let Some(s) = resolved.as_str() {
            if s.starts_with('=') {
                self.total += 1;
                if self.sample.len() < FORMULA_CELLS_SAMPLE_CAP {
                    self.sample
                        .insert(addr.to_string(), serde_json::Value::String(s.to_string()));
                }
            }
        }
    }

    /// Attach `formula_cells` (+ `formula_cells_total`/`_truncated` when the
    /// sample is partial) to a success response. No-op when nothing was a formula.
    fn attach(self, resp: &mut serde_json::Value) {
        if self.total == 0 {
            return;
        }
        let Some(obj) = resp.as_object_mut() else {
            return;
        };
        let truncated = self.total > self.sample.len();
        obj.insert(
            "formula_cells".to_string(),
            serde_json::Value::Object(self.sample),
        );
        if truncated {
            obj.insert(
                "formula_cells_total".to_string(),
                serde_json::json!(self.total),
            );
            obj.insert(
                "formula_cells_truncated".to_string(),
                serde_json::json!(true),
            );
        }
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;
    use serde_json::json;

    fn idx(v: &[i64]) -> Vec<serde_json::Value> {
        v.iter().map(|i| json!(i)).collect()
    }

    #[test]
    fn full_index_accepts_any_permutation_of_0_to_n() {
        assert!(validate_full_index(&idx(&[0, 1, 2, 3]), 4).is_ok());
        // sort() reorders labels but the set is still {0..N-1} → fine.
        assert!(validate_full_index(&idx(&[3, 0, 2, 1]), 4).is_ok());
    }

    #[test]
    fn full_index_rejects_subset() {
        // df.loc[mask] returning 2 of 4 rows → wrong-row footgun → rejected.
        let e = validate_full_index(&idx(&[1, 2]), 4).unwrap_err();
        assert!(e.contains("FULL df"));
    }

    #[test]
    fn full_index_rejects_out_of_range_and_duplicates() {
        assert!(validate_full_index(&idx(&[0, 1, 9]), 3).is_err()); // 9 >= N
        assert!(validate_full_index(&idx(&[0, 0, 1]), 3).is_err()); // duplicate (concat)
                                                                    // non-integer label (e.g. a string index)
        assert!(validate_full_index(&[json!("a"), json!("b")], 2).is_err());
    }

    #[test]
    fn addressable_skips_empty_and_duplicate_header_names() {
        // Mirrors the real "Hoja 16": two empty-named columns + dup.
        let header = vec![
            "CLIENT ID".to_string(),
            "".to_string(),
            "Cantidad".to_string(),
            "Tarifa".to_string(),
            "Importe".to_string(),
            "".to_string(),
            "Tarifa".to_string(), // duplicate name
        ];
        let map = addressable_columns(&header);
        assert_eq!(map.get("CLIENT ID"), Some(&0));
        assert_eq!(map.get("Cantidad"), Some(&2));
        assert_eq!(map.get("Importe"), Some(&4));
        // empty + duplicate names are NOT addressable.
        assert!(!map.contains_key(""));
        assert!(!map.contains_key("Tarifa"));
    }

    #[test]
    fn a1_addr_maps_position_to_letter() {
        // col 0→A, 18→S, 20→U, 21→V; row passed 1-based.
        assert_eq!(a1_addr(0, 2), "A2");
        assert_eq!(a1_addr(21, 20), "V20"); // Importe at sheet row 20
        assert_eq!(a1_addr(20, 20), "U20"); // Tarifa — the column pro wrongly hit
    }

    #[test]
    fn col_letter_handles_columns_past_z() {
        assert_eq!(col_letter(0), "A");
        assert_eq!(col_letter(18), "S"); // Cantidad
        assert_eq!(col_letter(25), "Z");
        assert_eq!(col_letter(26), "AA");
        assert_eq!(col_letter(27), "AB");
    }

    fn cols(pairs: &[(&str, usize)]) -> std::collections::HashMap<String, usize> {
        pairs.iter().map(|(n, i)| (n.to_string(), *i)).collect()
    }

    #[test]
    fn resolve_per_row_formula_to_real_a1() {
        // The headline case: the model writes column NAMES; the dispatcher emits
        // real A1 for the SAME row — `=S5*U5`, not the model's off-by-one `=R5*T5`.
        let map = cols(&[("Cantidad", 18), ("Tarifa", 20)]);
        let out =
            resolve_formula_placeholders(&json!("={{Cantidad}}*{{Tarifa}}"), &map, 5).unwrap();
        assert_eq!(out, json!("=S5*U5"));
    }

    #[test]
    fn resolve_column_name_with_space() {
        let map = cols(&[("CLIENT ID", 0)]);
        let out = resolve_formula_placeholders(&json!("={{CLIENT ID}}"), &map, 3).unwrap();
        assert_eq!(out, json!("=A3"));
    }

    #[test]
    fn resolve_unknown_column_errors_with_valid_list() {
        let map = cols(&[("Cantidad", 18), ("Tarifa", 20)]);
        let err = resolve_formula_placeholders(&json!("={{Cantdad}}*2"), &map, 5).unwrap_err();
        assert_eq!(err.unknown, "Cantdad");
        assert!(err.valid.contains(&"Cantidad".to_string()));
    }

    #[test]
    fn resolve_leaves_non_formula_and_scalars_untouched() {
        let map = cols(&[("x", 0)]);
        // braces but no leading '=' → not a formula → untouched
        assert_eq!(
            resolve_formula_placeholders(&json!("hola {{x}}"), &map, 2).unwrap(),
            json!("hola {{x}}")
        );
        assert_eq!(
            resolve_formula_placeholders(&json!(5), &map, 2).unwrap(),
            json!(5)
        );
        assert_eq!(
            resolve_formula_placeholders(&json!(null), &map, 2).unwrap(),
            json!(null)
        );
    }

    #[test]
    fn resolve_leaves_single_brace_array_literal_untouched() {
        // `={1,2;3,4}` is a real Sheets array literal — single braces must survive.
        let map = cols(&[("A", 0), ("B", 1)]);
        let out = resolve_formula_placeholders(&json!("={1,2;3,4}"), &map, 7).unwrap();
        assert_eq!(out, json!("={1,2;3,4}"));
    }

    #[test]
    fn resolve_multiple_tokens_with_surrounding_text() {
        let map = cols(&[("A", 2), ("B", 3)]); // C, D
        let out = resolve_formula_placeholders(&json!("=IF({{A}}>0,{{B}},0)"), &map, 3).unwrap();
        assert_eq!(out, json!("=IF(C3>0,D3,0)"));
    }

    #[test]
    fn formula_cells_echo_only_formulas_for_truthful_reporting() {
        let mut log = FormulaCellLog::default();
        log.record("V5", &json!("=S5*U5")); // formula → recorded
        log.record("B2", &json!(42)); // number → ignored
        log.record("C2", &json!("plain text")); // non-formula → ignored
        assert_eq!(log.total, 1);
        assert_eq!(log.sample.get("V5"), Some(&json!("=S5*U5")));

        let mut resp = json!({"tab": "Hoja 16"});
        log.attach(&mut resp);
        assert_eq!(resp["formula_cells"]["V5"], json!("=S5*U5"));
        // small write → no truncation signal
        assert!(resp.get("formula_cells_total").is_none());

        // empty set → field omitted entirely (no noise on pure-value writes)
        let mut resp2 = json!({"tab": "Hoja 16"});
        FormulaCellLog::default().attach(&mut resp2);
        assert!(resp2.get("formula_cells").is_none());
    }

    #[test]
    fn formula_cells_signal_truncation_past_cap() {
        // A whole-column fill (e.g. 130 rows) exceeds the 50-cell sample → the
        // response must announce the real total so the model reports it honestly.
        let mut log = FormulaCellLog::default();
        for r in 2..132 {
            log.record(&format!("V{r}"), &json!(format!("=S{r}*U{r}")));
        }
        assert_eq!(log.total, 130);
        let mut resp = json!({"tab": "Ventas"});
        log.attach(&mut resp);
        assert_eq!(
            resp["formula_cells"].as_object().unwrap().len(),
            FORMULA_CELLS_SAMPLE_CAP
        );
        assert_eq!(resp["formula_cells_total"], json!(130));
        assert_eq!(resp["formula_cells_truncated"], json!(true));
    }

    fn rec(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn plan_new_columns_appends_after_last_header() {
        let header = vec!["a".to_string(), "b".to_string()]; // A, B occupied → new at C (idx 2)
        let new = vec![
            rec(&[("a", json!(1)), ("b", json!(2)), ("Margen", json!("=C-D"))]),
            rec(&[("a", json!(3)), ("b", json!(4)), ("Margen", json!("=C-D"))]),
        ];
        let plan = plan_new_columns(&header, &new, &idx(&[0, 1]));
        assert_eq!(plan.added, vec![("Margen".to_string(), 2)]);
        assert_eq!(
            plan.cells,
            vec![
                PlannedCell {
                    col_idx: 2,
                    row: 1,
                    raw: json!("Margen")
                },
                PlannedCell {
                    col_idx: 2,
                    row: 2,
                    raw: json!("=C-D")
                },
                PlannedCell {
                    col_idx: 2,
                    row: 3,
                    raw: json!("=C-D")
                },
            ]
        );
    }

    #[test]
    fn plan_new_columns_multiple_get_consecutive_indices() {
        let header = vec!["a".to_string()]; // A occupied → new at B(1), C(2)
        let new = vec![rec(&[("a", json!(1)), ("x", json!(10)), ("y", json!(20))])];
        let plan = plan_new_columns(&header, &new, &idx(&[0]));
        assert_eq!(plan.added, vec![("x".to_string(), 1), ("y".to_string(), 2)]);
    }

    #[test]
    fn plan_new_columns_ignores_existing_header_names() {
        let header = vec!["a".to_string(), "b".to_string()];
        let new = vec![rec(&[("a", json!(1)), ("b", json!(9))])]; // both already in header
        let plan = plan_new_columns(&header, &new, &idx(&[0]));
        assert!(plan.added.is_empty());
        assert!(plan.cells.is_empty());
    }

    #[test]
    fn plan_new_columns_skips_all_null_column() {
        let header = vec!["a".to_string()];
        let new = vec![
            rec(&[("a", json!(1)), ("Empty", serde_json::Value::Null)]),
            rec(&[("a", json!(2)), ("Empty", serde_json::Value::Null)]),
        ];
        let plan = plan_new_columns(&header, &new, &idx(&[0, 1]));
        assert!(
            plan.added.is_empty(),
            "all-null column must not create an orphan header"
        );
        assert!(plan.cells.is_empty());
    }

    #[test]
    fn plan_new_columns_uses_df_index_for_row_mapping() {
        let header = vec!["a".to_string()]; // new col at B(1)
                                            // df_index out of natural order: record 0 → sheet row 3, record 1 → row 2.
        let new = vec![
            rec(&[("a", json!(1)), ("m", json!("r3"))]),
            rec(&[("a", json!(2)), ("m", json!("r2"))]),
        ];
        let plan = plan_new_columns(&header, &new, &idx(&[1, 0]));
        assert_eq!(
            plan.cells,
            vec![
                PlannedCell {
                    col_idx: 1,
                    row: 1,
                    raw: json!("m")
                },
                PlannedCell {
                    col_idx: 1,
                    row: 3,
                    raw: json!("r3")
                },
                PlannedCell {
                    col_idx: 1,
                    row: 2,
                    raw: json!("r2")
                },
            ]
        );
    }
}
