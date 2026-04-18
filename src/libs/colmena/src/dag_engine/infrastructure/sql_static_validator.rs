//! Static rule validator for SQL queries.
//!
//! Validates queries against permissions and pattern-based safety rules.
//! All checks run synchronously in <1ms with zero external dependencies.

use crate::dag_engine::domain::sql_permissions::{SqlOperation, SqlPermissions};
use crate::dag_engine::domain::sql_ports::{SqlValidatorPort, ValidationResult};

/// Stateless validator that checks SQL queries against static rules.
pub struct StaticRuleValidator;

impl StaticRuleValidator {
    /// Detect the primary SQL operation from a query string.
    fn detect_operation(query: &str) -> Option<SqlOperation> {
        let trimmed = query.trim_start();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("SELECT") {
            Some(SqlOperation::Select)
        } else if upper.starts_with("INSERT") {
            Some(SqlOperation::Insert)
        } else if upper.starts_with("UPDATE") {
            Some(SqlOperation::Update)
        } else if upper.starts_with("DELETE") {
            Some(SqlOperation::Delete)
        } else if upper.starts_with("CREATE TABLE") {
            Some(SqlOperation::CreateTable)
        } else if upper.starts_with("CREATE FUNCTION") || upper.starts_with("CREATE OR REPLACE FUNCTION") {
            Some(SqlOperation::CreateFunction)
        } else if upper.starts_with("TRUNCATE") {
            Some(SqlOperation::Truncate)
        } else if upper.starts_with("DROP") {
            Some(SqlOperation::Drop)
        } else if upper.starts_with("ALTER") {
            Some(SqlOperation::Alter)
        } else {
            None
        }
    }

    /// Extract schema references from the query (simple heuristic: `schema.table` patterns).
    fn extract_schemas(query: &str) -> Vec<String> {
        let re = regex::Regex::new(r"(?i)\b(\w+)\.(\w+)").unwrap();
        let mut schemas = Vec::new();
        for cap in re.captures_iter(query) {
            let schema = cap[1].to_lowercase();
            schemas.push(schema);
        }
        schemas.sort();
        schemas.dedup();
        schemas
    }

    /// Check if query contains a WHERE clause (for DELETE/UPDATE safety).
    fn has_where_clause(query: &str) -> bool {
        let upper = query.to_uppercase();
        upper.contains("WHERE")
    }

    /// Check if a CREATE statement includes a COMMENT ON statement.
    fn has_comment(query: &str) -> bool {
        let upper = query.to_uppercase();
        upper.contains("COMMENT ON")
    }
}

impl SqlValidatorPort for StaticRuleValidator {
    fn validate(
        &self,
        query: &str,
        permissions: &SqlPermissions,
    ) -> ValidationResult {
        let mut warnings: Vec<String> = Vec::new();

        // 1. Detect operation
        let operation = match Self::detect_operation(query) {
            Some(op) => op,
            None => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("Could not determine SQL operation type. Only SELECT, INSERT, UPDATE, DELETE, and CREATE FUNCTION are supported.".to_string()),
                    warnings: vec![],
                };
            }
        };

        // 2. Check if operation is always blocked
        match &operation {
            SqlOperation::Truncate => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("TRUNCATE is not allowed. Use DELETE with a WHERE clause instead.".to_string()),
                    warnings: vec![],
                };
            }
            SqlOperation::Drop => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("DROP is not allowed. You can only create objects in the sandbox schema.".to_string()),
                    warnings: vec![],
                };
            }
            SqlOperation::Alter => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("ALTER is not allowed on any schema.".to_string()),
                    warnings: vec![],
                };
            }
            _ => {}
        }

        // 3. Check permission for this operation
        if !permissions.is_allowed(&operation) {
            return ValidationResult {
                allowed: false,
                block_reason: Some(format!(
                    "{:?} is not permitted by the current permission preset. Allowed operations: {}",
                    operation,
                    permissions.describe_for_llm()
                )),
                warnings: vec![],
            };
        }

        // 4. Check schema access
        let schemas = Self::extract_schemas(query);
        for schema in &schemas {
            if !permissions.is_schema_allowed(schema) {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "Access to schema '{}' is not allowed. Allowed schemas: check your permissions config.",
                        schema
                    )),
                    warnings: vec![],
                };
            }
        }

        // 5. DELETE/UPDATE without WHERE
        if matches!(operation, SqlOperation::Delete | SqlOperation::Update)
            && !Self::has_where_clause(query)
        {
            return ValidationResult {
                allowed: false,
                block_reason: Some(format!(
                    "{:?} without a WHERE clause is not allowed. Specify which rows to affect.",
                    operation
                )),
                warnings: vec![],
            };
        }

        // 6. CREATE without COMMENT
        if matches!(operation, SqlOperation::CreateFunction) && !Self::has_comment(query) {
            return ValidationResult {
                allowed: false,
                block_reason: Some(
                    "CREATE FUNCTION requires a COMMENT ON FUNCTION statement describing the function's purpose. Include it in the same query.".to_string()
                ),
                warnings: vec![],
            };
        }

        // 7. Warnings (non-blocking)
        let upper = query.to_uppercase();
        if upper.contains("SELECT *") || upper.contains("SELECT  *") {
            warnings.push(
                "Prefer selecting specific columns instead of SELECT * to reduce data transfer and improve clarity.".to_string()
            );
        }

        ValidationResult {
            allowed: true,
            block_reason: None,
            warnings,
        }
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
        }))).unwrap()
    }

    fn full_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "full",
            "allowed_schemas": ["production", "sandbox", "public"],
            "sandbox_schema": "sandbox"
        }))).unwrap()
    }

    #[test]
    fn test_select_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate("SELECT id, name FROM production.users WHERE id = 1", &read_only_perms());
        assert!(r.allowed);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn test_select_star_warns() {
        let v = StaticRuleValidator;
        let r = v.validate("SELECT * FROM production.users WHERE id = 1", &read_only_perms());
        assert!(r.allowed);
        assert!(r.warnings.iter().any(|w| w.contains("SELECT *")));
    }

    #[test]
    fn test_insert_blocked_on_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate("INSERT INTO production.users (name) VALUES ('test')", &read_only_perms());
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
        let r = v.validate("UPDATE production.orders SET status = 'done'", &full_perms());
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
        let r = v.validate("CREATE TABLE public.todos (id SERIAL, title TEXT)", &full_perms());
        assert!(r.allowed);
    }

    #[test]
    fn test_create_table_blocked_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate("CREATE TABLE public.todos (id SERIAL)", &read_only_perms());
        assert!(!r.allowed);
    }
}
