# Secure Values in HTTP Nodes - Architecture & Design

> ⚠️ **Historical design document.** This file describes the original (≈2026-04) design of the Secure Values feature. The implementation has evolved since then — `agent_session_id`-scoped lookups (2026-05-08), sliding 24-hour TTL with outbound masking (2026-05-11), and fail-fast on missing `SECURE_VALUES_KEY` (2026-06-07) all post-date the snippets below.
>
> **For current behavior, code, and operator guidance, consult:**
> - [`docs/developer_guide/13_security_strategy.md`](../developer_guide/13_security_strategy.md) — runtime semantics, strategies, FAQ, the `SECURE_VALUES_KEY` requirement
> - [`src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs) — current source of truth
> - [`docs/developer_guide/30_database_schema.md`](../developer_guide/30_database_schema.md#secure_value_mappings-postgresql-only) — current table schema
>
> Code snippets in this document are kept for archeological context only and should NOT be copied. In particular, anywhere this doc shows `.unwrap_or_default()` on `SECURE_VALUES_KEY` reflects the original aspirational design — the production code now hard-fails at startup if the env var is missing (commit `1e27039`, see [[feedback_secure_values_key_required]]).

## Executive Summary

Implement a **Secure Values** system where HTTP nodes can mark their outputs as sensitive. When `secure: true`:

1. All values in the output body are **hashed and replaced** with placeholders (e.g., `<token_1>`)
2. Real values are stored **encrypted in the database**
3. **LLM nodes** receive and see only **hashes** (can't access secrets)
4. **Non-LLM nodes** (HTTP, etc.) get values **automatically injected** from the DB before execution
5. **Auto-cleanup** when grafo terminates (success or error)

---

## Architecture Overview

### Layer Mapping (Hexagonal Architecture)

```
┌─────────────────────────────────────────────────────────┐
│                    APPLICATION LAYER                    │
│  ┌──────────────────────────────────────────────────┐   │
│  │  DagRunUseCase (run_use_case.rs)                │   │
│  │  • Orchestrates node execution                  │   │
│  │  • Calls SecureValueService for encrypt/inject  │   │
│  │  • Calls SecureValueService for cleanup         │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────┐
│                    DOMAIN LAYER                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │  SecureValueRepository (trait)                  │   │
│  │  • async fn persist(mapping)                    │   │
│  │  • async fn decrypt(hash, session_id)           │   │
│  │  • async fn cleanup(session_id)                 │   │
│  └──────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────┐   │
│  │  SecureValueService (use case)                  │   │
│  │  • async fn hash_output(output, config)         │   │
│  │  • async fn inject_secrets(inputs, session_id)  │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────┐
│                INFRASTRUCTURE LAYER                     │
│  ┌──────────────────────────────────────────────────┐   │
│  │  PostgresSecureValueRepository (impl)           │   │
│  │  • Manages secure_value_mappings table          │   │
│  │  • pgcrypto pgp_sym_encrypt/decrypt (not GCM)   │   │
│  │  • Index on (session_id, hash_key)              │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

---

## Database Schema

### Table: `secure_value_mappings`

```sql
CREATE TABLE secure_value_mappings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    
    -- Session & Lifecycle
    session_id VARCHAR(255) NOT NULL,
    source_node_id VARCHAR(255) NOT NULL,
    
    -- Mapping
    hash_key VARCHAR(255) NOT NULL,          -- e.g., "<token_1>", "<api_key_2>"
    encrypted_value BYTEA NOT NULL,          -- pgp_sym_encrypt(real_value)
    
    -- Metadata
    field_name VARCHAR(255),                 -- e.g., "token", "authorization"
    field_path VARCHAR(500),                 -- e.g., "body.credentials.token" (for nested)
    
    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT NOW(),
    expires_at TIMESTAMPTZ DEFAULT (NOW() + INTERVAL '1 hour'),
    
    -- Indexes
    UNIQUE(session_id, hash_key),
    INDEX idx_session_id (session_id),
    INDEX idx_expires_at (expires_at)
);

-- Index for cleanup queries
CREATE INDEX idx_session_expires ON secure_value_mappings(session_id, expires_at);
```

---

## Data Flow Diagrams

### 1. HTTP Node with `secure: true` → Output Hashing

```
HTTP Node Execution
        │
        ↓
┌─────────────────────────────────────────────────┐
│ HTTP Response: {status: 200, body: {           │
│   token: "sk_live_abc123xyz",                   │
│   user_id: "456",                              │
│   message: "success"                            │
│ }}                                             │
└─────────────────────────────────────────────────┘
        │
        ↓ [Check config.secure == true]
        ↓
┌─────────────────────────────────────────────────┐
│ SecureValueService::hash_output()               │
│ • Recursively traverse body                     │
│ • Generate hash_key: <token_1>, <user_id_1>   │
│ • Encrypt real value: pgp_sym_encrypt(value)    │
│ • Store in DB with session_id                   │
└─────────────────────────────────────────────────┘
        │
        ↓ Persist to DB
        ↓
┌─────────────────────────────────────────────────┐
│ secure_value_mappings INSERT:                   │
│ • (session_123, http_node_1, <token_1>,        │
│    encrypted(sk_live_abc123xyz), "token")      │
│ • (session_123, http_node_1, <user_id_1>,     │
│    encrypted(456), "user_id")                   │
└─────────────────────────────────────────────────┘
        │
        ↓ Return hashed output
        ↓
┌─────────────────────────────────────────────────┐
│ Output to next node: {status: 200, body: {     │
│   token: "<token_1>",                           │
│   user_id: "<user_id_1>",                       │
│   message: "success"                            │
│ }}                                             │
└─────────────────────────────────────────────────┘
```

### 2. Node Execution Flow (LLM vs Non-LLM)

```
┌─────────────────────────────────┐
│  DagRunUseCase::execute_node()  │
└──────────────┬──────────────────┘
               │
        ┌──────┴────────────────────────────┐
        │                                   │
        ↓                                   ↓
  [LLM Node?]                    [Non-LLM Node?]
        │ YES                              │ NO
        │                                  ↓
        │                    ┌─────────────────────────┐
        │                    │ SecureValueService::    │
        │                    │   inject_secrets(...)   │
        │                    │                         │
        │                    │ FOR each <token_N> IN   │
        │                    │ inputs:                 │
        │                    │  1. Query DB:           │
        │                    │     encrypted_value     │
        │                    │  2. Decrypt (AES)       │
        │                    │  3. Replace in inputs   │
        │                    └────────┬────────────────┘
        │                             │
        │    ┌────────────────────────┘
        │    │
        ↓    ↓
    ┌─────────────────────────────┐
    │  node.execute(inputs, ...) │
    │  • LLM: sees <token_1>     │
    │  • HTTP: sees "abc123"     │
    └────────────┬────────────────┘
                 │
                 ↓
         ┌───────────────┐
         │  Store output │
         └───────────────┘
```

### 3. Cleanup Flow

```
┌──────────────────────────────┐
│  DagRunUseCase TERMINATES    │
│  (success or error)          │
└──────────────┬───────────────┘
               │
               ↓
    ┌──────────────────────┐
    │ SecureValueService:: │
    │   cleanup(session_id)│
    │                      │
    │ DELETE FROM          │
    │   secure_value_mappings
    │ WHERE                │
    │   session_id = ?     │
    └──────────────────────┘
               │
               ↓
    ┌──────────────────────┐
    │ [DB confirms DELETE] │
    └──────────────────────┘
               │
               ↓ (fallback: cron task deletes expired)
    ┌──────────────────────┐
    │ Timeout Cleanup:     │
    │ expires_at < NOW()   │
    │ (every 5 min)        │
    └──────────────────────┘
```

---

## Core Components

### 1. Domain Trait: `SecureValueRepository`

**File:** `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs` (NEW)

```rust
use crate::dag_engine::domain::error::DagError;
use async_trait::async_trait;
use serde_json::Value;

/// Manages encryption, storage, and decryption of sensitive values
/// Implementation uses pgcrypto pgp_sym_encrypt (OpenPGP CFB, not AES-256-GCM) in the database
#[async_trait]
pub trait SecureValueRepository: Send + Sync {
    /// Store encrypted value mapping
    /// 
    /// # Arguments
    /// * `session_id` - Grafo execution session
    /// * `source_node_id` - HTTP node that generated the value
    /// * `hash_key` - Placeholder like "<token_1>"
    /// * `real_value` - Original sensitive value (encrypted before storage)
    /// * `field_name` - Human-readable field name (for audit)
    async fn persist(
        &self,
        session_id: &str,
        source_node_id: &str,
        hash_key: &str,
        real_value: &str,
        field_name: &str,
    ) -> Result<(), DagError>;

    /// Retrieve and decrypt a single value
    /// 
    /// Returns None if hash not found (shouldn't happen in normal flow)
    async fn decrypt(
        &self,
        session_id: &str,
        hash_key: &str,
    ) -> Result<Option<String>, DagError>;

    /// Delete all mappings for a session (cleanup after grafo)
    async fn cleanup(&self, session_id: &str) -> Result<(), DagError>;

    /// Delete expired mappings (fallback/safety net)
    async fn cleanup_expired(&self) -> Result<u64, DagError>;
}
```

### 2. Domain Service: `SecureValueService`

**File:** `src/libs/colmena/src/dag_engine/application/secure_value_service.rs` (NEW)

```rust
use crate::dag_engine::domain::{
    error::DagError,
    secure_value_repository::SecureValueRepository,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub struct SecureValueService {
    repo: Arc<dyn SecureValueRepository>,
}

impl SecureValueService {
    pub fn new(repo: Arc<dyn SecureValueRepository>) -> Self {
        Self { repo }
    }

    /// Hash and persist all sensitive values in output
    /// Returns output with placeholders
    pub async fn hash_output(
        &self,
        output: &Value,
        config: &Value,
        session_id: &str,
        source_node_id: &str,
    ) -> Result<Value, DagError> {
        // Check if secure flag is enabled
        if config.get("secure").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(output.clone());
        }

        let mut hashed = output.clone();
        let mut counter = 1;

        // Recursively hash all values in the output
        self.hash_value_recursive(
            &mut hashed,
            session_id,
            source_node_id,
            &mut counter,
        ).await?;

        Ok(hashed)
    }

    /// Recursively traverse and hash values
    async fn hash_value_recursive(
        &self,
        value: &mut Value,
        session_id: &str,
        source_node_id: &str,
        counter: &mut u32,
    ) -> Result<(), DagError> {
        match value {
            // Skip status (metadata), only hash in body
            Value::Object(map) if map.contains_key("status") => {
                if let Some(body) = map.get_mut("body") {
                    self.hash_value_recursive(body, session_id, source_node_id, counter)
                        .await?;
                }
            }
            // Recursively hash object fields
            Value::Object(map) => {
                for (_, v) in map.iter_mut() {
                    self.hash_value_recursive(v, session_id, source_node_id, counter)
                        .await?;
                }
            }
            // Recursively hash array elements
            Value::Array(arr) => {
                for v in arr.iter_mut() {
                    self.hash_value_recursive(v, session_id, source_node_id, counter)
                        .await?;
                }
            }
            // Hash string and numeric values
            Value::String(_) | Value::Number(_) => {
                let real_value = value.to_string();
                let hash_key = format!("<value_{}>", counter);
                *counter += 1;

                // Persist to DB
                self.repo.persist(
                    session_id,
                    source_node_id,
                    &hash_key,
                    &real_value,
                    "value", // Generic field name
                ).await?;

                // Replace with hash
                *value = Value::String(hash_key);
            }
            _ => {} // Skip null, bool, etc.
        }

        Ok(())
    }

    /// Inject real values before executing non-LLM nodes
    pub async fn inject_secrets(
        &self,
        inputs: &mut Value,
        session_id: &str,
    ) -> Result<(), DagError> {
        self.inject_secrets_recursive(inputs, session_id).await
    }

    /// Recursively replace placeholders with real values
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
                // Check if this is a placeholder: <value_N>
                if s.starts_with('<') && s.ends_with('>') {
                    if let Some(real) = self.repo.decrypt(session_id, s).await? {
                        *value = Value::String(real);
                    }
                    // If not found, leave as-is (shouldn't happen)
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Cleanup all values for a session
    pub async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        self.repo.cleanup(session_id).await
    }
}
```

### 3. Infrastructure Impl: `PostgresSecureValueRepository`

**File:** `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs` (NEW)

```rust
use crate::dag_engine::domain::{
    error::DagError,
    secure_value_repository::SecureValueRepository,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

pub struct PostgresSecureValueRepository {
    pool: PgPool,
}

impl PostgresSecureValueRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Apply migrations (idempotent)
    pub async fn migrate(&self) -> Result<(), DagError> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS secure_value_mappings (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                session_id VARCHAR(255) NOT NULL,
                source_node_id VARCHAR(255) NOT NULL,
                hash_key VARCHAR(255) NOT NULL,
                encrypted_value BYTEA NOT NULL,
                field_name VARCHAR(255),
                field_path VARCHAR(500),
                created_at TIMESTAMPTZ DEFAULT NOW(),
                expires_at TIMESTAMPTZ DEFAULT (NOW() + INTERVAL '1 hour'),
                UNIQUE(session_id, hash_key),
                INDEX idx_session_id (session_id),
                INDEX idx_expires_at (expires_at)
            )"#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Migration error: {}", e)))?;

        Ok(())
    }

    /// Encrypt using pgcrypto pgp_sym_encrypt (OpenPGP CFB, not AES-256-GCM; requires pgcrypto extension)
    fn encrypt(&self, value: &str) -> Result<Vec<u8>, DagError> {
        // Use pgcrypto: encrypt(value::bytea, key::bytea, 'aes')
        // Key is derived from environment: SECURE_VALUES_KEY
        let key = std::env::var("SECURE_VALUES_KEY")
            .map_err(|_| DagError::StateError(
                "SECURE_VALUES_KEY env var not set".to_string()
            ))?;
        
        // Placeholder: actual encryption happens in SQL
        Ok(value.as_bytes().to_vec())
    }

    /// Decrypt using pgcrypto pgp_sym_decrypt
    fn decrypt(&self, encrypted: &[u8]) -> Result<String, DagError> {
        // Placeholder: actual decryption happens in SQL
        String::from_utf8(encrypted.to_vec())
            .map_err(|_| DagError::StateError("Decryption error".to_string()))
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
        // Use PostgreSQL pgcrypto for encryption
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
        .bind(std::env::var("SECURE_VALUES_KEY").unwrap_or_default())
        .bind(field_name)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Persist error: {}", e)))?;

        Ok(())
    }

    async fn decrypt(
        &self,
        session_id: &str,
        hash_key: &str,
    ) -> Result<Option<String>, DagError> {
        let row = sqlx::query(
            r#"
            SELECT pgp_sym_decrypt(encrypted_value, $1) as decrypted
            FROM secure_value_mappings
            WHERE session_id = $2 AND hash_key = $3
            "#,
        )
        .bind(std::env::var("SECURE_VALUES_KEY").unwrap_or_default())
        .bind(session_id)
        .bind(hash_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Decrypt error: {}", e)))?;

        Ok(row.map(|r| r.get::<String, _>("decrypted")))
    }

    async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Cleanup error: {}", e)))?;

        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<u64, DagError> {
        let result = sqlx::query(
            "DELETE FROM secure_value_mappings WHERE expires_at < NOW()"
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Cleanup expired error: {}", e)))?;

        Ok(result.rows_affected())
    }
}
```

### 4. Integration: Modifications to `DagRunUseCase`

**File:** `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (MODIFY)

```rust
// In execute_node method:

async fn execute_node(
    &self,
    node_id: &str,
    node_inputs: NodeInputs,
    graph: &DagGraph,
) -> Result<Value, DagError> {
    let node = graph.get_node(node_id)?;
    let config = node.config.clone();

    // ─── STEP 1: Inject secrets for non-LLM nodes ───────────────────
    let mut prepared_inputs = node_inputs;
    if node.node_type != "llm" {
        // Auto-inject for all nodes except LLM
        let session_id = &self.dag_state.session_id;
        self.secure_value_service
            .inject_secrets(&mut prepared_inputs.into(), session_id)
            .await?;
    }

    // ─── STEP 2: Execute node ─────────────────────────────────────────
    let output = node
        .execute(&prepared_inputs, &config, &self.dag_state.global_shared_state)
        .await?;

    // ─── STEP 3: Hash output if secure: true ──────────────────────────
    let processed_output = self.secure_value_service
        .hash_output(&output, &config, &self.dag_state.session_id, node_id)
        .await?;

    // ─── STEP 4: Store in state ───────────────────────────────────────
    self.dag_state.all_outputs.insert(node_id.to_string(), processed_output.clone());

    Ok(processed_output)
}

// At end of DAG execution (success or error):

async fn finalize_dag(&self, status: DagRunStatus) -> Result<(), DagError> {
    // ... existing finalization code ...

    // Cleanup all secure values
    self.secure_value_service
        .cleanup(&self.dag_state.session_id)
        .await?;

    Ok(())
}
```

---

## Configuration in JSON DAGs

### Example: HTTP Node with `secure: true`

```json
{
  "nodes": [
    {
      "id": "fetch_token",
      "type": "http",
      "config": {
        "base_url": "https://api.amadeus.com",
        "endpoint": "/v1/security/oauth2/token",
        "method": "POST",
        "secure": true,
        "body": {
          "client_id": "${AMADEUS_CLIENT_ID}",
          "client_secret": "${AMADEUS_CLIENT_SECRET}"
        }
      }
    },
    {
      "id": "use_token",
      "type": "http",
      "config": {
        "base_url": "https://api.amadeus.com",
        "endpoint": "/v2/reference-data/locations",
        "method": "GET"
      },
      "inputs": {
        "bearer_token": "${fetch_token.token}"  // Will inject real value!
      }
    },
    {
      "id": "llm_plan",
      "type": "llm",
      "config": {
        "model": "gpt-4",
        "system": "You are a flight search agent"
      },
      "inputs": {
        "user_message": "Find flights for ${fetch_token.token}"
        // ^ LLM sees: <token_1> instead of real value
      }
    }
  ]
}
```

---

## Implementation Phases

### Phase 1: MVP (Core Functionality)
- [ ] Create `SecureValueRepository` trait
- [ ] Create `SecureValueService` with hash + inject
- [ ] Implement `PostgresSecureValueRepository` with pgcrypto
- [ ] Modify `DagRunUseCase` to call service
- [ ] Test with amadeus_flight_search_dynamic.json
- [ ] Add cleanup at DAG end

### Phase 2: Enhancement
- [ ] Add `secure_fields: ["token", "api_key"]` for granular control
- [ ] Background cleanup task (every 5 min)
- [ ] Audit logging (which node accessed which secret)
- [ ] Metrics (secret usage, decrypt attempts)

### Phase 3: Advanced
- [ ] Recursive field name tracking (body.auth.token)
- [ ] Secret rotation policies
- [ ] Integration with secrets management (Vault, AWS Secrets Manager)

---

## Security Considerations

| Aspect | Measure |
|--------|---------|
| **Encryption at Rest** | pgcrypto `pgp_sym_encrypt` (OpenPGP CFB, not AES-256-GCM) in PostgreSQL |
| **Encryption in Transit** | TLS/HTTPS for DB connections |
| **Key Management** | `SECURE_VALUES_KEY` env var (must be 32+ chars) |
| **LLM Isolation** | LLM nodes never see real values, only hashes |
| **Cleanup** | Auto-delete on DAG end + timeout cleanup |
| **Audit Trail** | field_name + source_node_id logged |
| **Access Control** | Only DB user with pgcrypto can decrypt |

---

## Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_hash_output_creates_placeholders() { }
    
    #[tokio::test]
    async fn test_inject_secrets_restores_values() { }
    
    #[tokio::test]
    async fn test_cleanup_removes_all_mappings() { }
}
```

### Integration Tests
```rust
// tests/secure_values_integration.rs

#[tokio::test]
async fn test_http_secure_to_llm_sees_hashes() {
    // 1. Execute HTTP node with secure: true
    // 2. Verify output contains <token_1>
    // 3. Execute LLM node, verify it sees <token_1>
    // 4. Verify DB has encrypted mapping
}

#[tokio::test]
async fn test_http_secure_to_http_injects_real() {
    // 1. Execute HTTP node with secure: true
    // 2. Execute second HTTP node
    // 3. Verify it gets real token value injected
}

#[tokio::test]
async fn test_cleanup_on_dag_end() {
    // 1. Execute DAG with secure HTTP
    // 2. Verify secure_value_mappings has entries
    // 3. DAG ends
    // 4. Verify all mappings deleted
}
```

### Test Graphs
- `tests/graphs/security/http_secure_basic.json` — Simple secure HTTP → output
- `tests/graphs/security/http_secure_to_llm.json` — Secure HTTP → LLM (should see hashes)
- `tests/graphs/security/http_secure_to_http.json` — Secure HTTP → HTTP (should inject)
- `tests/graphs/security/amadeus_secure.json` — Real-world example

---

## References

- [PostgreSQL pgcrypto](https://www.postgresql.org/docs/current/pgcrypto.html)
- [sqlx async query execution](https://github.com/launchbadge/sqlx)
- Related: `FINAL_DELIVERY.md` (field_mapping system)

---

## Updates — 2026-05

The original design above (HTTP node hashes outputs, LLM nodes see handles, non-LLM nodes get real values injected) remains the foundation. Subsequent specs extended it:

- **Interactive secret collection — `secure_suspend` node** ([spec](../superpowers/specs/2026-05-07-secure-suspend-node-design.md)). Pauses the DAG to ask the user for one or more secrets in a single batch; persists encrypted; returns only handles. Usable as top-level DAG node or LLM tool. Adds the missing "ask the user" path that complements "hash an HTTP response".

- **Inject covers `config`, not just `inputs`** ([spec](../superpowers/specs/2026-05-07-inject-secrets-in-config-design.md)). Original design assumed handles flow through edges into inputs. The canvas-builder pattern places handles directly in node `config`, so the engine now runs `inject_secrets` on both. ~5 LoC change in `run_use_case.rs`.

- **`llm_call` propagates `SUSPENDED` from tools** ([spec](../superpowers/specs/2026-05-08-llm-call-tool-suspend-design.md)). When a tool returns `__colmena_status: SUSPENDED`, the agent loop short-circuits, the DAG pauses, and on resume the pending tool is re-dispatched with the user's answer routed in. Enables `secure_suspend` as an LLM tool.

- **`agent_session_id`-first lookup** ([spec](../superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md)). Added `agent_session_id TEXT` column to `secure_value_mappings`. Lookup uses agent_session_id when set, falls back to session_id. Same pattern as `llm_node_history` and `dag_runs`. Closes the cross-session use case where a meta-agent persists in one session and the built agent consumes in a later session with the same stable agent identifier.

End-to-end validated 2026-05-08 against live Postgres + httpbin: meta-agent suspend/resume flow works using only `--agent-session-id`, ephemeral `session_id` rotates per CLI run.
