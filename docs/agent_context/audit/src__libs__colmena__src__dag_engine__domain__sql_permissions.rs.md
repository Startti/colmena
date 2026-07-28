# src/libs/colmena/src/dag_engine/domain/sql_permissions.rs

**Layer:** domain  **Purpose:** Defines the granular permission model for SQL node operations with presets (read_only, read_write, read_write_delete, full) and optional deny lists; enforces principle of least privilege and separates introspection schema access.

## Symbols

- `SqlOperation` (enum, pub) — SQL operations that can be allowed or denied: Select, Insert, Update, Delete, CreateFunction, CreateTable, AddColumn, Truncate, Drop, Alter.
- `SqlOperation::from_str_loose` (fn, pub) — Parses an operation name from a string (case-insensitive) for deny list processing.
- `PermissionPreset` (enum, pub) — Four permission levels: ReadOnly, ReadWrite, ReadWriteDelete, Full.
- `PermissionPreset::from_str` (fn, private) — Parses a preset name from lowercase string; returns error on unknown preset.
- `PermissionPreset::allowed_operations` (fn, private) — Builds HashSet of operations enabled by each preset.
- `SqlPermissions` (struct, pub) — Resolved permissions instance: holds allowed_ops, allowed_schemas, sandbox_schema, tenant_user_id, tenant_column, auto_rls, create_schemas_if_missing.
- `INTROSPECTION_SCHEMAS` (const, private) — Array `["information_schema", "pg_catalog"]` always accessible regardless of allowed_schemas.
- `SqlPermissions::from_config` (fn, pub) — Builds permissions from optional JSON config; defaults to read_only with create_schemas_if_missing=true; applies deny list after preset selection.
- `SqlPermissions::is_allowed` (fn, pub) — Checks if operation is allowed; Truncate always returns false.
- `SqlPermissions::is_schema_allowed` (fn, pub) — Checks schema access; introspection schemas always allowed; empty allowed_schemas means all schemas allowed.
- `SqlPermissions::sandbox_schema` (fn, pub) — Returns sandbox schema name (default "sandbox") where agent creates functions/tables.
- `SqlPermissions::tenant_user_id` (fn, pub) — Returns optional tenant user ID for RLS isolation.
- `SqlPermissions::tenant_column` (fn, pub) — Returns tenant column name for isolation (default "user_id").
- `SqlPermissions::auto_rls` (fn, pub) — Returns whether to auto-create RLS policies at initialization.
- `SqlPermissions::create_schemas_if_missing` (fn, pub) — Returns whether to auto-provision allowed_schemas missing from database (opt-out, default true).
- `SqlPermissions::allowed_schemas_iter` (fn, pub) — Iterator over configured allowed_schemas; empty means no restriction.
- `SqlPermissions::describe_capabilities_nl` (fn, pub) — Returns Spanish-language capability statement of what agent can/cannot do; highlights permanent restrictions (DROP, TRUNCATE, ALTER variants).
- `SqlPermissions::describe_for_llm` (fn, pub) — Returns concise English summary (operations + schemas) for LLM context injection.
- `tests` (module, private) — 24 test cases covering presets, deny lists, schema allowlisting, tenant fields, defaults, and NL descriptions.

## File-level notes

- Well-structured domain layer: no external dependencies, pure data + logic.
- Comprehensive test coverage (24 tests) validates all presets, deny combos, schema allowlisting, tenant fields, and NL generation.
- Spanish capability descriptions (`describe_capabilities_nl`) intentionally exclude CreateFunction/CreateTable granularity (treats as one "create in sandbox" capability) since presets only expose them together.
- Introspection schemas hardcoded and immutable—correct for PostgreSQL information_schema and pg_catalog access.
- All methods properly documented with comments explaining intent and defaults.
