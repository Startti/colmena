//! Shared `update_in_place` diff algorithm for sheet write dispatchers.
//!
//! Given the current records of a tab + a new records list from the
//! LLM's pandas code + a key column + optional column whitelist,
//! compute the cell-level changes required to make the tab match the
//! new state, OR an error code matching the spec validation table.
//!
//! Pure: no I/O, no async, no Sheets/CRDT specifics. Dispatchers
//! translate the resulting cell-level changes into their respective
//! write APIs (`batch_update_cells` for gsheets,
//! `apply_set_cell_in_proc` for crdt).

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Cap on how many duplicate-key examples we surface in the error
/// envelope so the LLM has enough to debug without bloating tokens.
const MAX_DUP_EXAMPLES: usize = 3;

/// A single cell change: replace the value at row indexed by `key_value`,
/// column `column`, with `new_value`. The row's A1 address is computed
/// by the caller (which knows where the header lives in the tab).
#[derive(Debug, Clone, PartialEq)]
pub struct CellChange {
    pub key_value: String,
    pub column: String,
    pub old_value: Value,
    pub new_value: Value,
}

/// Successful diff result.
#[derive(Debug, Clone, Default)]
pub struct DiffResult {
    /// Cells to write.
    pub changes: Vec<CellChange>,
    /// How many input rows had at least one cell change.
    pub rows_changed: usize,
    /// How many input rows existed in current AND had no changes.
    pub rows_unchanged: usize,
    /// How many input rows had key values not present in current.
    pub rows_skipped_not_in_target: usize,
    /// How many input rows had null/missing key — dropped from the diff.
    pub rows_skipped_null_key: usize,
    /// Columns actually touched (subset of requested columns).
    pub columns_touched: Vec<String>,
}

/// Error result. Variants map 1-to-1 with the spec's validation table.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffError {
    KeyColumnMissingInTarget {
        key: String,
        available: Vec<String>,
    },
    KeyColumnMissingInInput {
        key: String,
        available: Vec<String>,
    },
    DuplicateKeyInTarget {
        key: String,
        examples: Vec<String>,
    },
    DuplicateKeyInInput {
        key: String,
        examples: Vec<String>,
    },
    ColumnMismatch {
        target_columns: Vec<String>,
        input_columns: Vec<String>,
        extra: Vec<String>,
        tab: String,
    },
    StrictMatchFailed {
        unmatched_keys: Vec<String>,
    },
}

impl DiffError {
    /// JSON envelope for return to the LLM.
    pub fn to_json(&self) -> Value {
        match self {
            Self::KeyColumnMissingInTarget { key, available } => serde_json::json!({
                "error": "KeyColumnMissing",
                "where": "target",
                "key": key,
                "available_columns": available,
                "message": format!("Key column '{key}' not found in target tab. Available columns: {available:?}."),
            }),
            Self::KeyColumnMissingInInput { key, available } => serde_json::json!({
                "error": "KeyColumnMissing",
                "where": "input",
                "key": key,
                "available_columns": available,
                "message": format!("Key column '{key}' not in your DataFrame. DataFrame columns: {available:?}."),
            }),
            Self::DuplicateKeyInTarget { key, examples } => serde_json::json!({
                "error": "DuplicateKeyInTarget",
                "key": key,
                "duplicate_examples": examples,
                "message": format!(
                    "Cannot update_in_place: key '{key}' has duplicate values in the target tab \
                     (examples: {examples:?}). Use mode=overwrite, or pick a unique-valued key column."
                ),
            }),
            Self::DuplicateKeyInInput { key, examples } => serde_json::json!({
                "error": "DuplicateKeyInInput",
                "key": key,
                "duplicate_examples": examples,
                "message": format!(
                    "Cannot update_in_place: key '{key}' has duplicate values in your DataFrame \
                     (examples: {examples:?}). Each row must be uniquely identified."
                ),
            }),
            Self::ColumnMismatch {
                target_columns,
                input_columns,
                extra,
                tab,
            } => serde_json::json!({
                "error": "ColumnMismatch",
                "tab": tab,
                "target_columns": target_columns,
                "input_columns": input_columns,
                "extra_columns_in_input": extra,
                "message": format!(
                    "Column mismatch: your DataFrame has columns {input_columns:?} but tab '{tab}' \
                     has {target_columns:?}. Column(s) {extra:?} don't exist in the tab. \
                     RECOMMENDED: use a different tab name (e.g., '{tab}_enriched') to write the \
                     new shape as a fresh tab."
                ),
            }),
            Self::StrictMatchFailed { unmatched_keys } => serde_json::json!({
                "error": "StrictMatchFailed",
                "unmatched_keys": unmatched_keys,
                "message": format!(
                    "strict_match=true and {} key value(s) in your DataFrame have no match in the \
                     target tab. Unmatched: {unmatched_keys:?}.",
                    unmatched_keys.len()
                ),
            }),
        }
    }
}

/// Compute the diff. `current` and `new` are records-shaped: each entry is
/// a JSON object with column names as keys. `key` is the column whose
/// values uniquely identify rows. `restrict_columns`, if `Some`, limits
/// the diff to those columns (filtering noise); else uses all common
/// columns between `current` and `new` (minus the key).
pub fn diff_records(
    current: &[Map<String, Value>],
    new: &[Map<String, Value>],
    key: &str,
    restrict_columns: Option<&[String]>,
    strict_match: bool,
    tab_label: &str,
) -> Result<DiffResult, DiffError> {
    // --- 1. Validate key existence ---
    let cur_cols: Vec<String> = column_set(current);
    let new_cols: Vec<String> = column_set(new);

    let cur_col_set: HashSet<String> = cur_cols.iter().cloned().collect();
    let new_col_set: HashSet<String> = new_cols.iter().cloned().collect();

    if !current.is_empty() && !cur_col_set.contains(key) {
        return Err(DiffError::KeyColumnMissingInTarget {
            key: key.to_string(),
            available: cur_cols,
        });
    }
    if !new.is_empty() && !new_col_set.contains(key) {
        return Err(DiffError::KeyColumnMissingInInput {
            key: key.to_string(),
            available: new_cols,
        });
    }

    // --- 2. Column mismatch (extras in new not in target) ---
    let extra: Vec<String> = new_cols
        .iter()
        .filter(|c| !cur_col_set.contains(c.as_str()))
        .cloned()
        .collect();
    if !extra.is_empty() {
        return Err(DiffError::ColumnMismatch {
            target_columns: cur_cols.clone(),
            input_columns: new_cols.clone(),
            extra,
            tab: tab_label.to_string(),
        });
    }

    // --- 3. Duplicate keys (strict — any duplicate triggers error) ---
    let dup_cur = duplicate_examples(current, key, MAX_DUP_EXAMPLES);
    if !dup_cur.is_empty() {
        return Err(DiffError::DuplicateKeyInTarget {
            key: key.to_string(),
            examples: dup_cur,
        });
    }
    let dup_new = duplicate_examples(new, key, MAX_DUP_EXAMPLES);
    if !dup_new.is_empty() {
        return Err(DiffError::DuplicateKeyInInput {
            key: key.to_string(),
            examples: dup_new,
        });
    }

    // --- 4. Index target by key string (skipping nulls) ---
    let cur_index: HashMap<String, &Map<String, Value>> = current
        .iter()
        .filter_map(|r| r.get(key).and_then(key_to_string).map(|s| (s, r)))
        .collect();

    // --- 5. Decide which columns to compare ---
    let comparable: Vec<String> = match restrict_columns {
        Some(list) => {
            // Reject any restrict column not present on either side.
            let missing: Vec<String> = list
                .iter()
                .filter(|c| !cur_col_set.contains(*c) || !new_col_set.contains(*c))
                .cloned()
                .collect();
            if !missing.is_empty() {
                return Err(DiffError::ColumnMismatch {
                    target_columns: cur_cols.clone(),
                    input_columns: new_cols.clone(),
                    extra: missing,
                    tab: tab_label.to_string(),
                });
            }
            list.iter().filter(|c| c.as_str() != key).cloned().collect()
        }
        None => {
            let mut cols: Vec<String> = cur_col_set
                .intersection(&new_col_set)
                .filter(|c| c.as_str() != key)
                .cloned()
                .collect();
            cols.sort();
            cols
        }
    };

    // --- 6. Compute cell changes row-by-row ---
    let mut out = DiffResult::default();
    let mut unmatched_keys: Vec<String> = Vec::new();
    let mut touched_cols: HashSet<String> = HashSet::new();
    for row in new {
        let Some(k_val) = row.get(key) else {
            out.rows_skipped_null_key += 1;
            continue;
        };
        let Some(k_str) = key_to_string(k_val) else {
            out.rows_skipped_null_key += 1;
            continue;
        };
        let Some(cur_row) = cur_index.get(&k_str) else {
            out.rows_skipped_not_in_target += 1;
            unmatched_keys.push(k_str);
            continue;
        };
        let mut row_changed = false;
        for col in &comparable {
            let old = cur_row.get(col).cloned().unwrap_or(Value::Null);
            let new_v = row.get(col).cloned().unwrap_or(Value::Null);
            if !values_equal(&old, &new_v) {
                out.changes.push(CellChange {
                    key_value: k_str.clone(),
                    column: col.clone(),
                    old_value: old,
                    new_value: new_v,
                });
                touched_cols.insert(col.clone());
                row_changed = true;
            }
        }
        if row_changed {
            out.rows_changed += 1;
        } else {
            out.rows_unchanged += 1;
        }
    }
    if strict_match && !unmatched_keys.is_empty() {
        return Err(DiffError::StrictMatchFailed { unmatched_keys });
    }
    out.columns_touched = {
        let mut v: Vec<String> = touched_cols.into_iter().collect();
        v.sort();
        v
    };
    Ok(out)
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Union of all column names found across records, in stable order
/// (first-seen wins).
fn column_set(records: &[Map<String, Value>]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for r in records {
        for k in r.keys() {
            if seen.insert(k.clone()) {
                out.push(k.clone());
            }
        }
    }
    out
}

/// Return up to `n` duplicate key examples found in records.
fn duplicate_examples(records: &[Map<String, Value>], key: &str, n: usize) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for r in records {
        if let Some(k) = r.get(key).and_then(key_to_string) {
            *counts.entry(k).or_insert(0) += 1;
        }
    }
    let mut dups: Vec<String> = counts
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .map(|(k, _)| k)
        .collect();
    dups.sort();
    dups.truncate(n);
    dups
}

/// Convert a JSON value used as a row key to a comparable string.
/// Returns `None` for null/array/object — those rows are silently
/// dropped from the diff.
fn key_to_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// NaN-safe value equality. Two nulls compare equal; two NaNs compare equal;
/// everything else uses serde_json's PartialEq.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Number(x), Value::Number(y)) => {
            let (xf, yf) = (x.as_f64(), y.as_f64());
            match (xf, yf) {
                (Some(a), Some(b)) if a.is_nan() && b.is_nan() => true,
                _ => xf == yf,
            }
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(fields: &[(&str, Value)]) -> Map<String, Value> {
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn diff_finds_changed_cells_only() {
        let current = vec![
            rec(&[("id", json!("a")), ("price", json!(10))]),
            rec(&[("id", json!("b")), ("price", json!(20))]),
            rec(&[("id", json!("c")), ("price", json!(30))]),
        ];
        let new = vec![
            rec(&[("id", json!("a")), ("price", json!(10))]), // no change
            rec(&[("id", json!("b")), ("price", json!(99))]), // changed
            rec(&[("id", json!("c")), ("price", json!(99))]), // changed
        ];
        let r = diff_records(&current, &new, "id", None, false, "Sales").unwrap();
        assert_eq!(r.changes.len(), 2);
        assert_eq!(r.rows_changed, 2);
        assert_eq!(r.rows_unchanged, 1);
        assert_eq!(r.rows_skipped_not_in_target, 0);
        assert_eq!(r.columns_touched, vec!["price".to_string()]);
    }

    #[test]
    fn restrict_columns_skips_other_diffs() {
        let current = vec![rec(&[
            ("id", json!("a")),
            ("price", json!(10)),
            ("stock", json!(5)),
        ])];
        let new = vec![rec(&[
            ("id", json!("a")),
            ("price", json!(99)),
            ("stock", json!(999)),
        ])];
        let r = diff_records(
            &current,
            &new,
            "id",
            Some(&["price".to_string()]),
            false,
            "Sales",
        )
        .unwrap();
        assert_eq!(r.changes.len(), 1);
        assert_eq!(r.changes[0].column, "price");
    }

    #[test]
    fn unmatched_keys_silently_skipped_by_default() {
        let current = vec![rec(&[("id", json!("a")), ("price", json!(1))])];
        let new = vec![
            rec(&[("id", json!("a")), ("price", json!(2))]),
            rec(&[("id", json!("ZZ")), ("price", json!(2))]), // not in current
        ];
        let r = diff_records(&current, &new, "id", None, false, "S").unwrap();
        assert_eq!(r.changes.len(), 1);
        assert_eq!(r.rows_skipped_not_in_target, 1);
    }

    #[test]
    fn strict_match_rejects_unmatched_keys() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let new = vec![rec(&[("id", json!("missing")), ("v", json!(2))])];
        let err = diff_records(&current, &new, "id", None, true, "S").unwrap_err();
        assert!(matches!(err, DiffError::StrictMatchFailed { .. }));
    }

    #[test]
    fn duplicate_keys_in_target_rejected() {
        let current = vec![
            rec(&[("id", json!("a")), ("v", json!(1))]),
            rec(&[("id", json!("a")), ("v", json!(2))]),
        ];
        let new = vec![rec(&[("id", json!("a")), ("v", json!(99))])];
        let err = diff_records(&current, &new, "id", None, false, "S").unwrap_err();
        assert!(matches!(err, DiffError::DuplicateKeyInTarget { .. }));
    }

    #[test]
    fn duplicate_keys_in_input_rejected() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let new = vec![
            rec(&[("id", json!("a")), ("v", json!(2))]),
            rec(&[("id", json!("a")), ("v", json!(3))]),
        ];
        let err = diff_records(&current, &new, "id", None, false, "S").unwrap_err();
        assert!(matches!(err, DiffError::DuplicateKeyInInput { .. }));
    }

    #[test]
    fn key_missing_in_target() {
        let current = vec![rec(&[("other", json!("x"))])];
        let new = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let err = diff_records(&current, &new, "id", None, false, "S").unwrap_err();
        assert!(matches!(err, DiffError::KeyColumnMissingInTarget { .. }));
    }

    #[test]
    fn key_missing_in_input() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let new = vec![rec(&[("v", json!(2))])];
        let err = diff_records(&current, &new, "id", None, false, "S").unwrap_err();
        assert!(matches!(err, DiffError::KeyColumnMissingInInput { .. }));
    }

    #[test]
    fn extra_columns_in_input_rejected() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let new = vec![rec(&[
            ("id", json!("a")),
            ("v", json!(2)),
            ("extra_col", json!("x")),
        ])];
        let err = diff_records(&current, &new, "id", None, false, "S").unwrap_err();
        match err {
            DiffError::ColumnMismatch { extra, .. } => {
                assert!(extra.contains(&"extra_col".to_string()));
            }
            other => panic!("expected ColumnMismatch, got {other:?}"),
        }
    }

    #[test]
    fn empty_new_returns_zero_changes() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let new: Vec<Map<String, Value>> = vec![];
        let r = diff_records(&current, &new, "id", None, false, "S").unwrap();
        assert_eq!(r.changes.len(), 0);
        assert_eq!(r.rows_changed, 0);
    }

    #[test]
    fn null_keys_silently_dropped() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(1))])];
        let new = vec![
            rec(&[("id", json!(null)), ("v", json!(99))]),
            rec(&[("id", json!("a")), ("v", json!(2))]),
        ];
        let r = diff_records(&current, &new, "id", None, false, "S").unwrap();
        assert_eq!(r.changes.len(), 1);
        assert_eq!(r.rows_skipped_null_key, 1);
    }

    #[test]
    fn null_to_null_is_no_change() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(null))])];
        let new = vec![rec(&[("id", json!("a")), ("v", json!(null))])];
        let r = diff_records(&current, &new, "id", None, false, "S").unwrap();
        assert_eq!(r.changes.len(), 0);
    }

    #[test]
    fn nan_to_nan_is_no_change() {
        use serde_json::Number;
        let nan_val = Value::Number(Number::from_f64(f64::NAN).unwrap_or_else(|| Number::from(0)));
        // If serde_json refuses NaN (it does by default), skip the test.
        if nan_val == Value::Number(Number::from(0)) {
            eprintln!("SKIPPED: serde_json refuses NaN serialization in this build");
            return;
        }
        assert!(
            values_equal(&nan_val, &nan_val),
            "NaN should compare equal to itself"
        );
    }

    #[test]
    fn diff_error_to_json_has_message_and_code() {
        let e = DiffError::KeyColumnMissingInTarget {
            key: "id".into(),
            available: vec!["x".into(), "y".into()],
        };
        let j = e.to_json();
        assert_eq!(j["error"], "KeyColumnMissing");
        assert_eq!(j["where"], "target");
        assert_eq!(j["key"], "id");
        assert!(j["message"].as_str().unwrap().contains("id"));
    }
}
