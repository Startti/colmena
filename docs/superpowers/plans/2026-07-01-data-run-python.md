# `data_run_python` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a single unified synthetic tool `data_run_python` that binds tabular data from CSV/XLSX attachments, Google Sheets, SQL SELECTs, and inline JSON into a pandas sandbox, and writes results back to SQL tables (`append`/`update`/`upsert`/`replace`), Google Sheets, and CSV/XLSX attachments — all server-side, so rows never enter the LLM context.

**Architecture:** New synthetic tool mirroring `gsheets_run_python`'s dispatcher shape. Polymorphic bindings resolved in parallel into sandbox globals; user pandas code assigns `output_tables` / `output_sheets` / `output_attachments` globals; a postlude packages them; the dispatcher fans each sink out to its backend. SQL access reuses the `sql_query` node's permission model, AST validator, and Postgres adapter. The three sinks live in focused modules (`table_writer.rs`, `sheet_writer.rs`, `attachment_writer.rs`); binding parsing lives in `tabular_bindings.rs`. Gating hides unconfigured sources from the LLM entirely.

**Tech Stack:** Rust (crate `colmena_dag_engine`), `sqlx` (Postgres), `sqlparser` (AST), `calamine` (XLSX read), `rust_xlsxwriter` (XLSX write), `serde_json`, `schemars`, `tokio`. Python sandbox via `execute_sandboxed_helper` (pandas/numpy/scipy). LLM-facing text in `text/tools/*.yaml` + `text/prompts/python_sandbox/*.md`.

## Global Constraints

- Crate name is `colmena_dag_engine`. Test a module with `cargo test --lib <module>`; never `-p colmena`.
- `[lints.rust] warnings = "deny"` — any warning fails the build. No unused imports/dead code. Use `#[allow(deprecated)]` only on test modules exercising deprecated APIs.
- Rust toolchain pinned to `1.95.0` (`rust-toolchain.toml`).
- Run `cargo test --verbose` before any push (CI runs `--verbose`; `--lib` hides doctest/integration failures + macOS/Linux FS races).
- Tests reading env vars (`DATABASE_URL`, `TEST_DATABASE_URL`, Google creds) MUST be `#[ignore = "requires X — run with \`cargo test -- --ignored\`"]`.
- Node outputs use the `{ "output": ... }` convention. Domain layer has ZERO infrastructure deps; external integrations go through ports (traits).
- Docs prose in Spanish; code comments + API docs in English.
- Conventional Commits only (`feat`/`fix`/`docs`/`refactor`/`test`/`chore`/…). NEVER `plan`/`spec`/`diag`.
- Sandbox caps (match siblings): file ≤ 50 MB, ≤ 100 000 rows, 30 s wall-clock, `output`/`stdout`/`error` ≤ 10 KB each.
- SQL bindings are SELECT-only, validated by `sqlparser` AST; respect `permissions.allowed_schemas`.
- Additive only — do NOT modify `gsheets_run_python`, `attachment_run_python`, `crdt_doc_run_python` behavior until the deprecation phase (Phase 10), which is gated on E2E verification + ADP sweep.
- Spec: `docs/superpowers/specs/2026-07-01-data-run-python-design.md`.

## Reused signatures (verified in-tree — do not redefine)

- `parse_attachment_to_records(bytes: &[u8], mime_or_filename, delimiter: Option<&str>, sheet_name: Option<&str>, header_row: u32) -> Result<(Vec<String>, Vec<Map<String,Value>>), String>` — `sql_bulk_tools.rs:562` (confirm exact arg list when implementing; `attachment_run_python.rs:237` calls it).
- `diff_records(current: &[Map<String,Value>], new: &[Map<String,Value>], key: &str, restrict_columns: Option<&[String]>, strict_match: bool, tab_label: &str) -> Result<DiffResult, DiffError>` — `diff_writer.rs:150`. `DiffResult{changes: Vec<CellChange>, rows_changed, rows_unchanged, rows_skipped_not_in_target, rows_skipped_null_key, columns_touched}`; `CellChange{key_value, column, old_value, new_value}`.
- `validate_table_against_allowlist(full_table: &str, allowed_schemas: &[String]) -> Result<(String,String), String>` and `split_qualified_table(full: &str) -> (String,String)` — `sql_bulk_tools.rs:1073,1084`.
- `execute_sandboxed_helper(code: &str, mode: &str, timeout_secs: u64, inputs: &serde_json::Map<String,Value>) -> Result<HelperResult, String>` where `HelperResult{output: Option<Value>, stdout: String}` — `python_node.rs`.
- `SqlConnectionPort::execute_query(&self, query: &str, max_rows: u64, tenant_user_id: Option<&str>) -> Result<QueryResult, SqlNodeError>`; `QueryResult{output: Value, row_count: u64, truncated: bool}` — `sql_ports.rs:81`.
- `SqlPortFactory::get_adapter(&self, url: &str, statement_timeout_ms: u64, work_mem_mb: u64) -> Result<Arc<PgPoolAdapter>, SqlNodeError>` — `sql_port_factory.rs:22`.
- `SqlPermissions` (domain type) — `sql/domain/sql_permissions.rs`; `StaticRuleValidator::validate(&self, query, &SqlPermissions) -> ValidationResult` — `sql_ports.rs`.
- gsheets: `SheetsClient` trait, `GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env())`, `write_output_sheets(...)` currently private in `gsheets_run_python.rs:548`.
- Executor: `DagToolExecutor` (`dag_tool_executor.rs:58`); attachment streaming `fetch_attachment_stream` (`:533`); routing block at `:1030`; per-tool `fixed_config` via `self.tool_configurations.get(<name>).map(|tc| tc.fixed_config.clone())`.

## File Structure

| File | Responsibility | New/Modify |
|---|---|---|
| `.../llm_synthetic_tools/data_run_python.rs` | Args, polymorphic bindings, deserializer, dispatcher orchestration, gating, response assembly | Create |
| `.../llm_synthetic_tools/tabular_bindings.rs` | Binding enum + structural discriminator + validation + parallel resolution into sandbox inputs | Create |
| `.../llm_synthetic_tools/table_writer.rs` | `output_tables` sink: parse specs, validate, infer DDL, modes, atomic transaction | Create |
| `.../llm_synthetic_tools/sheet_writer.rs` | `write_output_sheets` + snapshots, extracted from `gsheets_run_python.rs` | Create (move) |
| `.../llm_synthetic_tools/attachment_writer.rs` | `output_attachments` sink: serialize CSV/XLSX + register in catalog | Create |
| `.../llm_synthetic_tools/gsheets_run_python.rs` | Re-point to `sheet_writer` (no behavior change) | Modify |
| `.../llm_synthetic_tools/mod.rs` | `pub mod` the new modules + re-exports | Modify |
| `.../dag_tool_executor.rs` | Route `data_run_python` to its dispatcher with fixed_config | Modify |
| `.../nodes/llm.rs` | Build tool definition when enabled; dynamic description; gating | Modify |
| `text/tools/data_run_python.yaml` | Description + summary registry | Create |
| `text/prompts/python_sandbox/data_run_python_prelude.md` / `_postlude.md` | Sandbox prelude/postlude | Create |
| `skills/data-run-python-recipes/SKILL.md` (+ references/) | Opt-in recipes skill | Create |
| `tests/graphs/agents/data_run_python_*.json` | E2E graphs | Create |
| `docs/developer_guide/{23,39,41,43}*.md` | Doc updates | Modify |

---

## Phase 1 — Tool scaffolding, bindings, validation (pure, no I/O)

### Task 1: Binding types + structural discriminator + validation

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

**Interfaces:**
- Produces: `pub struct DataBinding { var: String, attachment_id: Option<String>, spreadsheet_id: Option<String>, sheet: Option<String>, range: Option<String>, query: Option<String>, data: Option<Value>, delimiter: Option<String>, sheet_name: Option<String>, header_row: Option<u32> }`; `pub enum BindingKind { Attachment, Gsheets, Sql, Inline }`; `pub fn classify_binding(b: &DataBinding) -> Result<BindingKind, String>`; `pub fn validate_bindings(bindings: &[DataBinding]) -> Result<(), Value>` (returns structured `invalid_args` JSON on error); `pub fn deserialize_bindings_flexible<'de,D>(d: D) -> Result<Vec<DataBinding>, D::Error>` (moved/generalized from gsheets).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn b(v: Value) -> DataBinding { serde_json::from_value(v).unwrap() }

    #[test]
    fn classify_recognizes_each_source() {
        assert_eq!(classify_binding(&b(json!({"var":"a","attachment_id":"doc1"}))).unwrap(), BindingKind::Attachment);
        assert_eq!(classify_binding(&b(json!({"var":"a","spreadsheet_id":"1x","sheet":"Q4"}))).unwrap(), BindingKind::Gsheets);
        assert_eq!(classify_binding(&b(json!({"var":"a","query":"SELECT 1"}))).unwrap(), BindingKind::Sql);
        assert_eq!(classify_binding(&b(json!({"var":"a","data":[]}))).unwrap(), BindingKind::Inline);
    }

    #[test]
    fn classify_rejects_ambiguous_and_empty() {
        assert!(classify_binding(&b(json!({"var":"a","attachment_id":"d","query":"SELECT 1"}))).is_err());
        assert!(classify_binding(&b(json!({"var":"a"}))).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_and_empty_var() {
        let dup = vec![b(json!({"var":"x","query":"SELECT 1"})), b(json!({"var":"x","data":[]}))];
        assert!(validate_bindings(&dup).is_err());
        let empty = vec![b(json!({"var":"","data":[]}))];
        assert!(validate_bindings(&empty).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib tabular_bindings 2>&1 | tail -20`
Expected: FAIL — module/types not found.

- [ ] **Step 3: Write minimal implementation**

Create `tabular_bindings.rs` with `DataBinding` (derive `Debug, Clone, Deserialize, JsonSchema`; `#[serde(default)]` on all optional fields; keep gsheets aliases `#[serde(alias="binding_name", alias="name")]` on `var`, `#[serde(alias="sheet_name")]` care — note `sheet_name` is its own field for attachments, so alias `sheet` only to `sheet`). Implement `BindingKind` (derive `Debug, PartialEq, Eq, Clone, Copy`). `classify_binding`: count present discriminators (`attachment_id`, `spreadsheet_id`+`sheet`, `query`, `data`); exactly one ⇒ Ok(kind), else `Err("binding '<var>' must have exactly one source: attachment_id | (spreadsheet_id+sheet) | query | data")`. `validate_bindings`: non-empty; every `var` non-empty & trimmed; no duplicate `var`s; every binding classifies; return structured `Err(json!({"error":"invalid_args","message":...}))`. Port `deserialize_bindings_flexible` from `gsheets_run_python.rs:124` but building `DataBinding` (accept array or `{var: obj}` map; reject bare-string values). Add `pub mod tabular_bindings;` to `mod.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib tabular_bindings 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "feat: polymorphic tabular bindings for data_run_python"
```

### Task 2: Tool args struct + builder + text registry entry

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`
- Create: `src/libs/colmena/text/tools/data_run_python.yaml`
- Modify: `mod.rs` (add `pub mod data_run_python;`)

**Interfaces:**
- Consumes: `DataBinding`, `deserialize_bindings_flexible` (Task 1).
- Produces: `pub const TOOL_DATA_RUN_PYTHON: &str = "data_run_python";`; `pub struct DataRunPythonArgs { bindings: Vec<DataBinding>, code: String, write_to_spreadsheet: Option<String> }`; `pub fn tool_data_run_python(enabled: &EnabledSources) -> ToolDefinition` (dynamic description); `pub struct EnabledSources { sql: bool, gsheets: bool }`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_definition_lists_only_enabled_sources() {
        let def = tool_data_run_python(&EnabledSources { sql: true, gsheets: false });
        let desc = def.description.to_lowercase();
        assert!(desc.contains("sql") || desc.contains("database"));
        assert!(!desc.contains("google sheet"));
    }
    #[test]
    fn args_parse_minimal() {
        let a: DataRunPythonArgs = serde_json::from_value(serde_json::json!({
            "bindings":[{"var":"x","query":"SELECT 1"}], "code":"output=1"
        })).unwrap();
        assert_eq!(a.bindings.len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: FAIL — not found.

- [ ] **Step 3: Write minimal implementation**

Create `data_run_python.rs`. `DataRunPythonArgs` derives `Deserialize, JsonSchema`; `bindings` uses `#[serde(deserialize_with = "super::tabular_bindings::deserialize_bindings_flexible")]`; `write_to_spreadsheet` `#[serde(default)]`. `EnabledSources` plain struct. `tool_data_run_python`: build via `super::build_synthetic_tool_with_summary::<DataRunPythonArgs>(TOOL_DATA_RUN_PYTHON, description, summary)` where `description` is assembled at runtime: start from `text::tool_description(TOOL_DATA_RUN_PYTHON)` (the static base) and append a dynamically-built "Available sources:" list — attachment + inline always; add "Google Sheets" if `enabled.gsheets`; add "SQL database tables" if `enabled.sql`. Create `text/tools/data_run_python.yaml` with `description` (base, English, opens with the routing rule from spec §11) + `summary` (one line). Add `pub mod data_run_python;` to `mod.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: data_run_python tool args + dynamic-source description"
```

---

## Phase 2 — `output_tables` sink core (the heart; pure + integration)

### Task 3: WriteSpec parsing + normalization (pure)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/table_writer.rs`
- Modify: `mod.rs`

**Interfaces:**
- Produces: `pub enum WriteMode { Append, Update, Upsert, Replace }`; `pub struct TableWriteSpec { table: String, mode: WriteMode, records: Vec<Map<String,Value>>, key: Option<Vec<String>>, columns: Option<Vec<String>> }`; `pub fn parse_output_tables(value: &Value) -> Result<Vec<TableWriteSpec>, Value>` (accepts `{ "schema.tab": df-or-spec }`; bare array ⇒ `Append`; spec dict reads `mode`/`df`/`key`/`columns`; `key` accepts string or array). Structured `Err` JSON on malformed input.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn bare_dataframe_is_append() {
        let v = json!({"analytics.t": [{"a":1}]});
        let specs = parse_output_tables(&v).unwrap();
        assert_eq!(specs[0].mode, WriteMode::Append);
        assert_eq!(specs[0].table, "analytics.t");
        assert_eq!(specs[0].records.len(), 1);
    }
    #[test]
    fn spec_dict_reads_mode_and_key() {
        let v = json!({"t": {"mode":"upsert","df":[{"id":1,"x":2}],"key":"id"}});
        let s = &parse_output_tables(&v).unwrap()[0];
        assert_eq!(s.mode, WriteMode::Upsert);
        assert_eq!(s.key.as_ref().unwrap(), &vec!["id".to_string()]);
    }
    #[test]
    fn invalid_mode_errors() {
        let v = json!({"t": {"mode":"delete","df":[]}});
        assert!(parse_output_tables(&v).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib table_writer 2>&1 | tail -20`
Expected: FAIL — not found.

- [ ] **Step 3: Write minimal implementation**

Implement `WriteMode` (`Deserialize` with `#[serde(rename_all="lowercase")]`), `TableWriteSpec`, `parse_output_tables`. For each entry: if value is a JSON array ⇒ `{mode:Append, records:<array-as-records>}`; if object ⇒ read `df` (array, required; else `Err EmptyDataFrame/InvalidSpec`), `mode` (default `append`; unknown ⇒ `Err InvalidMode`), `key` (string→`vec![s]`, array→vec, else None), `columns` (optional array). Normalize records: array of objects kept; 2-D array (first row header) converted (reuse a small local `rectangle_to_records` or import the gsheets one). Add `pub mod table_writer;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib table_writer 2>&1 | tail -20`
Expected: PASS (3).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: parse output_tables write specs"
```

### Task 4: Pre-write validation (pure)

**Files:**
- Modify: `table_writer.rs`

**Interfaces:**
- Consumes: `TableWriteSpec`, `WriteMode`, `validate_table_against_allowlist` (sql_bulk), `SqlPermissions`.
- Produces: `pub fn validate_write_spec(spec: &TableWriteSpec, allowed_schemas: &[String], perms: &SqlPermissions, table_exists: bool, table_columns: Option<&[String]>) -> Result<(), Value>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn update_requires_key() {
    let spec = TableWriteSpec { table:"a.t".into(), mode:WriteMode::Update, records:vec![], key:None, columns:None };
    let perms = SqlPermissions::preset("read_write");
    let err = validate_write_spec(&spec, &["a".into()], &perms, true, Some(&["id".into()])).unwrap_err();
    assert_eq!(err["error"], "KeyColumnMissing");
}
#[test]
fn append_needs_insert_permission() {
    let spec = TableWriteSpec { table:"a.t".into(), mode:WriteMode::Append, records:vec![serde_json::Map::new()], key:None, columns:None };
    let ro = SqlPermissions::preset("read_only");
    assert_eq!(validate_write_spec(&spec, &["a".into()], &ro, true, Some(&[])).unwrap_err()["error"], "OperationNotPermitted");
}
#[test]
fn schema_outside_allowlist_rejected() {
    let spec = TableWriteSpec { table:"secret.t".into(), mode:WriteMode::Append, records:vec![serde_json::Map::new()], key:None, columns:None };
    let perms = SqlPermissions::preset("read_write");
    assert_eq!(validate_write_spec(&spec, &["a".into()], &perms, true, Some(&[])).unwrap_err()["error"], "SchemaNotAllowed");
}
```

(Confirm the real `SqlPermissions` constructor name; if it isn't `preset`, use the actual builder found in `sql/domain/sql_permissions.rs` and adjust the test accordingly.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib table_writer::tests::validate 2>&1 | tail -20` (or run the whole module)
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`validate_write_spec` checks in order (each returns the structured error from spec §10): schema ∈ allowed (`validate_table_against_allowlist`) → `SchemaNotAllowed`; permission for mode (Append/Replace→insert, Replace also delete, Update→update, Upsert→insert+update) using `SqlPermissions` accessors → `OperationNotPermitted`; if `!table_exists`: Update ⇒ `TableNotFound`; Append/Upsert/Replace require preset allows `create_table` else `TableNotFound` (auto-create gated); Update/Upsert require `key` present in records-columns AND (if exists) table_columns → `KeyColumnMissing`; non-empty records for Update/Upsert → `EmptyDataFrame`; duplicate keys in input → `DuplicateKeyInInput`; column names non-empty/unique → `InvalidColumnName`; if `table_exists` and not Replace-auto-create: df columns ⊆ table columns → `ColumnMismatch{df_only,...}`; row count ≤ 100_000 → `TooManyRows`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib table_writer 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: pre-write validation for output_tables"
```

### Task 5: DDL type inference (pure)

**Files:**
- Modify: `table_writer.rs`

**Interfaces:**
- Produces: `pub fn infer_create_ddl(schema: &str, table: &str, records: &[Map<String,Value>], key: Option<&[String]>) -> String`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn infers_types_and_unique_key() {
    let recs = vec![
        serde_json::from_value(serde_json::json!({"id":1,"price":9.5,"active":true,"name":"a"})).unwrap()
    ];
    let ddl = infer_create_ddl("analytics", "t", &recs, Some(&["id".into()]));
    assert!(ddl.contains("\"id\" BIGINT"));
    assert!(ddl.contains("\"price\" DOUBLE PRECISION"));
    assert!(ddl.contains("\"active\" BOOLEAN"));
    assert!(ddl.contains("\"name\" TEXT"));
    assert!(ddl.contains("UNIQUE (\"id\")"));
    assert!(ddl.starts_with("CREATE TABLE IF NOT EXISTS \"analytics\".\"t\""));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib table_writer::tests::infers 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

For each column (union of keys across records, first-seen order): scan all non-null values — all integers ⇒ `BIGINT`; any float & rest numeric ⇒ `DOUBLE PRECISION`; all bool ⇒ `BOOLEAN`; all parse as RFC3339/ISO-8601 ⇒ `TIMESTAMPTZ`; else `TEXT`. All-null ⇒ `TEXT`. Quote identifiers with `"`. Append `, UNIQUE ("k1","k2")` when `key` present. Return the `CREATE TABLE IF NOT EXISTS "schema"."table" (...)` string.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib table_writer 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: DDL type inference for auto-created tables"
```

### Task 6: SQL statement builders (pure)

**Files:**
- Modify: `table_writer.rs`

**Interfaces:**
- Produces: `pub fn build_insert_sql(schema,table, cols:&[String], row_chunk:&[&Map<String,Value>]) -> (String, Vec<Value>)`; `pub fn build_upsert_sql(schema,table, cols, key:&[String], chunk) -> (String, Vec<Value>)`; `pub fn build_update_sql_from_changes(schema,table, key:&[String], changes:&[CellChange]) -> Vec<(String, Vec<Value>)>`. All use `$1..$n` placeholders (never string-interpolate values).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn insert_uses_placeholders() {
    let mut r = serde_json::Map::new(); r.insert("id".into(), serde_json::json!(1)); r.insert("x".into(), serde_json::json!("a"));
    let (sql, params) = build_insert_sql("s","t", &["id".into(),"x".into()], &[&r]);
    assert!(sql.starts_with("INSERT INTO \"s\".\"t\" (\"id\",\"x\") VALUES ($1,$2)"));
    assert_eq!(params.len(), 2);
}
#[test]
fn upsert_has_on_conflict() {
    let mut r = serde_json::Map::new(); r.insert("id".into(), serde_json::json!(1)); r.insert("x".into(), serde_json::json!(2));
    let (sql, _) = build_upsert_sql("s","t", &["id".into(),"x".into()], &["id".into()], &[&r]);
    assert!(sql.contains("ON CONFLICT (\"id\") DO UPDATE SET \"x\"=EXCLUDED.\"x\""));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib table_writer 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`build_insert_sql`: `INSERT INTO "s"."t" (cols) VALUES ($1,$2),($3,$4)...`, flat params row-major. `build_upsert_sql`: same + `ON CONFLICT (keys) DO UPDATE SET "c"=EXCLUDED."c"` for every non-key col. `build_update_sql_from_changes`: group `changes` by `key_value`, per group emit `UPDATE "s"."t" SET "c1"=$1,"c2"=$2 WHERE "k"=$N` (single-key path; for composite keys, `WHERE "k1"=.. AND "k2"=..` — but changes carry a single `key_value` string, so composite-key update falls back to per-row full-record UPDATE — document this limitation). Values as `Vec<Value>` (bound later via a `Value`→sqlx binder helper).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib table_writer 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: parameterized INSERT/UPSERT/UPDATE builders"
```

### Task 7: Transactional executor for output_tables (integration)

**Files:**
- Modify: `table_writer.rs`

**Interfaces:**
- Consumes: all Task 3-6 fns; `sqlx::PgPool`.
- Produces: `pub async fn write_output_tables(pool: &sqlx::PgPool, specs: Vec<TableWriteSpec>, allowed_schemas: &[String], perms: &SqlPermissions, tenant_user_id: Option<&str>, loaded_snapshots: &HashMap<String, Vec<Map<String,Value>>>) -> Value` — returns `{ "wrote_tables": [ {table, mode, rows_affected, created, changes?}... ] }` or a structured error (rolls back on first failure). `loaded_snapshots` keyed by qualified table name enables diff-driven `Update`.

- [ ] **Step 1: Write the failing test** (integration, real Postgres)

```rust
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
async fn append_autocreates_and_inserts() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new().connect(&url).await.unwrap();
    sqlx::query("DROP TABLE IF EXISTS drp_test.people").execute(&pool).await.ok();
    sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test").execute(&pool).await.unwrap();
    let specs = parse_output_tables(&serde_json::json!({
        "drp_test.people": [{"id":1,"name":"Ana"},{"id":2,"name":"Bo"}]
    })).unwrap();
    let perms = SqlPermissions::preset("full"); // allows create_table
    let out = write_output_tables(&pool, specs, &["drp_test".into()], &perms, None, &Default::default()).await;
    assert_eq!(out["wrote_tables"][0]["created"], true);
    assert_eq!(out["wrote_tables"][0]["rows_affected"], 2);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.people").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `TEST_DATABASE_URL=$DATABASE_URL cargo test --lib table_writer -- --ignored append_autocreates 2>&1 | tail -30`
Expected: FAIL — `write_output_tables` not found.

- [ ] **Step 3: Write minimal implementation**

`write_output_tables`: open `pool.begin()`; `SET LOCAL statement_timeout`/`work_mem` (constants ok for v1); if `tenant_user_id` set, `SELECT set_config('app.current_user_id', $1, true)`. For each spec: introspect existence + columns via `information_schema.columns` (one query per table); call `validate_write_spec`; if missing & auto-create allowed, run `infer_create_ddl` then execute it (record `created_ddl`, `created:true`); dispatch by mode — Append: chunk 1000, `build_insert_sql`, bind `Value`s (helper `bind_json_value(query, &Value)` mapping null/bool/i64/f64/string/else→text); Upsert: chunk 1000 `build_upsert_sql`; Replace: `DELETE FROM` (gated by `on_existing_table` earlier) then insert; Update: if `loaded_snapshots` has the table, `diff_records(snapshot, records, key0, columns, false, table)` → `build_update_sql_from_changes` (0 changes ⇒ skip, `rows_affected:0`); else per-row full UPDATE. Sum `rows_affected`. On any `Err`, `tx.rollback()` and return the structured error with the offending table. On success `tx.commit()` and return the `wrote_tables` array. Map any `sqlx` unique-violation on upsert to `UpsertKeyNotUnique`.

- [ ] **Step 4: Run test to verify it passes**

Run: `TEST_DATABASE_URL=$DATABASE_URL cargo test --lib table_writer -- --ignored 2>&1 | tail -30`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: transactional output_tables writer (append/update/upsert/replace)"
```

### Task 8: Upsert + update-diff + rollback integration coverage

**Files:**
- Modify: `table_writer.rs` (tests only)

- [ ] **Step 1: Write the failing tests**

Add `#[ignore]` integration tests: (a) `upsert_inserts_then_updates` — seed table w/ UNIQUE(id), upsert overlapping+new rows, assert final state; (b) `update_diff_only_changed_cells` — provide `loaded_snapshots`, change one cell, assert response `changes.cells==1` and untouched row unchanged; (c) `rollback_on_second_table_failure` — two specs where the 2nd violates a NOT NULL; assert the 1st table has zero rows after (full rollback); (d) `upsert_without_unique_constraint_errors` — assert `UpsertKeyNotUnique`.

- [ ] **Step 2: Run to verify they fail**

Run: `TEST_DATABASE_URL=$DATABASE_URL cargo test --lib table_writer -- --ignored 2>&1 | tail -30`
Expected: FAIL (behaviors not yet all correct) — fix `write_output_tables` until green.

- [ ] **Step 3: Fix implementation as needed** (make each pass; e.g. ensure `SET LOCAL` chunk sizing, unique-violation mapping, diff path).

- [ ] **Step 4: Run to verify pass**

Run: `TEST_DATABASE_URL=$DATABASE_URL cargo test --lib table_writer -- --ignored 2>&1 | tail -30`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "test: upsert, update-diff, and rollback coverage for output_tables"
```

---

## Phase 3 — Binding resolution + sandbox execution

### Task 9: Resolve bindings into sandbox inputs (SQL + inline, pure-ish)

**Files:**
- Modify: `tabular_bindings.rs`

**Interfaces:**
- Consumes: `SqlConnectionPort`, `SheetsClient`, executor attachment streaming (passed as closures/handles).
- Produces: `pub struct ResolvedBindings { inputs: serde_json::Map<String,Value>, loaded_columns: Value, sql_snapshots: HashMap<String, Vec<Map<String,Value>>> }`; `pub async fn resolve_bindings(bindings:&[DataBinding], sql: Option<&SqlBindingCtx>, sheets: Option<&Arc<dyn SheetsClient>>, attach: &AttachmentFetcher) -> Result<ResolvedBindings, Value>` where `SqlBindingCtx { pool_adapter: Arc<PgPoolAdapter>, permissions: SqlPermissions, allowed_schemas: Vec<String>, tenant: Option<String> }` and `AttachmentFetcher` is a boxed async fn `attachment_id -> Result<(bytes, mime_or_name), String>`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn inline_binding_resolves_to_records() {
    let b = vec![serde_json::from_value(serde_json::json!({"var":"t","data":[{"a":1},{"a":2}]})).unwrap()];
    // sql=None, sheets=None; attach fetcher never called
    let noop: AttachmentFetcher = Box::new(|_id| Box::pin(async { Err("no attach".to_string()) }));
    let r = resolve_bindings(&b, None, None, &noop).await.unwrap();
    assert_eq!(r.inputs.get("t").unwrap().as_array().unwrap().len(), 2);
    assert!(r.loaded_columns.get("t").is_some());
}
#[tokio::test]
async fn sql_binding_without_ctx_errors_source_not_enabled() {
    let b = vec![serde_json::from_value(serde_json::json!({"var":"t","query":"SELECT 1"})).unwrap()];
    let noop: AttachmentFetcher = Box::new(|_id| Box::pin(async { Err("x".to_string()) }));
    let e = resolve_bindings(&b, None, None, &noop).await.unwrap_err();
    assert_eq!(e["error"], "SourceNotEnabled");
    assert_eq!(e["source"], "sql");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib tabular_bindings 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`resolve_bindings`: classify each; INLINE ⇒ normalize array→records; ATTACHMENT ⇒ call `attach(id)`, then `parse_attachment_to_records`; GSHEETS ⇒ require `sheets` Some (else `SourceNotEnabled{source:"gsheets"}`), fetch via `client.read_range` with `as_records`; SQL ⇒ require `sql` Some (else `SourceNotEnabled{source:"sql"}`), AST-validate SELECT-only (`sqlparser`: parse, exactly one stmt, `Statement::Query`; else `BindingMustBeSelect`), run `StaticRuleValidator::validate` against `permissions`, then `pool_adapter.execute_query(query, MAX_BULK_INSERT_ROWS+1, tenant)`; if `row_count > 100_000` ⇒ `BindingTooLarge`; convert `QueryResult.output` (rows array) to records + retain in `sql_snapshots` keyed by the table if the query is a plain `SELECT * FROM t` (best-effort; else skip snapshot). Fetch SQL + gsheets + attachment concurrently via `join_all` over per-binding futures. Fill `inputs[var]`, `loaded_columns[var]`, plus `_loaded_columns` aggregate. Any binding error returns immediately with `binding: <var>` attached.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib tabular_bindings 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: resolve polymorphic bindings into sandbox inputs"
```

### Task 10: Prelude/postlude + sandbox wrap

**Files:**
- Create: `src/libs/colmena/text/prompts/python_sandbox/data_run_python_prelude.md`
- Create: `src/libs/colmena/text/prompts/python_sandbox/data_run_python_postlude.md`
- Modify: `data_run_python.rs`

**Interfaces:**
- Produces: `fn wrap_user_code(code: &str) -> String` (private); postlude packages `{user_output, output_sheets, output_tables, output_attachments}`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn wrapped_code_packages_all_sinks() {
    let w = wrap_user_code("output = 1");
    assert!(w.contains("output_tables"));
    assert!(w.contains("output_sheets"));
    assert!(w.contains("output_attachments"));
    assert!(w.contains("user_output"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

Prelude (`include_str!`): imports `pandas as pd, numpy as np, scipy.stats as stats`, sets `output=None; output_tables={}; output_sheets={}; output_attachments={}`. Postlude: coerce DataFrames in each `output_*` dict to records (`to_dict('records')`), build the wrapped `{"user_output":output, "output_sheets":..., "output_tables":..., "output_attachments":...}` as the helper's returned global (mirror gsheets postlude `wrap_user_code` at `gsheets_run_python.rs` — reuse its DataFrame-coercion helper text). `wrap_user_code` = prelude + "\n" + user code + "\n" + postlude.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: data_run_python sandbox prelude/postlude with three sinks"
```

---

## Phase 4 — Sheet + attachment sinks

### Task 11: Extract `sheet_writer` from gsheets_run_python (refactor, no behavior change)

**Files:**
- Create: `sheet_writer.rs`
- Modify: `gsheets_run_python.rs`, `mod.rs`

**Interfaces:**
- Produces: `pub async fn write_output_sheets(client:&Arc<dyn SheetsClient>, spreadsheet_id:&SpreadsheetId, sheets_value:&Value, policy:CollisionPolicy, loaded_snapshots:&HashMap<String,LoadedSnapshot>) -> Vec<Value>`; `pub struct LoadedSnapshot {...}` moved here.

- [ ] **Step 1: Baseline test stays green**

Run the existing gsheets tests first to capture green baseline:
Run: `cargo test --lib gsheets_run_python 2>&1 | tail -20`
Expected: PASS (baseline).

- [ ] **Step 2: Move code**

Cut `write_output_sheets`, `LoadedSnapshot`, and their private helpers from `gsheets_run_python.rs` into `sheet_writer.rs`; make them `pub`; add `pub mod sheet_writer;`. In `gsheets_run_python.rs` replace with `use super::sheet_writer::{write_output_sheets, LoadedSnapshot};`.

- [ ] **Step 3: Run tests to verify unchanged**

Run: `cargo test --lib gsheets_run_python 2>&1 | tail -20` and `cargo test --lib sheet_writer 2>&1 | tail -20`
Expected: PASS (same count as baseline).

- [ ] **Step 4: Verify no warnings**

Run: `cargo build 2>&1 | tail -20`
Expected: no warnings (deny-warnings would fail otherwise).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor: extract sheet_writer module (no behavior change)"
```

### Task 12: `output_attachments` sink

**Files:**
- Create: `attachment_writer.rs`
- Modify: `mod.rs`

**Interfaces:**
- Produces: `pub fn serialize_records(records:&[Map<String,Value>], fmt:&str, delimiter:Option<&str>) -> Result<Vec<u8>, String>` (fmt `csv`|`xlsx`); `pub async fn write_output_attachments(value:&Value, register: &AttachmentRegistrar) -> Value` where `AttachmentRegistrar` is a boxed async fn `(name, bytes) -> Result<String /*document_id*/, String>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn serialize_csv_has_header_and_rows() {
    let recs: Vec<serde_json::Map<String,serde_json::Value>> = vec![
        serde_json::from_value(serde_json::json!({"a":1,"b":"x"})).unwrap()
    ];
    let bytes = serialize_records(&recs, "csv", None).unwrap();
    let s = String::from_utf8(bytes).unwrap();
    assert!(s.starts_with("a,b"));
    assert!(s.contains("1,x"));
}
#[test]
fn serialize_xlsx_is_nonempty_zip() {
    let recs: Vec<serde_json::Map<String,serde_json::Value>> = vec![serde_json::from_value(serde_json::json!({"a":1})).unwrap()];
    let bytes = serialize_records(&recs, "xlsx", None).unwrap();
    assert_eq!(&bytes[0..2], b"PK"); // xlsx is a zip
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib attachment_writer 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`serialize_records`: column union first-seen order; `csv` via manual writer or `csv` crate (UTF-8, delimiter default `,`, quote values containing delimiter/newline); `xlsx` via `rust_xlsxwriter` (`Workbook`, one sheet, header row + body; write numbers as numbers, bool/text as text). `write_output_attachments`: for each entry pick fmt from filename extension (`.csv`/`.xlsx`; else `InvalidFormat`), read spec (bare df or `{df, delimiter}`), enforce ≤100k rows / ≤50MB, `serialize_records`, call `register(name, bytes)`, collect `{name, document_id, rows, bytes}`. Add `pub mod attachment_writer;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib attachment_writer 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: output_attachments sink (csv/xlsx export to catalog)"
```

---

## Phase 5 — Dispatcher, gating, executor wiring

### Task 13: fixed_config parsing + EnabledSources derivation

**Files:**
- Modify: `data_run_python.rs`

**Interfaces:**
- Produces: `pub struct SqlSinkConfig { connection_url:String, permissions:SqlPermissions, allowed_schemas:Vec<String>, statement_timeout_ms:u64, work_mem_mb:u64, on_missing_table:String, on_existing_table:String, tenant_user_id:Option<String> }`; `pub fn parse_sql_config(fixed:&HashMap<String,Value>) -> Result<Option<SqlSinkConfig>, String>` (None when no `sql` block; Err when present-but-invalid — missing `connection_url` or `permissions.allowed_schemas`); `pub fn enabled_sources(fixed:&HashMap<String,Value>, agent_has_gsheets:bool) -> EnabledSources`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn sql_config_absent_is_none() {
    let f = std::collections::HashMap::new();
    assert!(parse_sql_config(&f).unwrap().is_none());
}
#[test]
fn sql_config_missing_allowed_schemas_errors() {
    let mut f = std::collections::HashMap::new();
    f.insert("sql".into(), serde_json::json!({"connection_url":"postgres://x"}));
    assert!(parse_sql_config(&f).is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`parse_sql_config`: read `fixed["sql"]`; None if absent. Resolve `connection_url` via existing env-resolver (`resolve_env_vars`, same helper sql_bulk uses); deserialize `permissions` into `SqlPermissions`; require non-empty `allowed_schemas`; defaults `on_missing_table="create"`, `on_existing_table="fail"`, timeouts 30000/64. `enabled_sources`: `sql = fixed.contains_key("sql")`; `gsheets = agent_has_gsheets || fixed.get("enable_gsheets")==Some(true)`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: parse data_run_python sql fixed_config + source gating"
```

### Task 14: Dispatcher (orchestration) + executor routing

**Files:**
- Modify: `data_run_python.rs`, `dag_tool_executor.rs`, `mod.rs` (re-export)

**Interfaces:**
- Consumes: `resolve_bindings`, `write_output_tables`, `write_output_sheets`, `write_output_attachments`, `parse_sql_config`, `SqlPortFactory` (or short-lived pool), `fetch_attachment_stream`.
- Produces: `pub async fn dispatch_data_run_python_via_executor(exec:&DagToolExecutor, tool_call:&ToolCall, fixed:&HashMap<String,Value>) -> Value`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn dispatch_rejects_unconfigured_sql_source() {
    // fixed_config has no `sql` block; a binding asks for query → SourceNotEnabled
    let args = serde_json::json!({"bindings":[{"var":"t","query":"SELECT 1"}], "code":"output=1"});
    let out = dispatch_core(args, &EnabledSources{sql:false, gsheets:false}, None, None, &noop_attach()).await;
    assert_eq!(out["error"], "SourceNotEnabled");
}
```

(Introduce a testable inner `dispatch_core(args, enabled, sql_ctx, sheets, attach)` that the executor wrapper calls, so the orchestration is unit-testable without a full executor.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`dispatch_core`: parse args (`invalid_args` on error); `validate_bindings`; build `SqlBindingCtx` from `sql_ctx` if present; `resolve_bindings(...)`; `wrap_user_code`; run via `tokio::time::timeout(30s, spawn_blocking(execute_sandboxed_helper(...)))`; extract `user_output` + the three sink dicts from the wrapped output; if `output_tables` non-empty ⇒ require `sql_ctx` (else `SourceNotEnabled{source:"sql"}`) then `write_output_tables(...)`; if `output_sheets` non-empty ⇒ require `write_to_spreadsheet` + gsheets enabled then `write_output_sheets(...)`; if `output_attachments` non-empty ⇒ `write_output_attachments(...)`; assemble `{output, stdout, error, wrote_tables, wrote_sheets, wrote_attachments, _warning?}` with 10KB caps. The executor wrapper `dispatch_data_run_python_via_executor`: `parse_sql_config`; if Some, obtain a pool (via `SqlPortFactory` if the executor exposes one, else `build_short_lived_pool`) and build `SqlBindingCtx`; build the `AttachmentFetcher` closure over `exec.fetch_attachment_stream`; build the `AttachmentRegistrar` over the executor's storage-write path (same used by `gsheets_export_xlsx`); determine gsheets client via `GoogleSheetsHttpClient::from_config(from_env())` when enabled; call `dispatch_core`. Route in `dag_tool_executor.rs` near `:1030`: `if tool_call.function.name == TOOL_DATA_RUN_PYTHON { let fixed = self.tool_configurations.get(TOOL_DATA_RUN_PYTHON).map(|tc| tc.fixed_config.clone()).unwrap_or_default(); return dispatch_data_run_python_via_executor(self, tool_call, &fixed).await; }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib data_run_python 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: data_run_python dispatcher + executor routing"
```

### Task 15: Tool activation in llm.rs + build definition

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

**Interfaces:**
- Consumes: `tool_data_run_python`, `enabled_sources`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn data_run_python_activates_from_tool_configurations() {
    // Build a minimal llm_call config with tool_configurations.data_run_python and
    // assert the assembled tool list contains "data_run_python" with sql source listed.
    // (Mirror the existing attachment_run_python activation test around llm.rs:2489.)
}
```

Fill this with the concrete assertion following the pattern of the nearest existing tool-activation test in `llm.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib llm 2>&1 | grep -i data_run_python | tail -20`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

In the tool-assembly path (near `:2489` where `attachment_run_python` is opted-in by name), add: when `tool_configurations` (or `enabled_tools`) contains `data_run_python`, compute `agent_has_gsheets` (reuse the existing detection used for gsheets tools) + read the tool's `fixed_config`, call `enabled_sources(...)`, and push `tool_data_run_python(&enabled)`. Ensure `TOOL_DATA_RUN_PYTHON` is registered wherever the synthetic-tool name allowlist lives (mirror `attachment_run_python`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib llm 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: activate data_run_python tool from configuration"
```

---

## Phase 6 — E2E, skill, docs

### Task 16: E2E graphs

**Files:**
- Create: `tests/graphs/agents/data_run_python_xlsx_to_sql.json`
- Create: `tests/graphs/agents/data_run_python_sql_to_xlsx.json`
- Create: `tests/graphs/agents/data_run_python_sheet_sync.json`

- [ ] **Step 1: Author the graphs**

Each: an `llm_call` (default stack `google`/`gemini-2.5-flash`, `${DATABASE_URL}`) with `tool_configurations.data_run_python` carrying a real `fixed_config.sql` block (`connection_url: "${DATABASE_URL}"`, `permissions.preset: "full"`, `allowed_schemas`). Use realistic user-voice prompts (per repo rule): e.g. "acá está la lista de precios actualizada, impactala en la base de datos". `xlsx_to_sql` binds a real attachment + a SELECT of the target table + upsert. `sql_to_xlsx` binds a SELECT + `output_attachments`. `sheet_sync` binds a Google Sheet + a SELECT + writes `output_tables` and `output_sheets`. Metadata `requires_env`.

- [ ] **Step 2: Run xlsx_to_sql live**

```bash
set -a; source .env; set +a
mkdir -p /tmp/colmena_e2e
cargo run --release --bin dag_engine -- run tests/graphs/agents/data_run_python_xlsx_to_sql.json \
  --agent-session-id e2e_drp_$(date +%s) > /tmp/colmena_e2e/data_run_python_xlsx_to_sql.sse 2>&1
```
Expected: SSE shows a `data_run_python` tool call whose result has `wrote_tables` with `rows_affected > 0`; verify the row count in Postgres directly.

- [ ] **Step 3: Run the other two live** (sql_to_xlsx, sheet_sync) the same way; save SSE to `/tmp/colmena_e2e/`. For `sheet_sync` follow the local gsheets E2E runbook (Secret Manager creds; unset `COLMENA_LOCAL`).

- [ ] **Step 4: Present friendly reports** (input, key payload, tokens, summary) per repo rule; do not paste full SSE.

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/agents/data_run_python_*.json
git commit -m "test: E2E graphs for data_run_python (xlsx→sql, sql→xlsx, sheet sync)"
```

### Task 17: Recipes skill

**Files:**
- Create: `src/libs/colmena/skills/data-run-python-recipes/SKILL.md`
- Create: `src/libs/colmena/skills/data-run-python-recipes/references/*.md`

- [ ] **Step 1: Write SKILL.md** with frontmatter (`name: data-run-python-recipes`) and the 4 canonical recipes from spec §12 (spreadsheet→upsert; db→xlsx; cross CSV-vs-table; gsheet↔table sync). References: one file per recipe. Mirror `sql-query-best-practices` structure.

- [ ] **Step 2: Verify skill loads** — add/extend a unit test that the skill is discoverable via the built-in skills index (mirror the test for `sql-query-best-practices`).

Run: `cargo test --lib skills 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "docs: data-run-python-recipes built-in skill"
```

### Task 18: Developer-guide docs

**Files:**
- Create: `docs/developer_guide/48_data_run_python.md`
- Modify: `docs/developer_guide/41_builtin_tools_index.md`, `23_sql_node.md` (choice matrix), `39_gsheets.md`, `43_sheets_local_vs_gsheets.md`

- [ ] **Step 1: Write the guide** (Spanish): sources, sinks, `output_tables` modes, gating, fixed_config example, the 3 E2E graphs, choice-matrix update (transform/cross → `data_run_python`; raw 1:1 dump → `sql_bulk_insert_from_attachment`).

- [ ] **Step 2: Cross-link** from the four modified docs + `DEVELOPER_GUIDE.md` index.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "docs: developer guide for data_run_python + choice-matrix updates"
```

### Task 19: Full verification gate

- [ ] **Step 1: Unit + doctest sweep**

Run: `cargo test --verbose 2>&1 | tail -40`
Expected: PASS (no failures; note ignored integration tests).

- [ ] **Step 2: Integration sweep**

Run: `source .env; TEST_DATABASE_URL=$DATABASE_URL cargo test -- --ignored 2>&1 | tail -40`
Expected: PASS.

- [ ] **Step 3: Clippy + fmt**

Run: `cargo clippy --all-targets 2>&1 | tail -20 && cargo fmt --check`
Expected: clean.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A && git commit -m "chore: clippy/fmt cleanup for data_run_python"
```

---

## Phase 7 — Deprecation (GATED — only after Phase 6 verified + ADP sweep)

> Do NOT start until all Phase 6 E2E graphs pass against live services AND the ADP worker sweep (§below) is done. This is a breaking change.

### Task 20: ADP sweep + migrate in-repo graphs

- [ ] **Step 1: Sweep ADP** — in `/Users/danielgarcia/startti/adp`, grep `apps/service/ia/platform/{worker,api}/src/` and any agent graph JSON for `gsheets_run_python` / `attachment_run_python`. List every usage. (Confine ALL ADP changes to `apps/service/ia/platform/` per repo rule.)

- [ ] **Step 2: Migrate in-repo graphs** — replace `gsheets_run_python`/`attachment_run_python` usages under `tests/graphs/` with `data_run_python` per the §15.3 equivalence table.

- [ ] **Step 3: Run migrated graphs** to confirm parity; save SSE.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor: migrate in-repo graphs to data_run_python"
```

### Task 21: Delete the two redundant tools

**Files:**
- Delete: `gsheets_run_python.rs` (keep `sheet_writer.rs`), `attachment_run_python.rs`
- Modify: `mod.rs`, `dag_tool_executor.rs`, `llm.rs`, `text/tools/*`, registry/allowlists, docs

- [ ] **Step 1: Remove dispatchers, builders, routing, text entries, `enabled_tools` allowlist entries, and tests for both tools.** Keep `sheet_writer` (now consumed by `data_run_python`). Keep `sql_bulk_*` and `crdt_doc_run_python`.

- [ ] **Step 2: Update docs** (`23`, `39`, `41`, `43`) to drop the deleted tools and point to `data_run_python`.

- [ ] **Step 3: Full verification**

Run: `cargo test --verbose 2>&1 | tail -40 && cargo clippy --all-targets 2>&1 | tail -10`
Expected: PASS/clean (no dangling refs — deny-warnings enforces this).

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor: remove redundant gsheets_run_python and attachment_run_python (subsumed by data_run_python)"
```

---

## Self-Review notes (addressed)

- **Spec coverage:** bindings (T1,T9), gating (T2,T13,T14), output_tables all modes + auto-create + validations (T3-T8), output_sheets reuse (T11), output_attachments (T12), sandbox (T10), dispatcher/routing/activation (T14,T15), errors §10 (T3,T4,T7), disambiguation/skill/docs (T17,T18), deprecation §15 (T20,T21). Multi-tenant/RLS `SET LOCAL` (T7,T9). Perf chunking/COPY (T7 — COPY >5000 rows path may be deferred to a follow-up if `build_insert_sql` chunking meets E2E targets; note in T7 if so).
- **Type consistency:** `write_output_tables`/`write_output_sheets`/`write_output_attachments`, `resolve_bindings`→`ResolvedBindings`, `TableWriteSpec`/`WriteMode`, `EnabledSources`, `SqlSinkConfig`/`SqlBindingCtx`, `AttachmentFetcher`/`AttachmentRegistrar` used consistently across tasks.
- **Open confirmations for the implementer (verify against tree, don't guess):** exact `parse_attachment_to_records` arg list; `SqlPermissions` constructor + permission-accessor names; whether `DagToolExecutor` exposes a `SqlPortFactory` (else use `build_short_lived_pool` made `pub`); the executor's attachment-registration path used by `gsheets_export_xlsx`.
