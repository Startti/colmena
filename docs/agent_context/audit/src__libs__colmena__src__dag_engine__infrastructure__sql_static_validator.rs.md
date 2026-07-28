# src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs

**Layer:** infrastructure  **Purpose:** Static validator that enforces SQL query safety rules synchronously (<1ms) against permissions and pattern-based constraints with zero external dependencies.

## Symbols

- `StaticRuleValidator` (struct, pub) — Stateless unit struct implementing `SqlValidatorPort` for static rule validation
- `SqlValidatorPort::validate` (method, pub) — Validates parsed SQL statements through 7-step gated pipeline: parsing, COMMENT ON schema check, operation classification, DDL blocks, permission presets, schema allowlist, DELETE/UPDATE WHERE enforcement, CREATE FUNCTION COMMENT requirement; returns `ValidationResult` with allow decision and optional warnings
- `blocked` (function, private) — Helper that creates a rejected `ValidationResult` with given reason string and empty warnings
- `stmt_kind_name` (function, private) — Extracts human-readable name (`&'static str`) for unsupported SQL statement types (CREATE SCHEMA, CREATE INDEX, CREATE VIEW, GRANT, REVOKE, etc.)
- `tests::read_only_perms` (function, private) — Test helper; constructs read-only permission set for schema "production"
- `tests::full_perms` (function, private) — Test helper; constructs full-access permission set for schemas production, sandbox, public with sandbox_schema="sandbox"
- `tests::rwd_perms` (function, private) — Test helper; constructs read_write_delete permission set for schema "production"
- `tests::test_select_allowed` (test, private) — Verifies SELECT with WHERE on allowed schema passes with no warnings
- `tests::test_select_star_warns` (test, private) — Verifies SELECT * generates warning but is allowed
- `tests::test_insert_blocked_on_read_only` (test, private) — Verifies INSERT fails under read_only preset
- `tests::test_delete_without_where_blocked` (test, private) — Verifies DELETE without WHERE clause is rejected
- `tests::test_delete_with_where_allowed` (test, private) — Verifies DELETE with WHERE clause is allowed under full permissions
- `tests::test_update_without_where_blocked` (test, private) — Verifies UPDATE without WHERE clause is rejected
- `tests::test_truncate_always_blocked` (test, private) — Verifies TRUNCATE is always blocked
- `tests::test_drop_blocked` (test, private) — Verifies DROP TABLE is always blocked
- `tests::test_schema_not_allowed` (test, private) — Verifies schema allowlist is enforced for referenced tables
- `tests::test_introspection_always_allowed` (test, private) — Verifies information_schema queries are allowed without explicit permission
- `tests::test_create_function_without_comment_blocked` (test, private) — Verifies CREATE FUNCTION requires COMMENT ON FUNCTION in same query
- `tests::test_create_function_with_comment_allowed` (test, private) — Verifies CREATE FUNCTION is allowed when paired with COMMENT ON FUNCTION
- `tests::test_create_table_allowed_full` (test, private) — Verifies CREATE TABLE is allowed under full permissions
- `tests::test_create_table_blocked_read_only` (test, private) — Verifies CREATE TABLE is blocked under read_only permissions
- `tests::test_insert_with_decimal_literal_no_false_positive` (test, private) — Regression test: decimal literals like 1.81 must not be misread as schema.table references by AST parser
- `tests::test_create_schema_blocked_with_useful_message` (test, private) — Verifies CREATE SCHEMA is blocked with descriptive error message naming the statement type
- `tests::test_create_index_blocked` (test, private) — Verifies CREATE INDEX is always blocked
- `tests::test_unparseable_query_blocked` (test, private) — Verifies invalid SQL syntax is rejected with parse error message
- `tests::test_multistatement_validates_each` (test, private) — Verifies each statement in a multi-statement query is validated independently (regression: old validator only checked first keyword)
- `tests::test_multirow_insert_allowed` (test, private) — Verifies multi-row VALUES inserts are allowed for bulk loads
- `tests::test_comment_on_disallowed_schema_blocked` (test, private) — Verifies COMMENT ON respects schema allowlist even though metadata-only
- `tests::test_comment_on_allowed_schema_passes` (test, private) — Verifies COMMENT ON on allowed schema passes validation
- `tests::test_add_column_allowed_rwd` (test, private) — Verifies ALTER TABLE ADD COLUMN is allowed under read_write_delete preset
- `tests::test_add_column_allowed_full` (test, private) — Verifies ALTER TABLE ADD COLUMN is allowed under full preset
- `tests::test_add_column_blocked_read_only` (test, private) — Verifies ALTER TABLE ADD COLUMN is blocked under read_only preset
- `tests::test_drop_column_blocked_even_full` (test, private) — Verifies destructive ALTER (DROP COLUMN) stays blocked even with full permissions
- `tests::test_add_column_on_disallowed_schema_blocked` (test, private) — Verifies ALTER TABLE ADD COLUMN respects schema allowlist

## File-level notes

- **Architecture alignment**: Clean infrastructure adapter implementing domain-defined `SqlValidatorPort` trait; depends on domain (`SqlPermissions`, `SqlOperation`, `ValidationResult`) and sister module `sql_ast` for AST parsing and introspection.
- **Testing coverage**: Comprehensive test suite (24 tests) covering happy paths, permission presets, edge cases (decimal literals, multi-statement, metadata-only COMMENT ON), and regression prevention (old regex false positives, first-keyword-only bugs).
- **No dependencies inflation**: Uses only `sqlparser` crate and domain imports; all validation runs synchronously in <1ms as documented.
- **Code quality**: Linear validation pipeline with clear early returns; no dead code or TODO stubs observed.
