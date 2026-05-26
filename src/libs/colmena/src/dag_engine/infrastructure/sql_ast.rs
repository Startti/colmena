//! Shared SQL AST helpers built on `sqlparser` (PostgreSQL dialect).
//!
//! All consumers of structural SQL analysis (validator, execution service,
//! pool adapter, sql node) MUST go through this module rather than rolling
//! their own regex/substring heuristics — that is how the `1.81` schema
//! false-positive bug got in.

use crate::dag_engine::domain::sql_permissions::SqlOperation;
use sqlparser::ast::{
    visit_relations, ObjectName, ObjectNamePart, Query, Select, SelectItem, SetExpr,
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
    // Case-insensitive search for the keyword sequence. We rebuild the string in
    // one pass; when we hit the prefix, we copy the function name, then skip the
    // balanced `(...)` block, then continue.
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if let Some(after_prefix) = match_comment_on_function(input, i) {
            // Copy up through the function name (everything between `FUNCTION ` and `(`).
            // `after_prefix` points at the first non-whitespace, non-name char — either
            // `(` (parens to strip) or something else (no parens — leave alone).
            let (name_end, has_parens) = scan_function_name_end(input, after_prefix);
            out.push_str(&input[i..name_end]);
            i = name_end;
            if has_parens {
                // Skip a balanced `(...)` block, then continue copying.
                if let Some(close) = find_matching_paren(input, i) {
                    i = close + 1;
                }
            }
        } else {
            // Copy one char and advance.
            // Be careful with multi-byte chars: advance by the char's UTF-8 width.
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
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

/// Map a parsed statement to its `SqlOperation`, or `None` for statement kinds
/// we intentionally do not support (CREATE SCHEMA/INDEX/VIEW, GRANT, REVOKE,
/// COMMENT-only, etc.). The validator turns `None` into a precise block reason.
pub fn classify(stmt: &Statement) -> Option<SqlOperation> {
    match stmt {
        Statement::Query(_) => Some(SqlOperation::Select),
        Statement::Insert(_) => Some(SqlOperation::Insert),
        Statement::Update(_) => Some(SqlOperation::Update),
        Statement::Delete(_) => Some(SqlOperation::Delete),
        Statement::CreateTable(_) => Some(SqlOperation::CreateTable),
        Statement::CreateFunction(_) => Some(SqlOperation::CreateFunction),
        Statement::Truncate { .. } => Some(SqlOperation::Truncate),
        Statement::Drop { .. } => Some(SqlOperation::Drop),
        Statement::AlterTable { .. }
        | Statement::AlterIndex { .. }
        | Statement::AlterView { .. } => Some(SqlOperation::Alter),
        _ => None,
    }
}

/// Collect every distinct schema referenced by table identifiers in the
/// statement. Walks the AST via `visit_relations`, so numeric literals, JSON
/// arrow operators, and `obj.method` calls are never miscounted as schemas.
pub fn referenced_schemas(stmt: &Statement) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
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

/// True if the statement is a `SELECT` or a `WITH ... SELECT` (CTE).
pub fn is_query(stmt: &Statement) -> bool {
    matches!(stmt, Statement::Query(_))
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
            Some(SqlOperation::Alter)
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
            parse("SELECT u.id FROM core.users u JOIN audit.events e ON e.user_id = u.id")
                .unwrap();
        let mut s = referenced_schemas(&stmts[0]);
        s.sort();
        assert_eq!(s, vec!["audit".to_string(), "core".to_string()]);
    }

    #[test]
    fn referenced_schemas_handles_json_path() {
        // `data->>'key'` and `obj.field` outside identifier position must not be
        // counted as schema references.
        let stmts =
            parse("SELECT data->>'foo' FROM public.t WHERE data->>'bar' = 'x'").unwrap();
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
        assert!(has_where(&parse("UPDATE s.t SET a=1 WHERE id=2").unwrap()[0]));
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
        let query = "CREATE FUNCTION sandbox.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql; \
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
        assert_eq!(
            created_function_name(&stmts[0]).unwrap(),
            "sandbox.my_func"
        );
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
    fn is_query_recognises_with_cte() {
        let stmts = parse("WITH x AS (SELECT 1) SELECT * FROM x").unwrap();
        assert!(is_query(&stmts[0]));
        let stmts = parse("SELECT 1").unwrap();
        assert!(is_query(&stmts[0]));
        let stmts = parse("INSERT INTO s.t (a) VALUES (1)").unwrap();
        assert!(!is_query(&stmts[0]));
    }
}
