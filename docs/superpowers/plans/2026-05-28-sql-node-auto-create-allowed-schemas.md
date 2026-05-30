# Plan — SQL node: auto-create missing `allowed_schemas` at init

**Date:** 2026-05-28
**Branch:** `feature/create_schemas_if_not_present`
**Status:** Implemented & verified (2026-05-28)

## Goal

When a `sql_query` node (standalone or as an LLM tool) is configured with
`permissions.allowed_schemas`, ensure each listed schema **exists** in the target
database at node initialization. Schemas already present are left untouched;
missing ones are created. The list is checked **one schema at a time**, and the
ones that don't exist are created.

This is **operator-driven** provisioning: the schema names come from the node's
fixed config, not from the LLM. The existing rule that blocks `CREATE SCHEMA`
*issued by the LLM* in a query stays exactly as-is.

## Decisions (confirmed)

- **Activation:** opt-in flag `create_schemas_if_missing` on the `permissions`
  object, **default `true`**. Operators can set it to `false` to restore the old
  "validate-only" behavior.
- **On failure:** **hard-fail init.** If a missing schema cannot be created
  (e.g. the DB role lacks `CREATE` privilege), node initialization returns an
  error and the node does not start. This propagates naturally because
  `do_initialize_inner` returns `Result`.
- **Check-then-create:** for each schema, first check existence; only run
  `CREATE SCHEMA` for genuinely missing ones. This keeps default-`true` safe —
  a read-only agent pointing at schemas that already exist never issues a
  privilege-requiring `CREATE` and therefore never hard-fails.

## Current behavior (baseline)

- `allowed_schemas` is purely a validation allowlist
  (`SqlPermissions::is_schema_allowed`, `sql_static_validator.rs`). Empty = all.
- Only the `sandbox` schema is auto-created today, via
  `PgRegistryAdapter::ensure_schema()` → `CREATE SCHEMA IF NOT EXISTS`, called in
  `SqlNode::do_initialize_inner` (`sql.rs:107`).
- `allowed_schemas` are only read for table-metadata introspection (`sql.rs:111`).
- LLM-issued `CREATE SCHEMA` is explicitly blocked (`sql_static_validator.rs`
  `stmt_kind_name`, docs `23_sql_node.md:397`). **Unchanged by this work.**

## Implementation steps

### 1. Domain — `SqlPermissions` (`domain/sql_permissions.rs`)

- Add field `create_schemas_if_missing: bool` to the struct.
- Parse it in `from_config`: `config.get("create_schemas_if_missing").and_then(as_bool).unwrap_or(true)`.
- In the `None`-config branch, default to `true` as well (consistent with the
  opt-in-default-on decision).
- Add accessor `pub fn create_schemas_if_missing(&self) -> bool`.
- Add a helper to expose the schema list for provisioning, e.g.
  `pub fn allowed_schemas_iter(&self) -> impl Iterator<Item = &str>` (or a
  `&HashSet<String>` getter). Today `allowed_schemas` is private; the node reads
  the raw JSON instead. Prefer exposing it through the domain type for cleanliness.
- Unit tests: default is `true` when key absent (both `Some(config)` and `None`),
  `false` when explicitly set, parsing alongside other permission fields.

### 2. Port — `SqlConnectionPort` (`domain/sql_ports.rs`)

Add two async methods (or one combined). Recommended split for testability:

```rust
/// Return the subset of `schemas` that do not yet exist in the database.
async fn missing_schemas(&self, schemas: &[String]) -> Result<Vec<String>, SqlNodeError>;

/// Create a schema (idempotent: CREATE SCHEMA IF NOT EXISTS, quoted identifier).
async fn create_schema(&self, schema: &str) -> Result<(), SqlNodeError>;
```

Rationale for two methods: lets init log "one by one" and only attempt creation
for the genuinely-missing set, so existing-schema cases never need `CREATE` priv.

### 3. Adapter — `PgPoolAdapter` (`infrastructure/sql_pool_adapter.rs`)

- Implement `missing_schemas`: query
  `SELECT schema_name FROM information_schema.schemata WHERE schema_name = ANY($1)`
  (bind the slice), diff against the input to find absent ones. Skip
  `information_schema` / `pg_catalog` (never create introspection schemas).
- Implement `create_schema`: `CREATE SCHEMA IF NOT EXISTS "<quoted>"` using the
  existing `Self::quote_ident` helper to prevent injection on the identifier.
- Map errors to `SqlNodeError::ExecutionError` with a clear message naming the
  schema and hinting at a missing `CREATE` privilege.

### 4. Node init — `SqlNode::do_initialize_inner` (`infrastructure/nodes/sql.rs`)

Insert after the adapter is acquired (`sql.rs:98`) and before/around the sandbox
`ensure_schema` step (`sql.rs:105`):

```rust
if permissions.create_schemas_if_missing() {
    let listed: Vec<String> = permissions.allowed_schemas_iter().map(str::to_string).collect();
    if !listed.is_empty() {
        let conn: &dyn SqlConnectionPort = adapter.as_ref();
        let missing = conn.missing_schemas(&listed).await
            .map_err(|e| format!("Failed to check existing schemas: {}", e))?;
        for schema in &missing {
            println!("[SqlNode] allowed_schema '{}' missing — creating", schema);
            conn.create_schema(schema).await
                .map_err(|e| format!("Failed to create schema '{}': {}", schema, e))?; // hard-fail
        }
    }
}
```

Notes:
- This runs once per node instance (`OnceCell`), like the rest of init.
- Hard-fail: returning `Err` from `do_initialize_inner` fails `get_or_init`, which
  the `execute` path surfaces as `SqlNode initialization failed: ...`.
- Order: provisioning `allowed_schemas` first means a `sandbox_schema` that is
  also listed in `allowed_schemas` is created here; the subsequent
  `ensure_schema()` (CREATE SCHEMA IF NOT EXISTS) is then a no-op — harmless.

### 5. Tests

- **Unit (permissions):** default-true parsing + explicit-false (step 1).
- **Integration (`#[ignore]`, requires `DATABASE_URL`):**
  - Configure a node with `allowed_schemas: ["colmena_test_new_schema"]` and
    `create_schemas_if_missing: true`; init; assert the schema now exists; drop
    it in teardown.
  - `create_schemas_if_missing: false` → schema is NOT created.
  - Mark `#[ignore = "requires DATABASE_URL — run with \`cargo test -- --ignored\`"]`
    per the repo CI convention.
- Run `cargo test --verbose` (catches doctests) and `cargo clippy` before pushing
  (deny-warnings is on).

### 6. Docs

- `docs/developer_guide/23_sql_node.md`:
  - Add `create_schemas_if_missing` row to the Permissions Object table
    (default `true`).
  - Add a short subsection under "Initialization and Schema Introspection"
    describing operator-driven schema provisioning + hard-fail-on-error, and
    explicitly contrasting it with the still-blocked LLM `CREATE SCHEMA`.
- `docs/node_configurations.json`: add `create_schemas_if_missing` to the
  `permissions` field block (near `allowed_schemas`, ~line 1008), `default: true`.
- `docs/node_as_tools_reference.json`: add the field to the SQL tool permissions
  example block (~line 368, next to `auto_rls`).

### 7. Test graph (optional, validates end-to-end)

Add `tests/graphs/agents/sql_create_schema.json`: an `llm_call` with a
`sql_query` tool whose `permissions.allowed_schemas` includes a fresh schema and
`create_schemas_if_missing: true`. Run with `--agent-session-id` per repo rules.
Uses a real registered `sql_query` node (no mock) per the JSON-graph rule.

## Risk / breaking-change sweep

- **Default-true changes behavior** for existing SQL nodes that list
  `allowed_schemas`. Mitigated by check-then-create: existing schemas are never
  re-created, so no `CREATE` privilege is required for the common case. Only a
  configured-but-missing schema + insufficient privilege will now hard-fail
  (previously it failed later, at query time, with a less clear error).
- **ADP worker sweep:** this does not change colmena's public Rust API
  (`EngineConfig`, `ColmenaEngine`, exported trait *signatures* used by ADP) —
  only adds methods to an internal port trait and a permissions field. Confirm
  no ADP code implements `SqlConnectionPort` directly before pushing to develop
  (per breaking-change discipline in CLAUDE.md). Adding a method to a trait IS a
  breaking change for any external impls; verify none exist in
  `apps/service/ia/platform/{worker,api}/src/`.

## Out of scope

- Letting the LLM create schemas via query text (stays blocked).
- Dropping/altering schemas, or reconciling schema contents.
- Auto-adding created schemas to the sandbox/function-registry lifecycle beyond
  what already happens for `sandbox_schema`.
```
