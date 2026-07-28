# sql_ast.rs

**Layer:** infrastructure  **Purpose:** Centralized fail-closed SQL AST analysis via `sqlparser` PostgreSQL dialect, replacing regex/substring heuristics. Prevents bugs like the `1.81` decimal-literal false positive. All SQL-aware modules (validator, execution service, pool adapter, sql node) must use this module for structural analysis.

## Symbols

**Public API:**

- `parse(query: &str)` (pub fn) — Parse SQL script into statements vector via PostgreSQL dialect, preprocessing COMMENT ON FUNCTION parens for sqlparser 0.62 compat
- `classify(stmt: &Statement)` (pub fn) — Map statement to `SqlOperation` with fail-closed handling of CTE-wrapped mutations and MERGE
- `referenced_schemas(stmt: &Statement)` (pub fn) — Collect distinct schemas referenced in FROM/JOIN/COMMENT ON TABLE/ALTER TABLE positions via AST walk, deduped and sorted
- `select_has_wildcard(stmt: &Statement)` (pub fn) — True if SELECT projection includes `*` or qualified `t.*` in any nested query
- `has_where(stmt: &Statement)` (pub fn) — True only if DELETE/UPDATE has explicit WHERE clause
- `script_has_comment_on(stmts: &[Statement])` (pub fn) — True if any statement in vector is COMMENT ON
- `created_table_name(stmt: &Statement)` (pub fn) — Extract `(schema, table)` from CREATE TABLE, defaulting unqualified names to "public"
- `created_function_name(stmt: &Statement)` (pub fn) — Extract fully qualified function name string from CREATE FUNCTION
- `is_query(stmt: &Statement)` (pub fn) — True only for genuine reads: SELECT, CTE-SELECT, set-operations, VALUES, TABLE (fail-closed: CTE-wrapped mutations and MERGE return false)
- `query_has_limit(stmt: &Statement)` (pub fn) — True if top-level query has explicit LIMIT clause
- `Statement` (pub use re-export) — Re-export `sqlparser::ast::Statement` so downstream doesn't need sqlparser direct dependency

**Private Utilities:**

- `strip_comment_on_function_args(input: &str)` (fn) — Preprocess to remove `(arg_types)` from `COMMENT ON FUNCTION name(arg_types)` for sqlparser compat; preserves unrelated function calls and string literals
- `scan_string_literal(input: &str, pos: usize)` (fn) — Detect and skip over string literal (single-quoted or dollar-quoted) starting at pos; returns offset past closing delimiter or None if not at literal start
- `scan_single_quoted(input: &str, pos: usize)` (fn) — Consume single-quoted string with SQL doubled-apostrophe escape handling; returns offset past closing quote or EOF
- `scan_dollar_quoted(input: &str, pos: usize)` (fn) — Consume dollar-quoted string (`$$...$$` or `$tag$...$tag$`); validates tag as Postgres identifier; returns offset past closing delimiter or None if invalid tag syntax
- `match_comment_on_function(input: &str, pos: usize)` (fn) — Match "COMMENT ON FUNCTION " keyword sequence (case-insensitive) with left/right word boundaries and whitespace skip; return offset at name start or None
- `scan_function_name_end(input: &str, start: usize)` (fn) — Scan function name including dotted qualifiers (schema.func, quoted identifiers); return `(end_offset, has_parens)` to guide paren stripping
- `find_matching_paren(input: &str, open: usize)` (fn) — Find closing paren matching the opening paren at offset; track nesting depth and respect single-quoted strings to ignore quotes inside expressions
- `classify_query_body(q: &Query)` (fn) — Classify `Query` body as Read/Mutation/Blocked (fail-closed); nested walk function handles known mutations (INSERT/UPDATE/DELETE), read types (SELECT/SetOp/VALUES/TABLE), nested queries, MERGE, and unknown variants
- `split_object_name(name: &ObjectName, default_schema: &str)` (fn) → (String, String) — Extract `(schema, table)` from sqlparser ObjectName; default schema if unqualified; handles ObjectNamePart::as_ident filtering

**Private Enums:**

- `QueryBodyKind` (enum) — Classification of Statement::Query body: `Read` (genuine read forms), `Mutation(SqlOperation)` (INSERT/UPDATE/DELETE), `Blocked` (MERGE, unknown, any future variant — fail-closed)

## File-level notes

- **Fail-closed security design**: All classifiers (`classify_query_body`, `classify`) explicitly enumerate known-safe forms and default unknown/future variants to blocked/None, preventing silent reopen of security holes (e.g., MERGE or future sqlparser variants).
- **CTE-wrapped mutation regression coverage**: 7 tests (lines 746–822) verify that `WITH x AS (...) DELETE/UPDATE/INSERT/MERGE ...` correctly classifies as mutations or blocked, not reads — this closes the primary vulnerability the module was created to address.
- **String literal robustness**: 6 tests (lines 848–900) cover single-quoted (with doubled-apostrophe escapes), untagged and tagged dollar-quoted strings, ensuring the COMMENT ON FUNCTION preprocessing does not corrupt user data inside literals.
- **No unfinished code**: All functions have complete implementations; no todo!(), unimplemented!(), or FIXME comments.
- **Comprehensive test coverage**: 31 test cases cover parsing, classification, schema extraction, wildcard detection, LIMIT detection, CTE reads vs. mutations, DDL kinds, and preprocessing edge cases.
