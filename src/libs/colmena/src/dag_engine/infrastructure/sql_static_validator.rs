//! Static rule validator for SQL queries.
//!
//! Validates queries against permissions and pattern-based safety rules.
//! All checks run synchronously in <1ms with zero external dependencies.

use crate::dag_engine::domain::sql_permissions::{SqlOperation, SqlPermissions};
use crate::dag_engine::domain::sql_ports::{SqlValidatorPort, ValidationResult};
use crate::dag_engine::infrastructure::sql_ast;

/// Stateless validator that runs every parsed statement through static rules.
pub struct StaticRuleValidator;

impl SqlValidatorPort for StaticRuleValidator {
    fn validate(&self, query: &str, permissions: &SqlPermissions) -> ValidationResult {
        let stmts = match sql_ast::parse(query) {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("Empty SQL script.".to_string()),
                    warnings: vec![],
                };
            }
            Err(e) => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!("Failed to parse SQL: {}", e)),
                    warnings: vec![],
                };
            }
        };

        let mut warnings: Vec<String> = Vec::new();
        let mut create_function_seen = false;

        for stmt in &stmts {
            // COMMENT ON is metadata-only (no data exposure), so we skip classify and
            // the per-statement checks below, but we still enforce the schema allowlist
            // so an LLM can't annotate tables in schemas the operator never approved.
            if matches!(stmt, sqlparser::ast::Statement::Comment { .. }) {
                for schema in sql_ast::referenced_schemas(stmt) {
                    if !permissions.is_schema_allowed(&schema) {
                        return ValidationResult {
                            allowed: false,
                            block_reason: Some(format!(
                                "Access to schema '{}' is not allowed. \
                                 Allowed schemas: check your permissions config.",
                                schema
                            )),
                            warnings: vec![],
                        };
                    }
                }
                continue;
            }

            // 1. Classify
            let operation = match sql_ast::classify(stmt) {
                Some(op) => op,
                None => {
                    let kind = stmt_kind_name(stmt);
                    return ValidationResult {
                        allowed: false,
                        block_reason: Some(format!(
                            "{} is not supported. Only SELECT, INSERT, UPDATE, DELETE, \
                             CREATE TABLE, and CREATE FUNCTION are allowed. Manage schemas, \
                             indexes, and views via migrations.",
                            kind
                        )),
                        warnings: vec![],
                    };
                }
            };

            // 2. Always-blocked DDL
            match &operation {
                SqlOperation::Truncate => {
                    return blocked(
                        "TRUNCATE is not allowed. Use DELETE with a WHERE clause instead.",
                    );
                }
                SqlOperation::Drop => {
                    return blocked(
                        "DROP is not allowed. You can only create objects in the sandbox schema.",
                    );
                }
                SqlOperation::Alter => {
                    return blocked("ALTER is not allowed on any schema.");
                }
                _ => {}
            }

            // 3. Permission preset
            if !permissions.is_allowed(&operation) {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "{:?} is not permitted by the current permission preset. \
                         Allowed operations: {}",
                        operation,
                        permissions.describe_for_llm()
                    )),
                    warnings: vec![],
                };
            }

            // 4. Schema allowlist
            for schema in sql_ast::referenced_schemas(stmt) {
                if !permissions.is_schema_allowed(&schema) {
                    return ValidationResult {
                        allowed: false,
                        block_reason: Some(format!(
                            "Access to schema '{}' is not allowed. \
                             Allowed schemas: check your permissions config.",
                            schema
                        )),
                        warnings: vec![],
                    };
                }
            }

            // 5. DELETE/UPDATE without WHERE
            if matches!(operation, SqlOperation::Delete | SqlOperation::Update)
                && !sql_ast::has_where(stmt)
            {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "{:?} without a WHERE clause is not allowed. \
                         Specify which rows to affect.",
                        operation
                    )),
                    warnings: vec![],
                };
            }

            if matches!(operation, SqlOperation::CreateFunction) {
                create_function_seen = true;
            }

            // 6. SELECT * warning (non-blocking)
            if sql_ast::select_has_wildcard(stmt) {
                warnings.push(
                    "Prefer selecting specific columns instead of SELECT * to reduce \
                     data transfer and improve clarity."
                        .to_string(),
                );
            }
        }

        // 7. CREATE FUNCTION requires a COMMENT ON somewhere in the script
        if create_function_seen && !sql_ast::script_has_comment_on(&stmts) {
            return blocked(
                "CREATE FUNCTION requires a COMMENT ON FUNCTION statement describing \
                 the function's purpose. Include it in the same query.",
            );
        }

        ValidationResult {
            allowed: true,
            block_reason: None,
            warnings,
        }
    }
}

fn blocked(reason: &str) -> ValidationResult {
    ValidationResult {
        allowed: false,
        block_reason: Some(reason.to_string()),
        warnings: vec![],
    }
}

fn stmt_kind_name(stmt: &sqlparser::ast::Statement) -> &'static str {
    use sqlparser::ast::Statement::*;
    match stmt {
        CreateSchema { .. } => "CREATE SCHEMA",
        CreateIndex(_) => "CREATE INDEX",
        CreateView(_) => "CREATE VIEW",
        Grant(_) => "GRANT",
        Revoke(_) => "REVOKE",
        _ => "This statement type",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::sql_permissions::SqlPermissions;

    fn read_only_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["production"]
        })))
        .unwrap()
    }

    fn full_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "full",
            "allowed_schemas": ["production", "sandbox", "public"],
            "sandbox_schema": "sandbox"
        })))
        .unwrap()
    }

    #[test]
    fn test_select_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "SELECT id, name FROM production.users WHERE id = 1",
            &read_only_perms(),
        );
        assert!(r.allowed);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn test_select_star_warns() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "SELECT * FROM production.users WHERE id = 1",
            &read_only_perms(),
        );
        assert!(r.allowed);
        assert!(r.warnings.iter().any(|w| w.contains("SELECT *")));
    }

    #[test]
    fn test_insert_blocked_on_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "INSERT INTO production.users (name) VALUES ('test')",
            &read_only_perms(),
        );
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("Insert"));
    }

    #[test]
    fn test_delete_without_where_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("DELETE FROM production.orders", &full_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("WHERE"));
    }

    #[test]
    fn test_delete_with_where_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate("DELETE FROM production.orders WHERE id = 5", &full_perms());
        assert!(r.allowed);
    }

    #[test]
    fn test_update_without_where_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "UPDATE production.orders SET status = 'done'",
            &full_perms(),
        );
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("WHERE"));
    }

    #[test]
    fn test_truncate_always_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("TRUNCATE TABLE production.orders", &full_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("TRUNCATE"));
    }

    #[test]
    fn test_drop_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("DROP TABLE production.users", &full_perms());
        assert!(!r.allowed);
    }

    #[test]
    fn test_schema_not_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate("SELECT * FROM secret.passwords", &read_only_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("schema"));
    }

    #[test]
    fn test_introspection_always_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'production'",
            &read_only_perms(),
        );
        assert!(r.allowed);
    }

    #[test]
    fn test_create_function_without_comment_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "CREATE FUNCTION sandbox.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql",
            &full_perms(),
        );
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("COMMENT"));
    }

    #[test]
    fn test_create_function_with_comment_allowed() {
        let v = StaticRuleValidator;
        let query = "CREATE FUNCTION sandbox.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql; COMMENT ON FUNCTION sandbox.my_func() IS 'Does something'";
        let r = v.validate(query, &full_perms());
        assert!(r.allowed);
    }

    #[test]
    fn test_create_table_allowed_full() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "CREATE TABLE public.todos (id SERIAL, title TEXT)",
            &full_perms(),
        );
        assert!(r.allowed);
    }

    #[test]
    fn test_create_table_blocked_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate("CREATE TABLE public.todos (id SERIAL)", &read_only_perms());
        assert!(!r.allowed);
    }

    #[test]
    fn test_insert_with_decimal_literal_no_false_positive() {
        // Regression: the old regex matched `1.81` as schema="1", table="81".
        let v = StaticRuleValidator;
        let r = v.validate(
            "INSERT INTO seb_data.productos (sku, peso) VALUES ('A1', 1.81)",
            &SqlPermissions::from_config(Some(&serde_json::json!({
                "preset": "full",
                "allowed_schemas": ["seb_data"],
                "sandbox_schema": "seb_data"
            })))
            .unwrap(),
        );
        assert!(r.allowed, "decimal literal must not be misread as schema");
    }

    #[test]
    fn test_create_schema_blocked_with_useful_message() {
        let v = StaticRuleValidator;
        let r = v.validate("CREATE SCHEMA foo", &full_perms());
        assert!(!r.allowed);
        assert!(
            r.block_reason.as_deref().unwrap().contains("CREATE SCHEMA"),
            "block reason should name the rejected op"
        );
    }

    #[test]
    fn test_create_index_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "CREATE INDEX idx ON production.users (email)",
            &full_perms(),
        );
        assert!(!r.allowed);
    }

    #[test]
    fn test_unparseable_query_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("this is not sql", &full_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().to_lowercase().contains("parse"));
    }

    #[test]
    fn test_multistatement_validates_each() {
        // The old validator looked at the first keyword and let DROP slip through.
        let v = StaticRuleValidator;
        let r = v.validate(
            "SELECT * FROM production.users; DROP TABLE production.users",
            &full_perms(),
        );
        assert!(!r.allowed, "DROP in second statement must be blocked");
        assert!(r.block_reason.unwrap().contains("DROP"));
    }

    #[test]
    fn test_multirow_insert_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "INSERT INTO public.t (a, b) VALUES (1, 'x'), (2, 'y'), (3, 'z')",
            &full_perms(),
        );
        assert!(r.allowed, "multi-row INSERT must work for bulk loads");
    }

    #[test]
    fn test_comment_on_disallowed_schema_blocked() {
        // COMMENT ON is metadata-only, but it should still respect the schema
        // allowlist — otherwise an LLM could annotate tables in schemas the
        // operator never approved.
        let v = StaticRuleValidator;
        let r = v.validate(
            "COMMENT ON TABLE secret.passwords IS 'sensitive'",
            &read_only_perms(), // allowed_schemas = ["production"]
        );
        assert!(!r.allowed, "COMMENT on disallowed schema must be blocked");
        assert!(
            r.block_reason
                .as_deref()
                .unwrap()
                .to_lowercase()
                .contains("schema"),
            "block reason should mention the schema"
        );
    }

    #[test]
    fn test_comment_on_allowed_schema_passes() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "COMMENT ON TABLE production.users IS 'user accounts'",
            &read_only_perms(),
        );
        assert!(r.allowed, "COMMENT on allowed schema should pass");
    }

    fn rwd_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_write_delete",
            "allowed_schemas": ["production"]
        })))
        .unwrap()
    }

    #[test]
    fn test_add_column_allowed_rwd() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users ADD COLUMN nickname TEXT",
            &rwd_perms(),
        );
        assert!(r.allowed, "ADD COLUMN must be allowed under read_write_delete");
    }

    #[test]
    fn test_add_column_allowed_full() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users ADD COLUMN nickname TEXT",
            &full_perms(),
        );
        assert!(r.allowed);
    }

    #[test]
    fn test_add_column_blocked_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users ADD COLUMN nickname TEXT",
            &read_only_perms(),
        );
        assert!(!r.allowed, "ADD COLUMN must be blocked under read_only");
    }

    #[test]
    fn test_drop_column_blocked_even_full() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users DROP COLUMN email",
            &full_perms(),
        );
        assert!(!r.allowed, "destructive ALTER must stay blocked even with full");
        assert!(r.block_reason.unwrap().contains("ALTER"));
    }

    #[test]
    fn test_add_column_on_disallowed_schema_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE secret.users ADD COLUMN x TEXT",
            &rwd_perms(), // allowed_schemas = ["production"]
        );
        assert!(!r.allowed, "ADD COLUMN must respect the schema allowlist");
        assert!(r.block_reason.unwrap().to_lowercase().contains("schema"));
    }
}
