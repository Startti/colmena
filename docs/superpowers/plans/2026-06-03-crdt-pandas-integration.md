# CRDT Documents — Pandas/Python Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `crdt_doc_run_python(sheet_ids, code, write_to_sheet?)` LLM tool that runs sandboxed Python (with pandas/numpy/scipy.stats) against workbook data and optionally writes a new sheet from the result, separating "context the LLM sees" from "data the code processes" — saves 10x-1000x tokens for large Excel analysis.

**Architecture:** Tool dispatcher in `crdt_doc_tools.rs` orchestrates: extract sheet records from Y.Doc projection → wrap user code with prelude (build `dfs: dict[sheet_id, pd.DataFrame]`) and postlude (extract `output_sheet` as records) → run via `python_node::execute_sandboxed_helper` (extracted from existing PythonNode) → unpack results → optionally write `output_sheet` as new sheet in workbook with name-collision auto-suffix.

**Tech Stack:** Rust (PyO3 via existing python_node infra), Python sandbox (AST validation + import whitelist), pandas/numpy/scipy.stats (Python runtime deps — must be available in the worker's Python env).

**Spec:** [`docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md`](../specs/2026-06-03-crdt-pandas-integration-design.md)

---

## File map

| File | Action | Responsibility |
|---|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs` | Modify | Add `pandas`, `numpy`, `scipy.stats` to `_ALLOWED_IMPORTS`. Extract execution core as public helper `execute_sandboxed_helper(code, sandbox_mode, timeout_secs, inputs) -> Result<RunResult>` reusable from outside the node. |
| `src/libs/colmena/src/crdt_documents/df_records.rs` | Create | Y.Doc projection → records-style (`Vec<HashMap<String, serde_json::Value>>`). One sheet → one records vec. Size cap (combined 100MB JSON). |
| `src/libs/colmena/src/crdt_documents/df_writer.rs` | Create | Records + column list → Y.Doc sheet writes via `apply_add_sheet` + `apply_set_cell`. Sheet name collision auto-suffix. Atomic single transact_mut. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs` | Create | Tool definition + dispatch + orchestration: parse args, build records, wrap code with prelude/postlude, invoke python helper, extract `user_output` + `sheet_records` + `sheet_cols`, write back if requested, apply truncation, format response. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` | Modify | Re-export the new tool name + builder + dispatch function. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Modify | `pub mod crdt_doc_run_python;` + re-exports. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Modify | Wire dispatch for `crdt_doc_run_python` synthetic tool. |
| `src/libs/colmena/src/crdt_documents/mod.rs` | Modify | Export `df_records` + `df_writer` modules. |
| `src/libs/colmena/tests/crdt_doc_run_python_test.rs` | Create | Integration test: live server + peer + run_python with read-only and write_to_sheet. |
| `src/libs/colmena/tests/crdt_run_python_sandbox_test.rs` | Create | Verifies banned imports/builtins are rejected, allowed ones accepted. |
| `docs/developer_guide/38_crdt_documents.md` | Modify | New §5.6 documenting the tool + sandbox + tech-debt limits. |
| `docs/node_configurations.json` | Modify | If patterns enumerate tools — add `crdt_doc_run_python`. Otherwise skip. |
| `docs/BACKLOG.md` | Modify | Add "v1.1 — Configurable limits for run_python tool" entry. |
| `docs/CHANGELOG_2026-06.md` | Modify | Append "2. Pandas/Python Integration (subsistema C)" entry. |

---

## Task 1: Sandbox whitelist — add pandas, numpy, scipy.stats

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs:15`

- [ ] **Step 1: Read current python_node.rs whitelist block**

```bash
sed -n '10,25p' /Users/danielgarcia/startti/colmena/src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs
```

Confirm the current `_ALLOWED_IMPORTS = {...}` block contains: math, json, re, datetime, collections, itertools, functools, string, decimal, statistics.

- [ ] **Step 2: Modify the whitelist to include pandas, numpy, scipy.stats**

Edit line 15 area. The whitelist is defined as a Python string assigned to `_ALLOWED_IMPORTS`. Append the three new names. Preserve existing entries exactly.

Replace:
```python
_ALLOWED_IMPORTS = {
    'math', 'json', 're', 'datetime', 'collections',
    'itertools', 'functools', 'string', 'decimal', 'statistics',
}
```

with:
```python
_ALLOWED_IMPORTS = {
    'math', 'json', 're', 'datetime', 'collections',
    'itertools', 'functools', 'string', 'decimal', 'statistics',
    # crdt_doc_run_python additions (subsistema C, 2026-06):
    'pandas', 'numpy', 'scipy.stats',
}
```

The file is one Rust source containing a multi-line Python literal string — edit the literal carefully, preserving Rust string-escape semantics. If the original is `let sandbox_prelude = r#"..."#;` (raw string), the edit is plain. If it's a regular `"..."` string, you have to escape backslashes/quotes consistently with existing patterns. Check the surrounding code.

- [ ] **Step 3: Add a unit test that confirms pandas import passes validation**

In `python_node.rs`'s `#[cfg(test)] mod tests` (find the existing tests module — likely at the bottom). Add:

```rust
#[test]
fn restricted_mode_allows_pandas_import() {
    pyo3::prepare_freethreaded_python();
    pyo3::Python::with_gil(|py| {
        let result = validate_sandbox(py, "import pandas as pd\noutput = 1");
        match result {
            Ok(None) => {} // OK — no violation
            Ok(Some(v)) => panic!("expected pandas to pass, got violation: {v}"),
            Err(e) => panic!("validator errored: {e}"),
        }
    });
}

#[test]
fn restricted_mode_allows_numpy_import() {
    pyo3::prepare_freethreaded_python();
    pyo3::Python::with_gil(|py| {
        let result = validate_sandbox(py, "import numpy as np\noutput = np.array([1,2,3]).sum()");
        match result {
            Ok(None) => {}
            other => panic!("expected numpy to pass: {other:?}"),
        }
    });
}

#[test]
fn restricted_mode_allows_scipy_stats_import() {
    pyo3::prepare_freethreaded_python();
    pyo3::Python::with_gil(|py| {
        // scipy.stats is a submodule import — verify both forms.
        let result = validate_sandbox(py, "from scipy import stats\noutput = 1");
        match result {
            Ok(None) => {}
            other => panic!("expected `from scipy import stats` to pass: {other:?}"),
        }
    });
}

#[test]
fn restricted_mode_still_rejects_requests_import() {
    pyo3::prepare_freethreaded_python();
    pyo3::Python::with_gil(|py| {
        let result = validate_sandbox(py, "import requests\noutput = 1");
        match result {
            Ok(Some(_)) => {} // OK — violation reported
            other => panic!("requests should be rejected: {other:?}"),
        }
    });
}
```

(If `validate_sandbox` is `fn` not `pub fn`, make the tests inline in the same module so they have visibility — that's already the convention for unit tests.)

- [ ] **Step 4: Run the new tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib -p colmena_dag_engine python_node 2>&1 | tail -15
```

Expected: 4 new tests pass (the 3 import-allowed + the 1 requests-rejected). Plus any pre-existing python_node tests still pass.

Note: this is the FIRST place pandas is actually imported in a colmena test. If pandas is not installed in the system Python, the import test will fail with `ModuleNotFoundError`. If that happens, install pandas in the project's `.venv`:

```bash
.venv/bin/pip install pandas numpy scipy
```

And re-run the test. Document this requirement in the developer guide (Task 11).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs
git commit -m "feat(python_node): allow pandas, numpy, scipy.stats in restricted sandbox (C-T1)"
```

---

## Task 2: Extract `execute_sandboxed_helper` from PythonNode

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`

The current `PythonNode::execute` method contains the full execution logic inline. Task C-T5 will call this same logic from a different context (the run_python tool dispatcher). Extract it as a public helper.

- [ ] **Step 1: Read the current `execute` implementation**

```bash
sed -n '68,200p' /Users/danielgarcia/startti/colmena/src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs
```

Identify the core block that:
1. Validates sandbox (if `restricted`).
2. Calls `pyo3::Python::with_gil` → loads inputs → executes code → extracts `output`.
3. Handles timeout.
4. Returns the JSON-serialized `output`.

- [ ] **Step 2: Define a public helper function**

Right above `impl ExecutableNode for PythonNode` (or just below the `validate_sandbox` fn), add:

```rust
/// Result of running a Python code string via the sandboxed helper.
#[derive(Debug)]
pub struct PythonRunResult {
    /// The serialized value of the `output` variable in the user's namespace,
    /// or `None` if the user did not assign `output`.
    pub output: Option<serde_json::Value>,
    /// Captured stdout (best-effort — Python `print()` calls).
    pub stdout: String,
}

/// Run a Python code string with the same semantics as the `python_script`
/// DAG node. Used directly by other modules (e.g., `crdt_doc_run_python` tool)
/// that need fine-grained control of the namespace and result extraction.
///
/// `sandbox_mode`: `"none"` (full Python) or `"restricted"` (AST validation
///   + import whitelist + banned-builtin enforcement).
/// `timeout_secs`: applies only in `"restricted"` mode.
/// `inputs`: a map of variable_name → JSON value to inject as Python globals
///   before executing the code. Names starting with `__col_` are reserved.
///
/// Errors: returns `Err(String)` on sandbox violation, syntax error, timeout,
/// or runtime exception (with message + Python traceback when available).
pub fn execute_sandboxed_helper(
    code: &str,
    sandbox_mode: &str,
    timeout_secs: u64,
    inputs: &serde_json::Map<String, serde_json::Value>,
) -> Result<PythonRunResult, String> {
    // 1. Sandbox validation (if restricted).
    if sandbox_mode == "restricted" {
        pyo3::Python::with_gil(|py| {
            if let Some(violation) = validate_sandbox(py, code).map_err(|e| e.to_string())? {
                return Err(violation);
            }
            Ok(())
        })?;
    }

    // 2. Execute with GIL. Capture stdout via sys.stdout redirection.
    pyo3::Python::with_gil(|py| -> Result<PythonRunResult, String> {
        let globals = pyo3::types::PyDict::new(py);

        // Inject inputs as globals.
        for (k, v) in inputs.iter() {
            let py_val = pythonize::pythonize(py, v)
                .map_err(|e| format!("inject {k}: {e}"))?;
            globals
                .set_item(k, py_val)
                .map_err(|e| format!("set global {k}: {e}"))?;
        }

        // Redirect stdout to a StringIO for capture.
        let stdout_capture = py
            .eval_bound("__import__('io').StringIO()", None, None)
            .map_err(|e| format!("create StringIO: {e}"))?;
        let sys_module = py
            .import_bound("sys")
            .map_err(|e| format!("import sys: {e}"))?;
        let original_stdout = sys_module
            .getattr("stdout")
            .map_err(|e| format!("get sys.stdout: {e}"))?;
        sys_module
            .setattr("stdout", &stdout_capture)
            .map_err(|e| format!("redirect stdout: {e}"))?;

        // Execute. Use py.run_bound (no return value — assigns to namespace).
        let exec_result = py.run_bound(code, Some(&globals), None);

        // Restore stdout regardless of success.
        let _ = sys_module.setattr("stdout", original_stdout);

        // Capture stdout text.
        let stdout = stdout_capture
            .call_method0("getvalue")
            .and_then(|v| v.extract::<String>())
            .unwrap_or_default();

        // Surface execution errors.
        if let Err(e) = exec_result {
            return Err(format!("{e}"));
        }

        // Extract `output` if defined.
        let output_obj = globals.get_item("output").ok().flatten();
        let output = match output_obj {
            Some(obj) => {
                let val: serde_json::Value = pythonize::depythonize_bound(obj)
                    .map_err(|e| format!("serialize output: {e}"))?;
                Some(val)
            }
            None => None,
        };

        Ok(PythonRunResult { output, stdout })
    })
}
```

Note: this uses `pyo3` and `pythonize` patterns. Check the existing PythonNode for the exact `pyo3` API style (the project may use older pyo3 with non-`_bound` variants). Adapt to match.

The timeout enforcement is intentionally not implemented in this snippet because the existing PythonNode handles it via `tokio::time::timeout` around a `spawn_blocking`. The caller of `execute_sandboxed_helper` is responsible for wrapping in `spawn_blocking` + `tokio::time::timeout` if needed (C-T5 will do this).

- [ ] **Step 3: Refactor `PythonNode::execute` to call the helper**

Replace the inline execution logic in `PythonNode::execute` with:

```rust
// Inside execute(), after extracting sandbox_mode, timeout, code, inputs:
let helper_inputs: serde_json::Map<String, serde_json::Value> = inputs
    .iter()
    .filter(|(k, _)| !sandbox_keys.contains(&k.as_str()))
    .map(|(k, v)| (k.clone(), v.clone()))
    .collect();
let result = tokio::task::spawn_blocking(move || {
    execute_sandboxed_helper(&code, &sandbox_mode_clone, sandbox_timeout, &helper_inputs)
})
.await
.map_err(|e| format!("join: {e}"))?;

let output_json = match result {
    Ok(r) => r.output.unwrap_or(serde_json::Value::Null),
    Err(e) => return Err(format!("Python execution failed: {e}").into()),
};
// (No timeout wrapping yet — PythonNode keeps its existing timeout flow if
// sandbox_mode == "restricted". Refactor cautiously to preserve behavior.)
```

The exact replacement depends on how `PythonNode::execute` is structured today. Match the existing patterns. If the refactor risks behavior changes, add a regression test first or report DONE_WITH_CONCERNS.

- [ ] **Step 4: Verify existing PythonNode tests + new helper passes**

```bash
cargo test --lib -p colmena_dag_engine python_node 2>&1 | tail -20
```

Expected: all existing tests still pass. If any fail because of the refactor, the refactor introduced a regression — investigate, fix, or roll back.

- [ ] **Step 5: Add a unit test for the new helper**

In `python_node.rs` tests module:

```rust
#[test]
fn execute_sandboxed_helper_basic_addition() {
    pyo3::prepare_freethreaded_python();
    let mut inputs = serde_json::Map::new();
    inputs.insert("x".into(), serde_json::json!(5));
    inputs.insert("y".into(), serde_json::json!(7));
    let result = execute_sandboxed_helper(
        "output = x + y",
        "none",
        10,
        &inputs,
    ).unwrap();
    assert_eq!(result.output, Some(serde_json::json!(12)));
}

#[test]
fn execute_sandboxed_helper_captures_stdout() {
    pyo3::prepare_freethreaded_python();
    let inputs = serde_json::Map::new();
    let result = execute_sandboxed_helper(
        "print('hello'); output = 1",
        "none",
        10,
        &inputs,
    ).unwrap();
    assert!(result.stdout.contains("hello"));
    assert_eq!(result.output, Some(serde_json::json!(1)));
}

#[test]
fn execute_sandboxed_helper_no_output_returns_none() {
    pyo3::prepare_freethreaded_python();
    let inputs = serde_json::Map::new();
    let result = execute_sandboxed_helper(
        "x = 5  # no output assignment",
        "none",
        10,
        &inputs,
    ).unwrap();
    assert_eq!(result.output, None);
}

#[test]
fn execute_sandboxed_helper_restricted_rejects_violation() {
    pyo3::prepare_freethreaded_python();
    let inputs = serde_json::Map::new();
    let err = execute_sandboxed_helper(
        "import requests",
        "restricted",
        10,
        &inputs,
    ).unwrap_err();
    assert!(err.contains("requests") || err.contains("not allowed"));
}
```

```bash
cargo test --lib -p colmena_dag_engine python_node 2>&1 | tail -15
```

Expected: 4 new helper tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs
git commit -m "refactor(python_node): extract execute_sandboxed_helper for reuse (C-T2)"
```

---

## Task 3: `df_records.rs` — Y.Doc projection → records (no pandas in Rust)

**Files:**
- Create: `src/libs/colmena/src/crdt_documents/df_records.rs`
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs`

This module converts the workbook's IR projection into "records-style" data: a list of dicts where keys are column names (row 1) and values are cell values. This is what pandas calls `pd.DataFrame.from_records()` input.

- [ ] **Step 1: Write tests first**

In a NEW file `src/libs/colmena/src/crdt_documents/df_records.rs`, top:

```rust
//! Convert Y.Doc workbook sheets into records-style data
//! (`Vec<HashMap<String, serde_json::Value>>`) for ingestion by pandas
//! `DataFrame.from_records(...)` on the Python side. Assumes row 1 is
//! the header row; falls back to `col_A`, `col_B`, ... when headers are
//! missing or non-string.

use crate::crdt_documents::projection;
use serde_json::{Map, Value};
use std::collections::HashMap;
use yrs::Doc;

/// Combined size cap for the records produced across all sheets in one
/// `run_python` call (v1 hard limit; see BACKLOG for configurable path).
pub const COMBINED_RECORDS_SIZE_CAP_BYTES: usize = 100 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RecordsError {
    #[error("sheet not found: {0}")]
    SheetNotFound(String),
    #[error("combined records size {actual} bytes exceeds cap {limit} bytes")]
    SizeCapExceeded { actual: usize, limit: usize },
}

#[derive(Debug, Clone)]
pub struct SheetRecords {
    pub sheet_id: String,
    pub columns: Vec<String>,
    /// Row-major: each inner Map is one row, keyed by column name.
    pub records: Vec<Map<String, Value>>,
}

/// Build records for one sheet. Internal helper.
pub fn build_sheet_records(doc: &Doc, sheet_id: &str) -> Result<SheetRecords, RecordsError>;

/// Build records for multiple sheets in one call. Enforces combined size cap.
pub fn build_records_for_sheets(
    doc: &Doc,
    sheet_ids: &[String],
) -> Result<HashMap<String, SheetRecords>, RecordsError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};
    use yrs::Doc;

    fn make_doc_with_inventory() -> (Doc, String) {
        let doc = Doc::new();
        let sheet_id = apply_add_sheet(&doc, "Inventory");
        apply_set_cell_in_proc(&doc, &sheet_id, "A1", &serde_json::json!("Product"));
        apply_set_cell_in_proc(&doc, &sheet_id, "B1", &serde_json::json!("Qty"));
        apply_set_cell_in_proc(&doc, &sheet_id, "A2", &serde_json::json!("Apple"));
        apply_set_cell_in_proc(&doc, &sheet_id, "B2", &serde_json::json!(10));
        apply_set_cell_in_proc(&doc, &sheet_id, "A3", &serde_json::json!("Pear"));
        apply_set_cell_in_proc(&doc, &sheet_id, "B3", &serde_json::json!(20));
        (doc, sheet_id)
    }

    #[test]
    fn extracts_headers_and_rows() {
        let (doc, sid) = make_doc_with_inventory();
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns, vec!["Product".to_string(), "Qty".to_string()]);
        assert_eq!(recs.records.len(), 2);
        assert_eq!(recs.records[0]["Product"], serde_json::json!("Apple"));
        assert_eq!(recs.records[0]["Qty"], serde_json::json!(10));
        assert_eq!(recs.records[1]["Product"], serde_json::json!("Pear"));
    }

    #[test]
    fn missing_sheet_returns_not_found() {
        let doc = Doc::new();
        let err = build_sheet_records(&doc, "sh_does_not_exist").unwrap_err();
        assert!(matches!(err, RecordsError::SheetNotFound(_)));
    }

    #[test]
    fn empty_sheet_returns_empty_columns_and_rows() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "Blank");
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns.len(), 0);
        assert_eq!(recs.records.len(), 0);
    }

    #[test]
    fn headers_only_returns_zero_rows_with_columns() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "HeadersOnly");
        apply_set_cell_in_proc(&doc, &sid, "A1", &serde_json::json!("X"));
        apply_set_cell_in_proc(&doc, &sid, "B1", &serde_json::json!("Y"));
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns, vec!["X".to_string(), "Y".to_string()]);
        assert_eq!(recs.records.len(), 0);
    }

    #[test]
    fn non_string_headers_fall_back_to_col_letters() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "BadHeaders");
        // Numbers as "headers" — they're cell values but not valid Python col names.
        apply_set_cell_in_proc(&doc, &sid, "A1", &serde_json::json!(1.5));
        apply_set_cell_in_proc(&doc, &sid, "B1", &serde_json::json!(2.5));
        apply_set_cell_in_proc(&doc, &sid, "A2", &serde_json::json!("data"));
        let recs = build_sheet_records(&doc, &sid).unwrap();
        // Non-string headers should be stringified or replaced.
        // (Decision: stringify "1.5", "2.5" as column names.)
        assert_eq!(recs.columns, vec!["1.5".to_string(), "2.5".to_string()]);
    }

    #[test]
    fn sparse_cells_become_null_in_records() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "Sparse");
        apply_set_cell_in_proc(&doc, &sid, "A1", &serde_json::json!("X"));
        apply_set_cell_in_proc(&doc, &sid, "B1", &serde_json::json!("Y"));
        apply_set_cell_in_proc(&doc, &sid, "A2", &serde_json::json!("filled"));
        // B2 is missing.
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.records.len(), 1);
        assert_eq!(recs.records[0]["X"], serde_json::json!("filled"));
        assert_eq!(recs.records[0]["Y"], serde_json::json!(null));
    }

    #[test]
    fn build_multiple_sheets() {
        let doc = Doc::new();
        let s1 = apply_add_sheet(&doc, "First");
        let s2 = apply_add_sheet(&doc, "Second");
        apply_set_cell_in_proc(&doc, &s1, "A1", &serde_json::json!("A"));
        apply_set_cell_in_proc(&doc, &s1, "A2", &serde_json::json!(1));
        apply_set_cell_in_proc(&doc, &s2, "A1", &serde_json::json!("B"));
        apply_set_cell_in_proc(&doc, &s2, "A2", &serde_json::json!(2));
        let map = build_records_for_sheets(&doc, &[s1.clone(), s2.clone()]).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&s1));
        assert!(map.contains_key(&s2));
    }
}
```

- [ ] **Step 2: Implement the module body**

Replace the function stubs with full implementations:

```rust
pub fn build_sheet_records(doc: &Doc, sheet_id: &str) -> Result<SheetRecords, RecordsError> {
    let proj = projection::project(doc);
    let sheets = proj["sheets"].as_array().cloned().unwrap_or_default();
    let sheet = sheets
        .into_iter()
        .find(|s| s["id"].as_str() == Some(sheet_id))
        .ok_or_else(|| RecordsError::SheetNotFound(sheet_id.to_string()))?;
    let cells_map = sheet["cells"].as_object().cloned().unwrap_or_default();

    // 1. Parse all addresses → (row, col, value).
    let mut parsed: Vec<(u32, u32, Value)> = Vec::new();
    for (addr, value) in cells_map.into_iter() {
        if let Some((row, col)) = parse_a1(&addr) {
            parsed.push((row, col, value));
        }
    }
    if parsed.is_empty() {
        return Ok(SheetRecords {
            sheet_id: sheet_id.to_string(),
            columns: Vec::new(),
            records: Vec::new(),
        });
    }

    // 2. Determine column count = max col + 1.
    let max_col = parsed.iter().map(|(_, c, _)| *c).max().unwrap();
    let max_row = parsed.iter().map(|(r, _, _)| *r).max().unwrap();

    // 3. Build a dense grid: rows[r][c] = Value::Null by default.
    let mut grid: Vec<Vec<Value>> = (0..=max_row)
        .map(|_| vec![Value::Null; (max_col + 1) as usize])
        .collect();
    for (r, c, v) in parsed {
        grid[r as usize][c as usize] = v;
    }

    // 4. Headers from row 0 (1-indexed row 1).
    let columns: Vec<String> = grid[0]
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::String(s) => s.clone(),
            Value::Null => format!("col_{}", col_letter(i as u32)),
            other => other.to_string().trim_matches('"').to_string(),
        })
        .collect();

    // 5. Records from row 1 onward (1-indexed row 2+).
    let mut records: Vec<Map<String, Value>> = Vec::new();
    for row in grid.iter().skip(1) {
        // Skip entirely-empty rows.
        if row.iter().all(|v| v.is_null()) {
            continue;
        }
        let mut record = Map::new();
        for (i, v) in row.iter().enumerate() {
            let col_name = columns.get(i).cloned().unwrap_or_else(|| format!("col_{}", col_letter(i as u32)));
            record.insert(col_name, v.clone());
        }
        records.push(record);
    }

    Ok(SheetRecords {
        sheet_id: sheet_id.to_string(),
        columns,
        records,
    })
}

pub fn build_records_for_sheets(
    doc: &Doc,
    sheet_ids: &[String],
) -> Result<HashMap<String, SheetRecords>, RecordsError> {
    let mut total_bytes: usize = 0;
    let mut out = HashMap::new();
    for sid in sheet_ids {
        let recs = build_sheet_records(doc, sid)?;
        // Approximate size: serialize each record. Cheap upper bound.
        let approx = serde_json::to_vec(&recs.records)
            .map(|v| v.len())
            .unwrap_or(0);
        total_bytes = total_bytes.saturating_add(approx);
        if total_bytes > COMBINED_RECORDS_SIZE_CAP_BYTES {
            return Err(RecordsError::SizeCapExceeded {
                actual: total_bytes,
                limit: COMBINED_RECORDS_SIZE_CAP_BYTES,
            });
        }
        out.insert(sid.clone(), recs);
    }
    Ok(out)
}

// ── helpers ──────────────────────────────────────────────────────────────

fn parse_a1(addr: &str) -> Option<(u32, u32)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let col_part = &addr[..split];
    let row_part = &addr[split..];
    let row: u32 = row_part.parse().ok()?;
    let row = row.checked_sub(1)?;
    let mut col: u32 = 0;
    for ch in col_part.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        col = col * 26 + (ch as u32 - 'A' as u32 + 1);
    }
    Some((row, col.checked_sub(1)?))
}

fn col_letter(mut col: u32) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s
}
```

- [ ] **Step 3: Register module + export in mod.rs**

Edit `src/libs/colmena/src/crdt_documents/mod.rs`:

Add to the `pub mod` declarations:
```rust
pub mod df_records;
```

Add to the re-exports:
```rust
pub use df_records::{
    build_records_for_sheets, build_sheet_records, RecordsError, SheetRecords,
    COMBINED_RECORDS_SIZE_CAP_BYTES,
};
```

- [ ] **Step 4: Run the tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib -p colmena_dag_engine df_records 2>&1 | tail -15
```

Expected: 7 tests pass.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --lib --tests -p colmena_dag_engine 2>&1 | grep -E "warning|error" | head
```

Expected: empty.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/df_records.rs \
        src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "feat(crdt_documents): df_records — Y.Doc projection to pandas records (C-T3)"
```

---

## Task 4: `df_writer.rs` — records → Y.Doc cells

**Files:**
- Create: `src/libs/colmena/src/crdt_documents/df_writer.rs`
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs`

This module takes records (output_sheet from Python) + column list and writes them into a new sheet, including name-collision resolution.

- [ ] **Step 1: Write tests first**

Create `src/libs/colmena/src/crdt_documents/df_writer.rs`:

```rust
//! Convert records-style data (output_sheet from `crdt_doc_run_python`)
//! into Y.Doc sheet writes. Owns sheet creation, name collision resolution,
//! and atomic single-transact_mut write.

use crate::crdt_documents::projection;
use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};
use serde_json::{Map, Value};
use yrs::Doc;

/// Caps for v1. See BACKLOG for v1.1 configurable path.
pub const MAX_OUTPUT_SHEET_ROWS: usize = 100_000;
/// Excel xlsx hard limit on sheet name length.
pub const MAX_SHEET_NAME_LEN: usize = 31;

#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("sheet name '{0}' is empty")]
    EmptyName(String),
}

#[derive(Debug, Clone)]
pub struct WriteResult {
    pub sheet_id: String,
    pub resolved_name: String,
    pub n_rows: usize,
    pub n_cols: usize,
    pub truncated_at: Option<usize>,
}

/// Write `records` as a new sheet named `requested_name` (with auto-suffix
/// on collision). Returns the resolved sheet metadata.
pub fn write_records_as_new_sheet(
    doc: &Doc,
    requested_name: &str,
    columns: &[String],
    records: &[Map<String, Value>],
) -> Result<WriteResult, WriterError>;

/// Resolve a unique sheet name. Pure function; exposed for testing.
pub fn resolve_unique_sheet_name(doc: &Doc, requested: &str) -> String;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yrs::Doc;

    fn make_record(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), v.clone());
        }
        m
    }

    #[test]
    fn write_basic_records_creates_sheet_with_headers_and_data() {
        let doc = Doc::new();
        let cols = vec!["Region".to_string(), "Sales".to_string()];
        let records = vec![
            make_record(&[("Region", json!("North")), ("Sales", json!(450))]),
            make_record(&[("Region", json!("South")), ("Sales", json!(320))]),
        ];
        let result = write_records_as_new_sheet(&doc, "Summary", &cols, &records).unwrap();
        assert_eq!(result.resolved_name, "Summary");
        assert_eq!(result.n_rows, 2);
        assert_eq!(result.n_cols, 2);
        assert!(result.truncated_at.is_none());

        // Verify the sheet exists with correct content via projection.
        let proj = projection::project(&doc);
        let sheet = proj["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "Summary")
            .unwrap()
            .clone();
        assert_eq!(sheet["cells"]["A1"], json!("Region"));
        assert_eq!(sheet["cells"]["B1"], json!("Sales"));
        assert_eq!(sheet["cells"]["A2"], json!("North"));
        assert_eq!(sheet["cells"]["B2"], json!(450));
        assert_eq!(sheet["cells"]["A3"], json!("South"));
    }

    #[test]
    fn collision_resolution_appends_suffix() {
        let doc = Doc::new();
        let _ = apply_add_sheet(&doc, "Summary");
        let cols = vec!["A".to_string()];
        let records = vec![make_record(&[("A", json!(1))])];
        let result = write_records_as_new_sheet(&doc, "Summary", &cols, &records).unwrap();
        assert_eq!(result.resolved_name, "Summary (2)");
    }

    #[test]
    fn nested_collision_keeps_advancing() {
        let doc = Doc::new();
        let _ = apply_add_sheet(&doc, "Summary");
        let _ = apply_add_sheet(&doc, "Summary (2)");
        let _ = apply_add_sheet(&doc, "Summary (3)");
        let cols = vec!["A".to_string()];
        let records = vec![make_record(&[("A", json!(1))])];
        let result = write_records_as_new_sheet(&doc, "Summary", &cols, &records).unwrap();
        assert_eq!(result.resolved_name, "Summary (4)");
    }

    #[test]
    fn empty_records_writes_only_headers() {
        let doc = Doc::new();
        let cols = vec!["X".to_string(), "Y".to_string()];
        let result = write_records_as_new_sheet(&doc, "Empty", &cols, &[]).unwrap();
        assert_eq!(result.n_rows, 0);
        assert_eq!(result.n_cols, 2);
    }

    #[test]
    fn rejects_empty_name() {
        let doc = Doc::new();
        let err = write_records_as_new_sheet(&doc, "", &[], &[]).unwrap_err();
        assert!(matches!(err, WriterError::EmptyName(_)));
    }

    #[test]
    fn truncates_at_max_rows() {
        let doc = Doc::new();
        let cols = vec!["A".to_string()];
        let records: Vec<Map<String, Value>> = (0..MAX_OUTPUT_SHEET_ROWS + 100)
            .map(|i| make_record(&[("A", json!(i))]))
            .collect();
        let result = write_records_as_new_sheet(&doc, "Big", &cols, &records).unwrap();
        assert_eq!(result.n_rows, MAX_OUTPUT_SHEET_ROWS);
        assert_eq!(result.truncated_at, Some(MAX_OUTPUT_SHEET_ROWS));
    }
}
```

- [ ] **Step 2: Implement the module body**

```rust
pub fn write_records_as_new_sheet(
    doc: &Doc,
    requested_name: &str,
    columns: &[String],
    records: &[Map<String, Value>],
) -> Result<WriteResult, WriterError> {
    if requested_name.is_empty() {
        return Err(WriterError::EmptyName(requested_name.to_string()));
    }

    // Resolve name.
    let resolved = resolve_unique_sheet_name(doc, requested_name);
    // Truncate to Excel limit (31 chars).
    let resolved_capped = if resolved.len() > MAX_SHEET_NAME_LEN {
        resolved[..MAX_SHEET_NAME_LEN].to_string()
    } else {
        resolved
    };

    // Truncate rows.
    let (rows_to_write, truncated_at) = if records.len() > MAX_OUTPUT_SHEET_ROWS {
        (&records[..MAX_OUTPUT_SHEET_ROWS], Some(MAX_OUTPUT_SHEET_ROWS))
    } else {
        (records, None)
    };

    // Create sheet.
    let sheet_id = apply_add_sheet(doc, &resolved_capped);

    // Write headers (row 1).
    for (i, col_name) in columns.iter().enumerate() {
        let addr = format!("{}{}", col_letter(i as u32), 1);
        apply_set_cell_in_proc(doc, &sheet_id, &addr, &Value::String(col_name.clone()));
    }

    // Write rows (starting at row 2).
    for (r_idx, record) in rows_to_write.iter().enumerate() {
        let row_num = (r_idx + 2) as u32;
        for (c_idx, col_name) in columns.iter().enumerate() {
            let addr = format!("{}{}", col_letter(c_idx as u32), row_num);
            let val = record.get(col_name).cloned().unwrap_or(Value::Null);
            // Skip nulls (no-op write).
            if val.is_null() {
                continue;
            }
            apply_set_cell_in_proc(doc, &sheet_id, &addr, &val);
        }
    }

    Ok(WriteResult {
        sheet_id,
        resolved_name: resolved_capped,
        n_rows: rows_to_write.len(),
        n_cols: columns.len(),
        truncated_at,
    })
}

pub fn resolve_unique_sheet_name(doc: &Doc, requested: &str) -> String {
    let proj = projection::project(doc);
    let existing: std::collections::HashSet<String> = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s["name"].as_str().map(String::from))
        .collect();
    if !existing.contains(requested) {
        return requested.to_string();
    }
    for i in 2..1000 {
        let candidate = format!("{requested} ({i})");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    format!("{requested} {}", chrono::Utc::now().timestamp())
}

fn col_letter(mut col: u32) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s
}
```

- [ ] **Step 3: Register module + export**

In `src/libs/colmena/src/crdt_documents/mod.rs`:

```rust
pub mod df_writer;
pub use df_writer::{
    resolve_unique_sheet_name, write_records_as_new_sheet, WriteResult, WriterError,
    MAX_OUTPUT_SHEET_ROWS, MAX_SHEET_NAME_LEN,
};
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib -p colmena_dag_engine df_writer 2>&1 | tail -15
```

Expected: 6 tests pass.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --lib --tests -p colmena_dag_engine 2>&1 | grep -E "warning|error" | head
```

Expected: empty.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/df_writer.rs \
        src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "feat(crdt_documents): df_writer — records to Y.Doc sheet with collision resolution (C-T4)"
```

---

## Task 5: Tool dispatcher `crdt_doc_run_python`

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

This is the biggest task — combines orchestration, prelude/postlude wrapping, truncation, and wiring.

- [ ] **Step 1: Create the file**

```rust
//! LLM tool `crdt_doc_run_python` — runs sandboxed Python (pandas/numpy/
//! scipy.stats) against workbook data extracted from the current
//! `CrdtDocsContext`. See:
//!   - Spec: docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md
//!   - Plan: docs/superpowers/plans/2026-06-03-crdt-pandas-integration.md

use crate::crdt_documents::{
    build_records_for_sheets, write_records_as_new_sheet, RecordsError, SheetRecords,
};
use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use crate::llm::domain::tools::ToolDefinition;
use schemars::JsonSchema;
use serde::Deserialize;

pub use super::crdt_doc_context::CrdtDocsContext;

pub const TOOL_RUN_PYTHON: &str = "crdt_doc_run_python";

/// Caps that apply to the response sent back to the LLM.
const OUTPUT_BYTE_CAP: usize = 10 * 1024;
const STDOUT_BYTE_CAP: usize = 10 * 1024;
const ERROR_BYTE_CAP: usize = 10 * 1024;
const PREVIEW_ROWS_IN_WROTE_SHEET: usize = 5;
/// Sandbox timeout for code execution (v1; see BACKLOG for configurable path).
const CODE_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunPythonArgs {
    /// Sheets to load as pandas DataFrames. Available in the code as
    /// `dfs[<sheet_id>]`. At least one required.
    pub sheet_ids: Vec<String>,
    /// Python code to execute. Must define `output` (any JSON-serializable
    /// value) and/or `output_sheet` (a pandas DataFrame). Has access to
    /// `pandas as pd`, `numpy as np`, `scipy.stats as stats`.
    pub code: String,
    /// If set, `output_sheet` is written as a new sheet with this name.
    /// Name collisions append " (2)", " (3)" etc.
    #[serde(default)]
    pub write_to_sheet: Option<String>,
}

pub fn tool_run_python() -> ToolDefinition {
    super::build_synthetic_tool::<RunPythonArgs>(
        TOOL_RUN_PYTHON,
        "Run sandboxed Python code (pandas, numpy, scipy.stats) against \
         workbook data. Loads requested sheets as `dfs[sheet_id]` pandas \
         DataFrames. The code must define `output` (returned to you) and/or \
         `output_sheet` (a DataFrame). If `write_to_sheet` is set, \
         `output_sheet` is persisted as a new sheet with that name. \
         Use this for analysis on large sheets — only first 10 rows need to \
         pass through your context; the code runs server-side on the full data.",
    )
}

pub async fn execute_run_python(
    ctx: &CrdtDocsContext,
    args: RunPythonArgs,
) -> serde_json::Value {
    // 1. Validate args.
    if args.sheet_ids.is_empty() {
        return serde_json::json!({"error": "sheet_ids must be non-empty"});
    }

    // 2. Build records from the Y.Doc.
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({"error": "artifact_not_found"});
    };
    let records_by_sheet = match build_records_for_sheets(&doc, &args.sheet_ids) {
        Ok(m) => m,
        Err(RecordsError::SheetNotFound(id)) => {
            return serde_json::json!({"error": format!("sheet_not_found: {id}")});
        }
        Err(RecordsError::SizeCapExceeded { actual, limit }) => {
            return serde_json::json!({
                "error": "load_size_exceeded",
                "actual_bytes": actual,
                "limit_bytes": limit,
            });
        }
    };

    // 3. Build inputs for the python helper: `_dfs_raw` is a dict whose
    //    values are lists of dicts (records). The auto-prelude builds
    //    pandas DataFrames from these.
    let mut dfs_raw_json = serde_json::Map::new();
    for (sid, recs) in &records_by_sheet {
        dfs_raw_json.insert(
            sid.clone(),
            serde_json::Value::Array(
                recs.records.iter().cloned().map(serde_json::Value::Object).collect(),
            ),
        );
    }
    let mut inputs = serde_json::Map::new();
    inputs.insert("_dfs_raw".to_string(), serde_json::Value::Object(dfs_raw_json));

    // 4. Wrap user code with prelude (build dfs) and postlude (package output).
    let wrapped_code = wrap_user_code(&args.code);

    // 5. Run in spawn_blocking with a timeout.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(CODE_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            execute_sandboxed_helper(&wrapped_code, "restricted", CODE_TIMEOUT_SECS, &inputs)
        }),
    )
    .await;

    let helper_result = match result {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(e))) => {
            // Python sandbox/syntax/runtime error.
            return serde_json::json!({
                "output": serde_json::Value::Null,
                "wrote_sheet": serde_json::Value::Null,
                "stdout": "",
                "error": truncate(&e, ERROR_BYTE_CAP),
            });
        }
        Ok(Err(join_err)) => {
            return serde_json::json!({"error": format!("internal join error: {join_err}")});
        }
        Err(_) => {
            return serde_json::json!({
                "error": format!("code execution exceeded {CODE_TIMEOUT_SECS}s timeout"),
            });
        }
    };

    // 6. Unpack the wrapped output. The postlude assigns `output` to a dict
    //    like {"user_output": ..., "sheet_records": ..., "sheet_cols": ...}.
    let wrapped_output = helper_result.output.unwrap_or(serde_json::Value::Null);
    let user_output = wrapped_output
        .get("user_output")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let sheet_records = wrapped_output
        .get("sheet_records")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let sheet_cols = wrapped_output
        .get("sheet_cols")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // 7. If `write_to_sheet` was set AND `output_sheet` is non-null,
    //    write to Y.Doc.
    let mut wrote_sheet_response = serde_json::Value::Null;
    if let Some(target_name) = args.write_to_sheet.as_deref() {
        if let (Some(records_arr), Some(cols_arr)) = (sheet_records.as_array(), sheet_cols.as_array()) {
            let cols: Vec<String> = cols_arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            let records: Vec<serde_json::Map<String, serde_json::Value>> = records_arr
                .iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();
            match write_records_as_new_sheet(&doc, target_name, &cols, &records) {
                Ok(wr) => {
                    let preview: Vec<&serde_json::Map<String, serde_json::Value>> =
                        records.iter().take(PREVIEW_ROWS_IN_WROTE_SHEET).collect();
                    wrote_sheet_response = serde_json::json!({
                        "sheet_id": wr.sheet_id,
                        "name": wr.resolved_name,
                        "n_rows": wr.n_rows,
                        "n_cols": wr.n_cols,
                        "preview": preview,
                        "truncated_at": wr.truncated_at,
                    });
                    // Mark doc dirty + record event.
                    ctx.mark_dirty();
                    let origin = ctx
                        .session_id()
                        .map(|s| format!("agent:{s}"))
                        .unwrap_or_else(|| "agent:llm".to_string());
                    let _ = ctx
                        .backend()
                        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                            artifact_id: ctx.artifact_id().clone(),
                            sheet_id: Some(wr.sheet_id.clone()),
                            origin,
                            summary: format!(
                                "wrote {} rows via run_python to new sheet '{}'",
                                wr.n_rows, wr.resolved_name
                            ),
                        })
                        .await;
                }
                Err(e) => {
                    return serde_json::json!({
                        "output": user_output,
                        "wrote_sheet": serde_json::Value::Null,
                        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
                        "error": format!("write_to_sheet failed: {e}"),
                    });
                }
            }
        }
    }

    // 8. Truncate output JSON if too large.
    let (user_output_capped, output_truncated) = truncate_json(&user_output, OUTPUT_BYTE_CAP);

    let mut response = serde_json::json!({
        "output": user_output_capped,
        "wrote_sheet": wrote_sheet_response,
        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
        "error": serde_json::Value::Null,
    });
    if output_truncated {
        response["_output_truncated"] = serde_json::json!(true);
    }
    response
}

pub async fn dispatch_crdt_doc_run_python(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<RunPythonArgs>(args) {
        Ok(a) => execute_run_python(ctx, a).await,
        Err(e) => serde_json::json!({"error": format!("invalid_args: {e}")}),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn wrap_user_code(user_code: &str) -> String {
    format!(
        r#"
# === colmena auto-prelude (do not modify) ===
import pandas as pd
import numpy as np
from scipy import stats

dfs = {{k: pd.DataFrame(v) for k, v in _dfs_raw.items()}}
del _dfs_raw

# === user code starts here ===
{USER_CODE}
# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None
__col_sheet_records = None
__col_sheet_cols = None
if 'output_sheet' in dir() and output_sheet is not None:
    import pandas as _pd
    if isinstance(output_sheet, _pd.DataFrame):
        __col_sheet_records = output_sheet.to_dict('records')
        __col_sheet_cols = list(output_sheet.columns)

output = {{
    'user_output': __col_user_output,
    'sheet_records': __col_sheet_records,
    'sheet_cols': __col_sheet_cols,
}}
"#,
        USER_CODE = user_code,
    )
    .replace("{USER_CODE}", user_code)
}

fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        s.to_string()
    } else {
        format!("{}…[truncated at {} bytes]", &s[..cap], cap)
    }
}

fn truncate_json(v: &serde_json::Value, cap: usize) -> (serde_json::Value, bool) {
    let serialized = serde_json::to_string(v).unwrap_or_default();
    if serialized.len() <= cap {
        (v.clone(), false)
    } else {
        (
            serde_json::Value::String(format!(
                "{}…[truncated at {} bytes]",
                &serialized[..cap],
                cap
            )),
            true,
        )
    }
}
```

Note on `wrap_user_code`: the `format!` macro doesn't support named args directly with the `r#"..."#` raw string syntax in stable Rust easily; using `.replace("{USER_CODE}", user_code)` is the clean workaround. The `r#""#` raw string allows literal `{` and `}` in the prelude without escaping pandas dict syntax.

- [ ] **Step 2: Wire mod.rs**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`:

```rust
pub mod crdt_doc_run_python;

pub use crdt_doc_run_python::{
    dispatch_crdt_doc_run_python, execute_run_python, tool_run_python, RunPythonArgs,
    TOOL_RUN_PYTHON as CRDT_DOC_RUN_PYTHON_TOOL,
};
```

- [ ] **Step 3: Wire into `build_all_crdt_doc_tools`**

In `crdt_doc_tools.rs`, find `build_all_crdt_doc_tools()` and add:

```rust
pub fn build_all_crdt_doc_tools() -> Vec<ToolDefinition> {
    vec![
        tool_list_sheets(),
        tool_read(),
        tool_set_cell(),
        tool_set_range(),
        tool_add_sheet(),
        tool_get_recent_changes(),
        tool_list_my_artifacts(),
        tool_create_artifact(),
        super::tool_run_python(),  // ← NEW
    ]
}
```

- [ ] **Step 4: Wire into dispatcher table**

In `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`, find the existing dispatch for `crdt_doc_*` tools and add:

```rust
TOOL_RUN_PYTHON => dispatch_crdt_doc_run_python(ctx, args).await,
```

with appropriate import.

- [ ] **Step 5: Build + verify**

```bash
cd /Users/danielgarcia/startti/colmena
cargo build --bin dag_engine 2>&1 | tail -20
```

Expected: green build. If errors, fix incrementally.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(crdt_doc_run_python): tool dispatcher with code wrap + writeback (C-T5)"
```

---

## Task 6: Integration test — full agent flow

**Files:**
- Create: `src/libs/colmena/tests/crdt_doc_run_python_test.rs`

- [ ] **Step 1: Write the test**

```rust
//! End-to-end test for `crdt_doc_run_python` tool. Exercises:
//! - Reading a sheet's data as a pandas DataFrame.
//! - Computing aggregations server-side and returning to LLM.
//! - Writing a DataFrame back as a new sheet.
//! - Name collision resolution.

use colmena::crdt_documents::{
    ArtifactId, CrdtDocumentsRuntime,
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_run_python::{execute_run_python, RunPythonArgs},
};
use serde_json::json;
use std::sync::Arc;

async fn make_test_ctx() -> (CrdtDocsContext, ArtifactId, Arc<CrdtDocumentsRuntime>, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("rp_test_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({
        "storage_backend": "localfs",
        "storage_root": tmp.to_str().unwrap(),
    });
    let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid = ArtifactId::new();
    let entry = runtime.registry.get_or_create(&aid, "test");

    // Seed Inventory with sample data.
    let sheet_id = apply_add_sheet(&entry.doc, "Inventory");
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A1", &json!("Region"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B1", &json!("Sales"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A2", &json!("North"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B2", &json!(100));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A3", &json!("North"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B3", &json!(200));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "A4", &json!("South"));
    apply_set_cell_in_proc(&entry.doc, &sheet_id, "B4", &json!(150));

    let ctx = CrdtDocsContext::new_local(runtime.clone(), aid.clone(), Some("test_session".to_string()));
    (ctx, aid, runtime, tmp)
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python — install with .venv/bin/pip install pandas numpy scipy"]
async fn run_python_aggregation_returns_output_to_llm() {
    let (ctx, _aid, _rt, tmp) = make_test_ctx().await;
    let sheet_id = ctx
        .doc()
        .and_then(|doc| {
            colmena::crdt_documents::projection::project(&doc)["sheets"]
                .as_array()
                .and_then(|s| s.iter().find(|sh| sh["name"] == "Inventory").cloned())
        })
        .and_then(|sh| sh["id"].as_str().map(String::from))
        .unwrap();

    let args = RunPythonArgs {
        sheet_ids: vec![sheet_id.clone()],
        code: format!(r#"df = dfs["{sheet_id}"]
totals = df.groupby('Region')['Sales'].sum()
output = totals.to_dict()
"#),
        write_to_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(result["error"].is_null(), "got error: {:?}", result["error"]);
    let totals = result["output"].as_object().expect("output is object");
    assert_eq!(totals["North"], json!(300));
    assert_eq!(totals["South"], json!(150));
    assert!(result["wrote_sheet"].is_null());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn run_python_write_to_sheet_creates_new_sheet() {
    let (ctx, _aid, runtime, tmp) = make_test_ctx().await;
    let sheet_id = ctx
        .doc()
        .and_then(|doc| {
            colmena::crdt_documents::projection::project(&doc)["sheets"]
                .as_array()
                .and_then(|s| s.iter().find(|sh| sh["name"] == "Inventory").cloned())
        })
        .and_then(|sh| sh["id"].as_str().map(String::from))
        .unwrap();

    let args = RunPythonArgs {
        sheet_ids: vec![sheet_id.clone()],
        code: format!(r#"df = dfs["{sheet_id}"]
output_sheet = df.groupby('Region')['Sales'].sum().reset_index()
output = "summary written"
"#),
        write_to_sheet: Some("Summary".to_string()),
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(result["error"].is_null());
    let wrote = &result["wrote_sheet"];
    assert_eq!(wrote["name"], "Summary");
    assert_eq!(wrote["n_rows"], 2);
    assert_eq!(wrote["n_cols"], 2);

    // Verify sheet actually exists in the runtime.
    let entry = runtime.registry.get(ctx.artifact_id()).unwrap();
    let proj = colmena::crdt_documents::projection::project(&entry.doc);
    let sheets = proj["sheets"].as_array().unwrap();
    let summary = sheets.iter().find(|s| s["name"] == "Summary").expect("Summary exists");
    assert_eq!(summary["cells"]["A1"], json!("Region"));
    assert_eq!(summary["cells"]["B1"], json!("Sales"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+numpy in system Python"]
async fn run_python_name_collision_appends_suffix() {
    let (ctx, _aid, runtime, tmp) = make_test_ctx().await;
    // Pre-create a sheet named "Summary".
    let entry = runtime.registry.get(ctx.artifact_id()).unwrap();
    let _ = apply_add_sheet(&entry.doc, "Summary");

    let inv_id = colmena::crdt_documents::projection::project(&entry.doc)["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Inventory")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let args = RunPythonArgs {
        sheet_ids: vec![inv_id.clone()],
        code: format!(r#"df = dfs["{inv_id}"]
output_sheet = df.head(1)
output = "ok"
"#),
        write_to_sheet: Some("Summary".to_string()),
    };
    let result = execute_run_python(&ctx, args).await;
    assert_eq!(result["wrote_sheet"]["name"], "Summary (2)");

    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Run the test**

```bash
cd /Users/danielgarcia/startti/colmena
# Install pandas dependencies if not already.
.venv/bin/pip install pandas numpy scipy 2>&1 | tail -3
# Run only the ignored tests (they require pandas).
cargo test --test crdt_doc_run_python_test -- --ignored 2>&1 | tail -15
```

Expected: 3 tests pass.

If the tests fail because `pandas` not in PyO3's Python: PyO3 uses the same Python that the binary was linked against. To use the `.venv` Python, you may need to set `PYO3_PYTHON=/path/to/.venv/bin/python` and rebuild. Document this in the README.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/crdt_doc_run_python_test.rs
git commit -m "test(crdt_doc_run_python): integration tests for aggregation + writeback + collision (C-T6)"
```

---

## Task 7: Sandbox enforcement integration test

**Files:**
- Create: `src/libs/colmena/tests/crdt_run_python_sandbox_test.rs`

- [ ] **Step 1: Write the test**

```rust
//! Verifies that `crdt_doc_run_python` enforces the sandbox: banned
//! imports/builtins are rejected; allowed ones pass.

use colmena::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_run_python::{execute_run_python, RunPythonArgs},
};
use serde_json::json;
use std::sync::Arc;

async fn make_minimal_ctx() -> (CrdtDocsContext, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("rps_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({ "storage_backend": "localfs", "storage_root": tmp.to_str().unwrap() });
    let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid = ArtifactId::new();
    let entry = runtime.registry.get_or_create(&aid, "test");
    // Add one sheet so sheet_ids isn't empty.
    let sid = colmena::crdt_documents::tool_executor::apply_add_sheet(&entry.doc, "S");
    colmena::crdt_documents::tool_executor::apply_set_cell_in_proc(&entry.doc, &sid, "A1", &json!("x"));
    let ctx = CrdtDocsContext::new_local(runtime, aid, Some("sb_test".to_string()));
    (ctx, tmp)
}

#[tokio::test]
#[ignore = "requires pandas in system Python"]
async fn sandbox_rejects_requests_import() {
    let (ctx, tmp) = make_minimal_ctx().await;
    let sid = colmena::crdt_documents::projection::project(&ctx.doc().unwrap())["sheets"]
        .as_array().unwrap()[0]["id"].as_str().unwrap().to_string();
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: "import requests\noutput = 1".to_string(),
        write_to_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    let err = result["error"].as_str().expect("error string");
    assert!(err.contains("requests") || err.contains("not allowed"), "error: {err}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas in system Python"]
async fn sandbox_rejects_open_call() {
    let (ctx, tmp) = make_minimal_ctx().await;
    let sid = colmena::crdt_documents::projection::project(&ctx.doc().unwrap())["sheets"]
        .as_array().unwrap()[0]["id"].as_str().unwrap().to_string();
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: "f = open('/etc/passwd', 'r')\noutput = f.read()".to_string(),
        write_to_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    let err = result["error"].as_str().expect("error string");
    assert!(err.contains("open") || err.contains("not allowed"), "error: {err}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas in system Python"]
async fn sandbox_allows_numpy_computation() {
    let (ctx, tmp) = make_minimal_ctx().await;
    let sid = colmena::crdt_documents::projection::project(&ctx.doc().unwrap())["sheets"]
        .as_array().unwrap()[0]["id"].as_str().unwrap().to_string();
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: "import numpy as np\noutput = int(np.array([1,2,3]).sum())".to_string(),
        write_to_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(result["error"].is_null(), "unexpected error: {:?}", result["error"]);
    assert_eq!(result["output"], json!(6));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
#[ignore = "requires pandas+scipy in system Python"]
async fn sandbox_allows_scipy_stats() {
    let (ctx, tmp) = make_minimal_ctx().await;
    let sid = colmena::crdt_documents::projection::project(&ctx.doc().unwrap())["sheets"]
        .as_array().unwrap()[0]["id"].as_str().unwrap().to_string();
    let args = RunPythonArgs {
        sheet_ids: vec![sid],
        code: r#"from scipy import stats
result = stats.describe([1,2,3,4,5])
output = {"mean": result.mean, "n": result.nobs}
"#.to_string(),
        write_to_sheet: None,
    };
    let result = execute_run_python(&ctx, args).await;
    assert!(result["error"].is_null());
    assert_eq!(result["output"]["mean"], json!(3.0));
    assert_eq!(result["output"]["n"], json!(5));
    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Run**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --test crdt_run_python_sandbox_test -- --ignored 2>&1 | tail -15
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/crdt_run_python_sandbox_test.rs
git commit -m "test(crdt_doc_run_python): sandbox enforcement integration tests (C-T7)"
```

---

## Task 8: Docs — dev guide + node_configurations + BACKLOG + CHANGELOG

**Files:**
- Modify: `docs/developer_guide/38_crdt_documents.md`
- Modify: `docs/node_configurations.json` (only if pattern enumerates tools)
- Modify: `docs/BACKLOG.md`
- Modify: `docs/CHANGELOG_2026-06.md`

- [ ] **Step 1: Add §5.6 to dev guide**

In `docs/developer_guide/38_crdt_documents.md`, after §5.5 (from B), append:

```markdown
### 5.6 Python/pandas analysis (subsistema C)

Tool: `crdt_doc_run_python(sheet_ids, code, write_to_sheet?)`.

#### Por qué existe

Para Excel grandes (>1000 filas), pasar todo el contenido al LLM en su contexto es prohibitivo en tokens (~125k tokens para un workbook de 10k filas). La pattern: agente lee solo un sample con `crdt_doc_read("A1:Z10")` para entender el schema, después llama `run_python` con código que opera sobre el dataset completo server-side.

Ahorro típico: 10x-1000x en tokens dependiendo del tamaño.

#### Cómo se usa típicamente

```
Turn 1 — exploración (cheap):
   crdt_doc_list_sheets()
   crdt_doc_read(sh_inventory, "A1:Z10")
   
Turn 2 — análisis:
   crdt_doc_run_python(
       sheet_ids=["sh_inventory"],
       code="
           df = dfs['sh_inventory']
           output = df.groupby('Region')['Sales'].sum().to_dict()
       "
   )
   → output = {"North": 450, "South": 320, ...}
   
Turn 3 — persistir resultado en una nueva hoja:
   crdt_doc_run_python(
       sheet_ids=["sh_inventory"],
       code="
           df = dfs['sh_inventory']
           output_sheet = df.groupby('Region').agg({'Sales': 'sum', 'Qty': 'mean'}).reset_index()
       ",
       write_to_sheet="Summary by Region"
   )
   → wrote_sheet = {sheet_id: "sh_summary", name: "Summary by Region", n_rows: 4, preview: [...]}
```

#### Sandbox + librerías

Reusa la infra `restricted` de `python_script` (AST validation + import whitelist + banned builtins). v1 agrega `pandas`, `numpy`, `scipy.stats` a la whitelist. Bloqueados (sin cambio): `open, exec, eval, compile, __import__` + cualquier import fuera de la whitelist (incluye `requests`, `urllib`, `os`, `subprocess`, etc.).

#### Convenciones de I/O

- **Input**: `dfs: dict[sheet_id, pd.DataFrame]` — una DataFrame por sheet pedido. Row 1 del workbook = column names. Headers ausentes/no-string → fallback `col_A`, `col_B`.
- **Output al LLM**: variable `output` (cualquier JSON-serializable). Cap 10KB; trunca con `_output_truncated: true`.
- **Write-back**: variable `output_sheet` (pd.DataFrame). Solo se escribe si `write_to_sheet` está en args. Headers as row 1, sin index, type coercion automático. Name collisions → auto-suffix `" (2)"`, `" (3)"`. Cap 100k rows; trunca con `truncated_at` en response.

#### Límites v1 (hardcoded, deuda técnica)

| Límite | Valor | Path v1.1 |
|---|---|---|
| Combined records load | 100 MB | Configurable via `crdt_documents.run_python_limits.max_load_mb` |
| Code execution timeout | 30s | Idem (`timeout_secs`) |
| `output` to LLM | 10 KB | Idem |
| `stdout` / `error` | 10 KB cada uno | Idem |
| `output_sheet` rows | 100K | Idem + chunked writes para evitar transact_mut gigante |
| Sheet name | 31 chars (Excel xlsx limit) | Stays — hard limit |

Ver `docs/BACKLOG.md` → "Configurable limits for run_python tool".

#### Modo Local vs WsPeer

Mismo comportamiento. En WsPeer mode el worker tiene la réplica Y.Doc local via WS, entonces la construcción del DataFrame es local, sin roundtrip. Las escrituras de `output_sheet` van como mutaciones Y.Doc → propagan al server via WS → fan-out a browsers.

#### Requisito de runtime

Pandas, numpy y scipy deben estar disponibles en el Python embebido por PyO3 del worker. En el `.venv` del proyecto:

```bash
.venv/bin/pip install pandas numpy scipy
```

En producción ADP el worker container debe incluir estas deps. Si no están, los tests `#[ignore]` correspondientes se skipean y el tool retorna error de "module not found" en ejecución.
```

- [ ] **Step 2: Skip node_configurations.json (same logic as B-T16)**

The file documents node config blocks, not individual tool names. No changes needed.

- [ ] **Step 3: Add BACKLOG entry**

In `docs/BACKLOG.md`, after the last v1.1 entry, append:

```markdown
---

## CRDT Documents v1.1 — Configurable limits para `crdt_doc_run_python`

- **Origen:** scope-cut al implementar subsistema C (2026-06-03). Los límites de tamaño/timeout viven hardcoded en `crdt_doc_run_python.rs`.
- **Problema:** workbooks específicos pueden necesitar más memoria (datasets analíticos >100MB) o más tiempo de cómputo (joins complejos, statistical tests caros). El default conservador no acomoda casos legítimos.
- **Workaround actual:** el agente puede dividir el análisis en múltiples calls más chicos. Para casos genuinamente grandes (10M+ rows), no hay path.
- **Fix propuesto:**
  1. Estructurar limits como `RunPythonLimits` struct con defaults match v1.
  2. Cargar desde `crdt_documents.run_python_limits.*` (config del nodo) o env vars (`COLMENA_CRDT_PY_MAX_LOAD_MB`, `COLMENA_CRDT_PY_TIMEOUT_SECS`, etc).
  3. Mantener ceiling absoluto hardcoded para prevenir abuse (ej. nunca permitir >1GB load aunque config diga).
  4. Telemetry: counter por tipo de cap-hit.
  5. Para `output_sheet` > 100K rows: chunked transact_mut (escribir en lotes de 10K para no bloquear el CRDT subscription).
- **Acceptance criteria:**
  - Operator puede subir el cap de 100MB → 500MB vía env var.
  - Cap absoluto (1GB) sigue activo aunque config pida más.
  - Métrica de cap-hits visible en logs/metrics.
- **Estimación:** ~1 día dev + tests.
- **Cuándo retomar:** cuando observemos usuarios chocando caps regularmente, o un cliente concreto pida specifically.
```

- [ ] **Step 4: Add CHANGELOG entry**

In `docs/CHANGELOG_2026-06.md`, append a new section "## 2. ...":

```markdown
---

## 2. CRDT Documents — Pandas/Python integration (subsistema C)

**Qué cambió.** Nuevo tool `crdt_doc_run_python(sheet_ids, code, write_to_sheet?)` que ejecuta código Python sandboxed contra workbook data. El agente envía código que usa pandas/numpy/scipy.stats; el runtime carga las sheets pedidas como DataFrames server-side, ejecuta el código, y devuelve `output` (cualquier JSON) al LLM. Si `write_to_sheet` está set, opcionalmente persiste `output_sheet` (un DataFrame) como una nueva sheet en el workbook con auto-suffix de name collision.

**Por qué importa.** Para Excel grandes (>1000 filas), pasar todo al LLM en context es prohibitivo en tokens (~125k tokens para 10k filas). Esta pattern (read sample → generate code → execute server-side) ahorra 10x-1000x tokens. Es el approach standard para data analysis con LLMs (OpenAI Code Interpreter, LangChain pandas agent, etc.).

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md`](superpowers/specs/2026-06-03-crdt-pandas-integration-design.md)
- Plan: [`docs/superpowers/plans/2026-06-03-crdt-pandas-integration.md`](superpowers/plans/2026-06-03-crdt-pandas-integration.md)
- Dev guide §5.6: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Item v1.1 deferido: [`docs/BACKLOG.md`](BACKLOG.md) — "Configurable limits para `crdt_doc_run_python`".

**Commits (C-T1 a C-T9).** Ver `git log feature/docs --grep="C-T"`.

**Estado.** done.

**Requisitos de runtime.** El Python embebido por PyO3 del worker debe tener `pandas`, `numpy`, `scipy` instalados. Local dev: `.venv/bin/pip install pandas numpy scipy`. Producción ADP: incluir en el container del worker.

**Limitaciones conocidas v1.**
- Límites hardcoded (100MB load, 30s timeout, 10KB output, 100K rows write). Mejora: BACKLOG.
- Write-back solo a nueva sheet (no overwrite/append a sheet existente). Mejora: v1.1 cuando UX feedback lo amerite.
- No multi-artifact en un solo call (cross-workbook joins son subsistema F).
```

- [ ] **Step 5: Commit (4 separate commits per task or one bundled — doc commits don't need TDD)**

```bash
git add docs/developer_guide/38_crdt_documents.md
git commit -m "docs(crdt_documents): document subsystem C (pandas integration) — §5.6 (C-T8a)"

git add docs/BACKLOG.md
git commit -m "docs(backlog): defer configurable limits for run_python tool (C-T8b)"

git add docs/CHANGELOG_2026-06.md
git commit -m "docs(changelog): June 2026 — add subsystem C entry (C-T8c)"
```

---

## Task 9: Final sweep — cargo test + clippy + fmt + browser smoke

- [ ] **Step 1: Full test suite**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib -p colmena_dag_engine 2>&1 | tail -10
cargo test -p colmena_dag_engine --test crdt_doc_run_python_test --test crdt_run_python_sandbox_test -- --ignored 2>&1 | tail -10
```

Expected: lib tests pass (1200+); ignored integration tests pass when pandas is installed.

- [ ] **Step 2: Clippy clean**

```bash
cargo clippy --tests --lib -p colmena_dag_engine 2>&1 | grep -E "warning|error" | head -10
```

Expected: empty.

- [ ] **Step 3: cargo fmt**

```bash
cargo fmt --check 2>&1 | head -20
```

If diff, run `cargo fmt` and add a final commit.

- [ ] **Step 4: Manual browser smoke (optional but recommended)**

```bash
pkill -f "dag_engine" 2>/dev/null; sleep 1
DUMP=/tmp/crdt_c_smoke
rm -rf $DUMP && mkdir -p $DUMP

# Terminal A: server
DATABASE_URL="sqlite://$DUMP/events.sqlite3?mode=rwc" \
  cargo run --bin dag_engine -- crdt-yws --host 127.0.0.1 --port 8090 --dump-dir $DUMP &

# Wait for server
sleep 3

# Create artifact + import a sample xlsx
ID=$(curl -s -X POST http://127.0.0.1:8090/documents \
  -H 'content-type: application/json' \
  -d '{"name":"C Smoke","agent_session_id":"agent_c_smoke"}' | jq -r .artifact_id)
echo "ID=$ID"
curl -X POST "http://127.0.0.1:8090/documents/$ID/import" --data-binary @spike/fixtures/test.xlsx
open "http://127.0.0.1:8090/?artifact=$ID"

# Pin a smoke graph similar to b_recent_changes but using run_python:
# (write your own test graph for the smoke or adapt b_recent_changes_turn1.json
# replacing the prompt with something like:
#   "Group the Sales by Region using crdt_doc_run_python and write a Summary sheet.")
```

Verify: browser shows the new "Summary" sheet appear with aggregated data.

- [ ] **Step 5: Final commit if anything was touched**

```bash
git add -A
git commit -m "chore(crdt_documents): subsistema C final sweep — clippy + fmt + smoke (C-T9)" \
  || echo "no changes"
```

---

## Self-review checklist (run before handoff)

- [ ] **Spec coverage**: every section of `2026-06-03-crdt-pandas-integration-design.md` mapped to a task above (§4 architecture → T3/T4/T5; §4.5 DF construction → T3; §4.6 writer → T4; §4.7 collision → T4; §4.8 sandbox → T1; §4.9 helper refactor → T2; §6 limits → T4/T5; §7 testing → T6/T7).
- [ ] **Placeholder scan**: no "TBD", no "TODO", every step has concrete code or commands.
- [ ] **Type consistency**: `SheetRecords`, `WriteResult`, `RunPythonArgs`, `PythonRunResult` named identically wherever they appear.
- [ ] **`execute_sandboxed_helper` signature** (`code, sandbox_mode, timeout_secs, inputs`) matches in T2 (definition) and T5 (call site).
- [ ] **Tool name constant** `TOOL_RUN_PYTHON = "crdt_doc_run_python"` consistent.
- [ ] **Migration files end in `.sql`**: N/A (no migrations in C).
- [ ] **`#[ignore]` annotation on integration tests** for pandas-requiring tests: consistent (T6, T7).

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-03-crdt-pandas-integration.md`. Two execution options:

**1. Subagent-Driven (recommended)** - Fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** - Execute tasks in this session using executing-plans.

Which approach?
