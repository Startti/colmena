# Pandas Multi-Sheet Write-Back + Table Exploration Skills Implementation Plan (Beta)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two paired deliverables: (1) extend `gsheets_run_python` and `crdt_doc_run_python` so the agent's pandas script can declare `output_sheets = {<name>: DataFrame, ...}` and the dispatcher writes N new tabs directly — the LLM only sees metadata; (2) ship two new skills (`gsheets-table-exploration`, `crdt-doc-table-exploration`) that teach inspect-first, top-N via `nlargest`, type coercion, and the `output_sheets` contract.

**Architecture:**
- Both `run_python` tools already wrap user code with a prelude/postlude. The postlude gains a section that extracts an `output_sheets` global (dict from name to DataFrame) and surfaces it as `__col_output_sheets` in the final `output` JSON.
- The `gsheets_run_python` dispatcher gains an optional `write_to_spreadsheet: Option<String>` arg. When set AND the script's `output_sheets` is non-empty, the dispatcher iterates entries and calls `SheetsClient::add_sheet` + `SheetsClient::set_range` for each. Collisions auto-suffix `" (2)"`, `" (3)"`, capped at 10 attempts.
- The `crdt_doc_run_python` dispatcher gains the same `output_sheets` path; the existing `output_sheet` + `write_to_sheet` single-sheet code stays as a back-compat alias.
- Two new skill folders under `src/libs/colmena/skills/` mirror the existing cross-sheet-analysis skills but for single-table exploration.

**Tech Stack:** Rust 1.95, existing `SheetsClient` trait, `crdt_documents::write_records_as_new_sheet` (subsystem F), pandas (already in the sandbox whitelist), `include_str!`, `wiremock` for tests.

**Spec:** [docs/superpowers/specs/2026-06-06-pandas-multisheet-and-exploration-design.md](../specs/2026-06-06-pandas-multisheet-and-exploration-design.md)

---

## File Structure

**New files:**
- `src/libs/colmena/skills/gsheets-table-exploration/SKILL.md`
- `src/libs/colmena/skills/gsheets-table-exploration/references/01-inspect-schema-first.md`
- `src/libs/colmena/skills/gsheets-table-exploration/references/02-top-n-patterns.md`
- `src/libs/colmena/skills/gsheets-table-exploration/references/03-filter-and-query.md`
- `src/libs/colmena/skills/gsheets-table-exploration/references/04-group-and-aggregate.md`
- `src/libs/colmena/skills/gsheets-table-exploration/references/05-type-coercion.md`
- `src/libs/colmena/skills/gsheets-table-exploration/references/06-output-shaping.md`
- (Same tree under `src/libs/colmena/skills/crdt-doc-table-exploration/`, 7 files total)

**Modified files:**
- `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md`
- `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` — new arg, multi-sheet write loop
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs` — multi-sheet write loop alongside legacy single-sheet path
- `src/libs/colmena/text/tools/gsheets.yaml` — `gsheets_run_python` description mentions `output_sheets` and the new skill
- `src/libs/colmena/text/tools/crdt_doc.yaml` — same for `crdt_doc_run_python`
- `docs/developer_guide/42_builtin_skills_index.md` — append the 2 new skills (assumes Alpha shipped first)
- `docs/developer_guide/CHANGELOG_2026-06.md` (path is `docs/CHANGELOG_2026-06.md`)
- `docs/BACKLOG.md`

---

## Task Dependency Graph

```
Task 1 (postludes — both)
  ↓
Task 2 (gsheets dispatcher) ─┐
                              ├→ Task 4 (gsheets wiremock tests)
Task 3 (crdt_doc dispatcher)─┴→ Task 5 (crdt_doc integration test)
  ↓
Task 6 (tool YAML descriptions update)
  ↓
Task 7 (gsheets-table-exploration skill)
  ↓
Task 8 (crdt-doc-table-exploration skill — mirror of 7)
  ↓
Task 9 (docs sweep)
```

Task 1 must land before Tasks 2-3 (dispatcher reads the postlude's new field).

---

## Task 1 (E-T20a): Postlude updates — extract `output_sheets`

**Goal:** Both Python sandbox postludes gain a section that extracts `output_sheets` (dict from name to DataFrame) into `__col_output_sheets`, and the final `output` dict carries it.

**Files:**
- Modify: `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md`
- Modify: `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md`

- [ ] **Step 1: Read current postludes**

```bash
cd /Users/danielgarcia/startti/colmena
cat src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md
echo "---"
cat src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md
```

The CRDT postlude already has `__col_user_output` / `__col_sheet_records` / `__col_sheet_cols`. The gsheets postlude is minimal today.

- [ ] **Step 2: Rewrite `gsheets_run_python_postlude.md`**

Path: `src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md`

```

# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None

__col_output_sheets = None
if 'output_sheets' in dir() and output_sheets is not None:
    import pandas as _pd
    if isinstance(output_sheets, dict):
        __col_output_sheets = {}
        for k, v in output_sheets.items():
            if isinstance(v, _pd.DataFrame):
                __col_output_sheets[str(k)] = {
                    'records': v.to_dict('records'),
                    'cols': list(v.columns),
                }
            # Non-DataFrame entries are silently skipped; the dispatcher
            # surfaces a warning if everything ended up skipped.

output = {
    'user_output': __col_user_output,
    'output_sheets': __col_output_sheets,
}
```

The leading blank line is intentional — the existing prelude ends without a trailing newline, so the postlude starts with a blank line to separate.

- [ ] **Step 3: Extend `crdt_doc_run_python_postlude.md`**

Path: `src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md`

```

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

__col_output_sheets = None
if 'output_sheets' in dir() and output_sheets is not None:
    import pandas as _pd
    if isinstance(output_sheets, dict):
        __col_output_sheets = {}
        for k, v in output_sheets.items():
            if isinstance(v, _pd.DataFrame):
                __col_output_sheets[str(k)] = {
                    'records': v.to_dict('records'),
                    'cols': list(v.columns),
                }

output = {
    'user_output': __col_user_output,
    'sheet_records': __col_sheet_records,
    'sheet_cols': __col_sheet_cols,
    'output_sheets': __col_output_sheets,
}
```

- [ ] **Step 4: Build + tests (no dispatcher changes yet, so no behavior change)**

```bash
cd /Users/danielgarcia/startti/colmena
cargo build --lib 2>&1 | tail -3
cargo test --lib --quiet 2>&1 | tail -5
```

Expected: everything still passes. The dispatchers ignore the new `output_sheets` field today, so no regression.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/text/prompts/python_sandbox/gsheets_run_python_postlude.md \
        src/libs/colmena/text/prompts/python_sandbox/crdt_doc_run_python_postlude.md
git commit -m "feat(text-centralization): postludes extract output_sheets dict

E-T20a. Both Python sandbox postludes now extract an 'output_sheets'
global (dict from name to DataFrame) into the final output JSON as
__col_output_sheets. Each entry serializes as {records, cols}. The
existing __col_user_output and CRDT's single-sheet path are unchanged.

Dispatchers ignore the new field for now — Tasks 2 and 3 wire it up.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2 (E-T20b): `gsheets_run_python` dispatcher — multi-sheet write-back

**Goal:** Extend `GsheetsRunPythonArgs` with an optional `write_to_spreadsheet` field. When set AND `output_sheets` is non-empty, iterate the dict and call `add_sheet` + `set_range` per entry. Auto-suffix on name collision.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`

- [ ] **Step 1: Read the current dispatcher**

```bash
cd /Users/danielgarcia/startti/colmena
sed -n '40,140p' src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
```

Find `GsheetsRunPythonArgs` (around line 45) and the `dispatch_gsheets_run_python_with_client` function (around line 98).

- [ ] **Step 2: Add `write_to_spreadsheet` to the args struct**

In `GsheetsRunPythonArgs`, append a new field. Final shape:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GsheetsRunPythonArgs {
    pub bindings: Vec<GsheetsBinding>,
    pub code: String,

    /// Optional target spreadsheet for `output_sheets` returned by the script.
    /// When set AND the script assigns `output_sheets = {name: DataFrame, ...}`,
    /// the dispatcher creates one new tab per entry and writes the DataFrame
    /// as a values matrix (header row + body). If the name already exists,
    /// the dispatcher appends " (2)", " (3)", etc. (capped at 10 attempts).
    /// When omitted, any `output_sheets` produced by the script is ignored.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,
}
```

- [ ] **Step 3: Extract the multi-sheet write logic into a helper**

After the dispatcher function, add:

```rust
/// Write each `(name, DataFrame)` entry from `output_sheets` as a new tab
/// in the target spreadsheet. Returns one `WroteSheet` metadata struct per
/// entry. On name collision, retries with " (2)", " (3)", capped at 10.
async fn write_output_sheets(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    output_sheets: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(map) = output_sheets.as_object() else {
        return Vec::new();
    };
    let mut results: Vec<serde_json::Value> = Vec::new();
    for (raw_name, entry) in map {
        let (Some(records), Some(cols)) = (
            entry.get("records").and_then(|v| v.as_array()),
            entry.get("cols").and_then(|v| v.as_array()),
        ) else {
            results.push(serde_json::json!({
                "name": raw_name,
                "error": "entry missing records or cols",
            }));
            continue;
        };

        // Resolve a non-conflicting name. Try `raw_name`, then ` (2)` ... ` (10)`.
        let mut resolved_name = raw_name.clone();
        let mut sheet_meta = None;
        for attempt in 1..=10 {
            let candidate = if attempt == 1 {
                raw_name.clone()
            } else {
                format!("{raw_name} ({attempt})")
            };
            match client.add_sheet(spreadsheet_id, &candidate).await {
                Ok(meta) => {
                    resolved_name = candidate;
                    sheet_meta = Some(meta);
                    break;
                }
                Err(SheetsError::Http(status, body)) if status.as_u16() == 400
                    && body.to_lowercase().contains("already exists") =>
                {
                    // try next suffix
                    continue;
                }
                Err(e) => {
                    results.push(serde_json::json!({
                        "name": raw_name,
                        "error": format!("add_sheet failed: {e}"),
                    }));
                    sheet_meta = None;
                    break;
                }
            }
        }
        let Some(meta) = sheet_meta else { continue };

        // Build the matrix: header row + body rows.
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
                    match obj.get(key) {
                        Some(serde_json::Value::Null) | None => CellValue::Empty,
                        Some(serde_json::Value::Bool(b)) => CellValue::Boolean(*b),
                        Some(serde_json::Value::Number(n)) => n
                            .as_f64()
                            .map(CellValue::Number)
                            .unwrap_or(CellValue::Empty),
                        Some(serde_json::Value::String(s)) => {
                            CellValue::String(s.clone())
                        }
                        Some(other) => CellValue::String(other.to_string()),
                    }
                })
                .collect();
            matrix.push(row);
        }
        let n_rows = matrix.len();
        let n_cols = cols.len();

        match client
            .set_range(spreadsheet_id, &resolved_name, "A1", matrix)
            .await
        {
            Ok(_) => {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "resolved_name": resolved_name,
                    "sheet_id": meta.sheet_id.0,
                    "n_rows": n_rows,
                    "n_cols": n_cols,
                }));
            }
            Err(e) => {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "resolved_name": resolved_name,
                    "error": format!("set_range failed: {e}"),
                }));
            }
        }
    }
    results
}
```

The exact types used (`Arc<dyn SheetsClient>`, `SpreadsheetId`, `CellValue`, `SheetsError::Http`) come from `crate::gsheets::{domain, infrastructure}`. The implementer should add the necessary `use` imports at the top of the file based on what already exists for the other dispatchers.

- [ ] **Step 4: Wire the helper into the dispatcher**

In `dispatch_gsheets_run_python_with_client` (around line 98), after the `wrapped_output` is parsed:

```rust
// AFTER the postlude result is available as `wrapped_output: serde_json::Value`
// (the parsed result of executing the user code) — typically obtained by
// reading `wrapped_output.get("output_sheets")` after deserialization:

let user_output = wrapped_output
    .get("user_output")
    .cloned()
    .unwrap_or(serde_json::Value::Null);

let mut wrote_sheets_response = serde_json::Value::Null;
let output_sheets = wrapped_output.get("output_sheets");
if let (Some(spreadsheet_id_str), Some(sheets_value)) = (
    parsed.write_to_spreadsheet.as_deref(),
    output_sheets,
) {
    if !sheets_value.is_null() {
        let spreadsheet_id = SpreadsheetId(spreadsheet_id_str.to_string());
        let results = write_output_sheets(&client, &spreadsheet_id, sheets_value).await;
        wrote_sheets_response = serde_json::Value::Array(results);
    }
}

// Also surface helpful warnings:
let warning = match (
    parsed.write_to_spreadsheet.as_deref(),
    output_sheets.and_then(|v| if v.is_null() { None } else { Some(v) }),
) {
    (Some(_), None) => Some(
        "write_to_spreadsheet was set but the script did not assign \
         `output_sheets = {...}`; nothing was written.".to_string(),
    ),
    (None, Some(_)) => Some(
        "Script assigned `output_sheets` but no `write_to_spreadsheet` arg \
         was provided; the result was discarded.".to_string(),
    ),
    _ => None,
};

let mut response = serde_json::json!({
    "output": user_output,
    "stdout": ...,           // existing stdout truncation
    "error": ...,            // existing error handling
    "wrote_sheets": wrote_sheets_response,
});
if let Some(w) = warning {
    response["_warning"] = serde_json::Value::String(w);
}
response
```

The exact code shape depends on how the current dispatcher builds the response — the implementer adapts the wiring without changing what already works.

- [ ] **Step 5: Compile + run existing gsheets tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo build --lib 2>&1 | tail -5
cargo test --lib gsheets --quiet 2>&1 | tail -8
```

Expected: build succeeds, existing tests still pass. The new code path isn't exercised yet — that's Task 4.

- [ ] **Step 6: Commit (incremental — tests come in Task 4)**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(gsheets): output_sheets multi-tab write-back

E-T20b. GsheetsRunPythonArgs gains an optional write_to_spreadsheet field.
When set AND the script assigns output_sheets = {name: DataFrame, ...},
the dispatcher creates one new tab per entry via add_sheet + set_range.
Name collisions auto-suffix ' (2)', ' (3)', capped at 10 attempts.

Warnings are emitted on a misconfigured combination (write_to_spreadsheet
without output_sheets, or vice versa).

Tests land in E-T20d.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3 (E-T20c): `crdt_doc_run_python` dispatcher — multi-sheet write-back

**Goal:** Extend the CRDT dispatcher with the same `output_sheets` path, keeping the legacy `output_sheet` + `write_to_sheet` single-sheet code working.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs`

- [ ] **Step 1: Read the current dispatcher**

```bash
cd /Users/danielgarcia/startti/colmena
sed -n '160,260p' src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs
```

The legacy `output_sheet` + `write_to_sheet` path is around lines 169-258.

- [ ] **Step 2: Add the multi-sheet write block**

After the existing single-sheet write block (around line 260, before the response is built), insert:

```rust
// Multi-sheet path: if the script set `output_sheets = {name: df, ...}`,
// write each entry as a new sheet in the current artifact.
let mut wrote_sheets_response = serde_json::Value::Null;
if let Some(sheets_value) = wrapped_output.get("output_sheets") {
    if !sheets_value.is_null() {
        if let Some(map) = sheets_value.as_object() {
            let mut results: Vec<serde_json::Value> = Vec::new();
            for (raw_name, entry) in map {
                let (Some(records_arr), Some(cols_arr)) = (
                    entry.get("records").and_then(|v| v.as_array()),
                    entry.get("cols").and_then(|v| v.as_array()),
                ) else {
                    results.push(serde_json::json!({
                        "name": raw_name,
                        "error": "entry missing records or cols",
                    }));
                    continue;
                };
                let cols: Vec<String> = cols_arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                let records: Vec<serde_json::Map<String, serde_json::Value>> =
                    records_arr
                        .iter()
                        .filter_map(|v| v.as_object().cloned())
                        .collect();
                match write_records_as_new_sheet(&doc, raw_name, &cols, &records) {
                    Ok(wr) => {
                        results.push(serde_json::json!({
                            "name": raw_name,
                            "resolved_name": wr.resolved_name,
                            "sheet_id": wr.sheet_id,
                            "n_rows": wr.n_rows,
                            "n_cols": wr.n_cols,
                            "truncated_at": wr.truncated_at,
                        }));
                    }
                    Err(e) => {
                        results.push(serde_json::json!({
                            "name": raw_name,
                            "error": format!("write_records_as_new_sheet failed: {e}"),
                        }));
                    }
                }
            }
            ctx.mark_dirty();
            wrote_sheets_response = serde_json::Value::Array(results);
        }
    }
}
```

- [ ] **Step 3: Add the multi-sheet block to the response JSON**

The existing response JSON is built around line 265-275. Add `wrote_sheets`:

```rust
let mut response = serde_json::json!({
    "output": user_output_capped,
    "wrote_sheet": wrote_sheet_response,         // existing single-sheet
    "wrote_sheets": wrote_sheets_response,       // NEW
    "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
    "error": serde_json::Value::Null,
});
if output_truncated {
    response["_output_truncated"] = serde_json::json!(true);
}
// Optional: emit a warning if BOTH paths were used
if !wrote_sheet_response.is_null() && !wrote_sheets_response.is_null() {
    response["_warning"] = serde_json::json!(
        "Both 'output_sheet' (legacy) and 'output_sheets' were set; \
         both were written. Prefer 'output_sheets' for new code."
    );
}
response
```

- [ ] **Step 4: Build + run existing crdt_doc tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo build --lib 2>&1 | tail -3
cargo test --lib crdt_doc --quiet 2>&1 | tail -8
```

Expected: build succeeds, existing tests pass (the legacy path is untouched).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs
git commit -m "feat(crdt_doc): output_sheets multi-tab write-back

E-T20c. Dispatcher now extracts output_sheets (dict from name to
DataFrame) from the postlude result and calls write_records_as_new_sheet
for each entry. Sheet name collisions are handled by the existing helper
(suffix ' (2)', ' (3)' etc.).

The legacy output_sheet + write_to_sheet single-sheet path is preserved
for back-compat; if both paths produce writes, a _warning surfaces.

Tests land in E-T20e.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4 (E-T20d): gsheets wiremock tests

**Goal:** Two new wiremock tests covering the happy path (3-sheet write) and collision handling.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` (append to `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the happy-path test**

Append inside the existing `#[cfg(test)] mod tests` block:

```rust
#[tokio::test]
async fn multi_sheet_write_back_three_tabs() {
    use crate::gsheets::infrastructure::http_client::{
        GoogleSheetsHttpClient, GsheetsConfig,
    };
    use crate::gsheets::domain::SpreadsheetId;
    use std::sync::Arc;
    use wiremock::matchers::{method, path_regex, body_string_contains};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;

    // Mock the OAuth token endpoint (the test client uses ADC fixture).
    Mock::given(method("POST"))
        .and(path_regex(r"/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "fake-token",
            "expires_in": 3600,
        })))
        .mount(&server)
        .await;

    // Mock GET for both input bindings (products + sales).
    Mock::given(method("GET"))
        .and(path_regex(r"/sheet1.*values/Products"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "values": [
                ["product_id", "name", "price"],
                ["P001", "Laptop", 1200],
            ],
        })))
        .mount(&server)
        .await;

    // Mock POST :batchUpdate for add_sheet (3 calls expected).
    Mock::given(method("POST"))
        .and(path_regex(r"/sheet1:batchUpdate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "replies": [{
                "addSheet": {"properties": {"sheetId": 9001, "title": "Top10"}}
            }]
        })))
        .expect(3)
        .mount(&server)
        .await;

    // Mock PUT :values for set_range (3 calls expected).
    Mock::given(method("PUT"))
        .and(path_regex(r"/sheet1/values/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "updatedRange": "Top10!A1:C2",
            "updatedCells": 6,
        })))
        .expect(3)
        .mount(&server)
        .await;

    let cfg = GsheetsConfig {
        token_url: server.uri() + "/token",
        sheets_base: server.uri(),
        drive_base: server.uri(),
        service_account_key_json: serde_json::json!({
            "type": "service_account",
            "client_email": "test@example.com",
            "private_key": "-----BEGIN PRIVATE KEY-----\nfake\n-----END PRIVATE KEY-----\n",
            "token_uri": server.uri() + "/token",
        })
        .to_string(),
    };
    let client: Arc<dyn crate::gsheets::domain::SheetsClient> =
        Arc::new(GoogleSheetsHttpClient::new(cfg).expect("client"));
    let _ = client.token_test_seed("fake-token");

    let args = serde_json::json!({
        "bindings": [{
            "var": "products",
            "spreadsheet_id": "sheet1",
            "sheet": "Products"
        }],
        "code": "\
import pandas as pd\n\
df = pd.DataFrame(products)\n\
output_sheets = {\n\
    'Top10':   df.head(10),\n\
    'Bottom5': df.tail(5),\n\
    'All':     df,\n\
}\n\
output = {'tabs_written': 3}\n\
",
        "write_to_spreadsheet": "sheet1"
    });

    let result =
        super::dispatch_gsheets_run_python_with_client(client.clone(), args).await;

    assert!(result.get("error").map(|v| v.is_null()).unwrap_or(true),
        "unexpected error: {:?}", result);
    let wrote = result.get("wrote_sheets")
        .and_then(|v| v.as_array())
        .expect("wrote_sheets should be an array");
    assert_eq!(wrote.len(), 3, "expected 3 wrote_sheets entries, got {}", wrote.len());
}
```

- [ ] **Step 2: Write the collision-retry test**

Append after:

```rust
#[tokio::test]
async fn multi_sheet_collision_retries_with_suffix() {
    use crate::gsheets::infrastructure::http_client::{
        GoogleSheetsHttpClient, GsheetsConfig,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r"/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "fake-token",
            "expires_in": 3600,
        })))
        .mount(&server)
        .await;

    // First add_sheet returns 400 "already exists"; second returns 200.
    struct AddSheetResponder(AtomicUsize);
    impl Respond for AddSheetResponder {
        fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
            let attempt = self.0.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(400)
                    .set_body_string(r#"{"error":{"code":400,"message":"A sheet with the name \"Existing\" already exists. Please enter another name."}}"#)
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "replies": [{
                        "addSheet": {"properties": {"sheetId": 7777, "title": "Existing (2)"}}
                    }]
                }))
            }
        }
    }
    Mock::given(method("POST"))
        .and(path_regex(r"/sheet1:batchUpdate"))
        .respond_with(AddSheetResponder(AtomicUsize::new(0)))
        .expect(2)   // first 400, second 200
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path_regex(r"/sheet1/values/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "updatedRange": "Existing (2)!A1:B2",
            "updatedCells": 4,
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"/sheet1.*values/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "values": [["a"], ["1"]]
        })))
        .mount(&server)
        .await;

    let cfg = GsheetsConfig {
        token_url: server.uri() + "/token",
        sheets_base: server.uri(),
        drive_base: server.uri(),
        service_account_key_json: serde_json::json!({
            "type": "service_account",
            "client_email": "test@example.com",
            "private_key": "-----BEGIN PRIVATE KEY-----\nfake\n-----END PRIVATE KEY-----\n",
            "token_uri": server.uri() + "/token",
        })
        .to_string(),
    };
    let client: Arc<dyn crate::gsheets::domain::SheetsClient> =
        Arc::new(GoogleSheetsHttpClient::new(cfg).expect("client"));
    let _ = client.token_test_seed("fake-token");

    let args = serde_json::json!({
        "bindings": [{"var": "x", "spreadsheet_id": "sheet1", "sheet": "Whatever"}],
        "code": "\
import pandas as pd\n\
df = pd.DataFrame(x)\n\
output_sheets = {'Existing': df}\n\
output = {}\n\
",
        "write_to_spreadsheet": "sheet1"
    });

    let result =
        super::dispatch_gsheets_run_python_with_client(client.clone(), args).await;
    let wrote = result.get("wrote_sheets")
        .and_then(|v| v.as_array())
        .expect("wrote_sheets array");
    assert_eq!(wrote.len(), 1);
    let entry = &wrote[0];
    assert_eq!(entry["name"], "Existing");
    assert_eq!(entry["resolved_name"], "Existing (2)");
}
```

The exact MockServer / matcher API depends on the existing wiremock test scaffolding in the file — the implementer adapts the imports to match.

- [ ] **Step 3: Run the tests**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib multi_sheet_write_back_three_tabs multi_sheet_collision_retries_with_suffix --quiet 2>&1 | tail -10
```

Expected: 2 tests pass. If pandas is not in the sandbox env, the test may panic — the implementer detects this and either installs pandas via the existing `.venv` or marks the test `#[ignore]` with a clear message (matching the convention from earlier `gsheets_run_python` tests).

- [ ] **Step 4: Full suite + clippy**

```bash
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
```

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "test(gsheets): multi-sheet write-back (happy + collision)

E-T20d. Two new wiremock tests cover the output_sheets path shipped in
E-T20b:
  - multi_sheet_write_back_three_tabs: 3 add_sheet + 3 set_range calls
    produce 3 wrote_sheets metadata entries.
  - multi_sheet_collision_retries_with_suffix: first add_sheet returns
    400 'already exists'; dispatcher retries with ' (2)' and succeeds.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5 (E-T20e): CRDT integration test for multi-sheet write

**Goal:** An in-memory integration test verifying that `output_sheets = {<3 names>: df}` produces 3 new sheets in the Y.Doc.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs` (append to `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the test**

Append inside the existing test module:

```rust
#[tokio::test]
async fn multi_sheet_output_sheets_writes_three_new_tabs() {
    use crate::crdt_documents::CrdtDocumentsRuntime;
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_context::CrdtDocsContext;
    use std::sync::Arc;

    // Build an in-memory CRDT runtime + a single artifact with one tab "src".
    let runtime = CrdtDocumentsRuntime::in_memory_for_tests();
    let artifact_id = runtime
        .create_artifact("test_artifact", "test_session")
        .await
        .expect("create_artifact");
    runtime
        .set_range_in(&artifact_id, "src", "A1", vec![
            vec!["k".to_string(), "v".to_string()],
            vec!["a".to_string(), "1".to_string()],
            vec!["b".to_string(), "2".to_string()],
        ])
        .await
        .expect("seed src");

    let ctx = CrdtDocsContext::for_tests(Arc::new(runtime.clone()), artifact_id.clone());

    let args = super::RunPythonArgs {
        sheet_ids: vec!["src".to_string()],
        code: r#"
import pandas as pd
df = pd.DataFrame(dfs["src"])
output_sheets = {
    "tab_one":   df.head(1),
    "tab_two":   df.tail(1),
    "tab_three": df,
}
output = {"done": True}
"#.to_string(),
        write_to_sheet: None,
    };
    let result = super::execute_run_python(&ctx, args).await;

    // The response should include 3 wrote_sheets entries.
    let wrote = result.get("wrote_sheets")
        .and_then(|v| v.as_array())
        .expect("wrote_sheets should be an array");
    assert_eq!(wrote.len(), 3, "expected 3 sheets, got {}", wrote.len());

    // Verify the 3 new sheets actually exist in the Y.Doc by listing.
    let sheets = runtime
        .list_sheets(&artifact_id)
        .await
        .expect("list_sheets");
    let names: std::collections::HashSet<String> = sheets
        .iter()
        .map(|s| s.title.clone())
        .collect();
    for needle in &["tab_one", "tab_two", "tab_three"] {
        assert!(names.contains(*needle), "missing sheet '{}'; have {:?}", needle, names);
    }
}
```

The exact constructors for `CrdtDocumentsRuntime::in_memory_for_tests()` and `CrdtDocsContext::for_tests()` should mirror what's already used in other tests in the same file or in subsystem F's tests — the implementer adapts.

- [ ] **Step 2: Run the test**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib multi_sheet_output_sheets_writes_three_new_tabs --quiet 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 3: Full suite + clippy + commit**

```bash
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs
git commit -m "test(crdt_doc): multi-sheet output_sheets produces 3 new tabs

E-T20e. In-memory integration test asserts that assigning
output_sheets = {3 names: df} in the script produces 3 wrote_sheets
metadata entries and 3 new sheets in the Y.Doc, alongside the existing
'src' tab.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6 (E-T20f): Tool YAML description updates

**Goal:** Update the descriptions of `gsheets_run_python` and `crdt_doc_run_python` in `text/tools/*.yaml` so the LLM learns about `output_sheets` from the tool surface.

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml`
- Modify: `src/libs/colmena/text/tools/crdt_doc.yaml`

- [ ] **Step 1: Find the existing entries**

```bash
cd /Users/danielgarcia/startti/colmena
grep -nA 4 "^gsheets_run_python:" src/libs/colmena/text/tools/gsheets.yaml
grep -nA 4 "^crdt_doc_run_python:" src/libs/colmena/text/tools/crdt_doc.yaml
```

- [ ] **Step 2: Append the `output_sheets` paragraph to each description**

For `gsheets_run_python` in `gsheets.yaml`, add a new paragraph to the existing `description:` block-scalar (keep the prior text intact):

```yaml
gsheets_run_python:
  summary: Run sandboxed pandas analysis over sheet ranges loaded directly by the dispatcher (rows never pass through the LLM)
  description: |
    <PRESERVE EXISTING DESCRIPTION TEXT AS-IS>

    For results that belong in the spreadsheet (not the conversation),
    assign `output_sheets = {"<tab_name>": <DataFrame>, ...}` in your code
    and pass `write_to_spreadsheet: "<spreadsheet_id>"` as an arg. The
    dispatcher creates one new tab per dict entry; the LLM only receives
    metadata `{name, resolved_name, sheet_id, n_rows, n_cols}` per tab,
    never the row contents. Name collisions auto-suffix " (2)", " (3)".

    For more patterns and pitfalls, load the `gsheets-table-exploration`
    skill (single-table inspection) or `gsheets-cross-sheet-analysis`
    skill (joins, comparisons).
```

For `crdt_doc_run_python` in `crdt_doc.yaml`, the equivalent paragraph (no `write_to_spreadsheet` — writes go to the current artifact):

```yaml
crdt_doc_run_python:
  summary: Run sandboxed pandas analysis over CRDT sheets without loading rows through the LLM
  description: |
    <PRESERVE EXISTING DESCRIPTION TEXT AS-IS>

    For results that belong in the document (not the conversation),
    assign `output_sheets = {"<tab_name>": <DataFrame>, ...}` in your code.
    The dispatcher creates one new tab per dict entry in the current
    artifact; the LLM only receives metadata
    `{name, resolved_name, sheet_id, n_rows, n_cols}` per tab.

    The legacy single-sheet path (`output_sheet = <df>` +
    `write_to_sheet: "<name>"`) still works for back-compat but new code
    should use `output_sheets`.

    For more patterns and pitfalls, load the `crdt-doc-table-exploration`
    skill (single-table inspection) or `crdt-doc-cross-sheet-analysis`
    skill (joins, comparisons).
```

- [ ] **Step 3: Verify YAML parses and text-coverage tests pass**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib text_coverage_tests --quiet 2>&1 | tail -10
```

Expected: `every_registered_tool_has_text_entry`, `no_orphan_yaml_entries`, `tool_def_summary_matches_yaml` all pass. If `tool_def_summary_matches_yaml` fails, the summary string is the only one expected to be byte-identical — descriptions can drift freely.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml src/libs/colmena/text/tools/crdt_doc.yaml
git commit -m "docs(text): mention output_sheets in run_python tool descriptions

E-T20f. Adds a paragraph to gsheets_run_python and crdt_doc_run_python
descriptions explaining the new output_sheets multi-tab write-back path
(shipped in E-T20b/E-T20c). Also points the LLM at the two new
table-exploration skills (which land in E-T21).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7 (E-T21a): Skill — `gsheets-table-exploration`

**Goal:** Ship a new skill bundle teaching inspect-first, top-N, filters, group/agg, type coercion, output shaping.

**Files:**
- Create: `src/libs/colmena/skills/gsheets-table-exploration/SKILL.md`
- Create: 6 reference files under `references/`

- [ ] **Step 1: Create the directory + SKILL.md**

```bash
cd /Users/danielgarcia/startti/colmena
mkdir -p src/libs/colmena/skills/gsheets-table-exploration/references
```

Path: `src/libs/colmena/skills/gsheets-table-exploration/SKILL.md`

```markdown
---
name: gsheets-table-exploration
description: Patterns for exploring a single Google Sheets table — schema inspection first, top-N via nlargest, filters via query, type coercion, and how to ship tabular results back to the spreadsheet. Load BEFORE writing pandas code over one sheet.
when_to_load: When the agent needs to answer a question about one Google Sheets tab (top-N by some column, filter by category, group-and-aggregate) without joining other sheets. Pair this with `gsheets-cross-sheet-analysis` if you also need to merge across tabs.
references:
  - 01-inspect-schema-first
  - 02-top-n-patterns
  - 03-filter-and-query
  - 04-group-and-aggregate
  - 05-type-coercion
  - 06-output-shaping
---

# gsheets-table-exploration

Use this skill when you need to answer a question about ONE tab of a
Google Sheets spreadsheet — top-N, filters, groupings, simple stats.
The only tools you need are `gsheets_read` (peek + tiny reads) and
`gsheets_run_python` (real analysis).

## The cardinal rule

**ALWAYS inspect the table BEFORE writing analytics.** Load reference
`01-inspect-schema-first` for the exact pattern. Skipping this step is
the #1 source of analyses that silently produce wrong answers (missing
columns, type errors, mixed numeric / text values from Google Sheets'
auto-typing).

## Decision tree

1. **Do you know the column names and types?** → If not, load
   `01-inspect-schema-first` and run `df.head(3)` + `df.dtypes` first.
2. **Is the question a top-N by a column?** → Load `02-top-n-patterns`.
   Use `df.nlargest(N, 'col')`, not `sort_values().head()`.
3. **Is the question a filter (where-clause-like)?** → Load
   `03-filter-and-query`.
4. **Is the question a grouped aggregation (sum/avg by category)?** →
   Load `04-group-and-aggregate`.
5. **Are numeric columns coming back as strings?** → Load
   `05-type-coercion`. Google Sheets often imports digits as text.
6. **Does the result belong in the spreadsheet (a new tab) or in the
   conversation (a short text summary)?** → Load `06-output-shaping`.
   Multi-tab write-back via `output_sheets` keeps row data out of the
   LLM context entirely.

## When to load `gsheets-cross-sheet-analysis` instead

Switch to `gsheets-cross-sheet-analysis` when the question involves
**two or more sheets** (joins, enrichment, comparison). That skill
covers `pd.merge`, FK→PK joins, and the cross-artifact `import_sheet`
flow.
```

- [ ] **Step 2: Create `references/01-inspect-schema-first.md`**

```markdown
# 01 — Inspect schema first

**The rule:** before writing any analysis code, look at the table.
Always.

## Minimal recipe

```python
import pandas as pd
df = pd.DataFrame(products)   # binding name from gsheets_run_python args
print(df.head(3))             # see the first 3 rows
print(df.dtypes)              # see what pandas inferred per column
print(f"shape: {df.shape}")   # rows, cols
output = {
    "head_3":   df.head(3).to_dict('records'),
    "dtypes":   {c: str(t) for c, t in df.dtypes.items()},
    "n_rows":   len(df),
}
```

Run this FIRST, look at the response, THEN write the real analysis in a
second `gsheets_run_python` call. Two cheap calls beat one expensive
guess.

## Even cheaper: peek without pandas

If the spreadsheet is large and you only need column names + a sample,
use `gsheets_read` with a tiny range:

```json
{
  "spreadsheet_id": "...",
  "sheet": "products",
  "range": "A1:Z3",
  "value_render": "FORMATTED_VALUE",
  "as_records": true
}
```

This returns 3 rows of records (header + 2 examples) for the price of
one HTTP call. The LLM sees ~30 lines instead of 5000.

## Why this matters

Google Sheets has subtle behaviors that bite analyses:

- A column of integers may import as text if any cell starts with a
  leading apostrophe (`'44`).
- A "date" column may be a serial number, a string, or a mix.
- Names with non-ASCII characters (`Categoría`) may or may not survive
  round-tripping intact.

Looking at `head(3)` + `dtypes` catches all three before they corrupt
the analysis.
```

- [ ] **Step 3: Create `references/02-top-n-patterns.md`**

```markdown
# 02 — Top-N patterns

For "the N largest/smallest by some column", use `nlargest` / `nsmallest`.
Do NOT use `sort_values().head()` — the latter is a strict superset of
work for the same result.

## Top-N by one column

```python
import pandas as pd
df = pd.DataFrame(products)
top_5 = df.nlargest(5, 'price')[['product_id', 'name', 'price']]
output = top_5.to_dict('records')
```

## Bottom-N

```python
bottom_5 = df.nsmallest(5, 'price')
```

## Top-N by a computed column

```python
df['margin'] = df['price'] - df['cost']
top_5_margin = df.nlargest(5, 'margin')[['name', 'margin']]
```

## Top-N within each group

```python
# Top 3 per category
df.groupby('category').apply(
    lambda g: g.nlargest(3, 'price')
).reset_index(drop=True)
```

## Anti-pattern: do NOT use max_rows_to_load for top-N

There is no `max_rows_to_load` parameter on bindings. Even if there were,
loading only the first N rows then sorting would produce arbitrary
results — the first N rows are not the largest. Load the column you
need to rank by, do the ranking in pandas, then return just the top-N.

If the table is so large that loading is expensive, use `gsheets_read`
with a `range` (server-side row cap) to fetch only what you need
column-wise.
```

- [ ] **Step 4: Create `references/03-filter-and-query.md`**

```markdown
# 03 — Filter and query

Pandas offers two equivalent styles for filtering. Pick whichever reads
better for your query.

## Boolean mask (explicit)

```python
import pandas as pd
df = pd.DataFrame(products)
laptops = df[df['category'] == 'Laptops']
expensive = df[(df['price'] > 1000) & (df['category'] == 'Laptops')]
```

## `df.query()` (string DSL — often nicer for AND/OR chains)

```python
laptops = df.query("category == 'Laptops'")
expensive = df.query("price > 1000 and category == 'Laptops'")
# Reference an outer variable with @
threshold = 1500
above = df.query("price > @threshold")
```

## Set / range filters

```python
chosen = df[df['category'].isin(['Laptops', 'Smartphones'])]
in_range = df[df['price'].between(500, 1500)]
```

## Null handling

```python
df_clean = df[df['price'].notna()]      # drop rows with NaN price
df_zero  = df['price'].fillna(0)        # replace NaN with 0
```

## Combining filter + top-N

```python
top_3_laptops = df.query("category == 'Laptops'").nlargest(3, 'price')
```
```

- [ ] **Step 5: Create `references/04-group-and-aggregate.md`**

```markdown
# 04 — Group and aggregate

For "sum / avg / count by category", use `groupby` + `agg`.

## Single aggregation

```python
import pandas as pd
df = pd.DataFrame(sales)
revenue_by_region = df.groupby('region')['total'].sum().reset_index()
```

## Multiple aggregations on different columns

```python
summary = df.groupby('category').agg(
    total_revenue=('total', 'sum'),
    avg_price=('unit_price', 'mean'),
    n_orders=('sale_id', 'count'),
).reset_index()
```

## Multiple aggregations on the same column

```python
stats = df.groupby('category')['price'].agg(['min', 'max', 'mean', 'std']).reset_index()
```

## Group by multiple columns

```python
matrix = df.groupby(['region', 'channel'])['total'].sum().unstack(fill_value=0)
```

## Top-N within group (combining with reference 02)

```python
top_3_per_category = df.sort_values('price', ascending=False).groupby('category').head(3)
```

## `as_index=False` vs `reset_index()`

Both produce a "flat" result instead of one with the group as an index.
`as_index=False` is more concise; `reset_index()` is more explicit.
Pick one and stay consistent within a script.
```

- [ ] **Step 6: Create `references/05-type-coercion.md`**

```markdown
# 05 — Type coercion

Google Sheets sends numbers and dates in inconsistent ways. Always
coerce before doing math.

## Numbers

```python
import pandas as pd
df = pd.DataFrame(products)

# Convert a column that may contain stringy numbers (e.g., '44')
df['price'] = pd.to_numeric(df['price'], errors='coerce')
df['cost']  = pd.to_numeric(df['cost'],  errors='coerce')

# After this, rows with unparseable values become NaN. Filter or fill:
df = df.dropna(subset=['price'])
# OR
df['price'] = df['price'].fillna(0)
```

## Dates

```python
df['date'] = pd.to_datetime(df['date'], errors='coerce')
# Now you can use .dt accessors:
df['month'] = df['date'].dt.month
df_q1 = df[df['date'].dt.quarter == 1]
```

## The leading apostrophe trap

Google Sheets prefixes a cell with `'` to force "store as text". When
you read the cell via the API, the `'` does NOT appear in the value
(it's metadata), but the cell is still stored as a STRING. Pandas
inferring types will see a string column.

Symptom: `df['price'].sum()` produces a string concatenation instead of
a numeric sum.

Cure: always `pd.to_numeric` on columns you intend to do math on.

## Boolean values from Google Sheets

Booleans round-trip cleanly via the API. If you get strings `"TRUE"` /
`"FALSE"` instead, the column was imported as text:

```python
df['active'] = df['active'].astype(str).str.upper() == 'TRUE'
```
```

- [ ] **Step 7: Create `references/06-output-shaping.md`**

```markdown
# 06 — Output shaping

The result of your analysis can flow back in two ways. Pick based on
the size of the result and where the human is going to look.

## Small result → return via `output`

For top-10 lists, summary statistics, single-row answers — keep it in
the conversation. The LLM cap is 10 KB for `output`.

```python
import pandas as pd
df = pd.DataFrame(products)
top_5 = df.nlargest(5, 'price')[['product_id', 'name', 'price']]
output = {
    'top_5_by_price': top_5.to_dict('records'),
    'total_products': len(df),
}
```

## Tabular result → write back as a new tab

For full tables of results (top-100, all groups, anomaly lists), write
to a new tab in the spreadsheet. Use `output_sheets` (dict of
name→DataFrame) and pass `write_to_spreadsheet`:

```python
import pandas as pd
df = pd.DataFrame(sales)

by_region = df.groupby('region')['total'].sum().reset_index()
top10     = df.nlargest(10, 'total')
all_data  = df.assign(margin=df['total'] - df['cost'])

output_sheets = {
    'By Region':     by_region,
    'Top 10 Sales':  top10,
    'With Margin':   all_data,
}
output = {'tabs_written': 3}
```

Call shape:

```json
{
  "bindings": [{"var": "sales", "spreadsheet_id": "<id>", "sheet": "Sales"}],
  "code": "<script that sets output_sheets>",
  "write_to_spreadsheet": "<target_spreadsheet_id>"
}
```

The dispatcher creates 3 new tabs. The LLM only receives metadata
(`name`, `resolved_name`, `sheet_id`, `n_rows`, `n_cols`) per tab. The
row contents NEVER pass through your context.

## When `output_sheets` is misconfigured

- `write_to_spreadsheet` set but no `output_sheets` in code: dispatcher
  emits a `_warning`.
- `output_sheets` set but no `write_to_spreadsheet` arg: same warning.
  The result is discarded; no tabs created.

Read the warning and fix the call.

## Naming new tabs

If a tab named `By Region` already exists in the target, the dispatcher
auto-suffixes ` (2)`, ` (3)`, etc. (capped at 10). The response
includes both `name` (what you asked for) and `resolved_name` (what was
actually created).
```

- [ ] **Step 8: Verify the skill loads cleanly**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib --quiet 2>&1 | tail -5
cargo build --lib 2>&1 | tail -3
```

Expected: build succeeds. The `include_dir!` macro picks up the new folder automatically.

The Alpha-shipped test `index_doc_covers_all_registered_skills` will fail if the new skill is not yet listed in the index doc. Task 9 (docs sweep) adds the index entry; until then, the test fails locally. To make this task stand alone, OPTIONALLY add the index row here in step 8:

```bash
# Append the row to the index doc so the test passes after this task lands.
# (This is the same edit Task 9 step 1 will codify.)
```

Add to `docs/developer_guide/42_builtin_skills_index.md`:

```markdown
| `gsheets-table-exploration` | Single-table patterns for Google Sheets — inspect schema first, top-N via nlargest, filters via query, type coercion, multi-tab output. | [link](../../src/libs/colmena/skills/gsheets-table-exploration/SKILL.md) |
```

- [ ] **Step 9: Test passes + commit**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib index_doc_covers_all_registered_skills --quiet 2>&1 | tail -5
git add src/libs/colmena/skills/gsheets-table-exploration/ \
        docs/developer_guide/42_builtin_skills_index.md
git commit -m "feat(skills): gsheets-table-exploration

E-T21a. New skill bundle teaching inspect-first, top-N via nlargest,
filters via query, group/aggregate patterns, type coercion, and the
output_sheets multi-tab contract.

SKILL.md is the navigation index; 6 references cover one pattern each.

Index doc 42_builtin_skills_index.md updated so the CI completeness
test stays green.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8 (E-T21b): Skill — `crdt-doc-table-exploration` (mirror of Task 7)

**Goal:** Same 7 files as Task 7, but for CRDT doc tools (`crdt_doc_*` instead of `gsheets_*`).

**Files:**
- Create: `src/libs/colmena/skills/crdt-doc-table-exploration/SKILL.md`
- Create: 6 reference files under `references/`

- [ ] **Step 1: Create directory + SKILL.md**

```bash
cd /Users/danielgarcia/startti/colmena
mkdir -p src/libs/colmena/skills/crdt-doc-table-exploration/references
```

Path: `src/libs/colmena/skills/crdt-doc-table-exploration/SKILL.md`

```markdown
---
name: crdt-doc-table-exploration
description: Patterns for exploring a single CRDT document table — schema inspection first, top-N via nlargest, filters via query, type coercion, and how to ship tabular results back as new tabs. Load BEFORE writing pandas code over one CRDT sheet.
when_to_load: When the agent needs to answer a question about one CRDT sheet (top-N by some column, filter by category, group-and-aggregate). Pair with `crdt-doc-cross-sheet-analysis` if you also need to merge across tabs or artifacts.
references:
  - 01-inspect-schema-first
  - 02-top-n-patterns
  - 03-filter-and-query
  - 04-group-and-aggregate
  - 05-type-coercion
  - 06-output-shaping
---

# crdt-doc-table-exploration

Use this skill when you need to answer a question about ONE tab of a
CRDT document — top-N, filters, groupings, simple stats. The only tools
you need are `crdt_doc_read` (peek + tiny reads) and `crdt_doc_run_python`
(real analysis).

## The cardinal rule

**ALWAYS inspect the table BEFORE writing analytics.** Load reference
`01-inspect-schema-first` for the exact pattern.

## Decision tree

1. Don't know the columns → `01-inspect-schema-first` (`df.head(3)` +
   `df.dtypes`).
2. Top-N by a column → `02-top-n-patterns` (`nlargest` / `nsmallest`).
3. Where-clause-like filter → `03-filter-and-query`.
4. Sum/avg by category → `04-group-and-aggregate`.
5. Numeric columns as strings → `05-type-coercion`.
6. Result belongs in the document, not the conversation →
   `06-output-shaping` (use `output_sheets`).

## When to load `crdt-doc-cross-sheet-analysis` instead

For joins and comparisons across multiple sheets (in the current
artifact or across artifacts via `import_sheet`).
```

- [ ] **Step 2: Create the 6 reference files**

Each reference file mirrors the gsheets equivalent from Task 7 but
substitutes:

- `gsheets_read` → `crdt_doc_read`
- `gsheets_run_python` → `crdt_doc_run_python`
- Remove `write_to_spreadsheet` (CRDT writes go to the current
  artifact — no target arg needed)
- Code-block examples reference `dfs["sheet_name"]` (the CRDT pattern)
  rather than `pd.DataFrame(<binding>)` (the gsheets pattern)

Path: `src/libs/colmena/skills/crdt-doc-table-exploration/references/01-inspect-schema-first.md`

```markdown
# 01 — Inspect schema first

**The rule:** before writing any analysis code, look at the table.
Always.

## Minimal recipe

```python
import pandas as pd
df = pd.DataFrame(dfs["products"])   # sheet_id from crdt_doc_run_python args
print(df.head(3))
print(df.dtypes)
print(f"shape: {df.shape}")
output = {
    "head_3":   df.head(3).to_dict('records'),
    "dtypes":   {c: str(t) for c, t in df.dtypes.items()},
    "n_rows":   len(df),
}
```

## Even cheaper: peek without pandas

```json
{
  "artifact_id": "...",
  "sheet": "products",
  "range": "A1:Z3",
  "include_formulas": false
}
```

## Why this matters

Same caveats as Google Sheets — leading apostrophes, mixed numeric/text,
dates as serials. Always inspect.
```

Repeat for `02-top-n-patterns.md`, `03-filter-and-query.md`,
`04-group-and-aggregate.md`, `05-type-coercion.md`, `06-output-shaping.md`,
adapting:

- `pd.DataFrame(products)` → `pd.DataFrame(dfs["products"])`
- `"spreadsheet_id"` → `"artifact_id"`
- Drop `write_to_spreadsheet` references (CRDT uses current artifact)
- Mention that `output_sheets` writes go to the **current artifact**,
  not a configurable target

For `06-output-shaping.md` specifically, replace the gsheets call-shape
example with:

```json
{
  "sheet_ids": ["sales"],
  "code": "<script that sets output_sheets>"
}
```

And note explicitly: "There is no `write_to_spreadsheet` arg in
`crdt_doc_run_python`. New tabs created via `output_sheets` always land
in the current artifact (the one `ctx.artifact_id` points at)."

- [ ] **Step 3: Add index entry**

Append to `docs/developer_guide/42_builtin_skills_index.md`:

```markdown
| `crdt-doc-table-exploration` | Single-table patterns for CRDT documents — inspect schema first, top-N via nlargest, filters via query, type coercion, multi-tab output. | [link](../../src/libs/colmena/skills/crdt-doc-table-exploration/SKILL.md) |
```

- [ ] **Step 4: Test + commit**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib index_doc_covers_all_registered_skills --quiet 2>&1 | tail -5
cargo build --lib 2>&1 | tail -3
git add src/libs/colmena/skills/crdt-doc-table-exploration/ \
        docs/developer_guide/42_builtin_skills_index.md
git commit -m "feat(skills): crdt-doc-table-exploration

E-T21b. Mirror of gsheets-table-exploration (E-T21a) for CRDT doc tools.
Same six references, adapted to crdt_doc_run_python's dfs[<sheet>] data
loading pattern and the implicit current-artifact write target (no
write_to_spreadsheet arg).

Index doc updated.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9 (E-T21c): Docs sweep

**Goal:** Update CHANGELOG, BACKLOG, and (if needed) the dev-guide index. Pure markdown.

**Files:**
- Modify: `docs/CHANGELOG_2026-06.md`
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Append to `docs/CHANGELOG_2026-06.md`**

```markdown
- **E-T20 shipped 2026-06-06** — pandas multi-sheet write-back.
  `gsheets_run_python` gains a `write_to_spreadsheet` arg and both
  `gsheets_run_python` + `crdt_doc_run_python` recognise an
  `output_sheets = {name: DataFrame, ...}` global in the user code. The
  dispatcher creates one new tab per entry (auto-suffix on collision)
  and returns metadata-only `wrote_sheets: [...]` to the LLM. Row
  contents NEVER pass through the LLM. Existing `output_sheet` +
  `write_to_sheet` single-sheet path preserved for back-compat.
- **E-T21 shipped 2026-06-06** — two new skills under
  `src/libs/colmena/skills/`:
  `gsheets-table-exploration` and `crdt-doc-table-exploration`. Each
  bundles `SKILL.md` + 6 references covering inspect-first, top-N via
  `nlargest`, filter+query, group+aggregate, type coercion, and output
  shaping (with multi-tab write-back). Tool descriptions in
  `text/tools/{gsheets,crdt_doc}.yaml` updated to point at the new
  skills.
```

- [ ] **Step 2: Append to `docs/BACKLOG.md`**

```markdown
- **Format options per output_sheet** — opt-in `header_style`,
  `column_widths`, `freeze_top_row` accompanying each entry in
  `output_sheets`. Today the dispatcher writes raw values only.
- **Diff-aware sheet write** — opt-in arg to overwrite an existing tab
  instead of creating a suffixed one. The current "create-new-with-suffix"
  default is safe; the opt-in makes destructive writes explicit.
- **Auto-naming for unnamed DataFrames** — if a DataFrame appears in
  `output_sheets` under a `None` or numeric key, the dispatcher assigns
  `Untitled (1)`, `Untitled (2)`. Today the script must name every
  entry.
- **Direct hyperlink to new tab in response** — `wrote_sheets[i].url`
  pointing to `https://docs.google.com/spreadsheets/d/<id>/edit#gid=<sheet_id>`
  for quick user navigation.
```

- [ ] **Step 3: Final sweep**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

If `cargo fmt --check` shows a diff, run `cargo fmt && git add -u && git commit -m "style: fmt"`.

- [ ] **Step 4: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add docs/CHANGELOG_2026-06.md docs/BACKLOG.md
git commit -m "docs(E-T20+T21): changelog + backlog entries

E-T20 (multi-sheet write-back) + E-T21 (two new table-exploration skills)
recorded in CHANGELOG_2026-06.md. BACKLOG.md captures 4 deferred follow-ups
(formatting per sheet, overwrite mode, auto-naming, hyperlink in response).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Final sweep

- [ ] **Step 1: Full lib test suite**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib --quiet 2>&1 | tail -5
```

Expected: all tests pass, including the new wiremock + integration tests.

- [ ] **Step 2: Clippy + fmt**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

- [ ] **Step 3 (optional): Smoke test the multi-sheet write end-to-end**

If desired, run a graph similar to the existing cross-sheet demo but with
the agent declaring `output_sheets = {...}` rather than `set_range`
afterward. The result should be N new tabs in the target spreadsheet,
each with the agent's analysis output, and the SSE log should show only
metadata (no row data) flowing back through the LLM.

---

## Self-review checklist

| Spec section | Plan task(s) | OK? |
|---|---|---|
| §1 Goal #1 (multi-sheet write-back) | Tasks 1, 2, 3, 4, 5, 6 | ✅ |
| §1 Goal #2 (table-exploration skills) | Tasks 7, 8 | ✅ |
| §3 Open-source rule | Honoured: skills + dispatcher additions are generic | ✅ |
| §4.1 Postlude updates | Task 1 | ✅ |
| §4.2 gsheets dispatcher (`write_to_spreadsheet`, add_sheet+set_range loop, collision retry) | Task 2 | ✅ |
| §4.2 crdt_doc dispatcher (multi-sheet path + legacy back-compat) | Task 3 | ✅ |
| §4.3 LLM-visible response (`wrote_sheets`, `_warning` when misconfigured) | Tasks 2, 3 | ✅ |
| §4.4 Two new skill folders | Tasks 7, 8 | ✅ |
| §4.5 Tool YAML description updates | Task 6 | ✅ |
| §5 Edge cases (non-dict `output_sheets`, non-DataFrame value, collision, both `output_sheet` + `output_sheets`, missing target, etc.) | Tasks 1-3 implement; Tasks 4-5 verify | ✅ |
| §6 Tests (wiremock happy + collision, CRDT integration, postlude shape, skill load) | Tasks 4, 5, 7, 8 | ✅ |
| §7 Task breakdown 1:1 mapping | Tasks 1-9 map exactly to E-T20a-f + E-T21a-c | ✅ |
| §8 Back-compat | Final sweep re-runs every test | ✅ |
| §9 Future BACKLOG (4 entries) | Task 9 step 2 records all four | ✅ |
