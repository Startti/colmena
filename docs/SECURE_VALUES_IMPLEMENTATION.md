# Secure Values - Step-by-Step Implementation Guide

## Files to Create & Modify

### ✅ PHASE 1: Create Domain Layer

#### File 1: `src/dag_engine/domain/secure_value_repository.rs` (NEW)

```rust
use crate::dag_engine::domain::error::DagError;
use async_trait::async_trait;

/// Repository trait for managing secure values (encrypted storage)
/// Implementations handle AES-256 encryption/decryption
#[async_trait]
pub trait SecureValueRepository: Send + Sync {
    /// Store a sensitive value with encryption
    /// 
    /// # Arguments
    /// * `session_id` - DAG execution session ID
    /// * `source_node_id` - ID of the HTTP node that generated this value
    /// * `hash_key` - Placeholder identifier (e.g., "<token_1>")
    /// * `real_value` - The actual sensitive value to encrypt
    /// * `field_name` - Human-readable field name for auditing
    async fn persist(
        &self,
        session_id: &str,
        source_node_id: &str,
        hash_key: &str,
        real_value: &str,
        field_name: &str,
    ) -> Result<(), DagError>;

    /// Retrieve and decrypt a value by its hash key
    /// Returns None if the hash key doesn't exist
    async fn decrypt(
        &self,
        session_id: &str,
        hash_key: &str,
    ) -> Result<Option<String>, DagError>;

    /// Delete all secure values for a session (cleanup after DAG)
    async fn cleanup(&self, session_id: &str) -> Result<(), DagError>;

    /// Delete expired values (safety net, called periodically)
    async fn cleanup_expired(&self) -> Result<u64, DagError>;
}
```

---

### ✅ PHASE 2: Create Application Service Layer

#### File 2: `src/dag_engine/application/secure_value_service.rs` (NEW)

```rust
use crate::dag_engine::domain::{
    error::DagError,
    secure_value_repository::SecureValueRepository,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// Business logic for hashing outputs and injecting secrets
pub struct SecureValueService {
    repo: Arc<dyn SecureValueRepository>,
}

impl SecureValueService {
    pub fn new(repo: Arc<dyn SecureValueRepository>) -> Self {
        Self { repo }
    }

    /// Process output from a secure HTTP node: hash all sensitive values
    /// 
    /// Returns the output with placeholders instead of real values,
    /// and stores encrypted mappings in the database.
    pub async fn hash_output(
        &self,
        output: &Value,
        config: &Value,
        session_id: &str,
        source_node_id: &str,
    ) -> Result<Value, DagError> {
        // If secure flag not set, return output unchanged
        if config.get("secure").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(output.clone());
        }

        let mut hashed = output.clone();
        let mut counter = 1u32;

        // Recursively traverse and hash all sensitive values
        self.hash_value_recursive(
            &mut hashed,
            session_id,
            source_node_id,
            &mut counter,
        )
        .await?;

        Ok(hashed)
    }

    /// Recursively traverse JSON structure and hash values
    /// Skips metadata fields like "status"
    async fn hash_value_recursive(
        &self,
        value: &mut Value,
        session_id: &str,
        source_node_id: &str,
        counter: &mut u32,
    ) -> Result<(), DagError> {
        match value {
            // For HTTP response: only hash the "body", not "status"
            Value::Object(map) if map.contains_key("status") => {
                if let Some(body) = map.get_mut("body") {
                    self.hash_value_recursive(body, session_id, source_node_id, counter)
                        .await?;
                }
            }
            // Recursively process object values
            Value::Object(map) => {
                for (_, v) in map.iter_mut() {
                    self.hash_value_recursive(v, session_id, source_node_id, counter)
                        .await?;
                }
            }
            // Recursively process array elements
            Value::Array(arr) => {
                for v in arr.iter_mut() {
                    self.hash_value_recursive(v, session_id, source_node_id, counter)
                        .await?;
                }
            }
            // Hash string and number values
            Value::String(_) | Value::Number(_) => {
                let real_value = value.to_string();
                let hash_key = format!("<value_{}>", counter);
                *counter += 1;

                // Persist encrypted mapping to database
                self.repo
                    .persist(
                        session_id,
                        source_node_id,
                        &hash_key,
                        &real_value,
                        "value",
                    )
                    .await?;

                // Replace actual value with placeholder
                *value = Value::String(hash_key);
            }
            // Skip null and boolean values (not sensitive)
            _ => {}
        }

        Ok(())
    }

    /// Inject real values back into inputs before non-LLM node execution
    /// Automatically detects placeholders (<value_N>) and replaces them
    pub async fn inject_secrets(
        &self,
        inputs: &mut Value,
        session_id: &str,
    ) -> Result<(), DagError> {
        self.inject_secrets_recursive(inputs, session_id).await
    }

    /// Recursively replace placeholders with decrypted values
    async fn inject_secrets_recursive(
        &self,
        value: &mut Value,
        session_id: &str,
    ) -> Result<(), DagError> {
        match value {
            Value::Object(map) => {
                for (_, v) in map.iter_mut() {
                    self.inject_secrets_recursive(v, session_id).await?;
                }
            }
            Value::Array(arr) => {
                for v in arr.iter_mut() {
                    self.inject_secrets_recursive(v, session_id).await?;
                }
            }
            Value::String(s) => {
                // Check if string is a placeholder: <value_N>
                if s.starts_with('<') && s.ends_with('>') && s.len() > 2 {
                    if let Some(real) = self.repo.decrypt(session_id, s).await? {
                        *value = Value::String(real);
                    }
                    // If not found in DB, leave as-is (shouldn't happen)
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Cleanup all secure values for a session
    pub async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        self.repo.cleanup(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use async_trait::async_trait;

    /// Mock repository for testing
    struct MockSecureValueRepository {
        storage: std::sync::Mutex<std::collections::HashMap<String, String>>,
    }

    #[async_trait]
    impl SecureValueRepository for MockSecureValueRepository {
        async fn persist(
            &self,
            _session_id: &str,
            _source_node_id: &str,
            hash_key: &str,
            real_value: &str,
            _field_name: &str,
        ) -> Result<(), DagError> {
            self.storage
                .lock()
                .unwrap()
                .insert(hash_key.to_string(), real_value.to_string());
            Ok(())
        }

        async fn decrypt(
            &self,
            _session_id: &str,
            hash_key: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(self.storage.lock().unwrap().get(hash_key).cloned())
        }

        async fn cleanup(&self, _session_id: &str) -> Result<(), DagError> {
            self.storage.lock().unwrap().clear();
            Ok(())
        }

        async fn cleanup_expired(&self) -> Result<u64, DagError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn test_hash_output_with_secure_flag() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(std::collections::HashMap::new()),
        });
        let service = SecureValueService::new(repo);

        let output = json!({
            "status": 200,
            "body": {
                "token": "sk_live_abc123",
                "user_id": "456"
            }
        });

        let config = json!({ "secure": true });

        let hashed = service
            .hash_output(&output, &config, "session_1", "http_node_1")
            .await
            .unwrap();

        // Status should be unchanged
        assert_eq!(hashed["status"], 200);

        // Body values should be hashed
        let token_val = hashed["body"]["token"].as_str().unwrap();
        assert!(token_val.starts_with('<') && token_val.ends_with('>'));

        let user_id_val = hashed["body"]["user_id"].as_str().unwrap();
        assert!(user_id_val.starts_with('<') && user_id_val.ends_with('>'));
    }

    #[tokio::test]
    async fn test_hash_output_without_secure_flag() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(std::collections::HashMap::new()),
        });
        let service = SecureValueService::new(repo);

        let output = json!({
            "status": 200,
            "body": {
                "token": "sk_live_abc123"
            }
        });

        let config = json!({ "secure": false });

        let result = service
            .hash_output(&output, &config, "session_1", "http_node_1")
            .await
            .unwrap();

        // Should be unchanged
        assert_eq!(result["body"]["token"].as_str(), Some("sk_live_abc123"));
    }

    #[tokio::test]
    async fn test_inject_secrets_restores_values() {
        let repo = Arc::new(MockSecureValueRepository {
            storage: std::sync::Mutex::new(std::collections::HashMap::new()),
        });
        let service = SecureValueService::new(repo);

        // First: hash
        let output = json!({
            "status": 200,
            "body": { "token": "sk_live_abc123" }
        });

        let config = json!({ "secure": true });
        let hashed = service
            .hash_output(&output, &config, "session_1", "http_node_1")
            .await
            .unwrap();

        // Then: inject
        let mut inputs = json!({
            "bearer_token": hashed["body"]["token"].clone()
        });

        service
            .inject_secrets(&mut inputs, "session_1")
            .await
            .unwrap();

        // Should be restored
        assert_eq!(inputs["bearer_token"].as_str(), Some("sk_live_abc123"));
    }
}
```

---

### ✅ PHASE 3: Create Infrastructure Layer

#### File 3: `src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs` (NEW)

```rust
use crate::dag_engine::domain::{
    error::DagError,
    secure_value_repository::SecureValueRepository,
};
use async_trait::async_trait;
use sqlx::PgPool;

/// PostgreSQL implementation using pgcrypto for AES-256 encryption
pub struct PostgresSecureValueRepository {
    pool: PgPool,
}

impl PostgresSecureValueRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run migrations to create the secure_value_mappings table
    pub async fn migrate(&self) -> Result<(), DagError> {
        // Enable pgcrypto extension
        sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                DagError::StateError(format!("Failed to enable pgcrypto: {}", e))
            })?;

        // Create table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS secure_value_mappings (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                
                -- Session and source
                session_id VARCHAR(255) NOT NULL,
                source_node_id VARCHAR(255) NOT NULL,
                
                -- Mapping: placeholder -> encrypted value
                hash_key VARCHAR(255) NOT NULL,
                encrypted_value BYTEA NOT NULL,
                
                -- Metadata
                field_name VARCHAR(255),
                
                -- Lifecycle
                created_at TIMESTAMPTZ DEFAULT NOW(),
                expires_at TIMESTAMPTZ DEFAULT (NOW() + INTERVAL '1 hour'),
                
                -- Indexes and constraints
                UNIQUE(session_id, hash_key)
            );
            
            CREATE INDEX IF NOT EXISTS idx_secure_session_id 
                ON secure_value_mappings(session_id);
            CREATE INDEX IF NOT EXISTS idx_secure_expires_at 
                ON secure_value_mappings(expires_at);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            DagError::StateError(format!("Failed to create secure_value_mappings table: {}", e))
        })?;

        Ok(())
    }
}

#[async_trait]
impl SecureValueRepository for PostgresSecureValueRepository {
    async fn persist(
        &self,
        session_id: &str,
        source_node_id: &str,
        hash_key: &str,
        real_value: &str,
        field_name: &str,
    ) -> Result<(), DagError> {
        let encryption_key =
            std::env::var("SECURE_VALUES_KEY").unwrap_or_else(|_| "default-key".to_string());

        sqlx::query(
            r#"
            INSERT INTO secure_value_mappings
                (session_id, source_node_id, hash_key, encrypted_value, field_name)
            VALUES ($1, $2, $3, pgp_sym_encrypt($4::text, $5), $6)
            ON CONFLICT (session_id, hash_key) DO UPDATE SET
                encrypted_value = EXCLUDED.encrypted_value,
                expires_at = NOW() + INTERVAL '1 hour'
            "#,
        )
        .bind(session_id)
        .bind(source_node_id)
        .bind(hash_key)
        .bind(real_value)
        .bind(&encryption_key)
        .bind(field_name)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            DagError::StateError(format!("Failed to persist secure value: {}", e))
        })?;

        Ok(())
    }

    async fn decrypt(
        &self,
        session_id: &str,
        hash_key: &str,
    ) -> Result<Option<String>, DagError> {
        let encryption_key =
            std::env::var("SECURE_VALUES_KEY").unwrap_or_else(|_| "default-key".to_string());

        let row = sqlx::query(
            r#"
            SELECT pgp_sym_decrypt(encrypted_value, $1) as decrypted
            FROM secure_value_mappings
            WHERE session_id = $2 AND hash_key = $3
            "#,
        )
        .bind(&encryption_key)
        .bind(session_id)
        .bind(hash_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            DagError::StateError(format!("Failed to decrypt value: {}", e))
        })?;

        Ok(row.map(|r| r.get::<String, _>("decrypted")))
    }

    async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Cleanup failed: {}", e)))?;

        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<u64, DagError> {
        let result = sqlx::query("DELETE FROM secure_value_mappings WHERE expires_at < NOW()")
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Cleanup expired failed: {}", e)))?;

        Ok(result.rows_affected())
    }
}
```

---

### ✅ PHASE 4: Update Module Exports

#### File 4: Modify `src/dag_engine/domain/mod.rs`

Add these lines:

```rust
pub mod secure_value_repository;

pub use secure_value_repository::SecureValueRepository;
```

---

#### File 5: Modify `src/dag_engine/application/mod.rs`

Add these lines:

```rust
pub mod secure_value_service;

pub use secure_value_service::SecureValueService;
```

---

#### File 6: Modify `src/dag_engine/infrastructure/persistence/mod.rs`

Add these lines:

```rust
pub mod postgres_secure_value_repository;

pub use postgres_secure_value_repository::PostgresSecureValueRepository;
```

---

### ✅ PHASE 5: Integrate into DagRunUseCase

#### File 7: Modify `src/dag_engine/application/run_use_case.rs`

**Step 1:** Add to imports at the top:

```rust
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::infrastructure::persistence::postgres_secure_value_repository::PostgresSecureValueRepository;
```

**Step 2:** Add field to `DagRunUseCase` struct:

```rust
pub struct DagRunUseCase {
    // ... existing fields ...
    secure_value_service: Arc<SecureValueService>,
}
```

**Step 3:** Update the constructor:

```rust
impl DagRunUseCase {
    pub fn new(
        state_repository: Arc<dyn DagStateRepository>,
        llm_service: Arc<LlmCallUseCase>,
        dag_state: DagRunState,
        pool: PgPool, // Add this parameter
    ) -> Self {
        let secure_value_repo = Arc::new(PostgresSecureValueRepository::new(pool.clone()));
        let secure_value_service = Arc::new(SecureValueService::new(secure_value_repo));

        Self {
            state_repository,
            llm_service,
            dag_state,
            secure_value_service,
        }
    }
}
```

**Step 4:** Modify `execute_node` method:

```rust
async fn execute_node(
    &self,
    node_id: &str,
    mut node_inputs: NodeInputs,
    graph: &DagGraph,
    // ... other parameters ...
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let node = graph.get_node(node_id)?;
    let config = node.config.clone();

    // ─── STEP 1: Inject secrets for non-LLM nodes ───────────────────
    if node.node_type != "llm" {
        // Convert NodeInputs to Value, inject, convert back
        let mut inputs_value: Value = serde_json::to_value(&node_inputs)?;
        self.secure_value_service
            .inject_secrets(&mut inputs_value, &self.dag_state.session_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        node_inputs = serde_json::from_value(inputs_value)?;
    }

    // ─── STEP 2: Execute node ─────────────────────────────────────────
    let output = node
        .execute(&node_inputs, &config, &self.dag_state.global_shared_state)
        .await?;

    // ─── STEP 3: Hash output if secure: true ──────────────────────────
    let processed_output = self.secure_value_service
        .hash_output(&output, &config, &self.dag_state.session_id, node_id)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

    // ─── STEP 4: Store in state ───────────────────────────────────────
    self.dag_state.all_outputs.insert(node_id.to_string(), processed_output.clone());

    Ok(processed_output)
}
```

**Step 5:** Add cleanup at end of DAG execution:

```rust
async fn finalize_graph(&self, status: DagRunStatus) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ... existing finalization code ...

    // Cleanup all secure values
    self.secure_value_service
        .cleanup(&self.dag_state.session_id)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

    Ok(())
}
```

---

## ✅ Implementation Checklist

- [ ] Create `src/dag_engine/domain/secure_value_repository.rs`
- [ ] Create `src/dag_engine/application/secure_value_service.rs`
- [ ] Create `src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`
- [ ] Update `src/dag_engine/domain/mod.rs` (exports)
- [ ] Update `src/dag_engine/application/mod.rs` (exports)
- [ ] Update `src/dag_engine/infrastructure/persistence/mod.rs` (exports)
- [ ] Update `src/dag_engine/application/run_use_case.rs` (integration)
- [ ] Run migrations: `sqlx migrate run`
- [ ] Build: `cargo build`
- [ ] Run tests: `cargo test`
- [ ] Test with amadeus_flight_search_dynamic.json

---

## Testing

### 1. Unit Tests (Included in service file)

```bash
cargo test secure_value_service::
```

### 2. Integration Test

Create `tests/secure_values_integration.rs`:

```rust
#[tokio::test]
async fn test_http_secure_to_llm_hashes_values() {
    // Load amadeus_flight_search_dynamic.json
    // Execute HTTP node with secure: true
    // Verify output contains <value_N> placeholders
    // Execute LLM node
    // Verify LLM sees hashes
    // Verify DB cleanup
}
```

### 3. Manual Test

```bash
# Set encryption key
export SECURE_VALUES_KEY="my-secret-key-at-least-32-chars"

# Run with secure graph
cargo run --bin dag_engine -- run tests/graphs/security/http_secure_basic.json
```

---

## Environment Variables

```bash
# Required for secure values encryption
SECURE_VALUES_KEY=your-secret-key-at-least-32-characters-long

# Optional: database
DATABASE_URL=postgres://user:pass@localhost/colmena
```

---

## Next Steps

1. Implement Phase 1 files
2. Run migrations
3. Test with existing amadeus graph
4. Add to `travel_agent_amadeus.json`: `"secure": true` on HTTP node
5. Monitor test execution
6. Move to Phase 2 (granular `secure_fields`)
