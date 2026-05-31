//! Granular permission model for the SQL node.
//!
//! Permissions are configured via presets (`read_only`, `read_write`, `full`) with an
//! optional `deny` list for fine-tuning. When no permissions config is provided, defaults
//! to `read_only` (principle of least privilege).

use serde_json::Value;
use std::collections::HashSet;

/// SQL operations that can be allowed or denied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SqlOperation {
    Select,
    Insert,
    Update,
    Delete,
    CreateFunction,
    CreateTable,
    /// Always blocked — no preset enables this.
    Truncate,
    /// Always blocked on protected schemas.
    Drop,
    /// Always blocked on protected schemas.
    Alter,
}

impl SqlOperation {
    /// Parse an operation name from a string (used for `deny` list parsing).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "select" => Some(Self::Select),
            "insert" => Some(Self::Insert),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "create_function" => Some(Self::CreateFunction),
            "create_table" => Some(Self::CreateTable),
            "truncate" => Some(Self::Truncate),
            "drop" => Some(Self::Drop),
            "alter" => Some(Self::Alter),
            _ => None,
        }
    }
}

/// Permission presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionPreset {
    ReadOnly,
    ReadWrite,
    Full,
}

impl PermissionPreset {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "read_only" => Ok(Self::ReadOnly),
            "read_write" => Ok(Self::ReadWrite),
            "full" => Ok(Self::Full),
            other => Err(format!("Unknown permission preset: '{}'", other)),
        }
    }

    fn allowed_operations(&self) -> HashSet<SqlOperation> {
        match self {
            Self::ReadOnly => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set
            }
            Self::ReadWrite => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set.insert(SqlOperation::Insert);
                set.insert(SqlOperation::Update);
                set
            }
            Self::Full => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set.insert(SqlOperation::Insert);
                set.insert(SqlOperation::Update);
                set.insert(SqlOperation::Delete);
                set.insert(SqlOperation::CreateFunction);
                set.insert(SqlOperation::CreateTable);
                set
            }
        }
    }
}

/// Resolved permissions for a SQL node instance.
#[derive(Debug, Clone)]
pub struct SqlPermissions {
    allowed_ops: HashSet<SqlOperation>,
    allowed_schemas: HashSet<String>,
    sandbox_schema: String,
    tenant_user_id: Option<String>,
    tenant_column: String,
    auto_rls: bool,
    create_schemas_if_missing: bool,
}

/// Schemas that are always accessible for introspection (not configurable).
const INTROSPECTION_SCHEMAS: &[&str] = &["information_schema", "pg_catalog"];

impl SqlPermissions {
    /// Build permissions from the JSON config `permissions` object.
    /// If `config` is `None`, defaults to `read_only` with no schema restrictions.
    pub fn from_config(config: Option<&Value>) -> Result<Self, String> {
        let config = match config {
            Some(c) => c,
            None => {
                return Ok(Self {
                    allowed_ops: PermissionPreset::ReadOnly.allowed_operations(),
                    allowed_schemas: HashSet::new(),
                    sandbox_schema: "sandbox".to_string(),
                    tenant_user_id: None,
                    tenant_column: "user_id".to_string(),
                    auto_rls: false,
                    create_schemas_if_missing: true,
                });
            }
        };

        // Parse preset (default: read_only)
        let preset_str = config
            .get("preset")
            .and_then(|v| v.as_str())
            .unwrap_or("read_only");
        let preset = PermissionPreset::from_str(preset_str)?;
        let mut allowed_ops = preset.allowed_operations();

        // Apply deny list
        if let Some(deny_arr) = config.get("deny").and_then(|v| v.as_array()) {
            for deny_val in deny_arr {
                if let Some(deny_str) = deny_val.as_str() {
                    if let Some(op) = SqlOperation::from_str_loose(deny_str) {
                        allowed_ops.remove(&op);
                    }
                }
            }
        }

        // Parse allowed_schemas
        let allowed_schemas: HashSet<String> = config
            .get("allowed_schemas")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Parse sandbox_schema (default: "sandbox")
        let sandbox_schema = config
            .get("sandbox_schema")
            .and_then(|v| v.as_str())
            .unwrap_or("sandbox")
            .to_string();

        let tenant_user_id = config
            .get("tenant_user_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let tenant_column = config
            .get("tenant_column")
            .and_then(|v| v.as_str())
            .unwrap_or("user_id")
            .to_string();

        let auto_rls = config
            .get("auto_rls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Default: true (opt-out). Operators provision the listed schemas
        // declaratively; set to false to restore validate-only behavior.
        let create_schemas_if_missing = config
            .get("create_schemas_if_missing")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Ok(Self {
            allowed_ops,
            allowed_schemas,
            sandbox_schema,
            tenant_user_id,
            tenant_column,
            auto_rls,
            create_schemas_if_missing,
        })
    }

    /// Check if an operation is allowed.
    pub fn is_allowed(&self, op: &SqlOperation) -> bool {
        // Truncate, Drop, Alter are never directly allowed via presets
        match op {
            SqlOperation::Truncate => false,
            _ => self.allowed_ops.contains(op),
        }
    }

    /// Check if a schema is accessible.
    /// Introspection schemas (information_schema, pg_catalog) are always allowed.
    /// If `allowed_schemas` is empty, all schemas are allowed (no restriction).
    pub fn is_schema_allowed(&self, schema: &str) -> bool {
        if INTROSPECTION_SCHEMAS.contains(&schema) {
            return true;
        }
        if self.allowed_schemas.is_empty() {
            return true;
        }
        self.allowed_schemas.contains(schema)
    }

    /// The sandbox schema name where the agent can create functions/tables.
    pub fn sandbox_schema(&self) -> &str {
        &self.sandbox_schema
    }

    /// The tenant user ID for RLS isolation. None means no multi-tenancy.
    pub fn tenant_user_id(&self) -> Option<&str> {
        self.tenant_user_id.as_deref()
    }

    /// The column name used for tenant isolation (default: "user_id").
    pub fn tenant_column(&self) -> &str {
        &self.tenant_column
    }

    /// Whether to auto-create RLS policies during initialization.
    pub fn auto_rls(&self) -> bool {
        self.auto_rls
    }

    /// Whether the node should create any `allowed_schemas` that don't yet exist
    /// during initialization (operator-driven provisioning). Defaults to `true`.
    pub fn create_schemas_if_missing(&self) -> bool {
        self.create_schemas_if_missing
    }

    /// Iterate over the configured `allowed_schemas`. Empty means "no restriction"
    /// (all schemas allowed) and therefore nothing to provision.
    pub fn allowed_schemas_iter(&self) -> impl Iterator<Item = &str> {
        self.allowed_schemas.iter().map(String::as_str)
    }

    /// Return a human-readable summary for LLM context injection.
    pub fn describe_for_llm(&self) -> String {
        let ops: Vec<&str> = [
            (SqlOperation::Select, "SELECT"),
            (SqlOperation::Insert, "INSERT"),
            (SqlOperation::Update, "UPDATE"),
            (SqlOperation::Delete, "DELETE"),
            (SqlOperation::CreateFunction, "CREATE FUNCTION"),
            (SqlOperation::CreateTable, "CREATE TABLE"),
        ]
        .iter()
        .filter(|(op, _)| self.allowed_ops.contains(op))
        .map(|(_, name)| *name)
        .collect();

        format!(
            "Permissions: {} | Schemas: {}",
            ops.join(", "),
            if self.allowed_schemas.is_empty() {
                "all".to_string()
            } else {
                self.allowed_schemas
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_only_preset() {
        let perms = SqlPermissions::from_config(None).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(!perms.is_allowed(&SqlOperation::Insert));
        assert!(!perms.is_allowed(&SqlOperation::Update));
        assert!(!perms.is_allowed(&SqlOperation::Delete));
        assert!(!perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_read_write_preset() {
        let config = serde_json::json!({ "preset": "read_write" });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(perms.is_allowed(&SqlOperation::Insert));
        assert!(perms.is_allowed(&SqlOperation::Update));
        assert!(!perms.is_allowed(&SqlOperation::Delete));
        assert!(!perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_full_preset_with_deny() {
        let config = serde_json::json!({
            "preset": "full",
            "deny": ["delete"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(perms.is_allowed(&SqlOperation::Insert));
        assert!(perms.is_allowed(&SqlOperation::Update));
        assert!(!perms.is_allowed(&SqlOperation::Delete));
        assert!(perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_truncate_always_blocked() {
        let config = serde_json::json!({ "preset": "full" });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(!perms.is_allowed(&SqlOperation::Truncate));
    }

    #[test]
    fn test_allowed_schemas() {
        let config = serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["production", "analytics"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_schema_allowed("production"));
        assert!(perms.is_schema_allowed("analytics"));
        assert!(!perms.is_schema_allowed("secret_data"));
        // information_schema and pg_catalog always allowed (introspection)
        assert!(perms.is_schema_allowed("information_schema"));
        assert!(perms.is_schema_allowed("pg_catalog"));
    }

    #[test]
    fn test_sandbox_schema_defaults() {
        let config = serde_json::json!({
            "preset": "full",
            "allowed_schemas": ["production", "sandbox"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert_eq!(perms.sandbox_schema(), "sandbox");
    }

    #[test]
    fn test_no_config_defaults_read_only() {
        let perms = SqlPermissions::from_config(None).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(!perms.is_allowed(&SqlOperation::Insert));
    }

    #[test]
    fn test_tenant_fields_parsed() {
        let config = serde_json::json!({
            "preset": "read_write",
            "allowed_schemas": ["public"],
            "tenant_user_id": "user-abc-123",
            "tenant_column": "owner_id",
            "auto_rls": true
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert_eq!(perms.tenant_user_id(), Some("user-abc-123"));
        assert_eq!(perms.tenant_column(), "owner_id");
        assert!(perms.auto_rls());
    }

    #[test]
    fn test_tenant_defaults() {
        let config = serde_json::json!({
            "preset": "read_only",
            "tenant_user_id": "user-123"
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert_eq!(perms.tenant_user_id(), Some("user-123"));
        assert_eq!(perms.tenant_column(), "user_id");
        assert!(!perms.auto_rls());
    }

    #[test]
    fn test_no_tenant_backwards_compatible() {
        let config = serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["public"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert_eq!(perms.tenant_user_id(), None);
        assert!(!perms.auto_rls());
    }

    #[test]
    fn test_none_config_no_tenant() {
        let perms = SqlPermissions::from_config(None).unwrap();
        assert_eq!(perms.tenant_user_id(), None);
        assert_eq!(perms.tenant_column(), "user_id");
        assert!(!perms.auto_rls());
    }

    #[test]
    fn test_full_preset_includes_create_table() {
        let config = serde_json::json!({ "preset": "full" });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_allowed(&SqlOperation::CreateTable));
    }

    #[test]
    fn test_read_write_no_create_table() {
        let config = serde_json::json!({ "preset": "read_write" });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(!perms.is_allowed(&SqlOperation::CreateTable));
    }

    #[test]
    fn test_create_schemas_if_missing_defaults_true() {
        // Absent key with a config object.
        let config = serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["public"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.create_schemas_if_missing());

        // Absent config entirely.
        let perms = SqlPermissions::from_config(None).unwrap();
        assert!(perms.create_schemas_if_missing());
    }

    #[test]
    fn test_create_schemas_if_missing_explicit_false() {
        let config = serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["public"],
            "create_schemas_if_missing": false
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(!perms.create_schemas_if_missing());
    }

    #[test]
    fn test_allowed_schemas_iter() {
        let config = serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["analytics", "public"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        let mut schemas: Vec<&str> = perms.allowed_schemas_iter().collect();
        schemas.sort_unstable();
        assert_eq!(schemas, vec!["analytics", "public"]);
    }
}
