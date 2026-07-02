//! Shared SQL AST helpers built on `sqlparser` (PostgreSQL dialect).
//!
//! All consumers of structural SQL analysis (validator, execution service,
//! pool adapter, sql node) MUST go through this module rather than rolling
//! their own regex/substring heuristics — that is how the `1.81` schema
//! false-positive bug got in.

use crate::dag_engine::domain::sql_permissions::SqlOperation;
use sqlparser::ast::{
    visit_relations, AlterTableOperation, ObjectName, ObjectNamePart, Query, Select, SelectItem,
    SetExpr,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::{Parser, ParserError};
use std::ops::ControlFlow;

// Re-export so downstream consumers (validator, execution service, pool adapter,
// sql node) don't each need `use sqlparser::ast::Statement;`.
pub use sqlparser::ast::Statement;

/// Parse a SQL script into one or more statements using the PostgreSQL dialect.
/// Returns the full statement vector so callers can validate / inspect each.
pub fn parse(query: &str) -> Result<Vec<Statement>, ParserError> {
    let preprocessed = strip_comment_on_function_args(query);
    Parser::parse_sql(&PostgreSqlDialect {}, &preprocessed)
}

/// Strip `(arg_types)` from `COMMENT ON FUNCTION name(arg_types) IS '...'`.
///
/// sqlparser 0.62 rejects the canonical Postgres syntax with parens. Since the
/// validator doesn't care about the argument-type list (it only checks that a
/// `COMMENT ON` statement exists), we drop those parens before parsing.
///
/// The pre-pass triggers only on the exact keyword sequence
/// `COMMENT ON FUNCTION <name>(...)`, so unrelated function calls in SELECT or
/// elsewhere are untouched. Nested parens inside the argument list (e.g.
/// `NUMERIC(10, 2)`) are balanced correctly.
fn strip_comment_on_function_args(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        // 1. If we're at the start of a string literal, copy the whole literal
        //    verbatim and skip ahead. This is what prevents the keyword scan
        //    from firing inside user data.
        if let Some(end) = scan_string_literal(input, i) {
            out.push_str(&input[i..end]);
            i = end;
            continue;
        }

        // 2. Outside any literal: try the COMMENT ON FUNCTION trigger.
        if let Some(after_prefix) = match_comment_on_function(input, i) {
            let (name_end, has_parens) = scan_function_name_end(input, after_prefix);
            out.push_str(&input[i..name_end]);
            i = name_end;
            if has_parens {
                if let Some(close) = find_matching_paren(input, i) {
                    i = close + 1;
                }
            }
            continue;
        }

        // 3. Otherwise, copy one char (UTF-8-safe).
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// If `input[pos..]` starts a string literal (single-quoted or dollar-quoted),
/// return the byte offset just past its closing delimiter. Returns `None` if
/// `pos` is not at a literal opening.
fn scan_string_literal(input: &str, pos: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    match bytes.get(pos)? {
        b'\'' => Some(scan_single_quoted(input, pos)),
        b'$' => scan_dollar_quoted(input, pos),
        _ => None,
    }
}

/// Walk a single-quoted string starting at `pos` (which points at the opening
/// `'`). Doubled apostrophes (`''`) are the SQL escape for one apostrophe.
/// Returns the offset just past the closing `'` (or end-of-input if unterminated).
fn scan_single_quoted(input: &str, pos: usize) -> usize {
    let bytes = input.as_bytes();
    debug_assert_eq!(bytes.get(pos).copied(), Some(b'\''));
    let mut i = pos + 1;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            // Doubled-quote escape — consume both and stay inside.
            if bytes.get(i + 1).copied() == Some(b'\'') {
                i += 2;
                continue;
            }
            // Real closing quote.
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// Walk a dollar-quoted string starting at `pos` (which points at the opening
/// `$`). Forms: `$$...$$` (empty tag) and `$tag$...$tag$` (tag is a Postgres
/// identifier — letters, digits, underscore; cannot start with a digit).
/// Returns the offset just past the closing delimiter, or `None` if `pos` is
/// not actually at a dollar-quote opening (e.g. a parameter reference like `$1`).
fn scan_dollar_quoted(input: &str, pos: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    debug_assert_eq!(bytes.get(pos).copied(), Some(b'$'));

    // Find the tag — chars up to the next `$`.
    let tag_start = pos + 1;
    let mut tag_end = tag_start;
    while tag_end < bytes.len() {
        let b = bytes[tag_end];
        if b == b'$' {
            break;
        }
        // Postgres identifier rule: first char letter/underscore, rest alnum/_.
        let is_first = tag_end == tag_start;
        let valid = if is_first {
            b.is_ascii_alphabetic() || b == b'_'
        } else {
            b.is_ascii_alphanumeric() || b == b'_'
        };
        if !valid {
            // Not a dollar-quote opening — could be `$1`, `$NUMBER`, or `$$$` etc.
            return None;
        }
        tag_end += 1;
    }
    // Must end on `$`.
    if bytes.get(tag_end).copied() != Some(b'$') {
        return None;
    }
    let body_start = tag_end + 1;
    let tag = &input[tag_start..tag_end]; // may be empty (plain `$$`)
    let closing = format!("${}$", tag);

    // Find the closing delimiter starting from body_start.
    let mut search = body_start;
    while search + closing.len() <= bytes.len() {
        if input[search..search + closing.len()] == closing {
            return Some(search + closing.len());
        }
        search += 1;
    }
    // Unterminated — consume the rest as the body.
    Some(bytes.len())
}

/// If `input[pos..]` starts (case-insensitively, modulo whitespace) with
/// `COMMENT ON FUNCTION ` and is a word boundary, return the byte offset
/// immediately after that prefix (pointing at the function name's first char).
fn match_comment_on_function(input: &str, pos: usize) -> Option<usize> {
    // Word-boundary on the left.
    if pos > 0 {
        let prev = input.as_bytes()[pos - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return None;
        }
    }
    const PREFIX: &str = "COMMENT ON FUNCTION";
    let remaining = input.get(pos..)?;
    if remaining.len() < PREFIX.len() {
        return None;
    }
    if !remaining
        .as_bytes()
        .iter()
        .zip(PREFIX.as_bytes())
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        return None;
    }
    // Must be followed by whitespace.
    let after = pos + PREFIX.len();
    let next = input.as_bytes().get(after).copied()?;
    if !next.is_ascii_whitespace() {
        return None;
    }
    // Skip whitespace to land on the first name char.
    let mut j = after;
    while j < input.len() && input.as_bytes()[j].is_ascii_whitespace() {
        j += 1;
    }
    Some(j)
}

/// Starting at the first char of a function name, scan to the end of the name
/// (including dotted qualifiers like `schema.func`). Returns `(end_offset, has_parens)`.
fn scan_function_name_end(input: &str, start: usize) -> (usize, bool) {
    let bytes = input.as_bytes();
    let mut j = start;
    while j < bytes.len() {
        let b = bytes[j];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'"' {
            j += 1;
        } else {
            break;
        }
    }
    // Skip whitespace between name and `(` or `IS`.
    let mut k = j;
    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
        k += 1;
    }
    let has_parens = k < bytes.len() && bytes[k] == b'(';
    if has_parens {
        // Caller will skip from the `(` onwards.
        (k, true)
    } else {
        (j, false)
    }
}

/// Given an offset pointing at `(`, return the offset of the matching `)`.
/// Tracks nesting; respects single-quoted strings so apostrophes inside
/// argument-list expressions don't fool the matcher.
fn find_matching_paren(input: &str, open: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    debug_assert_eq!(bytes.get(open).copied(), Some(b'('));
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\'' {
                // SQL escapes a quote by doubling it.
                if bytes.get(i + 1).copied() == Some(b'\'') {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
        } else {
            match b {
                b'\'' => in_str = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Inspect a `Query`'s body: return the mutation `SqlOperation` when the body is
/// a data-modifying form (a CTE-wrapped `DELETE`/`UPDATE`/`INSERT`, which
/// sqlparser 0.62 nests inside `SetExpr`), or `None` for a genuine read query
/// (`SELECT`, set-operation, `VALUES`, `TABLE t`, or a nested read subquery).
///
/// This is what closes the "SELECT-only preset runs a CTE-wrapped mutation" hole.
fn query_body_operation(q: &Query) -> Option<SqlOperation> {
    fn walk(body: &SetExpr) -> Option<SqlOperation> {
        match body {
            SetExpr::Insert(_) => Some(SqlOperation::Insert),
            SetExpr::Update(_) => Some(SqlOperation::Update),
            SetExpr::Delete(_) => Some(SqlOperation::Delete),
            // A parenthesised subquery body — recurse (defends against
            // `WITH x AS (...) (DELETE ...)`-style nesting).
            SetExpr::Query(inner) => walk(&inner.body),
            // Genuine reads: SELECT, VALUES, TABLE, and set operations.
            _ => None,
        }
    }
    walk(&q.body)
}

/// Map a parsed statement to its `SqlOperation`, or `None` for statement kinds
/// we intentionally do not support (CREATE SCHEMA/INDEX/VIEW, GRANT, REVOKE,
/// COMMENT-only, etc.). The validator turns `None` into a precise block reason.
pub fn classify(stmt: &Statement) -> Option<SqlOperation> {
    match stmt {
        // sqlparser 0.62 parses CTE-wrapped mutations
        // (`WITH x AS (...) DELETE/UPDATE/INSERT ...`) as a single
        // `Statement::Query` whose *body* is the mutation. Inspect the body so a
        // SELECT-only preset applies the correct (blocking) permission instead of
        // waving it through as a read.
        Statement::Query(q) => Some(query_body_operation(q).unwrap_or(SqlOperation::Select)),
        Statement::Insert(_) => Some(SqlOperation::Insert),
        Statement::Update(_) => Some(SqlOperation::Update),
        Statement::Delete(_) => Some(SqlOperation::Delete),
        Statement::CreateTable(_) => Some(SqlOperation::CreateTable),
        Statement::CreateFunction(_) => Some(SqlOperation::CreateFunction),
        Statement::Truncate { .. } => Some(SqlOperation::Truncate),
        Statement::Drop { .. } => Some(SqlOperation::Drop),
        Statement::AlterTable(alter) => {
            // Allow ONLY when every operation is ADD COLUMN. Any destructive or
            // mixed operation (DROP COLUMN, type change, RENAME, …) classifies as
            // `Alter`, which the validator hard-blocks for all presets.
            if !alter.operations.is_empty()
                && alter
                    .operations
                    .iter()
                    .all(|op| matches!(op, AlterTableOperation::AddColumn { .. }))
            {
                Some(SqlOperation::AddColumn)
            } else {
                Some(SqlOperation::Alter)
            }
        }
        Statement::AlterIndex { .. } | Statement::AlterView { .. } => Some(SqlOperation::Alter),
        _ => None,
    }
}

/// Collect every distinct schema referenced by table identifiers in the
/// statement. Walks the AST via `visit_relations`, so numeric literals, JSON
/// arrow operators, and `obj.method` calls are never miscounted as schemas.
pub fn referenced_schemas(stmt: &Statement) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();

    // `visit_relations` only visits relation names that appear in FROM/JOIN
    // positions. `COMMENT ON TABLE schema.table IS '...'` stores its target in
    // `object_name`, which the derive-generated visitor does not classify as a
    // relation.  Handle it explicitly so schema enforcement is consistent.
    if let Statement::Comment { object_name, .. } = stmt {
        if object_name.0.len() >= 2 {
            if let Some(ident) = object_name.0[0].as_ident() {
                let schema = ident.value.to_lowercase();
                if !found.contains(&schema) {
                    found.push(schema);
                }
            }
        }
    }

    // `ALTER TABLE schema.table …` stores its target in `name`, which
    // `visit_relations` also does not classify as a FROM/JOIN relation.
    if let Statement::AlterTable(alter) = stmt {
        if alter.name.0.len() >= 2 {
            if let Some(ident) = alter.name.0[0].as_ident() {
                let schema = ident.value.to_lowercase();
                if !found.contains(&schema) {
                    found.push(schema);
                }
            }
        }
    }

    let _: ControlFlow<()> = visit_relations(stmt, |name: &ObjectName| {
        // ObjectName.0 is `Vec<ObjectNamePart>` — 2+ parts means
        // `schema.table[.col]`. We only care about Identifier parts.
        if name.0.len() >= 2 {
            if let Some(ident) = name.0[0].as_ident() {
                let schema = ident.value.to_lowercase();
                if !found.contains(&schema) {
                    found.push(schema);
                }
            }
        }
        ControlFlow::Continue(())
    });
    found.sort();
    found
}

/// True when a SELECT has `*` or `t.*` in any projection slot.
pub fn select_has_wildcard(stmt: &Statement) -> bool {
    fn walk_query(q: &Query) -> bool {
        walk_set(&q.body)
    }
    fn walk_set(expr: &SetExpr) -> bool {
        match expr {
            SetExpr::Select(s) => walk_select(s),
            SetExpr::Query(q) => walk_query(q),
            SetExpr::SetOperation { left, right, .. } => walk_set(left) || walk_set(right),
            _ => false,
        }
    }
    fn walk_select(s: &Select) -> bool {
        s.projection.iter().any(|p| {
            matches!(
                p,
                SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
            )
        })
    }
    match stmt {
        Statement::Query(q) => walk_query(q),
        _ => false,
    }
}

/// True if a DELETE/UPDATE has a WHERE clause. Always false for other kinds.
pub fn has_where(stmt: &Statement) -> bool {
    match stmt {
        Statement::Update(u) => u.selection.is_some(),
        Statement::Delete(d) => d.selection.is_some(),
        _ => false,
    }
}

/// True if any statement in the script is `COMMENT ON ...`.
/// Used to enforce that CREATE FUNCTION ships with a description.
pub fn script_has_comment_on(stmts: &[Statement]) -> bool {
    stmts.iter().any(|s| matches!(s, Statement::Comment { .. }))
}

/// For a `CREATE TABLE`, return `(schema, table)`. Defaults schema to `"public"`
/// when the table name is unqualified.
pub fn created_table_name(stmt: &Statement) -> Option<(String, String)> {
    if let Statement::CreateTable(ct) = stmt {
        return Some(split_object_name(&ct.name, "public"));
    }
    None
}

/// For a `CREATE FUNCTION`, return its fully qualified name as a single string
/// (matches the legacy `extract_function_name` return shape).
pub fn created_function_name(stmt: &Statement) -> Option<String> {
    if let Statement::CreateFunction(cf) = stmt {
        return Some(cf.name.to_string());
    }
    None
}

/// True only for a *genuine read* query: a `SELECT`, `WITH ... SELECT` (CTE),
/// set-operation, `VALUES`, or `TABLE t`. A CTE-wrapped mutation
/// (`WITH x AS (...) DELETE/UPDATE/INSERT ...`) parses as `Statement::Query` in
/// sqlparser 0.62 but is NOT a read, so this returns `false` for it.
pub fn is_query(stmt: &Statement) -> bool {
    match stmt {
        Statement::Query(q) => query_body_operation(q).is_none(),
        _ => false,
    }
}

/// True if the top-level query already has an explicit `LIMIT` clause.
pub fn query_has_limit(stmt: &Statement) -> bool {
    match stmt {
        Statement::Query(q) => q.limit_clause.is_some(),
        _ => false,
    }
}

fn split_object_name(name: &ObjectName, default_schema: &str) -> (String, String) {
    let idents: Vec<&str> = name
        .0
        .iter()
        .filter_map(ObjectNamePart::as_ident)
        .map(|i| i.value.as_str())
        .collect();
    match idents.as_slice() {
        [single] => (default_schema.to_string(), (*single).to_string()),
        [schema, table, ..] => ((*schema).to_string(), (*table).to_string()),
        _ => (default_schema.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_select() {
        let stmts = parse("SELECT 1").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn parses_multi_statement() {
        let stmts = parse("SELECT 1; SELECT 2;").unwrap();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn errors_on_garbage() {
        assert!(parse("this is not sql").is_err());
    }

    #[test]
    fn classify_select() {
        let stmts = parse("SELECT id FROM s.t WHERE id=1").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Select));
    }

    #[test]
    fn classify_insert() {
        let stmts = parse("INSERT INTO s.t (a) VALUES (1)").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Insert));
    }

    #[test]
    fn classify_update_delete_create() {
        assert_eq!(
            classify(&parse("UPDATE s.t SET a=1 WHERE id=2").unwrap()[0]),
            Some(SqlOperation::Update)
        );
        assert_eq!(
            classify(&parse("DELETE FROM s.t WHERE id=2").unwrap()[0]),
            Some(SqlOperation::Delete)
        );
        assert_eq!(
            classify(&parse("CREATE TABLE s.t (id INT)").unwrap()[0]),
            Some(SqlOperation::CreateTable)
        );
        assert_eq!(
            classify(
                &parse("CREATE FUNCTION s.f() RETURNS VOID AS $$ BEGIN END; $$ LANGUAGE plpgsql")
                    .unwrap()[0]
            ),
            Some(SqlOperation::CreateFunction)
        );
        assert_eq!(
            classify(&parse("TRUNCATE TABLE s.t").unwrap()[0]),
            Some(SqlOperation::Truncate)
        );
        assert_eq!(
            classify(&parse("DROP TABLE s.t").unwrap()[0]),
            Some(SqlOperation::Drop)
        );
        assert_eq!(
            classify(&parse("ALTER TABLE s.t ADD COLUMN x INT").unwrap()[0]),
            Some(SqlOperation::AddColumn)
        );
    }

    #[test]
    fn classify_new_ddl_returns_unsupported() {
        // CREATE SCHEMA / INDEX / VIEW / GRANT / REVOKE are explicitly NOT mapped
        // to a SqlOperation — the validator will block them with a clear message.
        assert!(classify(&parse("CREATE SCHEMA foo").unwrap()[0]).is_none());
        assert!(classify(&parse("CREATE INDEX i ON s.t(a)").unwrap()[0]).is_none());
        assert!(classify(&parse("CREATE VIEW s.v AS SELECT 1").unwrap()[0]).is_none());
    }

    #[test]
    fn referenced_schemas_ignores_numeric_literals() {
        // The bug that triggered this whole refactor: `1.81` was matched as
        // `schema='1', table='81'` by the old regex.
        let stmts =
            parse("INSERT INTO seb_data.productos (sku, peso_kg) VALUES ('A1', 1.81)").unwrap();
        let schemas = referenced_schemas(&stmts[0]);
        assert_eq!(schemas, vec!["seb_data".to_string()]);
    }

    #[test]
    fn referenced_schemas_finds_joins() {
        let stmts =
            parse("SELECT u.id FROM core.users u JOIN audit.events e ON e.user_id = u.id").unwrap();
        let mut s = referenced_schemas(&stmts[0]);
        s.sort();
        assert_eq!(s, vec!["audit".to_string(), "core".to_string()]);
    }

    #[test]
    fn referenced_schemas_handles_json_path() {
        // `data->>'key'` and `obj.field` outside identifier position must not be
        // counted as schema references.
        let stmts = parse("SELECT data->>'foo' FROM public.t WHERE data->>'bar' = 'x'").unwrap();
        let schemas = referenced_schemas(&stmts[0]);
        assert_eq!(schemas, vec!["public".to_string()]);
    }

    #[test]
    fn select_has_wildcard_detects_star() {
        let stmts = parse("SELECT * FROM s.t").unwrap();
        assert!(select_has_wildcard(&stmts[0]));
        let stmts = parse("SELECT id, name FROM s.t").unwrap();
        assert!(!select_has_wildcard(&stmts[0]));
    }

    #[test]
    fn has_where_for_update_delete() {
        assert!(has_where(
            &parse("UPDATE s.t SET a=1 WHERE id=2").unwrap()[0]
        ));
        assert!(!has_where(&parse("UPDATE s.t SET a=1").unwrap()[0]));
        assert!(has_where(&parse("DELETE FROM s.t WHERE id=2").unwrap()[0]));
        assert!(!has_where(&parse("DELETE FROM s.t").unwrap()[0]));
    }

    #[test]
    fn parse_accepts_comment_on_function_with_parens() {
        // Canonical Postgres syntax: `COMMENT ON FUNCTION name(arg_types) IS '...'`.
        // sqlparser 0.62 rejects the `()` after the function name, so `parse` must
        // strip those parens (and their contents) as a pre-pass.
        let stmts = parse("COMMENT ON FUNCTION s.f() IS 'doc'").unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0], Statement::Comment { .. }));
    }

    #[test]
    fn parse_accepts_comment_on_function_with_typed_args() {
        let stmts = parse("COMMENT ON FUNCTION s.f(INT, TEXT) IS 'doc'").unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(matches!(stmts[0], Statement::Comment { .. }));
    }

    #[test]
    fn parse_accepts_create_function_plus_comment_with_parens() {
        // The combination that Task 3's existing test `test_create_function_with_comment_allowed`
        // depends on.
        let query =
            "CREATE FUNCTION sandbox.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql; \
                     COMMENT ON FUNCTION sandbox.my_func() IS 'Does something'";
        let stmts = parse(query).unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(script_has_comment_on(&stmts));
    }

    #[test]
    fn parse_preprocessing_ignores_unrelated_function_call() {
        // The pre-pass must ONLY strip `()` when it follows `COMMENT ON FUNCTION <name>`.
        // A bare `SELECT my_func()` must NOT be touched.
        let stmts = parse("SELECT my_func()").unwrap();
        assert_eq!(stmts.len(), 1);
        // If preprocessing wrongly touched this, it would still parse but as something weird.
        // Sanity check: it's still a SELECT.
        assert!(is_query(&stmts[0]));
    }

    #[test]
    fn parse_preprocessing_handles_nested_parens_in_args() {
        let stmts = parse("COMMENT ON FUNCTION s.f(NUMERIC(10, 2)) IS 'doc'").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn script_has_comment_on_detects_multistatement() {
        // sqlparser 0.62 PG dialect doesn't accept `COMMENT ON FUNCTION s.f() IS ...`
        // (it rejects the `()` after the function name), but `COMMENT ON TABLE ...`
        // parses fine and exercises the same `Statement::Comment` variant — which is
        // all `script_has_comment_on` cares about.
        let stmts = parse(
            "CREATE TABLE s.t (id INT);\
             COMMENT ON TABLE s.t IS 'doc'",
        )
        .unwrap();
        assert!(script_has_comment_on(&stmts));
    }

    #[test]
    fn created_table_name_extracts_schema_and_name() {
        let stmts = parse("CREATE TABLE seb_data.productos (id INT)").unwrap();
        let (schema, table) = created_table_name(&stmts[0]).unwrap();
        assert_eq!(schema, "seb_data");
        assert_eq!(table, "productos");

        // unqualified → defaults to "public"
        let stmts = parse("CREATE TABLE t (id INT)").unwrap();
        let (schema, table) = created_table_name(&stmts[0]).unwrap();
        assert_eq!(schema, "public");
        assert_eq!(table, "t");
    }

    #[test]
    fn created_function_name_extracts() {
        let stmts = parse(
            "CREATE FUNCTION sandbox.my_func() RETURNS VOID AS $$ BEGIN END; $$ LANGUAGE plpgsql",
        )
        .unwrap();
        assert_eq!(created_function_name(&stmts[0]).unwrap(), "sandbox.my_func");
    }

    #[test]
    fn query_has_limit_detects_only_real_limit() {
        let stmts = parse("SELECT * FROM s.t LIMIT 10").unwrap();
        assert!(query_has_limit(&stmts[0]));
        // The old substring check matched literals like 'LIMIT' inside WHERE.
        let stmts = parse("SELECT * FROM s.t WHERE name = 'LIMIT'").unwrap();
        assert!(!query_has_limit(&stmts[0]));
    }

    #[test]
    fn classify_add_column_only() {
        let stmts = parse("ALTER TABLE finanzas.gastos ADD COLUMN metodo_pago TEXT").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::AddColumn));
    }

    #[test]
    fn classify_alter_drop_column_is_blocked_alter() {
        let stmts = parse("ALTER TABLE finanzas.gastos DROP COLUMN monto").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Alter));
    }

    #[test]
    fn classify_alter_mixed_ops_is_blocked_alter() {
        let stmts = parse("ALTER TABLE finanzas.gastos ADD COLUMN a INT, DROP COLUMN b").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Alter));
    }

    #[test]
    fn referenced_schemas_includes_alter_target() {
        let stmts = parse("ALTER TABLE finanzas.gastos ADD COLUMN x INT").unwrap();
        assert!(referenced_schemas(&stmts[0]).contains(&"finanzas".to_string()));
    }

    #[test]
    fn is_query_recognises_with_cte() {
        let stmts = parse("WITH x AS (SELECT 1) SELECT * FROM x").unwrap();
        assert!(is_query(&stmts[0]));
        let stmts = parse("SELECT 1").unwrap();
        assert!(is_query(&stmts[0]));
        let stmts = parse("INSERT INTO s.t (a) VALUES (1)").unwrap();
        assert!(!is_query(&stmts[0]));
    }

    // --- Security regression: CTE-wrapped mutations must NOT classify as reads. ---
    // sqlparser 0.62 parses `WITH x AS (...) DELETE/UPDATE/INSERT ...` as a single
    // `Statement::Query` whose body is the mutation. Before the fix, both
    // `classify` and `is_query` treated any `Statement::Query` as a SELECT, letting
    // a read-only/SELECT-only caller run a data-modifying CTE.

    #[test]
    fn classify_cte_wrapped_delete_is_delete_not_select() {
        let stmts =
            parse("WITH x AS (SELECT 1) DELETE FROM t WHERE id IN (SELECT id FROM x)").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Delete));
        assert_ne!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(!is_query(&stmts[0]));
    }

    #[test]
    fn classify_cte_wrapped_update_is_update_not_select() {
        let stmts =
            parse("WITH x AS (SELECT 1) UPDATE t SET a=1 WHERE id IN (SELECT id FROM x)").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Update));
        assert_ne!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(!is_query(&stmts[0]));
    }

    #[test]
    fn classify_cte_wrapped_insert_is_insert_not_select() {
        let stmts = parse("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Insert));
        assert_ne!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(!is_query(&stmts[0]));
    }

    #[test]
    fn classify_legit_cte_select_stays_select() {
        // Legitimate CTE read must still classify as Select / be a query.
        let stmts = parse("WITH x AS (SELECT 1) SELECT * FROM x").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(is_query(&stmts[0]));
    }

    #[test]
    fn classify_set_operation_and_values_stay_select() {
        let stmts = parse("SELECT 1 UNION SELECT 2").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(is_query(&stmts[0]));
        let stmts = parse("VALUES (1), (2)").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(is_query(&stmts[0]));
    }

    #[test]
    fn classify_plain_select_and_delete_unchanged() {
        let stmts = parse("SELECT * FROM t").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Select));
        assert!(is_query(&stmts[0]));
        let stmts = parse("DELETE FROM t WHERE id=1").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Delete));
        assert!(!is_query(&stmts[0]));
    }

    #[test]
    fn parse_does_not_corrupt_string_literal_containing_trigger() {
        // Critical bug: pre-pass must NOT touch the trigger keywords inside a
        // single-quoted string literal.
        let q = "SELECT 'COMMENT ON FUNCTION foo(a INT)' AS msg";
        let stmts = parse(q).unwrap();
        assert_eq!(stmts.len(), 1);
        // The literal must round-trip unchanged through the parse → unparse loop.
        // We can't easily inspect string literal bytes from sqlparser AST cheaply,
        // so instead assert that the *preprocessed* input still contains the
        // original literal verbatim.
        assert!(
            strip_comment_on_function_args(q).contains("'COMMENT ON FUNCTION foo(a INT)'"),
            "string literal must not be modified by preprocessing"
        );
    }

    #[test]
    fn parse_does_not_corrupt_doubled_quote_inside_literal() {
        // `'it''s'` is a single string literal with one apostrophe in it.
        // The doubled-quote escape must not be misread as "literal ends, new
        // literal begins".
        let q = "SELECT 'it''s COMMENT ON FUNCTION x()' AS msg";
        let processed = strip_comment_on_function_args(q);
        assert!(
            processed.contains("'it''s COMMENT ON FUNCTION x()'"),
            "doubled-quote inside literal must keep the literal continuous; got: {processed}"
        );
    }

    #[test]
    fn parse_does_not_corrupt_dollar_quoted_body() {
        // Dollar-quoted strings (PL/pgSQL bodies) must also be opaque to the pre-pass.
        let q = "CREATE FUNCTION sandbox.f() RETURNS void AS \
                 $$ BEGIN RAISE NOTICE 'COMMENT ON FUNCTION x(y INT)'; END; $$ \
                 LANGUAGE plpgsql";
        let processed = strip_comment_on_function_args(q);
        assert!(
            processed.contains("COMMENT ON FUNCTION x(y INT)"),
            "text inside $$...$$ must not be touched; got: {processed}"
        );
    }

    #[test]
    fn parse_does_not_corrupt_tagged_dollar_quoted_body() {
        let q = "DO $tag$ \
                 RAISE NOTICE 'COMMENT ON FUNCTION x(z INT)'; \
                 $tag$";
        let processed = strip_comment_on_function_args(q);
        assert!(
            processed.contains("COMMENT ON FUNCTION x(z INT)"),
            "text inside $tag$...$tag$ must not be touched; got: {processed}"
        );
    }
}
