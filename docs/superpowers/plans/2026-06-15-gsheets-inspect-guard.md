# gsheets inspect-before-python guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Force an agent to see a sheet's real structure before its `gsheets_run_python` code executes — intercept the first blind run_python on an unread sheet, return a bounded markdown preview, and make the agent re-call with informed code.

**Architecture:** Per-turn read-state lives on `DagToolExecutor` (a `Mutex<HashSet<String>>` keyed `"spreadsheet_id::sheet"`); the executor is built once per `llm_call` execution so the set is naturally per-turn. `gsheets_read` marks a sheet seen; `gsheets_run_python` checks its sheet bindings and, if any are unseen, short-circuits with an `inspect_first` envelope (bounded markdown preview, no code run) instead of dispatching. Pure decision/rendering helpers live in a new focused module; the executor wiring is thin glue verified by an E2E.

**Tech Stack:** Rust (crate `colmena_dag_engine`), `serde_json`, Google Sheets API v4. Spec: [`docs/superpowers/specs/2026-06-15-gsheets-inspect-guard-design.md`](../specs/2026-06-15-gsheets-inspect-guard-design.md). Builds on the text fix in [`2026-06-14-gsheets-inspect-before-python-design.md`](../specs/2026-06-14-gsheets-inspect-before-python-design.md) (merged, PR #98).

**Build/test commands:**
- Module unit tests: `cargo test --lib -p colmena_dag_engine <module>`
- Full pre-push (CI command): `cargo test --verbose` — capture cargo's real exit code, NOT through a `| tail` pipe (zsh has no pipefail; the pipe masks failures). Run as `cargo test --verbose > /tmp/full.log 2>&1; echo $?`.
- Format/lint: `cargo fmt` and `cargo clippy -p colmena_dag_engine` — deny-warnings is ON (no unused imports / dead code / useless allow).
- E2E graph: `./target/release/dag_engine run <graph.json>` after `cargo build --release --bin dag_engine` (LLM-facing text + behavior are compiled in; rebuild release before E2E).

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_inspect_guard.rs` | Pure helpers: `sheet_key`, `SheetBindingRef`, `unseen_sheet_bindings`, `truncate_markdown_preview`, `columns_from_markdown_header`. No I/O. | **Create** |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Wire `pub mod gsheets_inspect_guard;` | **Modify** |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Add per-turn `gsheets_seen_sheets` field; mark on `gsheets_read`; intercept on `gsheets_run_python` via a new `gsheets_run_python_guarded` method. | **Modify** |
| `tests/graphs/agents/gsheets_inspect_guard_e2e.json` | E2E: vague "+10 a las frutas" prompt with flash → guard forces a read → correct result. | **Create** |
| `docs/CHANGELOG_2026-06.md` | §37 entry. | **Modify** |

`gsheets_run_python.rs` and `gsheets_tools.rs` are NOT modified — the guard sits in the executor in front of `dispatch_gsheets_run_python`, and reuses `dispatch_gsheets_read` (already imported in the executor) for previews.

---

## Task 1: Pure guard helpers (`gsheets_inspect_guard.rs`)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_inspect_guard.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Create the module with its failing tests**

Create `gsheets_inspect_guard.rs`:

```rust
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
pub fn sheet_key(spreadsheet_id: &str, sheet: &str) -> String {
    format!("{spreadsheet_id}::{sheet}")
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
    fn sheet_key_joins_id_and_sheet() {
        assert_eq!(sheet_key("abc", "Ventas"), "abc::Ventas");
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
        let got = unseen_sheet_bindings(&args, &seen_with(&["abc::Ventas"]));
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
        let got = unseen_sheet_bindings(&args, &seen_with(&["ss::Seen"]));
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
        let md = "| Categoria | Producto | Monto |\n| --- | --- | --- |\n| Frutas | Manzana | 100 |";
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
```

- [ ] **Step 2: Wire the module**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, add alongside the other `pub mod` lines (e.g. after `pub mod gsheets_tools;` near line 15):

```rust
pub mod gsheets_inspect_guard;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib -p colmena_dag_engine gsheets_inspect_guard`
Expected: 10 tests pass.

- [ ] **Step 4: Lint + format**

Run: `cargo fmt && cargo clippy -p colmena_dag_engine 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_inspect_guard.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "feat(gsheets): pure helpers for inspect-before-python guard"
```

---

## Task 2: Wire the guard into `DagToolExecutor`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Add the per-turn seen-set field**

In the `DagToolExecutor` struct (ends at line ~128, after `max_tool_result_bytes: usize,`), add:

```rust
    /// Per-turn set of sheets the agent has already read, keyed
    /// `"spreadsheet_id::sheet"`. Populated when `gsheets_read` succeeds (and
    /// when the inspect guard surfaces a preview). Checked before
    /// `gsheets_run_python` executes: any bound sheet not in here triggers the
    /// inspect-first interception. The executor is built once per `llm_call`
    /// execution, so this set is naturally per-turn (no cross-turn persistence —
    /// consistent with the no-cache stance of expand-merges).
    gsheets_seen_sheets: std::sync::Mutex<std::collections::HashSet<String>>,
```

In `DagToolExecutor::new` (the `Self { ... }` literal at lines 188-205), add the field initializer (e.g. after `max_tool_result_bytes: DEFAULT_MAX_TOOL_RESULT_STRING_BYTES,`):

```rust
            gsheets_seen_sheets: std::sync::Mutex::new(std::collections::HashSet::new()),
```

- [ ] **Step 2: Add the guard + mark helper methods**

Add to an `impl DagToolExecutor` block (e.g. the one starting at line 136). Note the `use` for the pure helpers at the top of the methods:

```rust
    /// Mark a sheet as read this turn (idempotent).
    fn mark_gsheets_sheet_seen(&self, spreadsheet_id: &str, sheet: &str) {
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_inspect_guard::sheet_key;
        self.gsheets_seen_sheets
            .lock()
            .unwrap()
            .insert(sheet_key(spreadsheet_id, sheet));
    }

    /// Inspect-before-python guard. If every sheet binding in `args` was already
    /// read this turn (or there are none), dispatch `gsheets_run_python`
    /// normally. Otherwise short-circuit: read a bounded markdown preview of each
    /// unread sheet, mark it seen, and return an `inspect_first` envelope WITHOUT
    /// running the code, forcing the agent to re-call with informed code.
    async fn gsheets_run_python_guarded(&self, args: serde_json::Value) -> serde_json::Value {
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::dispatch_gsheets_read;
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::dispatch_gsheets_run_python;
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_inspect_guard::{
            columns_from_markdown_header, truncate_markdown_preview, unseen_sheet_bindings,
        };

        let unseen = {
            let seen = self.gsheets_seen_sheets.lock().unwrap();
            unseen_sheet_bindings(&args, &seen)
        };
        if unseen.is_empty() {
            return dispatch_gsheets_run_python(args).await;
        }

        let mut inspected = serde_json::Map::new();
        for b in &unseen {
            let read_args = serde_json::json!({
                "spreadsheet_id": b.spreadsheet_id,
                "sheet": b.sheet,
                "range": b.range.clone().unwrap_or_else(|| "1:6".to_string()),
                "format": "markdown",
            });
            let read_res = dispatch_gsheets_read(read_args).await;
            // If the preview read itself errored (missing sheet / permission),
            // surface that — run_python would have failed too.
            if matches!(&read_res, serde_json::Value::Object(m) if m.contains_key("error")) {
                return read_res;
            }
            let md_full = read_res
                .get("markdown")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let preview = truncate_markdown_preview(md_full, 5);
            let columns = columns_from_markdown_header(&preview);
            inspected.insert(
                b.var.clone(),
                serde_json::json!({
                    "spreadsheet_id": b.spreadsheet_id,
                    "sheet": b.sheet,
                    "columns": columns,
                    "preview_markdown": preview,
                }),
            );
            self.mark_gsheets_sheet_seen(&b.spreadsheet_id, &b.sheet);
        }

        serde_json::json!({
            "status": "inspect_first",
            "inspected_sheets": inspected,
            "advice": "Antes de correr código sobre una hoja hay que conocer sus columnas reales. Acá está el preview (primeras filas) de cada hoja. Volvé a llamar gsheets_run_python con el MISMO código, corregido si hace falta para usar estas columnas/valores reales (p.ej. filtrar por la columna correcta, no adivinar nombres).",
            "next_action": "re-call gsheets_run_python"
        })
    }
```

- [ ] **Step 3: Mark on `gsheets_read`, intercept on `gsheets_run_python`**

In the gsheets dispatch match (lines ~1085-1116), replace the two arms.

Replace:
```rust
                    n if n == GSHEETS_READ_TOOL => dispatch_gsheets_read(args).await,
```
with:
```rust
                    n if n == GSHEETS_READ_TOOL => {
                        let ss = args
                            .get("spreadsheet_id")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let sheet = args
                            .get("sheet")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let r = dispatch_gsheets_read(args).await;
                        let is_err = matches!(&r, serde_json::Value::Object(m) if m.contains_key("error"));
                        if !is_err {
                            if let (Some(ss), Some(sheet)) = (ss, sheet) {
                                self.mark_gsheets_sheet_seen(&ss, &sheet);
                            }
                        }
                        r
                    }
```

Replace:
```rust
                    n if n == TOOL_GSHEETS_RUN_PYTHON => dispatch_gsheets_run_python(args).await,
```
with:
```rust
                    n if n == TOOL_GSHEETS_RUN_PYTHON => {
                        self.gsheets_run_python_guarded(args).await
                    }
```

Note: after this change, `dispatch_gsheets_run_python` is no longer called directly in this match arm (it's called inside `gsheets_run_python_guarded`), but it is still imported in this block's `use` (line 1037). If clippy flags the import as unused here, remove `dispatch_gsheets_run_python` from the line-1034 `use` block (it stays imported inside the guard method). Verify with clippy.

- [ ] **Step 4: Build + existing tests + lint**

Run: `cargo build -p colmena_dag_engine 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test --lib -p colmena_dag_engine dag_tool_executor 2>&1 | tail -15`
Expected: existing executor tests still pass.

Run: `cargo fmt && cargo clippy -p colmena_dag_engine 2>&1 | tail -8`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(gsheets): intercept blind gsheets_run_python — read sheet first (inspect guard)"
```

---

## Task 3: E2E real verification + ADP sweep + changelog

**This task is mandatory.** The guard's whole point is making a weak model safe; only a live run against real Google proves it. Per repo rule, the feature is not done until verified E2E.

**Files:**
- Create: `tests/graphs/agents/gsheets_inspect_guard_e2e.json`
- Modify: `docs/CHANGELOG_2026-06.md`

**Prerequisites (gsheets E2E runbook — from memory `ref_local_gsheets_e2e_runbook`):**
- OAuth creds from GCP Secret Manager (`startti-dev`), injected in-memory only — never to disk/commit/print. Vars: `COLMENA_GOOGLE_OAUTH_CLIENT_ID/_CLIENT_SECRET/_REFRESH_TOKEN`, `COLMENA_GOOGLE_SHARE_EMAIL`.
- `gcloud auth login` may be needed first (interactive; ask the user if non-interactive access fails).
- `gemini` key from repo `.env`. Embedded python needs `PYTHONPATH=$PWD/.venv/lib/python3.14/site-packages` for pandas.
- A sheet shared (Editor) with the agent account, with a `Ventas` tab: column A `Categoria` merged vertically (Frutas over 3 rows, Verduras over 2), columns `Producto`, `Monto` (100/200/50/30/70). (Reuse the one from the inspect-before-python verification if still present.)

- [ ] **Step 1: Rebuild the release binary (compiled-in behavior)**

Run: `cargo build --release --bin dag_engine 2>&1 | tail -3`
Expected: exit 0.

- [ ] **Step 2: Create the E2E graph**

Create `tests/graphs/agents/gsheets_inspect_guard_e2e.json` (placeholder ID per owner rule — never a real ID in the repo):

```json
{
  "comment": "E2E: the inspect-before-python guard makes a weak model safe. With a VAGUE prompt ('subí 10 al monto de todas las frutas'), flash used to go straight to gsheets_run_python and guess the semantics (0 rows). With the guard, the first run_python is intercepted with a markdown preview of 'Ventas'; the agent re-calls with code using the real Categoria column and applies +10 to the 3 frutas (Manzana/Banana/Pera). Replace <SPREADSHEET_ID> per the gsheets E2E runbook.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/inspect_guard", "method": "POST", "test_payload": {
        "prompt": "Use this exact spreadsheet_id (copy character-by-character): <SPREADSHEET_ID>\nEn la hoja 'Ventas' de esa planilla, subí 10 al monto de todas las frutas y guardá el cambio en la misma hoja. Avisame qué cambiaste."
      }}
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "temperature": 0,
        "system_message": "Sos un asistente que trabaja con Google Sheets. Copiá el spreadsheet_id verbatim. No inventes datos.",
        "enabled_tools": ["gsheets"]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [ { "from": "trigger", "to": "agent" }, { "from": "agent", "to": "log" } ]
}
```

- [ ] **Step 3: Run and verify against real Google**

Set up the runbook env (Secret Manager creds in-memory, `PYTHONPATH`, `.env`). Substitute the real ID into a temp copy and run:

```bash
mkdir -p /tmp/colmena_e2e
SID=<real id>
sed "s|<SPREADSHEET_ID>|$SID|g" tests/graphs/agents/gsheets_inspect_guard_e2e.json > /tmp/colmena_e2e/guard.json
./target/release/dag_engine run /tmp/colmena_e2e/guard.json --agent-session-id e2e_guard_$(date +%s) --include-extra-info > /tmp/colmena_e2e/guard.sse 2>&1
```

Verify, in order:
1. **Tool sequence** shows `gsheets_run_python` intercepted then re-called — i.e. at least two `gsheets_run_python` entries (first returns `inspect_first`, second executes), OR a `gsheets_read` does not even need to appear because the guard supplies the preview. Grep the SSE for `inspect_first` to confirm the intercept fired.
2. **The sheet, read directly from the Sheets API**, shows Monto = 110/210/60 for Manzana/Banana/Pera and 30/70 unchanged for Lechuga/Tomate. This is the source of truth — do NOT trust the agent's words.

Expected: VEREDICTO CORRECTO — the guard fired (`inspect_first` present) AND the fruits got +10. Save the SSE to `/tmp/colmena_e2e/` and present a friendly report (don't paste the whole SSE).

> If flash STILL gets it wrong after the intercept (e.g. re-calls but uses the wrong column), that's the honest ceiling: report it. The guard guarantees the agent SEES the table, not that it uses it correctly. But the live run from the text-fix verification showed reading-first leads flash much closer; confirm empirically.

- [ ] **Step 4: ADP sweep**

Confirm no ADP breakage — the change is internal to the executor; the `inspect_first` envelope is a new tool result the agent consumes in-loop and does not cross the SSE boundary in a way ADP parses.

```bash
cd /Users/danielgarcia/startti/adp && grep -rnE "inspect_first|gsheets_run_python" apps/service/ia/platform/ 2>/dev/null | head
```
Expected: no matches that would break (ADP does not special-case run_python results). Note in the PR.

- [ ] **Step 5: Changelog**

Add a `## 37.` section to `docs/CHANGELOG_2026-06.md` summarizing: structural guard intercepts the first blind `gsheets_run_python` on an unread sheet, returns a bounded markdown preview (`inspect_first` envelope), per-turn seen-set on `DagToolExecutor`, reuses `dispatch_gsheets_read`; E2E result; ADP-clean. Reference the spec.

- [ ] **Step 6: Commit**

```bash
git add tests/graphs/agents/gsheets_inspect_guard_e2e.json docs/CHANGELOG_2026-06.md
git commit -m "test(gsheets): E2E for inspect-before-python guard + changelog §37"
```

---

## Final verification (after all tasks)

- [ ] **Full CI-equivalent run:** `cargo test --verbose > /tmp/full.log 2>&1; echo "EXIT=$?"` — confirm `EXIT=0` and grep `/tmp/full.log` for `0 failed` / no `FAILED`. (Do not read the exit code through a `| tail` pipe.)
- [ ] **Lint clean:** `cargo clippy -p colmena_dag_engine 2>&1 | tail -5`.
- [ ] Hand off to `superpowers:finishing-a-development-branch` — push + PR only after the E2E in Task 3 passes (repo rule: push/PR gated on verified E2E success).

---

## Self-Review notes (author)

- **Spec coverage:** scope per-turn → Task 2 field (built per `llm_call`, confirmed at llm.rs:1991); bounded markdown preview → `truncate_markdown_preview` (Task 1) + `range or "1:6"` read (Task 2); `inspect_first` envelope no-error → Task 2 method; lives in `DagToolExecutor` → Task 2; key `spreadsheet_id::sheet` → `sheet_key` (Task 1); edges (inline skip, mixed, range, read-error surface) → `unseen_sheet_bindings` tests + the error-surface branch in Task 2; testing (unit + E2E) → Tasks 1 & 3; ADP → Task 3 Step 4. All covered.
- **Type consistency:** `SheetBindingRef` fields and the helper signatures are identical across Tasks 1 and 2. The envelope keys (`status`, `inspected_sheets`, `advice`, `next_action`) match the spec.
- **No-loop invariant:** the intercept marks each previewed sheet seen before returning, so the agent's re-call finds them seen and executes — verified by the `mark_gsheets_sheet_seen` call inside the preview loop (Task 2).
