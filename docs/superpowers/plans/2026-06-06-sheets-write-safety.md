# Sheets Write Safety — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the gsheets and crdt_doc dispatchers from silently overwriting existing tabs, and give the LLM a token-cheap way to PATCH specific rows in a large sheet via diff-writing.

**Architecture:** Two dispatchers (`gsheets_run_python` and `crdt_doc_run_python`) gain a shared collision-policy module (default `on_existing_sheet: "fail"` returning a structured `SheetExists` error) and a shared diff-writer module that computes cell-level changes between current sheet state and a pandas DataFrame, then applies them via one `batchUpdate` (gsheets) or per-cell ops (crdt). The legacy single-tab path (`write_to_sheet` + `output_sheet`) in `crdt_doc_run_python` is removed; 3 test graphs are migrated. The pandas postlude is extended so `output_sheets` entries can be either a bare DataFrame (current behavior, mode `replace`) or a spec dict (`{mode, df, key, columns}`).

**Tech Stack:** Rust 1.95, PyO3 (sandbox), pandas (postlude), `google_sheets_v4` REST API (batchUpdate), `yrs` (CRDT), `tokio`/`async-trait`.

**Spec:** `docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`

---

## File Map

### Create
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs` — shared records-diff algorithm (pure, no I/O).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs` — `CollisionPolicy` enum + parsing + error envelope builder (data-only; no API calls).
- `tests/graphs/agents/gsheets_update_in_place.json` — E2E test graph.

### Modify
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — declare new modules.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` — wire policy + spec-dict modes + diff-writer.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs` — same + remove legacy single-tab path.
- `src/libs/colmena/src/gsheets/domain/traits.rs` — add `batch_update_cells` method to `SheetsClient`.
- `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` — impl `batch_update_cells`.
- `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md` — emit spec-dict shape.
- `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md` — emit spec-dict shape, drop `output_sheet` block.
- `src/libs/colmena/text/tools/gsheets.yaml` — document new modes + collision policy.
- `src/libs/colmena/text/tools/crdt_doc.yaml` — same + drop legacy mentions.
- `src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md` — add "updating existing tabs" section.
- `src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md` — mirror.
- `tests/graphs/crdt_documents/c_import_analysis.json` — migrate off legacy.
- `tests/graphs/crdt_documents/f_cross_artifact_smoke.json` — migrate.
- `tests/graphs/crdt_documents/c_pandas_smoke.json` — migrate.

---

## Task 1: Shared `sheet_collision.rs` — policy enum + error envelope

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

This module is pure data — no async, no I/O. It owns the `CollisionPolicy` enum, the `parse_policy` function (with friendly error on unknown values), and a `build_sheet_exists_error` function that takes metadata and returns the JSON envelope from the spec. The dispatchers call it.

- [ ] **Step 1: Add module declaration**

Open `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` and add (alphabetically, near other `pub mod` lines):

```rust
pub mod diff_writer;
pub mod sheet_collision;
```

(`diff_writer` is created in Task 2 — declaring both now avoids a second mod.rs edit.)

- [ ] **Step 2: Write failing test for policy parsing**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs` with:

```rust
//! Shared collision-policy + error-envelope helpers for sheet write
//! dispatchers (`gsheets_run_python` and `crdt_doc_run_python`).
//!
//! This module is data-only: no async, no I/O. The dispatchers fetch
//! the live metadata and call into here to (a) parse the operator-
//! supplied policy and (b) build the structured "SheetExists" error
//! returned to the LLM when the policy is `fail`.

use serde::{Deserialize, Serialize};

/// What the dispatcher does when a target tab already exists.
/// Configurable per-tool via `fixed_config.on_existing_sheet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Default. Dispatcher returns a structured `SheetExists` error
    /// without writing anything. LLM must explicitly choose
    /// `update_in_place`, `overwrite`, or rename.
    Fail,
    /// Legacy behavior — write as `"Name (2)"` silently.
    AutoSuffix,
    /// Replace the existing tab. Operator accepts the risk; the LLM
    /// doesn't get a round-trip.
    Overwrite,
}

impl Default for CollisionPolicy {
    fn default() -> Self {
        Self::Fail
    }
}

/// Parse the operator-supplied policy string. Unknown values yield a
/// clear error listing valid options.
pub fn parse_policy(s: &str) -> Result<CollisionPolicy, String> {
    match s {
        "fail" => Ok(CollisionPolicy::Fail),
        "auto_suffix" => Ok(CollisionPolicy::AutoSuffix),
        "overwrite" => Ok(CollisionPolicy::Overwrite),
        other => Err(format!(
            "unknown on_existing_sheet value '{other}'; valid: fail, auto_suffix, overwrite"
        )),
    }
}

/// Metadata about an existing tab that the LLM tried to write to.
/// Surfaced inside the `SheetExists` error so the LLM can decide
/// what to do (rename / update / overwrite) without another round-trip.
#[derive(Debug, Clone)]
pub struct TabMeta {
    pub n_rows: u64,
    pub n_cols: u64,
    pub columns: Vec<String>,
}

/// Build the structured `SheetExists` error payload that the
/// dispatcher returns to the LLM when `policy == Fail`.
pub fn build_sheet_exists_error(
    tab: &str,
    spreadsheet_id: Option<&str>,
    meta: &TabMeta,
) -> serde_json::Value {
    let advice = format!(
        "The tab '{tab}' already exists with data. Recommended: use a different name \
         (e.g., '{tab}_analysis'). If you must touch the existing tab, choose \
         update_in_place (patch specific rows) or overwrite (replace everything — \
         destructive)."
    );
    let mut payload = serde_json::json!({
        "error": "SheetExists",
        "tab": tab,
        "current_state": {
            "n_rows": meta.n_rows,
            "n_cols": meta.n_cols,
            "columns": meta.columns,
        },
        "advice": advice,
        "valid_next_moves": [
            {
                "action": "rename",
                "example_code": format!("output_sheets = {{'{tab}_review': df}}"),
            },
            {
                "action": "update_in_place",
                "example_code": format!(
                    "output_sheets = {{'{tab}': {{'mode':'update_in_place','df':df,'key':'<unique_col>'}}}}"
                ),
            },
            {
                "action": "overwrite",
                "example_code": format!(
                    "output_sheets = {{'{tab}': {{'mode':'overwrite','df':df}}}}"
                ),
            },
        ],
    });
    if let Some(sid) = spreadsheet_id {
        payload["spreadsheet_id"] = serde_json::Value::String(sid.to_string());
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parses_known_values() {
        assert_eq!(parse_policy("fail").unwrap(), CollisionPolicy::Fail);
        assert_eq!(
            parse_policy("auto_suffix").unwrap(),
            CollisionPolicy::AutoSuffix
        );
        assert_eq!(
            parse_policy("overwrite").unwrap(),
            CollisionPolicy::Overwrite
        );
    }

    #[test]
    fn policy_default_is_fail() {
        assert_eq!(CollisionPolicy::default(), CollisionPolicy::Fail);
    }

    #[test]
    fn policy_parse_error_lists_valid_values() {
        let err = parse_policy("clobber").unwrap_err();
        assert!(err.contains("clobber"));
        assert!(err.contains("fail"));
        assert!(err.contains("auto_suffix"));
        assert!(err.contains("overwrite"));
    }

    #[test]
    fn sheet_exists_error_has_required_fields() {
        let meta = TabMeta {
            n_rows: 4998,
            n_cols: 12,
            columns: vec!["product_id".into(), "name".into(), "price".into()],
        };
        let err = build_sheet_exists_error("Sales", Some("1xyz"), &meta);
        assert_eq!(err["error"], "SheetExists");
        assert_eq!(err["tab"], "Sales");
        assert_eq!(err["spreadsheet_id"], "1xyz");
        assert_eq!(err["current_state"]["n_rows"], 4998);
        assert_eq!(err["current_state"]["n_cols"], 12);
        assert_eq!(err["current_state"]["columns"][0], "product_id");

        let advice = err["advice"].as_str().unwrap();
        assert!(advice.contains("Sales"));
        assert!(advice.contains("update_in_place"));
        assert!(advice.contains("overwrite"));

        let moves = err["valid_next_moves"].as_array().unwrap();
        assert_eq!(moves.len(), 3);
        let actions: Vec<&str> = moves
            .iter()
            .map(|m| m["action"].as_str().unwrap())
            .collect();
        assert!(actions.contains(&"rename"));
        assert!(actions.contains(&"update_in_place"));
        assert!(actions.contains(&"overwrite"));
    }

    #[test]
    fn sheet_exists_error_omits_spreadsheet_id_when_absent() {
        let meta = TabMeta {
            n_rows: 1,
            n_cols: 1,
            columns: vec!["x".into()],
        };
        let err = build_sheet_exists_error("S", None, &meta);
        assert!(err.get("spreadsheet_id").is_none());
    }
}
```

- [ ] **Step 3: Run tests — must fail (module not declared yet)**

Run: `cargo test --lib -p colmena_dag_engine sheet_collision`
Expected: PASS — the module's own unit tests run after Step 1 declared it. If failing, ensure Step 1's `pub mod` edit landed.

- [ ] **Step 4: Run `cargo check`**

Run: `cargo check -p colmena_dag_engine`
Expected: clean (no warnings — repo enforces deny-warnings).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs
git commit -m "$(cat <<'EOF'
feat(synthetic_tools): shared sheet_collision policy + error envelope

CollisionPolicy enum (fail|auto_suffix|overwrite, default fail) +
parse_policy() with friendly errors + build_sheet_exists_error() that
returns the structured SheetExists payload the dispatchers send back
to the LLM (with current_state, advice, valid_next_moves).

Pure data — no async, no I/O. Used by gsheets_run_python and
crdt_doc_run_python dispatchers (wired in later tasks).

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Shared `diff_writer.rs` — records diff algorithm + validations

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs`

Pure function: given current records, new records, key column, optional column whitelist → returns either a `DiffResult` (cell changes + stats) or a `DiffError` (with a code matching the spec table). Both dispatchers call this.

- [ ] **Step 1: Write tests + module skeleton**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs`:

```rust
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
            Self::ColumnMismatch { target_columns, input_columns, extra, tab } => serde_json::json!({
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

    if !current.is_empty() && !cur_cols.contains(&key.to_string()) {
        return Err(DiffError::KeyColumnMissingInTarget {
            key: key.to_string(),
            available: cur_cols,
        });
    }
    if !new_cols.contains(&key.to_string()) {
        return Err(DiffError::KeyColumnMissingInInput {
            key: key.to_string(),
            available: new_cols,
        });
    }

    // --- 2. Column mismatch (extras in new not in target) ---
    let extra: Vec<String> = new_cols
        .iter()
        .filter(|c| !cur_cols.contains(c))
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
    let dup_cur = duplicate_examples(current, key, 3);
    if !dup_cur.is_empty() {
        return Err(DiffError::DuplicateKeyInTarget {
            key: key.to_string(),
            examples: dup_cur,
        });
    }
    let dup_new = duplicate_examples(new, key, 3);
    if !dup_new.is_empty() {
        return Err(DiffError::DuplicateKeyInInput {
            key: key.to_string(),
            examples: dup_new,
        });
    }

    // --- 4. Index target by key string (skipping nulls) ---
    let cur_index: HashMap<String, &Map<String, Value>> = current
        .iter()
        .filter_map(|r| r.get(key).and_then(|k| key_to_string(k)).map(|s| (s, r)))
        .collect();

    // --- 5. Decide which columns to compare ---
    let cur_col_set: HashSet<String> = cur_cols.iter().cloned().collect();
    let new_col_set: HashSet<String> = new_cols.iter().cloned().collect();
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
        None => cur_col_set
            .intersection(&new_col_set)
            .filter(|c| c.as_str() != key)
            .cloned()
            .collect(),
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
fn duplicate_examples(
    records: &[Map<String, Value>],
    key: &str,
    n: usize,
) -> Vec<String> {
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

/// NaN-safe value equality. Two nulls compare equal; everything else
/// uses serde_json's PartialEq.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        // Treat numeric-with-different-precision as equal when they round-trip the same.
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
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
            rec(&[("id", json!("a")), ("price", json!(10))]),  // no change
            rec(&[("id", json!("b")), ("price", json!(99))]),  // changed
            rec(&[("id", json!("c")), ("price", json!(99))]),  // changed
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
    fn nan_to_nan_is_no_change() {
        let current = vec![rec(&[("id", json!("a")), ("v", json!(null))])];
        let new = vec![rec(&[("id", json!("a")), ("v", json!(null))])];
        let r = diff_records(&current, &new, "id", None, false, "S").unwrap();
        assert_eq!(r.changes.len(), 0);
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
```

- [ ] **Step 2: Run tests — must pass**

Run: `cargo test --lib -p colmena_dag_engine diff_writer`
Expected: all 12 tests pass.

- [ ] **Step 3: Run `cargo check`**

Run: `cargo check -p colmena_dag_engine`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs
git commit -m "$(cat <<'EOF'
feat(synthetic_tools): shared diff_writer for update_in_place mode

diff_records() is a pure function: given current records, new records,
key column, optional column whitelist → returns either a DiffResult
(cell changes + per-row stats) or a DiffError (variants mapping 1-to-1
to the spec's validation table: KeyColumnMissing, DuplicateKeyInTarget,
DuplicateKeyInInput, ColumnMismatch, StrictMatchFailed).

Used by both gsheets_run_python and crdt_doc_run_python in later tasks.
Strict duplicate semantics — any duplicate in key column rejects the
diff (per spec D1 = strict mode).

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Extend `SheetsClient` trait — `batch_update_cells`

**Files:**
- Modify: `src/libs/colmena/src/gsheets/domain/traits.rs`
- Modify: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`

Apply diff = one HTTPS round-trip. Today `SheetsClient` only has `set_cell` (one cell at a time) and `set_range` (rectangular). Add `batch_update_cells(id, sheet, Vec<(A1, CellValue)>)` that internally uses `spreadsheets.values.batchUpdate` with one `ValueRange` per (sheet, cell).

- [ ] **Step 1: Extend trait definition**

Open `src/libs/colmena/src/gsheets/domain/traits.rs`. Add the new method to the `SheetsClient` trait (preserve the existing methods exactly; do NOT touch them):

```rust
    /// Apply N cell-level writes in one HTTPS round-trip via
    /// `spreadsheets.values.batchUpdate`. Each `(addr, value)` tuple is
    /// an A1-addressed cell on `sheet`. Returns total cells updated.
    async fn batch_update_cells(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        updates: Vec<(String, CellValue)>,
    ) -> Result<SetRangeResponse, SheetsError>;
```

- [ ] **Step 2: Write failing test (HTTP impl, wiremock)**

Find existing wiremock tests in `http_client.rs` for `set_range` (search `fn set_range`). Add a new test in the same `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn batch_update_cells_posts_value_ranges() {
        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/ss_b/values:batchUpdate"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalUpdatedCells": 3,
                "responses": [
                    {"updatedRange": "Sheet1!B5"},
                    {"updatedRange": "Sheet1!B6"},
                    {"updatedRange": "Sheet1!B7"},
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let updates = vec![
            ("B5".to_string(), crate::gsheets::domain::CellValue::Number(99.0)),
            ("B6".to_string(), crate::gsheets::domain::CellValue::Number(99.0)),
            ("B7".to_string(), crate::gsheets::domain::CellValue::Number(99.0)),
        ];
        let resp = client
            .batch_update_cells(
                &crate::gsheets::domain::SpreadsheetId("ss_b".to_string()),
                "Sheet1",
                updates,
            )
            .await
            .expect("batch_update_cells must succeed");
        assert_eq!(resp.updated_cells, 3);
    }
```

- [ ] **Step 3: Run — must fail (method doesn't exist)**

Run: `cargo test -p colmena_dag_engine --lib batch_update_cells_posts`
Expected: FAIL with "no method named batch_update_cells".

- [ ] **Step 4: Implement on `GoogleSheetsHttpClient`**

Open `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`. Find the `impl SheetsClient for GoogleSheetsHttpClient` block and add at the end (preserve every other method intact):

```rust
    async fn batch_update_cells(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        updates: Vec<(String, CellValue)>,
    ) -> Result<SetRangeResponse, SheetsError> {
        if updates.is_empty() {
            return Ok(SetRangeResponse {
                updated_cells: 0,
                updated_range: String::new(),
            });
        }
        let token = self.acquire_token().await?;
        let value_ranges: Vec<serde_json::Value> = updates
            .iter()
            .map(|(addr, val)| {
                serde_json::json!({
                    "range": format!("{sheet}!{addr}"),
                    "majorDimension": "ROWS",
                    "values": [[val.to_json()]],
                })
            })
            .collect();
        let body = serde_json::json!({
            "valueInputOption": "USER_ENTERED",
            "data": value_ranges,
        });
        let url = format!("{}/{}/values:batchUpdate", self.sheets_base, id.0);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| SheetsError::Http(format!("batch_update_cells request: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| SheetsError::Http(format!("batch_update_cells body: {e}")))?;
        if !status.is_success() {
            return Err(map_status_to_error(status, &text));
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| SheetsError::Http(format!("batch_update_cells parse: {e}")))?;
        let updated = parsed
            .get("totalUpdatedCells")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok(SetRangeResponse {
            updated_cells: updated,
            updated_range: format!("{} cells in {sheet}", updates.len()),
        })
    }
```

If `map_status_to_error` is not the exact name used by other methods in the same file, search for the existing pattern with `grep -n "fn.*status.*Error\|map_.*error" http_client.rs` and use the same helper the other methods do. Mirror the auth path of `set_range`.

- [ ] **Step 5: Run — must pass**

Run: `cargo test -p colmena_dag_engine --lib batch_update_cells_posts`
Expected: PASS.

- [ ] **Step 6: Run full gsheets tests + clippy**

Run: `cargo test -p colmena_dag_engine --lib gsheets:: && cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/gsheets/domain/traits.rs \
        src/libs/colmena/src/gsheets/infrastructure/http_client.rs
git commit -m "$(cat <<'EOF'
feat(gsheets): SheetsClient::batch_update_cells for diff-writes

Adds batch_update_cells(id, sheet, Vec<(A1, CellValue)>) that issues
one POST to spreadsheets.values:batchUpdate. Used by the upcoming
update_in_place mode in gsheets_run_python — patching 47 cells in a
1000-row tab becomes 1 round-trip instead of 47 set_cell calls.

valueInputOption=USER_ENTERED matches existing set_range behavior
(strings starting with '=' are evaluated as formulas).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Update gsheets postlude — emit spec-dict shape

**Files:**
- Modify: `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md`

The postlude packs whatever the user's code produced into a JSON payload the Rust dispatcher reads. Today each `output_sheets` value is converted to `{records, cols}` assuming it's a DataFrame. Extend so values can be either a DataFrame OR a spec dict — and emit a normalized shape: `{mode, df_records, df_cols, key, columns, strict_match}` per entry.

- [ ] **Step 1: Write new postlude content**

Replace `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md` entirely with:

````markdown

# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None

__col_output_sheets = None
if 'output_sheets' in dir() and output_sheets is not None:
    import pandas as _pd
    if isinstance(output_sheets, dict):
        __col_output_sheets = {}
        for k, v in output_sheets.items():
            entry = None
            # Shape 1: bare DataFrame → defaults to mode="replace".
            if isinstance(v, _pd.DataFrame):
                entry = {
                    'mode': 'replace',
                    'df_records': v.to_dict('records'),
                    'df_cols': list(v.columns),
                    'key': None,
                    'columns': None,
                    'strict_match': False,
                    'allow_schema_change': False,
                }
            # Shape 2: spec dict with required "mode" and "df".
            elif isinstance(v, dict):
                mode = v.get('mode', 'replace')
                df = v.get('df')
                if isinstance(df, _pd.DataFrame):
                    entry = {
                        'mode': str(mode),
                        'df_records': df.to_dict('records'),
                        'df_cols': list(df.columns),
                        'key': v.get('key'),
                        'columns': v.get('columns'),
                        'strict_match': bool(v.get('strict_match', False)),
                        'allow_schema_change': bool(v.get('allow_schema_change', False)),
                    }
                else:
                    # df missing or wrong type — surface a marker the
                    # dispatcher turns into a clear error.
                    entry = {
                        'mode': str(mode),
                        '_postlude_error': 'spec dict missing a pandas DataFrame in the "df" field',
                    }
            # Anything else (list, scalar, etc.) is silently skipped;
            # the dispatcher logs a warning if all entries skipped.
            if entry is not None:
                __col_output_sheets[str(k)] = entry

output = {
    'user_output': __col_user_output,
    'output_sheets': __col_output_sheets,
}
````

- [ ] **Step 2: Confirm the postlude file is `include_str!`'d at compile time**

The existing `gsheets_run_python.rs` line 32 already does `include_str!("../../../../../text/prompts/python_sandbox/gsheets_run_python_postlude.md")`. No code change needed — `cargo build` picks up the new content.

- [ ] **Step 3: Run `cargo build`**

Run: `cargo build -p colmena_dag_engine`
Expected: succeeds. (No assertion that runtime behavior changed — the dispatcher's tests will exercise the new shape in Task 5.)

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md
git commit -m "$(cat <<'EOF'
feat(gsheets): postlude emits normalized spec-dict shape

output_sheets entries can now be either a bare DataFrame (mode=replace,
current behavior) or a spec dict {mode, df, key, columns, strict_match,
allow_schema_change}. The postlude normalizes both into a single shape
the Rust dispatcher reads: {mode, df_records, df_cols, key, columns,
strict_match, allow_schema_change}.

Wiring into the dispatcher comes in the next commit.

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md §2

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Wire collision policy + 3 modes into `gsheets_run_python` dispatcher

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`

This is the biggest task — rewrite `write_output_sheets` to:
1. Read the normalized per-entry shape from the postlude.
2. Branch on `mode`: `replace`/`update_in_place`/`overwrite`.
3. For `replace` and `overwrite`: pre-flight collision check via `list_sheets`. On collision, apply policy.
4. For `update_in_place`: fetch current tab, call `diff_records`, apply via `batch_update_cells`.

Pass the policy from `fixed_config.on_existing_sheet` through the existing config plumbing. The dispatcher signature already takes args — we add a separate `policy: CollisionPolicy` parameter, defaulting to `Fail`.

- [ ] **Step 1: Find the `fixed_config` plumbing for gsheets_run_python**

The synthetic-tool dispatcher receives a `fixed_config` (JSON) merged into args before parsing. Verify by running:

```bash
grep -n "fixed_config\|on_existing_sheet\|merge_fixed" src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs | head -20
```

If `fixed_config` is merged ABOVE the dispatcher entry point, the policy will arrive in `args`. Extract it from the args JSON in Step 3 by reading `args["__on_existing_sheet"]` (or whatever convention the executor uses) — DO NOT add a public param. Run the grep first; the implementation in Step 3 must match the existing pattern.

- [ ] **Step 2: Add args struct fields**

Open `gsheets_run_python.rs`. Modify `GsheetsRunPythonArgs` (around line 44) to add a field for policy passed in via fixed_config:

```rust
    /// Operator-supplied collision policy. Default `fail`. Wired via
    /// `fixed_config.on_existing_sheet`. Accepted values: `fail`,
    /// `auto_suffix`, `overwrite`.
    #[serde(default, alias = "on_existing_sheet")]
    pub on_existing_sheet: Option<String>,
```

(The alias keeps it grokkable if the merger uses snake_case or camelCase.)

- [ ] **Step 3: Write integration tests for the new modes**

In the existing `#[cfg(test)] mod tests` block in `gsheets_run_python.rs`, append these tests (all use the wiremock client pattern that's already in the file):

```rust
    // ── update_in_place mode tests ────────────────────────────────────

    #[tokio::test]
    async fn update_in_place_writes_only_changed_cells() {
        pyo3::prepare_freethreaded_python();

        let server = wiremock::MockServer::start().await;
        let client =
            GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // (a) read_range mock — returns current "Sales" with 3 rows.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_u/values/Sales"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Sales!A1:C4",
                "majorDimension": "ROWS",
                "values": [
                    ["id", "price"],
                    ["a",  10],
                    ["b",  20],
                    ["c",  30],
                ],
            })))
            .mount(&server)
            .await;

        // (b) list_sheets mock — "Sales" tab exists.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_u\?"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{
                    "properties": {
                        "sheetId": 1, "title": "Sales", "index": 0,
                        "gridProperties": {"rowCount": 4, "columnCount": 2}
                    }
                }]
            })))
            .mount(&server)
            .await;

        // (c) batchUpdate (cells) mock — expect exactly ONE call with 2
        //     cell updates (rows b & c, column price).
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/ss_u/values:batchUpdate"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalUpdatedCells": 2,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "sales", "spreadsheet_id": "ss_u", "sheet": "Sales"}],
            "code": "\
import pandas as pd\n\
df = pd.DataFrame(sales)\n\
df.loc[df['id'].isin(['b','c']), 'price'] = 99\n\
output_sheets = {'Sales': {'mode': 'update_in_place', 'df': df, 'key': 'id'}}\n\
output = {}\n\
",
            "write_to_spreadsheet": "ss_u"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
            panic!("unexpected error: {res}");
        }
        let wrote = res
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .expect("wrote_sheets must be array");
        assert_eq!(wrote.len(), 1, "expected 1 wrote_sheets entry; got: {res}");
        let entry = &wrote[0];
        assert_eq!(entry["mode"], "update_in_place");
        assert_eq!(entry["changes"]["rows"], 2);
        assert_eq!(entry["changes"]["cells"], 2);
        assert_eq!(entry["unchanged"]["rows"], 1);
    }

    #[tokio::test]
    async fn replace_mode_default_fail_returns_sheet_exists() {
        pyo3::prepare_freethreaded_python();

        let server = wiremock::MockServer::start().await;
        let client =
            GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // list_sheets — "Existing" already there.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_f\?"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{
                    "properties": {
                        "sheetId": 7, "title": "Existing", "index": 0,
                        "gridProperties": {"rowCount": 100, "columnCount": 3}
                    }
                }]
            })))
            .mount(&server)
            .await;

        // read_range (for header columns) — used by fail-error builder.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_f/values/Existing"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Existing!A1:Z1",
                "majorDimension": "ROWS",
                "values": [["a","b","c"]],
            })))
            .mount(&server)
            .await;

        // NO batchUpdate, NO add_sheet, NO set_range calls expected.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "ss_f", "sheet": "Existing"}],
            "code": "\
import pandas as pd\n\
df = pd.DataFrame(x)\n\
output_sheets = {'Existing': df}\n\
output = {}\n\
",
            "write_to_spreadsheet": "ss_f"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
        }
        let wrote = res
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .expect("wrote_sheets must be array");
        assert_eq!(wrote.len(), 1);
        assert_eq!(wrote[0]["error"], "SheetExists");
        let advice = wrote[0]["advice"].as_str().unwrap();
        assert!(advice.contains("Existing"));
    }

    #[tokio::test]
    async fn auto_suffix_policy_preserves_old_behavior() {
        // With fixed_config.on_existing_sheet="auto_suffix", colliding
        // "Existing" should write to "Existing (2)" silently — same as
        // pre-spec behavior.
        pyo3::prepare_freethreaded_python();

        let server = wiremock::MockServer::start().await;
        let client =
            GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_a\?"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{"properties": {"sheetId": 1, "title": "Existing", "index": 0,
                    "gridProperties": {"rowCount": 1, "columnCount": 1}}}]
            })))
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/ss_a:batchUpdate"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "replies": [{"addSheet": {"properties": {"sheetId": 9, "title": "Existing (2)"}}}]
            })))
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path_regex(r"/ss_a/values/"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "updatedRange": "Existing (2)!A1:B2",
                "updatedCells": 4,
            })))
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "ss_a", "sheet": "Existing"}],
            "code": "\
import pandas as pd\n\
df = pd.DataFrame(x)\n\
output_sheets = {'Existing': df}\n\
output = {}\n\
",
            "write_to_spreadsheet": "ss_a",
            "on_existing_sheet": "auto_suffix"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
            panic!("unexpected error: {res}");
        }
        let wrote = res["wrote_sheets"].as_array().expect("array");
        assert_eq!(wrote[0]["resolved_name"], "Existing (2)");
    }
```

- [ ] **Step 4: Run — must fail (modes not implemented)**

Run: `cargo test -p colmena_dag_engine --lib gsheets_run_python::tests`
Expected: the 3 new tests FAIL (missing `mode`/`changes`/`advice` fields in response). Existing tests still pass.

- [ ] **Step 5: Rewrite `write_output_sheets` and add helpers**

In `gsheets_run_python.rs`, replace the existing `write_output_sheets` function (line ~433 to line ~548) with the dispatch-on-mode implementation below. Also add the new helpers used by it.

Top of the file: add to the existing `use` block:

```rust
use super::diff_writer::{diff_records, DiffError};
use super::sheet_collision::{
    build_sheet_exists_error, parse_policy, CollisionPolicy, TabMeta,
};
```

Add a helper at the bottom of the file (above `mod tests`):

```rust
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
    // Header row (A1:Z1) — small read, gives us real column names.
    let read = client
        .read_range(
            spreadsheet_id,
            tab,
            Some("A1:Z1"),
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
    Ok(Some(TabMeta {
        n_rows: meta.row_count as u64,
        n_cols: meta.col_count as u64,
        columns,
    }))
}

/// Convert column letter+row index to A1 string for `batch_update_cells`.
/// `row_index` is 1-based, where row 1 is the header.
fn a1_addr(col_index: usize, row_index: usize) -> String {
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
    format!("{col}{row_index}")
}
```

Then replace `write_output_sheets` entirely with:

```rust
/// Dispatch each normalized `output_sheets` entry by mode. Returns one
/// metadata entry per attempted write.
async fn write_output_sheets(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    output_sheets: &serde_json::Value,
    policy: CollisionPolicy,
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
        let mode = entry.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
        match mode {
            "update_in_place" => {
                results.push(do_update_in_place(client, spreadsheet_id, raw_name, entry).await);
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
                    "error": format!("unknown mode '{other}'; valid: replace, update_in_place, overwrite"),
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
                // Find a free suffixed name and create it.
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
    if let Ok(Some(meta)) = fetch_tab_meta(client, spreadsheet_id, raw_name).await {
        let input_cols: Vec<String> = entry
            .get("df_cols")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
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
            "name": raw_name,
            "error": "update_in_place requires `key` field in the spec dict",
        });
    };
    let restrict: Option<Vec<String>> = entry
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect());
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
                "name": raw_name,
                "error": format!("read failed: {e}"),
            });
        }
    };
    let current_records: Vec<serde_json::Map<String, serde_json::Value>> = match read.values {
        serde_json::Value::Array(a) => a.into_iter().filter_map(|v| match v {
            serde_json::Value::Object(o) => Some(o),
            _ => None,
        }).collect(),
        _ => Vec::new(),
    };
    // Header order (for A1 mapping) — re-read header row directly so column order is preserved.
    let header_read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            Some("A1:Z1"),
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
                "name": raw_name,
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
    let mut key_to_row: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
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

    let mut cell_updates: Vec<(String, crate::gsheets::domain::CellValue)> = Vec::new();
    for chg in &diff.changes {
        let (Some(row), Some(col_idx)) = (key_to_row.get(&chg.key_value), col_to_index.get(&chg.column)) else {
            continue;
        };
        let addr = a1_addr(*col_idx, *row);
        cell_updates.push((addr, crate::gsheets::domain::CellValue::from_json(&chg.new_value)));
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

    match client.batch_update_cells(spreadsheet_id, raw_name, cell_updates).await {
        Ok(_) => serde_json::json!({
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
        }),
        Err(e) => serde_json::json!({
            "name": raw_name,
            "error": format!("batch_update_cells failed: {e}"),
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
    let header_row: Vec<CellValue> = cols
        .iter()
        .map(|c| CellValue::String(c.as_str().unwrap_or("").to_string()))
        .collect();
    let mut matrix: Vec<Vec<CellValue>> = vec![header_row];
    for rec in records {
        let Some(obj) = rec.as_object() else { continue };
        let row: Vec<CellValue> = cols
            .iter()
            .map(|c| {
                let key = c.as_str().unwrap_or("");
                CellValue::from_json(obj.get(key).unwrap_or(&serde_json::Value::Null))
            })
            .collect();
        matrix.push(row);
    }
    let n_rows = matrix.len();
    let n_cols = cols.len();
    match client.set_range(spreadsheet_id, name, "A1", matrix).await {
        Ok(_) => serde_json::json!({
            "name": raw_name,
            "resolved_name": name,
            "n_rows": n_rows,
            "n_cols": n_cols,
        }),
        Err(e) => serde_json::json!({
            "name": raw_name,
            "resolved_name": name,
            "error": format!("set_range failed: {e}"),
        }),
    }
}

fn columns_match(a: &[String], b: &[String]) -> bool {
    let sa: std::collections::HashSet<&String> = a.iter().collect();
    let sb: std::collections::HashSet<&String> = b.iter().collect();
    sa == sb
}
```

And update the caller in `dispatch_gsheets_run_python_with_client` (around line 376) — replace:

```rust
        let results = write_output_sheets(&client, &spreadsheet_id, sheets_value).await;
```

with:

```rust
        let policy = parsed
            .on_existing_sheet
            .as_deref()
            .map(parse_policy)
            .transpose()
            .unwrap_or_else(|e| {
                eprintln!("[gsheets] invalid on_existing_sheet, defaulting to fail: {e}");
                None
            })
            .unwrap_or_default();
        let results = write_output_sheets(&client, &spreadsheet_id, sheets_value, policy).await;
```

- [ ] **Step 6: Run new tests — must pass**

Run: `cargo test -p colmena_dag_engine --lib gsheets_run_python::tests`
Expected: all tests pass (3 new ones + all existing).

- [ ] **Step 7: Clippy + fmt**

Run: `cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings && cargo fmt`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "$(cat <<'EOF'
feat(gsheets): collision policy + update_in_place + overwrite modes

write_output_sheets now branches on entry mode: replace (default — runs
collision policy; default fail), overwrite (explicit, with schema-change
guard), and update_in_place (fetches current tab, diffs via diff_writer,
applies via batch_update_cells in one HTTPS round-trip).

`on_existing_sheet` arg (wired through fixed_config) controls collision
behavior: fail (default) / auto_suffix (legacy) / overwrite. fail returns
the structured SheetExists payload from sheet_collision.rs so the LLM
can decide between rename / update_in_place / overwrite without an
extra round-trip.

3 new dispatcher tests cover update_in_place, default-fail collision,
and the auto_suffix opt-back-in.

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md §1-3

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Mirror in `crdt_doc_run_python` — collision policy + update_in_place

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Modify: `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md`

Same shape as Task 5 but the backing store is the CRDT runtime. Cell writes go through `apply_set_cell_in_proc` (the per-cell write that recalculates formulas).

- [ ] **Step 1: Update the CRDT postlude (mirror gsheets shape)**

Replace `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md` entirely with the same content as the gsheets postlude from Task 4 — the spec-dict normalization is the same. (The legacy `output_sheet` / `sheet_records` / `sheet_cols` block is removed in Task 7. For now we ADD the new shape without removing the legacy — both paths coexist for one commit.)

Open the file. The current content has BOTH the legacy `output_sheet` block AND the existing `output_sheets` (bare-DataFrame) handling. Replace just the `output_sheets` section (lines ~14-24) with the normalized spec-dict shape from Task 4:

```python
__col_output_sheets = None
if 'output_sheets' in dir() and output_sheets is not None:
    import pandas as _pd
    if isinstance(output_sheets, dict):
        __col_output_sheets = {}
        for k, v in output_sheets.items():
            entry = None
            if isinstance(v, _pd.DataFrame):
                entry = {
                    'mode': 'replace',
                    'df_records': v.to_dict('records'),
                    'df_cols': list(v.columns),
                    'key': None,
                    'columns': None,
                    'strict_match': False,
                    'allow_schema_change': False,
                }
            elif isinstance(v, dict):
                mode = v.get('mode', 'replace')
                df = v.get('df')
                if isinstance(df, _pd.DataFrame):
                    entry = {
                        'mode': str(mode),
                        'df_records': df.to_dict('records'),
                        'df_cols': list(df.columns),
                        'key': v.get('key'),
                        'columns': v.get('columns'),
                        'strict_match': bool(v.get('strict_match', False)),
                        'allow_schema_change': bool(v.get('allow_schema_change', False)),
                    }
                else:
                    entry = {'mode': str(mode), '_postlude_error': 'spec dict missing pandas DataFrame in "df"'}
            if entry is not None:
                __col_output_sheets[str(k)] = entry
```

The legacy `output_sheet` (singular) block above this remains for now — removed in Task 7.

- [ ] **Step 2: Add CRDT-side helpers + rewrite the multi-sheet branch**

Open `crdt_doc_run_python.rs`. Add to the top `use` block:

```rust
use super::diff_writer::{diff_records, DiffError};
use super::sheet_collision::{
    build_sheet_exists_error, parse_policy, CollisionPolicy, TabMeta,
};
```

Add to `RunPythonArgs`:

```rust
    /// Operator-supplied collision policy. Default `fail`. Wired via
    /// `fixed_config.on_existing_sheet`. Accepted: fail, auto_suffix, overwrite.
    #[serde(default, alias = "on_existing_sheet")]
    pub on_existing_sheet: Option<String>,
```

Replace the entire `Section 8 — multi-sheet path` block (currently lines ~268-336) with:

```rust
    // 8. Multi-sheet path (new + diff-aware).
    let mut wrote_sheets_response = serde_json::Value::Null;
    if let Some(sheets_value) = wrapped_output.get("output_sheets") {
        if !sheets_value.is_null() {
            let policy: CollisionPolicy = args
                .on_existing_sheet
                .as_deref()
                .map(parse_policy)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            let map = sheets_value.as_object().cloned().unwrap_or_default();
            let mut results: Vec<serde_json::Value> = Vec::new();
            for (raw_name, entry) in map {
                if let Some(err) = entry.get("_postlude_error").and_then(|v| v.as_str()) {
                    results.push(serde_json::json!({"name": raw_name, "error": err}));
                    continue;
                }
                let mode = entry.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
                let result = match mode {
                    "update_in_place" => {
                        crdt_update_in_place(&doc, ctx, &raw_name, &entry).await
                    }
                    "overwrite" => {
                        crdt_overwrite(&doc, ctx, &raw_name, &entry).await
                    }
                    "replace" => {
                        crdt_replace(&doc, ctx, &raw_name, &entry, policy).await
                    }
                    other => serde_json::json!({
                        "name": raw_name,
                        "error": format!("unknown mode '{other}'; valid: replace, update_in_place, overwrite"),
                    }),
                };
                results.push(result);
            }
            wrote_sheets_response = serde_json::Value::Array(results);
        }
    }
```

Then add these helper functions ABOVE the existing `wrap_user_code` definition (search "fn wrap_user_code"):

```rust
/// Fetch existing-sheet metadata in CRDT: row/col count + header column names.
fn crdt_tab_meta(doc: &yrs::Doc, target: &str) -> Option<TabMeta> {
    use crate::crdt_documents::list_sheets_with_meta;
    let sheets = list_sheets_with_meta(doc);
    let m = sheets.into_iter().find(|s| s.name == target)?;
    Some(TabMeta {
        n_rows: m.row_count as u64,
        n_cols: m.col_count as u64,
        columns: m.header_columns,
    })
}

async fn crdt_replace(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    entry: &serde_json::Value,
    policy: CollisionPolicy,
) -> serde_json::Value {
    if let Some(meta) = crdt_tab_meta(doc, raw_name) {
        match policy {
            CollisionPolicy::Fail => {
                return build_sheet_exists_error(raw_name, None, &meta);
            }
            CollisionPolicy::Overwrite => {
                return crdt_write_full(doc, ctx, raw_name, raw_name, entry).await;
            }
            CollisionPolicy::AutoSuffix => {
                for attempt in 2i32..=10 {
                    let candidate = format!("{raw_name} ({attempt})");
                    if crdt_tab_meta(doc, &candidate).is_none() {
                        return crdt_write_full(doc, ctx, raw_name, &candidate, entry).await;
                    }
                }
                return serde_json::json!({
                    "name": raw_name,
                    "error": "auto_suffix: all 10 name attempts already exist",
                });
            }
        }
    }
    crdt_write_full(doc, ctx, raw_name, raw_name, entry).await
}

async fn crdt_overwrite(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    if let Some(meta) = crdt_tab_meta(doc, raw_name) {
        let input_cols: Vec<String> = entry
            .get("df_cols")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let allow = entry.get("allow_schema_change").and_then(|v| v.as_bool()).unwrap_or(false);
        let mismatch = {
            let sa: std::collections::HashSet<&String> = meta.columns.iter().collect();
            let sb: std::collections::HashSet<&String> = input_cols.iter().collect();
            sa != sb
        };
        if mismatch && !allow {
            return serde_json::json!({
                "name": raw_name,
                "error": "SchemaChange",
                "current_columns": meta.columns,
                "input_columns": input_cols,
                "message": format!(
                    "Overwriting '{raw_name}' would change its schema: current {:?} → new {:?}. \
                     RECOMMENDED: use a different tab name. To proceed anyway, add \
                     'allow_schema_change: true' to the spec dict.",
                    meta.columns, input_cols
                ),
            });
        }
    }
    crdt_write_full(doc, ctx, raw_name, raw_name, entry).await
}

async fn crdt_update_in_place(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    use crate::crdt_documents::{
        apply_set_cell_in_proc, build_records_for_sheets, list_sheets_with_meta, RecordsError,
    };
    let Some(key) = entry.get("key").and_then(|v| v.as_str()) else {
        return serde_json::json!({"name": raw_name, "error": "update_in_place requires `key` field"});
    };
    // Resolve sheet_id by name.
    let Some(target_meta) = list_sheets_with_meta(doc).into_iter().find(|s| s.name == raw_name)
    else {
        return serde_json::json!({
            "name": raw_name,
            "error": "UpdateRequiresExistingTab",
            "message": format!("update_in_place needs sheet '{raw_name}' to exist; use mode=replace to create it."),
        });
    };
    let records_map = match build_records_for_sheets(doc, &[target_meta.sheet_id.clone()]) {
        Ok(m) => m,
        Err(RecordsError::SheetNotFound(_)) => {
            return serde_json::json!({"name": raw_name, "error": "UpdateRequiresExistingTab"});
        }
        Err(RecordsError::SizeCapExceeded { actual, limit }) => {
            return serde_json::json!({
                "name": raw_name,
                "error": "load_size_exceeded",
                "actual_bytes": actual,
                "limit_bytes": limit,
            });
        }
    };
    let recs = records_map.get(&target_meta.sheet_id).cloned().unwrap_or_default();
    let current_records: Vec<serde_json::Map<String, serde_json::Value>> = recs.records;
    let header_cols: Vec<String> = recs.columns.clone();
    let new_records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();
    let restrict: Option<Vec<String>> = entry
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect());
    let strict = entry.get("strict_match").and_then(|v| v.as_bool()).unwrap_or(false);
    let diff = match diff_records(
        &current_records, &new_records, key, restrict.as_deref(), strict, raw_name,
    ) {
        Ok(d) => d,
        Err(e) => return e.to_json(),
    };
    // Apply each cell change via apply_set_cell_in_proc on the CRDT.
    let mut col_to_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, c) in header_cols.iter().enumerate() {
        col_to_index.insert(c.clone(), i);
    }
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
    let mut cells_written = 0usize;
    for chg in &diff.changes {
        let (Some(&row), Some(&col_idx)) = (key_to_row.get(&chg.key_value), col_to_index.get(&chg.column)) else {
            continue;
        };
        let addr = {
            let mut col = String::new();
            let mut n = col_idx;
            loop {
                col.insert(0, (b'A' + (n % 26) as u8) as char);
                n /= 26;
                if n == 0 { break; }
                n -= 1;
            }
            format!("{col}{row}")
        };
        let _ = apply_set_cell_in_proc(doc, &target_meta.sheet_id, &addr, chg.new_value.clone());
        cells_written += 1;
    }
    if cells_written > 0 {
        ctx.mark_dirty();
        let origin = ctx
            .session_id()
            .map(|s| format!("agent:{s}"))
            .unwrap_or_else(|| "agent:llm".to_string());
        let event_id = ctx
            .backend()
            .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                artifact_id: ctx.artifact_id().clone(),
                sheet_id: Some(target_meta.sheet_id.clone()),
                origin,
                summary: format!(
                    "update_in_place: patched {} cell(s) across {} row(s) in '{raw_name}'",
                    cells_written, diff.rows_changed
                ),
            })
            .await
            .unwrap_or(0);
        ctx.record_event_id(event_id);
    }
    serde_json::json!({
        "tab": raw_name,
        "sheet_id": target_meta.sheet_id,
        "mode": "update_in_place",
        "changes": {
            "rows": diff.rows_changed,
            "cells": cells_written,
            "columns": diff.columns_touched,
        },
        "unchanged": {"rows": diff.rows_unchanged},
        "skipped": {
            "rows_not_in_target": diff.rows_skipped_not_in_target,
            "rows_null_key": diff.rows_skipped_null_key,
        },
    })
}

async fn crdt_write_full(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    write_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    use crate::crdt_documents::write_records_as_new_sheet;
    let records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();
    let cols: Vec<String> = entry
        .get("df_cols")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
        .unwrap_or_default();
    match write_records_as_new_sheet(doc, write_name, &cols, &records) {
        Ok(wr) => {
            ctx.mark_dirty();
            let origin = ctx
                .session_id()
                .map(|s| format!("agent:{s}"))
                .unwrap_or_else(|| "agent:llm".to_string());
            let event_id = ctx
                .backend()
                .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                    artifact_id: ctx.artifact_id().clone(),
                    sheet_id: Some(wr.sheet_id.clone()),
                    origin,
                    summary: format!(
                        "wrote {} rows via run_python to sheet '{}'",
                        wr.n_rows, wr.resolved_name
                    ),
                })
                .await
                .unwrap_or(0);
            ctx.record_event_id(event_id);
            serde_json::json!({
                "name": raw_name,
                "resolved_name": wr.resolved_name,
                "sheet_id": wr.sheet_id,
                "n_rows": wr.n_rows,
                "n_cols": wr.n_cols,
            })
        }
        Err(e) => serde_json::json!({
            "name": raw_name,
            "error": format!("write_records_as_new_sheet failed: {e}"),
        }),
    }
}
```

**Note** — the helpers reference `list_sheets_with_meta` and assume `SheetWithMeta { name, sheet_id, row_count, col_count, header_columns }`. If those don't exist verbatim, search the crdt_documents module for the closest equivalent and adapt. The intent is: get sheet metadata + header columns; use what's there. The plan can adapt — what matters is that the per-mode dispatch + diff_writer integration are wired correctly.

If `list_sheets_with_meta` does not exist, the implementer should look at how `execute_list_sheets` already extracts sheet info (search `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` for `execute_list_sheets`) and use the same primitives. The implementation MAY add a small helper to `crdt_documents/` to expose `(name, sheet_id, row_count, col_count, header_columns)` if no existing function returns all five.

- [ ] **Step 3: Write a dispatcher test for update_in_place via CRDT**

In the existing `#[cfg(test)] mod tests` block of `crdt_doc_run_python.rs`, append:

```rust
    #[tokio::test]
    async fn update_in_place_patches_only_changed_cells_in_crdt() {
        pyo3::prepare_freethreaded_python();
        let rt = make_runtime().await;
        let id = ArtifactId::new();
        let _ = rt.registry.get_or_create(&id, "workbook");
        let ctx = CrdtDocsContext::new_local(rt, id, Some("test_session".to_string()));

        let add = execute_add_sheet(&ctx, AddSheetArgs { name: "Sales".into() }).await;
        let sheet_id = add["sheet_id"].as_str().unwrap().to_string();
        execute_set_range(&ctx, SetRangeArgs {
            sheet_id: sheet_id.clone(),
            start_addr: "A1".into(),
            values_2d: vec![
                vec![json!("id"), json!("price")],
                vec![json!("a"), json!(10)],
                vec![json!("b"), json!(20)],
                vec![json!("c"), json!(30)],
            ],
        }).await;

        let args = RunPythonArgs {
            sheet_ids: vec![sheet_id.clone()],
            code: format!(r#"
import pandas as pd
df = pd.DataFrame(dfs["{sid}"])
df.loc[df['id'] == 'b', 'price'] = 99
output_sheets = {{
    'Sales': {{'mode': 'update_in_place', 'df': df, 'key': 'id'}},
}}
output = {{}}
"#, sid = sheet_id),
            write_to_sheet: None,
            on_existing_sheet: None,
        };
        let res = execute_run_python(&ctx, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
        }
        let wrote = res["wrote_sheets"].as_array().expect("wrote_sheets array");
        assert_eq!(wrote.len(), 1);
        let entry = &wrote[0];
        assert_eq!(entry["mode"], "update_in_place");
        assert_eq!(entry["changes"]["cells"], 1);
    }
```

- [ ] **Step 4: Build + run tests**

Run: `cargo build -p colmena_dag_engine && cargo test -p colmena_dag_engine --lib crdt_doc_run_python::tests`
Expected: all pass. If `list_sheets_with_meta` doesn't compile, refer back to Step 2's note and adapt to whatever crdt_documents exposes. The test verifies the contract; what matters is that update_in_place returns `changes.cells = 1`.

- [ ] **Step 5: Clippy + fmt**

Run: `cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings && cargo fmt`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
        src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md
git commit -m "$(cat <<'EOF'
feat(crdt_doc): collision policy + update_in_place mirror

crdt_doc_run_python gains the same modes as gsheets: replace/overwrite/
update_in_place. Per-mode dispatch in the multi-sheet branch reuses
the shared diff_writer + sheet_collision modules.

CRDT cell writes go through apply_set_cell_in_proc (formula-aware).
update_in_place fetches current records via build_records_for_sheets,
diffs vs the new DataFrame, applies each change as a single cell write,
and emits one change-tracker event per write batch.

The legacy single-tab path (write_to_sheet + output_sheet) still exists
and works — removed in the next commit.

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md §1-3 (CRDT mirror)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Remove legacy single-tab path from `crdt_doc_run_python`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Modify: `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md`

The legacy `write_to_sheet: Option<String>` field + `output_sheet` Python global + the dispatcher branch that handled it. ADP has no consumers; in-repo test graphs are migrated in Task 8.

- [ ] **Step 1: Remove the postlude block for `output_sheet`**

In `crdt_doc_run_python_postlude.md`, delete lines (current numbering after Task 6):

```python
__col_sheet_records = None
__col_sheet_cols = None
if 'output_sheet' in dir() and output_sheet is not None:
    import pandas as _pd
    if isinstance(output_sheet, _pd.DataFrame):
        __col_sheet_records = output_sheet.to_dict('records')
        __col_sheet_cols = list(output_sheet.columns)
```

And update the final `output =` dict to drop `sheet_records` and `sheet_cols`. Final shape:

```python
output = {
    'user_output': __col_user_output,
    'output_sheets': __col_output_sheets,
}
```

- [ ] **Step 2: Remove the legacy fields and branches from the Rust dispatcher**

In `crdt_doc_run_python.rs`:

1. Delete the field from `RunPythonArgs`:

   ```rust
       /// If set, `output_sheet` is written as a new sheet with this name.
       /// Name collisions append " (2)", " (3)" etc.
       #[serde(default)]
       pub write_to_sheet: Option<String>,
   ```

2. Update the doc-comment on `code` to drop `output_sheet` mention. Change "Must define `output` (any JSON-serializable value) and/or `output_sheet` (a pandas DataFrame)." to "Must define `output` (any JSON-serializable value) and/or `output_sheets` (a dict of names → DataFrames)."

3. Delete the postlude-extraction lines for `sheet_records` and `sheet_cols`:

   ```rust
       let sheet_records = wrapped_output
           .get("sheet_records")
           .cloned()
           .unwrap_or(serde_json::Value::Null);
       let sheet_cols = wrapped_output
           .get("sheet_cols")
           .cloned()
           .unwrap_or(serde_json::Value::Null);
   ```

4. Delete the entire "Section 7" `if let Some(target_name) = args.write_to_sheet.as_deref()` block (the whole legacy single-tab branch, currently lines ~169-264).

5. Drop `wrote_sheet_response` from the response payload — the response now only includes `wrote_sheets` (multi-tab). Adjust the final response JSON to:

   ```rust
       let mut response = serde_json::json!({
           "output": user_output_capped,
           "wrote_sheets": wrote_sheets_response,
           "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
           "error": serde_json::Value::Null,
       });
   ```

6. Delete the `Both 'output_sheet' (legacy) and 'output_sheets'` warning block.

7. Update the existing test `multi_sheet_output_sheets_writes_three_new_tabs` so its `RunPythonArgs` struct literal no longer mentions `write_to_sheet`. (If the test fixture has `write_to_sheet: None` — remove the field.)

8. Update the `update_in_place_patches_only_changed_cells_in_crdt` test added in Task 6 to drop the now-deleted `write_to_sheet: None,` field.

- [ ] **Step 3: Run all crdt tests — must still pass**

Run: `cargo test -p colmena_dag_engine --lib crdt_doc_run_python::tests`
Expected: tests pass (legacy field removed from struct literals; multi-sheet path intact).

- [ ] **Step 4: Confirm no other module references the legacy field**

Run: `grep -rn "write_to_sheet\|output_sheet[^s]" src/libs/colmena/src/ | grep -v "test\|fixture"`
Expected: empty (or only doc-comment mentions, if any). If any source file still references it, fix the caller (probably an executor test or docs example).

- [ ] **Step 5: Clippy + fmt**

Run: `cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings && cargo fmt`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
        src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md
git commit -m "$(cat <<'EOF'
refactor(crdt_doc): remove legacy single-tab write path

Drops:
- RunPythonArgs::write_to_sheet field
- output_sheet Python global handling in the postlude
- The dispatcher branch that wrote a single tab via that path
- The response field wrote_sheet
- The dual-path warning

Replaced by output_sheets = {name: DataFrame | spec_dict} which is
strictly more expressive (N tabs + per-tab mode). ADP confirmed to
have no consumers depending on the legacy API.

In-repo test graphs are migrated in the next commit.

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md §4

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Migrate the 3 legacy test graphs

**Files:**
- Modify: `tests/graphs/crdt_documents/c_import_analysis.json`
- Modify: `tests/graphs/crdt_documents/f_cross_artifact_smoke.json`
- Modify: `tests/graphs/crdt_documents/c_pandas_smoke.json`

Each currently has a system message or prompt mentioning `write_to_sheet` and `output_sheet`. Replace with `output_sheets = {"Name": df}` instructions and remove any explicit `write_to_sheet` argument shapes.

- [ ] **Step 1: Read each graph and identify the legacy mentions**

For each file:

```bash
grep -n "write_to_sheet\|output_sheet" tests/graphs/crdt_documents/c_import_analysis.json
grep -n "write_to_sheet\|output_sheet" tests/graphs/crdt_documents/f_cross_artifact_smoke.json
grep -n "write_to_sheet\|output_sheet" tests/graphs/crdt_documents/c_pandas_smoke.json
```

Expected: each shows 1-3 lines that mention either a `write_to_sheet` JSON arg or instructions in the prompt that tell the agent to use `output_sheet`/`write_to_sheet`.

- [ ] **Step 2: Migrate `c_pandas_smoke.json`**

Open the file. Find any prompt / system_message text or tool_configurations / fixed_config mentioning `write_to_sheet` or `output_sheet` (singular). Rewrite to use `output_sheets`:

- If the instruction says: "use `output_sheet = df` and pass `write_to_sheet='Result'`" → change to "use `output_sheets = {'Result': df}` (no extra arg needed; the dispatcher writes to the artifact in scope)."
- If `tool_configurations.crdt_doc_run_python.fixed_config` has a hard-coded `write_to_sheet` value → remove that key (it's not on the args struct anymore).

Save the file.

- [ ] **Step 3: Run the graph to verify it still works**

Run: `cargo run --release --bin dag_engine -- run tests/graphs/crdt_documents/c_pandas_smoke.json --agent-session-id agent_migrate_$(date +%s) 2>&1 | tail -40`
Expected: graph completes; the agent writes the expected sheet via `output_sheets`. Save the SSE to `/tmp/colmena_e2e/c_pandas_smoke_migrated.sse` so the reviewer can audit.

- [ ] **Step 4: Migrate `c_import_analysis.json`**

Same procedure as Step 2. Run to verify per Step 3 (`c_import_analysis_migrated.sse`).

- [ ] **Step 5: Migrate `f_cross_artifact_smoke.json`**

Same procedure. Verify per Step 3 (`f_cross_artifact_migrated.sse`).

- [ ] **Step 6: Final grep to confirm no remaining references**

Run: `grep -rn "write_to_sheet\|output_sheet[^s]" tests/graphs/`
Expected: empty.

- [ ] **Step 7: Commit**

```bash
git add tests/graphs/crdt_documents/c_import_analysis.json \
        tests/graphs/crdt_documents/f_cross_artifact_smoke.json \
        tests/graphs/crdt_documents/c_pandas_smoke.json
git commit -m "$(cat <<'EOF'
test(graphs): migrate crdt_doc graphs off legacy write_to_sheet

3 graphs that wrote a single tab via output_sheet + write_to_sheet
now use the multi-tab output_sheets pattern (verbose form unchanged,
but with output_sheets[name] = df instead).

Verified end-to-end with cargo run --bin dag_engine -- run for each.
Sees in /tmp/colmena_e2e/<name>_migrated.sse for reviewer audit.

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md §4

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: YAML descriptions + skill updates + E2E test graph

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml`
- Modify: `src/libs/colmena/text/tools/crdt_doc.yaml`
- Modify: `src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md`
- Modify: `src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md`
- Create: `tests/graphs/agents/gsheets_update_in_place.json`

Document the new modes in the LLM-facing surfaces and add an E2E test graph against the real Sheets API.

- [ ] **Step 1: Update `gsheets.yaml` description for `gsheets_run_python`**

Open `src/libs/colmena/text/tools/gsheets.yaml`. Append a new section to the existing `gsheets_run_python.description` block, between the "Multi-sheet write-back" section and the closing "For more patterns..." reference. Add:

````markdown

    ## Output modes (replace / update_in_place / overwrite)

    Each entry in `output_sheets` can be either a bare DataFrame (mode
    defaults to `replace`) OR a spec dict:

    ```python
    # Mode 1 — replace (default; tab must not exist or policy applies)
    output_sheets = {'NewTab': df}

    # Mode 2 — update_in_place (patch specific cells, never overwrites a full tab)
    output_sheets = {
        'Sales': {
            'mode': 'update_in_place',
            'df': df_modified,
            'key': 'product_id',          # column identifying rows
            'columns': ['price'],         # optional — only patch these columns
        }
    }

    # Mode 3 — overwrite (replace existing tab; explicit consent)
    output_sheets = {'Sales': {'mode': 'overwrite', 'df': df}}
    ```

    **Collision policy.** By default, if a tab named in `output_sheets`
    already exists, the tool fails with a `SheetExists` error returning
    metadata (row/col count, columns) and three suggested next moves
    (rename, update_in_place, overwrite). Operators can opt back into
    silent auto-suffix by setting `fixed_config.on_existing_sheet:
    "auto_suffix"` on the tool config.

    ⚠️ For 47 row updates in a 1000-row tab, `update_in_place` is the
    correct choice — it writes only the 47 changed cells via a single
    batchUpdate, instead of replacing 12K cells.
````

- [ ] **Step 2: Update `crdt_doc.yaml` description for `crdt_doc_run_python`**

Open `src/libs/colmena/text/tools/crdt_doc.yaml`. Find the `crdt_doc_run_python` entry. Remove any mention of `write_to_sheet` or `output_sheet`. Add the same modes section as Step 1, adapted: replace "Google Sheets"/"Sheets API" with "CRDT artifact" and replace "batchUpdate" with "per-cell ops".

- [ ] **Step 3: Add new section to `gsheets-cross-sheet-analysis/SKILL.md`**

Open `src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md`. Find a good place after the existing "joining tables" content. Add:

```markdown
## Updating existing tabs in place

When you want to change values for SOME rows in an existing tab WITHOUT
re-uploading the whole tab, use `update_in_place`. Example: change the
price of all electronics in a 1000-row Sales tab.

```python
import pandas as pd
sales = pd.DataFrame(sales_records)
mask = sales['category'] == 'Electronics'
sales.loc[mask, 'price'] = sales.loc[mask, 'price'] * 0.9  # 10% discount

output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': sales,
        'key': 'product_id',
        'columns': ['price'],  # only patch this column
    }
}
output = {'updated_rows': int(mask.sum())}
```

The dispatcher computes the diff vs the live tab and writes ONLY the
changed cells via one batchUpdate — your 47-row change is 47 cell writes,
not 12000.

**Rules:**
- `key` must be a column with unique values (duplicates in either side reject).
- `columns` is optional — omit to patch all common columns.
- If your DataFrame has rows not in the tab, they're silently skipped
  (set `strict_match: True` to reject instead).
- If you want to CREATE a new tab, use the simple form `output_sheets = {name: df}`.

**Avoid:**
- Don't use `update_in_place` to ADD new rows (it only patches existing
  ones). Use `mode: 'overwrite'` or write a fresh tab via the bare-DataFrame form.
```

- [ ] **Step 4: Mirror in `crdt-doc-cross-sheet-analysis/SKILL.md`**

Same content adapted for CRDT — replace "tab"/"batchUpdate" with "sheet"/"per-cell ops". Keep the `key`/`columns`/`strict_match` semantics identical.

- [ ] **Step 5: Build to ensure no broken includes**

Run: `cargo build -p colmena_dag_engine && cargo test -p colmena_dag_engine --lib text`
Expected: builds clean; text-loader tests still pass.

- [ ] **Step 6: Create the E2E test graph `gsheets_update_in_place.json`**

Create `tests/graphs/agents/gsheets_update_in_place.json`:

```json
{
  "_comment": "E2E: agent pre-seeds a 100-row Sales tab via gsheets_set_range, then uses gsheets_run_python with mode=update_in_place to apply a 10% discount only to 'Electronics' rows. Asserts cells_changed << total_rows. Requires GOOGLE_APPLICATION_CREDENTIALS + a writable spreadsheet.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/gsheets-update-in-place",
        "method": "POST",
        "test_payload": {
          "prompt": "Use spreadsheet 1F7AsFx4yW4uVnJRaRWwpzQuSNvruqGohI2B2NRygT-Y (Products test sheet, you have write access). On the 'Sales' tab, apply a 10% discount to all products whose category is 'Electronics'. Use mode=update_in_place with key=product_id and columns=['price'] so only the changed cells are written. Report cells_changed."
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
        "max_iterations": 10,
        "connection_url": "${DATABASE_URL}",
        "system_message": "You are a data analyst. Use gsheets_run_python with output_sheets in update_in_place mode (see the skill). Do NOT read and re-upload rows; you want a diff-write.",
        "enabled_tools": ["gsheets", "load_skill"]
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

(Note: the test graph references a known-writable spreadsheet for local runs. The reviewer can swap the ID for their own writable sheet. This graph is intended for manual E2E validation, not CI — Sheets API is external. Mark with `#[ignore]`-style guidance in the file comment.)

- [ ] **Step 7: Run the E2E graph manually + capture report**

Run:
```bash
set -a; source .env; set +a
export GOOGLE_APPLICATION_CREDENTIALS=/Users/danielgarcia/colmena-sa.json
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_update_in_place.json \
  --agent-session-id e2e_update_$(date +%s) > /tmp/colmena_e2e/gsheets_update_in_place.sse 2>&1
```
Expected: agent uses `update_in_place`, response shows `cells_changed` much less than total row count. If pre-seeding wasn't done in the test sheet, the agent should report `SheetExists` for the existing data — verify the LLM correctly chose `update_in_place` after seeing the error.

If the test sheet doesn't have a `Sales` tab, the graph will hit `UpdateRequiresExistingTab` — this is expected and is itself a valid coverage of the error path. Edit the prompt to first create the tab via `gsheets_run_python` with a small `replace` write, then a second call doing the `update_in_place`.

- [ ] **Step 8: Final full sweep — clippy + tests + fmt**

Run in parallel:
```bash
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings &
cargo test -p colmena_dag_engine --lib &
cargo fmt --check &
wait
```
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml \
        src/libs/colmena/text/tools/crdt_doc.yaml \
        src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md \
        src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md \
        tests/graphs/agents/gsheets_update_in_place.json
git commit -m "$(cat <<'EOF'
docs: document modes + collision policy + E2E update_in_place graph

- gsheets.yaml + crdt_doc.yaml: document replace/update_in_place/
  overwrite modes + collision policy in tool descriptions.
- gsheets-cross-sheet-analysis + crdt-doc-cross-sheet-analysis skills:
  add "Updating existing tabs in place" section with concrete pandas
  pattern for 'change N rows out of K' workflows.
- tests/graphs/agents/gsheets_update_in_place.json: E2E graph that
  exercises update_in_place against a real spreadsheet (manual run).

Spec: docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md §5

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Final Verification Checklist

After all 9 tasks complete, run these BEFORE declaring done:

- [ ] `cargo test --verbose -p colmena_dag_engine` — full unit + doctest pass
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — clean
- [ ] `cargo fmt --check` — clean
- [ ] `git log --oneline feature/docs..HEAD` — exactly 9 commits, all signed off
- [ ] `grep -rn "write_to_sheet\|output_sheet[^s]" src/ tests/ 2>/dev/null` — empty (legacy fully removed)
- [ ] Manually inspect the gsheets E2E run output in `/tmp/colmena_e2e/gsheets_update_in_place.sse` — confirm `update_in_place` mode was used by the agent

If any check fails, fix inline and add a follow-up commit.

---

## Why each task is one PR-ready commit

Each task ends in a single `git commit` so that:
- Reviewers can revert any individual task without unraveling others.
- The CRDT removal (Task 7) is separable from the wiring (Task 6) — a reviewer who needs a rollback path for the postlude removal can revert one commit cleanly.
- Test-graph migration (Task 8) is gated by the dispatcher being ready (Task 7).
- Subagent-driven execution gets a clear checkpoint per task — review the diff, confirm tests pass, move on.
