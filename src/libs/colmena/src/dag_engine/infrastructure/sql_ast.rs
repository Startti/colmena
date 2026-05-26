//! Shared SQL AST helpers built on `sqlparser` (PostgreSQL dialect).
//!
//! All consumers of structural SQL analysis (validator, execution service,
//! pool adapter, sql node) MUST go through this module rather than rolling
//! their own regex/substring heuristics — that is how the `1.81` schema
//! false-positive bug got in.

use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::{Parser, ParserError};

/// Parse a SQL script into one or more statements using the PostgreSQL dialect.
/// Returns the full statement vector so callers can validate / inspect each.
pub fn parse(query: &str) -> Result<Vec<Statement>, ParserError> {
    Parser::parse_sql(&PostgreSqlDialect {}, query)
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
}
