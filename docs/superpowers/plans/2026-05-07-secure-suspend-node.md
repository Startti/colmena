# Secure Suspend Node Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new `secure_suspend` ExecutableNode that pauses the DAG to ask the user for one or more secrets, persists them encrypted via the existing `SecureValueRepository`, and returns only opaque handles. Designed to be invoked as an LLM tool by the canvas-builder meta-agent.

**Architecture:** New file `secure_suspend.rs` under `infrastructure/nodes/`. Reuses the existing `SuspendNode` semantics (SUSPENDED status + `questions[]` output, resume via `__colmena_resume_answer`), and the existing `SecureValueService`/`SecureValueRepository` for encrypted persistence and automatic injection at execution time. Registered conditionally — only when `SecureValueService` is wired into the registry.

**Tech Stack:** Rust 1.95.0, async-trait, serde_json, regex, mockall (existing). No new crates required.

**Spec:** [docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md](../specs/2026-05-07-secure-suspend-node-design.md)

---

## File Structure

**New files:**

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` | The node: struct, ctor, `ExecutableNode` impl, parser, unit tests. ~250 LoC. |
| `tests/secure_suspend_integration.rs` | Integration test against a real Postgres `SecureValueRepository`. |
| `tests/graphs/basic/secure_suspend_smoke.json` | Minimal graph used by the integration test (one `secure_suspend` + one `log`). |

**Modified files:**

| Path | Change |
|---|---|
| `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs` | Add `exists(session_id, hash_key) -> Result<bool, DagError>` to the trait. |
| `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs` | Implement `exists`. |
| `src/libs/colmena/src/dag_engine/application/secure_value_service.rs` | Add `pub async fn handle_exists(...)` and `pub async fn persist_secret(...)` helpers. Update its inline mock to implement `exists`. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` | Add `pub mod secure_suspend;`. |
| `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` | Register `"secure_suspend"` conditionally on `secure_value_service.is_some()`. |
| `docs/node_configurations.json` | Add the new node entry. |
| `docs/agent_context/node_ports_reference.md` | Document ports/outputs. |

**Not touched (deliberately):** the engine, `SuspendNode`, the LLM tool dispatch path, `dag_tool_executor.rs`. The node integrates through existing extension points only.

---

## Task 1: Add `exists` to `SecureValueRepository` trait + update inline mock

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs`
- Modify: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs:178-211` (the `MockSecureValueRepository` inside `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** in `secure_value_service.rs` (append to the existing `#[cfg(test)] mod tests`):

```rust
#[tokio::test]
async fn test_repo_exists_true_after_persist() {
    let repo = Arc::new(MockSecureValueRepository {
        storage: std::sync::Mutex::new(HashMap::new()),
    });
    repo.persist("s1", "n1", "<sv_token>", "real", "secret")
        .await
        .unwrap();
    assert!(repo.exists("s1", "<sv_token>").await.unwrap());
    assert!(!repo.exists("s1", "<sv_other>").await.unwrap());
}
```

- [ ] **Step 2: Run the test, confirm it fails to compile**

Run: `cargo test --lib -p colmena_dag_engine secure_value_service::tests::test_repo_exists_true_after_persist 2>&1 | tail -20`
Expected: error E0599 — no method named `exists` found on `Arc<MockSecureValueRepository>`.

- [ ] **Step 3: Add the trait method** to `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs`. The full file becomes:

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
    async fn decrypt(&self, session_id: &str, hash_key: &str) -> Result<Option<String>, DagError>;

    /// Check whether a hash_key already exists in this session.
    /// Cheaper than `decrypt` when only existence matters and avoids loading
    /// the secret value into memory unnecessarily.
    async fn exists(&self, session_id: &str, hash_key: &str) -> Result<bool, DagError>;

    /// Delete all secure values for a session (cleanup after DAG)
    async fn cleanup(&self, session_id: &str) -> Result<(), DagError>;

    /// Delete expired values (safety net, called periodically)
    async fn cleanup_expired(&self) -> Result<u64, DagError>;
}
```

- [ ] **Step 4: Implement `exists` in the inline mock** in `secure_value_service.rs`. Add this method to `impl SecureValueRepository for MockSecureValueRepository` (between `decrypt` and `cleanup`):

```rust
async fn exists(
    &self,
    _session_id: &str,
    hash_key: &str,
) -> Result<bool, DagError> {
    Ok(self.storage.lock().unwrap().contains_key(hash_key))
}
```

- [ ] **Step 5: Run the test, confirm it passes**

Run: `cargo test --lib -p colmena_dag_engine secure_value_service::tests::test_repo_exists_true_after_persist`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 6: Run all repository-touching tests to ensure nothing broke**

Run: `cargo test --lib -p colmena_dag_engine secure_value`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs \
        src/libs/colmena/src/dag_engine/application/secure_value_service.rs
git commit -m "feat(secure-values): add exists() to SecureValueRepository trait"
```

---

## Task 2: Implement `exists` in `PostgresSecureValueRepository`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`

- [ ] **Step 1: Read the existing file to find the `decrypt` impl as a template**

Run: `grep -n "fn decrypt\|fn persist\|fn cleanup\|impl SecureValueRepository" src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs`
Note the line numbers and copy the pattern decrypt uses (it does the SELECT — we'll do a SELECT 1 EXISTS).

- [ ] **Step 2: Write the failing integration-style test** at the bottom of the file (inside its `#[cfg(test)] mod tests` if one exists; otherwise add the module). Mark it `#[ignore]`:

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn test_postgres_exists_returns_false_for_unknown_key() {
    use sqlx::postgres::PgPoolOptions;
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new().connect(&url).await.unwrap();
    let repo = PostgresSecureValueRepository::new(pool);
    let exists = repo
        .exists("nonexistent_session_xyz", "<sv_nope>")
        .await
        .unwrap();
    assert!(!exists);
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn test_postgres_exists_returns_true_after_persist() {
    use sqlx::postgres::PgPoolOptions;
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new().connect(&url).await.unwrap();
    let repo = PostgresSecureValueRepository::new(pool);
    let session = format!("test_session_{}", uuid::Uuid::new_v4());
    repo.persist(&session, "node1", "<sv_x>", "secret_value", "test")
        .await
        .unwrap();
    assert!(repo.exists(&session, "<sv_x>").await.unwrap());
    repo.cleanup(&session).await.unwrap();
}
```

If `uuid` is not in `Cargo.toml` dev-dependencies, fall back to `format!("test_session_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0))`.

- [ ] **Step 3: Run the tests, confirm they fail with "no method `exists`"**

Run: `cargo test --lib -p colmena_dag_engine -- --ignored postgres_exists 2>&1 | tail -20`
Expected: compile error E0599.

- [ ] **Step 4: Implement `exists` in `impl SecureValueRepository for PostgresSecureValueRepository`**. Add the method (mirror the structure of the existing `decrypt`):

```rust
async fn exists(
    &self,
    session_id: &str,
    hash_key: &str,
) -> Result<bool, DagError> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM secure_values WHERE session_id = $1 AND hash_key = $2)"
    )
    .bind(session_id)
    .bind(hash_key)
    .fetch_optional(&self.pool)
    .await
    .map_err(|e| DagError::StateError(format!("secure_values exists query failed: {e}")))?;
    Ok(row.map(|(b,)| b).unwrap_or(false))
}
```

If the actual table or column names differ, run `grep -n "FROM\|INSERT INTO\|UPDATE" src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs` to get the correct names and adjust the SQL.

- [ ] **Step 5: Run the ignored tests against a live DB**

Run: `source .env && cargo test --lib -p colmena_dag_engine -- --ignored postgres_exists`
Expected: both `test_postgres_exists_returns_false_for_unknown_key` and `test_postgres_exists_returns_true_after_persist` pass.

- [ ] **Step 6: Run cargo check on the whole crate to confirm no warnings (deny-warnings is on)**

Run: `cargo check --all-targets`
Expected: clean, no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs
git commit -m "feat(secure-values): implement exists() for PostgresSecureValueRepository"
```

---

## Task 3: Add `handle_exists` and `persist_secret` to `SecureValueService`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs`

These two service-level methods give `secure_suspend` a clean API without exposing the raw repo.

- [ ] **Step 1: Write the failing tests** at the bottom of the existing `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn test_handle_exists_after_persist_secret() {
    let repo = Arc::new(MockSecureValueRepository {
        storage: std::sync::Mutex::new(HashMap::new()),
    });
    let service = SecureValueService::new(repo);
    service
        .persist_secret("session_a", "node1", "amadeus_client_id", "real_id_value")
        .await
        .unwrap();
    assert!(
        service
            .handle_exists("session_a", "<sv_amadeus_client_id>")
            .await
            .unwrap()
    );
    assert!(
        !service
            .handle_exists("session_a", "<sv_unknown>")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_persist_secret_can_be_decrypted_via_inject() {
    let repo = Arc::new(MockSecureValueRepository {
        storage: std::sync::Mutex::new(HashMap::new()),
    });
    let service = SecureValueService::new(repo);
    service
        .persist_secret("s2", "node1", "tok", "real_token_xyz")
        .await
        .unwrap();
    let mut inputs = json!({"bearer": "<sv_tok>"});
    service.inject_secrets(&mut inputs, "s2").await.unwrap();
    assert_eq!(inputs["bearer"].as_str(), Some("real_token_xyz"));
}
```

- [ ] **Step 2: Run the tests to confirm failure**

Run: `cargo test --lib -p colmena_dag_engine secure_value_service::tests::test_handle_exists 2>&1 | tail -10`
Expected: error E0599 — `persist_secret` and `handle_exists` not found.

- [ ] **Step 3: Add the methods** to `impl SecureValueService` (just after `inject_secrets`, before `cleanup`):

```rust
/// Store a single named secret as `<sv_<name>>` and persist its real value.
/// The handle returned to callers is `format!("<sv_{}>", name)`.
/// Used by the `secure_suspend` node to record user-supplied secrets.
pub async fn persist_secret(
    &self,
    session_id: &str,
    source_node_id: &str,
    name: &str,
    real_value: &str,
) -> Result<String, DagError> {
    let handle = format!("<sv_{}>", name);
    self.repo
        .persist(session_id, source_node_id, &handle, real_value, "secret")
        .await?;
    Ok(handle)
}

/// Check whether a handle is already registered in this session.
/// Used by `secure_suspend` to detect collisions before persisting.
pub async fn handle_exists(
    &self,
    session_id: &str,
    handle: &str,
) -> Result<bool, DagError> {
    self.repo.exists(session_id, handle).await
}
```

- [ ] **Step 4: Run the tests, confirm pass**

Run: `cargo test --lib -p colmena_dag_engine secure_value_service::tests`
Expected: all tests pass (including the existing ones plus the two new).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/secure_value_service.rs
git commit -m "feat(secure-values): add persist_secret and handle_exists service methods"
```

---

## Task 4: Scaffold `SecureSuspendNode` struct and stub `ExecutableNode` impl

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

This task creates the file with the type, constructor and a minimal `execute` that returns a placeholder error. The next tasks fill in real behavior.

- [ ] **Step 1: Add the module declaration** to `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`. The file becomes:

```rust
// Hacemos públicos los módulos de nodos
pub mod api_explorer;
pub mod critic;
pub mod current_time;
pub mod debug;
pub mod document_nodes;
pub mod echo_toolkit;
pub mod extraction;
pub mod http;
pub mod input;
pub mod llm;
pub mod llm_synthetic_tools;
pub mod loop_controller;
pub mod math;
pub mod orchestrator;
pub mod output;
pub mod planner;
pub mod python_node;
pub mod reactor;
pub mod secure_suspend;
pub mod socketio;
pub mod sql;
pub mod subgraph;
pub mod suspend;
pub mod task_memory_writer;
pub mod tavily_client;
pub mod trigger;
```

- [ ] **Step 2: Create `secure_suspend.rs` with the scaffold**:

```rust
//! `secure_suspend` — pauses the DAG to ask the user for one or more secrets,
//! persists each encrypted, and returns only opaque handles `<sv_<name>>`.
//!
//! See `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`.

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

/// Node that batches user-secret collection through the suspend mechanism
/// and stores the answers in the encrypted secure-values table.
pub struct SecureSuspendNode {
    secure_value_service: Arc<SecureValueService>,
}

impl SecureSuspendNode {
    pub fn new(secure_value_service: Arc<SecureValueService>) -> Self {
        Self {
            secure_value_service,
        }
    }
}

#[async_trait]
impl ExecutableNode for SecureSuspendNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // Implementation lands in subsequent tasks.
        Err("secure_suspend: not implemented".into())
    }

    fn default_input(&self) -> Option<&str> {
        Some("secrets")
    }

    fn default_output(&self) -> Option<&str> {
        Some("handles")
    }

    fn schema(&self) -> Value {
        json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::secure_value_repository::SecureValueRepository;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Test double that records persist calls and supports exists.
    pub(super) struct StubRepo {
        pub(super) storage: Mutex<HashMap<String, String>>,
    }

    impl StubRepo {
        pub(super) fn new() -> Arc<Self> {
            Arc::new(Self {
                storage: Mutex::new(HashMap::new()),
            })
        }
    }

    #[async_trait]
    impl SecureValueRepository for StubRepo {
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
        async fn exists(
            &self,
            _session_id: &str,
            hash_key: &str,
        ) -> Result<bool, DagError> {
            Ok(self.storage.lock().unwrap().contains_key(hash_key))
        }
        async fn cleanup(&self, _session_id: &str) -> Result<(), DagError> {
            self.storage.lock().unwrap().clear();
            Ok(())
        }
        async fn cleanup_expired(&self) -> Result<u64, DagError> {
            Ok(0)
        }
    }

    pub(super) fn build_node() -> (SecureSuspendNode, Arc<StubRepo>) {
        let repo = StubRepo::new();
        let svc = Arc::new(SecureValueService::new(repo.clone()));
        (SecureSuspendNode::new(svc), repo)
    }

    pub(super) fn inputs_with(node_id: &str, session_id: &str) -> NodeInputs {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("__node_id".into(), Value::String(node_id.into()));
        m.insert(
            "__colmena_session_id".into(),
            Value::String(session_id.into()),
        );
        m
    }

    #[tokio::test]
    async fn scaffold_returns_not_implemented_for_now() {
        let (node, _repo) = build_node();
        let mut state = Value::Null;
        let result = node
            .execute(
                &inputs_with("ask_secret", "s1"),
                &json!({}),
                &mut state,
                None,
            )
            .await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Build & run the scaffold test**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::scaffold_returns_not_implemented_for_now`
Expected: pass.

- [ ] **Step 4: Run cargo check to confirm no warnings**

Run: `cargo check --all-targets`
Expected: clean (the unused `secure_value_service` field is allowed because it's read in subsequent tasks; if Clippy complains add `#[allow(dead_code)]` only if necessary — preferred is to leave it; the field is used through the impl in the next task).

If a warning appears for `secure_value_service` being unused: do NOT silence with `_` rename. Instead, proceed straight to Task 5 within the same commit and leverage it there. To unblock the build alone here, run `cargo build --tests --lib -p colmena_dag_engine` and confirm the field is consumed by the test scaffold via `build_node` — the constructor reads it.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(secure-suspend): scaffold node skeleton with stub execute"
```

---

## Task 5: Config validation in suspend-path

Validates `secrets` is a non-empty array, each item has a valid `name` (regex `^[a-z][a-z0-9_]{2,63}$`), names are unique, questions are unique. Validation runs **before** emitting SUSPENDED.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

- [ ] **Step 1: Add a regex dep guard**. First check if `regex` is already in `Cargo.toml`:

Run: `grep -n '^regex' src/libs/colmena/Cargo.toml`
Expected: a line like `regex = "1"` (or similar). If absent, add `regex = "1"` under `[dependencies]` and run `cargo build`.

- [ ] **Step 2: Add the validation tests** into `secure_suspend.rs` `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn validation_rejects_missing_secrets() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let result = node
        .execute(
            &inputs_with("ask", "s1"),
            &json!({}),
            &mut state,
            None,
        )
        .await;
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("secrets list missing or empty"),
        "got: {msg}"
    );
}

#[tokio::test]
async fn validation_rejects_empty_secrets_array() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let result = node
        .execute(
            &inputs_with("ask", "s1"),
            &json!({"secrets": []}),
            &mut state,
            None,
        )
        .await;
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("secrets list missing or empty"));
}

#[tokio::test]
async fn validation_rejects_invalid_name() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [{ "question": "Q", "name": "BadName" }]
    });
    let result = node
        .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
        .await;
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("name 'BadName' invalid"), "got: {msg}");
}

#[tokio::test]
async fn validation_rejects_duplicate_names() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [
            { "question": "Q1", "name": "dup_name" },
            { "question": "Q2", "name": "dup_name" }
        ]
    });
    let result = node
        .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
        .await;
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("duplicate name 'dup_name'"), "got: {msg}");
}

#[tokio::test]
async fn validation_rejects_duplicate_questions() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [
            { "question": "Same?", "name": "n_one" },
            { "question": "Same?", "name": "n_two" }
        ]
    });
    let result = node
        .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
        .await;
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("duplicate question text"),
        "got: {msg}"
    );
}
```

- [ ] **Step 3: Run the tests, confirm they fail**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::validation`
Expected: 5 failures, each with "secure_suspend: not implemented" instead of the expected message.

- [ ] **Step 4: Implement the validation logic.** Replace the stub `execute` body and add a private `Secret` struct + parser. The complete relevant section of `secure_suspend.rs` becomes:

```rust
use once_cell::sync::Lazy;
use regex::Regex;

static NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_]{2,63}$").expect("name regex"));

#[derive(Debug, Clone)]
struct Secret {
    question: String,
    name: String,
}

fn parse_and_validate_secrets(config: &Value) -> Result<Vec<Secret>, String> {
    let arr = config
        .get("secrets")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "secure_suspend: secrets list missing or empty".to_string())?;

    let mut out: Vec<Secret> = Vec::with_capacity(arr.len());
    for item in arr {
        let question = item
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "secure_suspend: each secret must have a 'question' string".to_string()
            })?;
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "secure_suspend: each secret must have a 'name' string".to_string()
            })?;
        if !NAME_RE.is_match(name) {
            return Err(format!(
                "secure_suspend: name '{name}' invalid (expected lowercase slug, 3-64 chars)"
            ));
        }
        out.push(Secret {
            question: question.to_string(),
            name: name.to_string(),
        });
    }

    // Uniqueness checks (O(n^2) is fine — list is bounded).
    for i in 0..out.len() {
        for j in (i + 1)..out.len() {
            if out[i].name == out[j].name {
                return Err(format!(
                    "secure_suspend: duplicate name '{}' in secrets list",
                    out[i].name
                ));
            }
            if out[i].question == out[j].question {
                return Err(
                    "secure_suspend: duplicate question text — make each question unique"
                        .to_string(),
                );
            }
        }
    }

    Ok(out)
}
```

Then update `execute` to call validation (still returning a placeholder error after that — emission lands in Task 6):

```rust
async fn execute(
    &self,
    _inputs: &NodeInputs,
    config: &Value,
    _global_state: &mut Value,
    _observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let _secrets = parse_and_validate_secrets(config).map_err(|e| {
        Box::<dyn Error + Send + Sync>::from(e)
    })?;
    // Emission of SUSPENDED comes in Task 6.
    Err("secure_suspend: emission not implemented".into())
}
```

- [ ] **Step 5: Add `once_cell` dep guard and `regex` dep**

Run: `grep -n '^regex\|^once_cell' src/libs/colmena/Cargo.toml`
If `regex` and/or `once_cell` are missing under `[dependencies]`, add:
```toml
once_cell = "1"
regex = "1"
```

- [ ] **Step 6: Run the validation tests**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::validation`
Expected: 5 passes.

- [ ] **Step 7: Update the scaffold test** (it now reaches the post-validation error). Replace `scaffold_returns_not_implemented_for_now` with a meaningful test that uses a valid config:

```rust
#[tokio::test]
async fn execute_with_valid_config_reaches_emission_phase() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [{"question": "Q?", "name": "valid_name"}]
    });
    let err = node
        .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("emission not implemented"));
}
```

- [ ] **Step 8: Run all node tests**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend`
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs \
        src/libs/colmena/Cargo.toml
git commit -m "feat(secure-suspend): validate secrets list, names regex, uniqueness"
```

---

## Task 6: Suspend-path emission of `questions[]`

When `__colmena_resume_answer` is absent and validation passes, emit the SUSPENDED status with one `questions[]` entry per secret, all of `type: "secret"`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

- [ ] **Step 1: Add the test**

```rust
#[tokio::test]
async fn suspend_path_emits_n_questions_with_secret_type() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [
            { "question": "Cliente ID?",     "name": "amadeus_client_id" },
            { "question": "Cliente secret?", "name": "amadeus_client_secret" }
        ]
    });
    let out = node
        .execute(&inputs_with("ask_creds", "sx"), &cfg, &mut state, None)
        .await
        .unwrap();

    assert_eq!(out["__colmena_status"], "SUSPENDED");
    let qs = out["questions"].as_array().expect("questions array");
    assert_eq!(qs.len(), 2);
    assert_eq!(qs[0]["question"], "Cliente ID?");
    assert_eq!(qs[0]["type"], "secret");
    assert_eq!(qs[0]["id"], "ask_creds__1");
    assert_eq!(qs[0]["options"], Value::Null);
    assert_eq!(qs[1]["question"], "Cliente secret?");
    assert_eq!(qs[1]["id"], "ask_creds__2");
}

#[tokio::test]
async fn suspend_path_uses_explicit_id_from_config() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "id": "custom_block",
        "secrets": [{"question": "Q?", "name": "n_a"}]
    });
    let out = node
        .execute(&inputs_with("ignored_node_id", "sx"), &cfg, &mut state, None)
        .await
        .unwrap();
    assert_eq!(out["questions"][0]["id"], "custom_block__1");
}
```

- [ ] **Step 2: Run the tests, confirm failure**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::suspend_path`
Expected: 2 failures with "emission not implemented".

- [ ] **Step 3: Implement the emission.** Update `execute` to detect the suspend-path (no `__colmena_resume_answer` in inputs) and emit:

```rust
async fn execute(
    &self,
    inputs: &NodeInputs,
    config: &Value,
    _global_state: &mut Value,
    _observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let secrets = parse_and_validate_secrets(config)
        .map_err(|e| Box::<dyn Error + Send + Sync>::from(e))?;

    // Resume-path: handled in Task 7. For now, fall through to suspend.
    if inputs.get("__colmena_resume_answer").is_some() {
        return Err("secure_suspend: resume not implemented".into());
    }

    // Suspend-path emission.
    let base_id = config
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            inputs
                .get("__node_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "secure_suspend".to_string());

    let questions: Vec<Value> = secrets
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "id": format!("{base_id}__{}", i + 1),
                "question": s.question,
                "type": "secret",
                "options": Value::Null
            })
        })
        .collect();

    Ok(json!({
        "__colmena_status": "SUSPENDED",
        "questions": questions
    }))
}
```

- [ ] **Step 4: Run the tests, confirm pass**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::suspend_path`
Expected: 2 passes. Update or remove `execute_with_valid_config_reaches_emission_phase` (it's superseded — replace its assertion to expect the SUSPENDED output now):

```rust
#[tokio::test]
async fn execute_with_valid_config_emits_suspended() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [{"question": "Q?", "name": "valid_name"}]
    });
    let out = node
        .execute(&inputs_with("ask", "s1"), &cfg, &mut state, None)
        .await
        .unwrap();
    assert_eq!(out["__colmena_status"], "SUSPENDED");
}
```

- [ ] **Step 5: Run all node tests**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "feat(secure-suspend): suspend-path emits questions[] with type=secret"
```

---

## Task 7: Resume answer parser

Implements the anchored parser: given the answer string and the `Vec<Secret>` from config, returns the values per secret.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

- [ ] **Step 1: Add parser tests**

```rust
#[test]
fn parser_extracts_two_values_in_order() {
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    let answer = "Q1?\nval-one\nQ2?\nval-two";
    let values = parse_answers(answer, &secrets).unwrap();
    assert_eq!(values, vec!["val-one".to_string(), "val-two".to_string()]);
}

#[test]
fn parser_preserves_internal_newlines_in_value() {
    let secrets = vec![
        Secret { question: "Paste private key:".into(), name: "pk".into() },
        Secret { question: "Paste public key:".into(), name: "pubk".into() },
    ];
    let multiline = "-----BEGIN-----\nline1\nline2\n-----END-----";
    let answer = format!(
        "Paste private key:\n{multiline}\nPaste public key:\nABCDEF"
    );
    let values = parse_answers(&answer, &secrets).unwrap();
    assert_eq!(values[0], multiline);
    assert_eq!(values[1], "ABCDEF");
}

#[test]
fn parser_errors_on_missing_question() {
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    let answer = "Q1?\nval-one";
    let err = parse_answers(answer, &secrets).unwrap_err();
    assert!(err.contains("missing answer for secret 'n2'"), "got: {err}");
}

#[test]
fn parser_errors_on_empty_value() {
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    // No newline-value between Q1? and Q2? → empty value for n1.
    let answer = "Q1?\nQ2?\nv2";
    let err = parse_answers(answer, &secrets).unwrap_err();
    assert!(err.contains("empty value for secret 'n1'"), "got: {err}");
}

#[test]
fn parser_errors_when_questions_out_of_order() {
    let secrets = vec![
        Secret { question: "FIRST?".into(), name: "n1".into() },
        Secret { question: "SECOND?".into(), name: "n2".into() },
    ];
    let answer = "SECOND?\nv2\nFIRST?\nv1";
    let err = parse_answers(answer, &secrets).unwrap_err();
    assert!(err.contains("missing answer for secret 'n1'"), "got: {err}");
}
```

- [ ] **Step 2: Run the tests, confirm failure**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::parser_`
Expected: compile error — `parse_answers` not defined.

- [ ] **Step 3: Implement `parse_answers`** (private fn). Add to `secure_suspend.rs`:

```rust
/// Anchored parser: each `secret.question` must appear in `answer` in the same
/// order as in `secrets`. The value associated with `secrets[i]` is everything
/// between the `\n` after question i and the start of question i+1 (or end of
/// string for the last one). Trailing `\n` is stripped from the value but
/// internal newlines are preserved. Empty values are rejected.
fn parse_answers(answer: &str, secrets: &[Secret]) -> Result<Vec<String>, String> {
    let mut positions: Vec<usize> = Vec::with_capacity(secrets.len());
    let mut search_from = 0usize;
    for s in secrets {
        match answer[search_from..].find(s.question.as_str()) {
            Some(rel) => {
                let abs = search_from + rel;
                positions.push(abs);
                search_from = abs + s.question.len();
            }
            None => {
                return Err(format!(
                    "secure_suspend: missing answer for secret '{}' (question not found in response)",
                    s.name
                ));
            }
        }
    }

    let mut values: Vec<String> = Vec::with_capacity(secrets.len());
    for i in 0..secrets.len() {
        let q_end = positions[i] + secrets[i].question.len();
        // Skip the single newline immediately after the question, if present.
        let value_start = if answer[q_end..].starts_with('\n') {
            q_end + 1
        } else {
            q_end
        };
        let value_end = if i + 1 < secrets.len() {
            positions[i + 1]
        } else {
            answer.len()
        };
        let raw = &answer[value_start..value_end];
        // Strip trailing newlines but keep internal ones.
        let trimmed = raw.trim_end_matches('\n');
        if trimmed.is_empty() {
            return Err(format!(
                "secure_suspend: empty value for secret '{}'",
                secrets[i].name
            ));
        }
        values.push(trimmed.to_string());
    }
    Ok(values)
}
```

- [ ] **Step 4: Run the parser tests, confirm pass**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::parser_`
Expected: 5 passes.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "feat(secure-suspend): anchored parser for resume answer string"
```

---

## Task 8: Resume-path persistence + handles map

Wire the parser into `execute` for the resume-path: parse, persist each secret via `SecureValueService::persist_secret`, check collision via `handle_exists`, return the handles map. Crucially the output must NEVER contain the real values.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

- [ ] **Step 1: Add resume-path tests**

```rust
#[tokio::test]
async fn resume_persists_two_secrets_and_returns_handles_map() {
    let (node, repo) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [
            { "question": "Cliente ID?",     "name": "amadeus_client_id" },
            { "question": "Cliente secret?", "name": "amadeus_client_secret" }
        ]
    });
    let mut inputs = inputs_with("ask_creds", "sx");
    inputs.insert(
        "__colmena_resume_answer".into(),
        Value::String(
            "Cliente ID?\nABC-CLI-ID\nCliente secret?\nXYZ-CLI-SEC".into(),
        ),
    );

    let out = node
        .execute(&inputs, &cfg, &mut state, None)
        .await
        .unwrap();

    assert_eq!(out["status"], "resumed");
    assert_eq!(
        out["handles"]["amadeus_client_id"],
        "<sv_amadeus_client_id>"
    );
    assert_eq!(
        out["handles"]["amadeus_client_secret"],
        "<sv_amadeus_client_secret>"
    );

    // Real values must NEVER appear in the output.
    let serialized = out.to_string();
    assert!(!serialized.contains("ABC-CLI-ID"), "real value leaked: {serialized}");
    assert!(!serialized.contains("XYZ-CLI-SEC"), "real value leaked: {serialized}");

    // But they ARE persisted in the repo.
    let stored = repo.storage.lock().unwrap();
    assert_eq!(
        stored.get("<sv_amadeus_client_id>").map(String::as_str),
        Some("ABC-CLI-ID")
    );
    assert_eq!(
        stored.get("<sv_amadeus_client_secret>").map(String::as_str),
        Some("XYZ-CLI-SEC")
    );
}

#[tokio::test]
async fn resume_errors_on_handle_collision() {
    let (node, repo) = build_node();
    // Pre-populate a colliding handle.
    repo.storage
        .lock()
        .unwrap()
        .insert("<sv_dup_token>".into(), "preexisting".into());

    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [{ "question": "Token?", "name": "dup_token" }]
    });
    let mut inputs = inputs_with("ask", "sx");
    inputs.insert(
        "__colmena_resume_answer".into(),
        Value::String("Token?\nnew-value".into()),
    );

    let err = node
        .execute(&inputs, &cfg, &mut state, None)
        .await
        .unwrap_err();
    assert!(
        format!("{err}").contains("handle <sv_dup_token> already exists"),
        "got: {err}"
    );

    // Repo must NOT have been mutated.
    assert_eq!(
        repo.storage
            .lock()
            .unwrap()
            .get("<sv_dup_token>")
            .map(String::as_str),
        Some("preexisting")
    );
}

#[tokio::test]
async fn resume_propagates_parser_errors() {
    let (node, _) = build_node();
    let mut state = Value::Null;
    let cfg = json!({
        "secrets": [{ "question": "Q?", "name": "nx" }]
    });
    let mut inputs = inputs_with("ask", "sx");
    inputs.insert(
        "__colmena_resume_answer".into(),
        Value::String("UnrelatedText\nval".into()),
    );
    let err = node
        .execute(&inputs, &cfg, &mut state, None)
        .await
        .unwrap_err();
    assert!(
        format!("{err}").contains("missing answer for secret 'nx'"),
        "got: {err}"
    );
}
```

- [ ] **Step 2: Run the tests, confirm failure**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::resume_`
Expected: failures with "resume not implemented".

- [ ] **Step 3: Implement the resume branch.** Replace the `if inputs.get("__colmena_resume_answer").is_some()` block in `execute`:

```rust
if let Some(answer_val) = inputs.get("__colmena_resume_answer") {
    let answer = answer_val
        .as_str()
        .ok_or_else(|| {
            Box::<dyn Error + Send + Sync>::from(
                "secure_suspend: __colmena_resume_answer must be a string",
            )
        })?;
    let session_id = inputs
        .get("__colmena_session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Box::<dyn Error + Send + Sync>::from(
                "secure_suspend: missing __colmena_session_id in inputs",
            )
        })?;
    let node_id = inputs
        .get("__node_id")
        .and_then(|v| v.as_str())
        .unwrap_or("secure_suspend");

    let values = parse_answers(answer, &secrets)
        .map_err(|e| Box::<dyn Error + Send + Sync>::from(e))?;

    // Collision pre-check (all secrets) before any write.
    for s in &secrets {
        let handle = format!("<sv_{}>", s.name);
        if self
            .secure_value_service
            .handle_exists(session_id, &handle)
            .await
            .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("{e}")))?
        {
            return Err(Box::<dyn Error + Send + Sync>::from(format!(
                "secure_suspend: handle {handle} already exists in session — use a different name"
            )));
        }
    }

    // Persist + collect handles.
    let mut handles = serde_json::Map::new();
    for (s, v) in secrets.iter().zip(values.iter()) {
        let handle = self
            .secure_value_service
            .persist_secret(session_id, node_id, &s.name, v)
            .await
            .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("{e}")))?;
        handles.insert(s.name.clone(), Value::String(handle));
    }

    return Ok(json!({
        "status": "resumed",
        "handles": Value::Object(handles)
    }));
}
```

- [ ] **Step 4: Run the resume tests, confirm pass**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::resume_`
Expected: 3 passes.

- [ ] **Step 5: Run all secure_suspend tests**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend`
Expected: all pass (now ~15 tests).

- [ ] **Step 6: Run full crate test suite to ensure nothing else broke**

Run: `cargo test --lib -p colmena_dag_engine`
Expected: no regressions.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "feat(secure-suspend): resume-path persists secrets and returns handles map"
```

---

## Task 9: Logging-hygiene test

Confirms the implementation never emits the real secret value through `tracing`. Uses `tracing-subscriber` test capture if available, otherwise grep stderr in a process spawn — but the simpler approach is to assert via `tracing_subscriber::fmt::TestWriter` capture, which is already used in this crate.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`

- [ ] **Step 1: Inspect existing test patterns**

Run: `grep -rn "tracing-test\|TestWriter\|with_test_writer" /home/daniel-garcia4/startti/colmena/src/libs/colmena/`
If `tracing-test` is already a dev-dependency, use it. Otherwise the test below uses `tracing_subscriber::fmt::TestWriter`.

- [ ] **Step 2: Add the test**

```rust
#[tokio::test]
async fn resume_does_not_log_real_values() {
    use std::io::Write;
    use std::sync::Mutex;
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt;

    /// Writer that buffers everything written by tracing.
    #[derive(Clone, Default)]
    struct BufWriter(std::sync::Arc<Mutex<Vec<u8>>>);
    impl<'a> fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriterHandle;
        fn make_writer(&'a self) -> Self::Writer {
            BufWriterHandle(self.0.clone())
        }
    }
    struct BufWriterHandle(std::sync::Arc<Mutex<Vec<u8>>>);
    impl Write for BufWriterHandle {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf = BufWriter::default();
    let subscriber = fmt::Subscriber::builder()
        .with_writer(buf.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();

    let secret_marker = "SUPER_SECRET_MARKER_qwerty12345";
    let cfg = json!({
        "secrets": [{ "question": "Marker?", "name": "marker_secret" }]
    });
    let (node, _repo) = build_node();
    let mut inputs = inputs_with("ask", "sx");
    inputs.insert(
        "__colmena_resume_answer".into(),
        Value::String(format!("Marker?\n{secret_marker}")),
    );
    let mut state = Value::Null;

    with_default(subscriber, || {
        // Block on the future inside the subscriber scope.
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let _out = node
                .execute(&inputs, &cfg, &mut state, None)
                .await
                .unwrap();
        });
    });

    let captured = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(
        !captured.contains(secret_marker),
        "secret leaked into tracing: {captured}"
    );
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test --lib -p colmena_dag_engine secure_suspend::tests::resume_does_not_log_real_values`
Expected: pass on first try (the implementation does not log any input values; the test guards future regressions).

If the test fails because the implementation accidentally logs the answer string, fix the code (remove or redact the log call). Do not weaken the test.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "test(secure-suspend): assert real secret values never reach tracing output"
```

---

## Task 10: Register the node in the engine registry (conditional)

Only register when `secure_value_service` is `Some`. If the engine is built without secure values, the node is unavailable — graphs that reference it fail with the standard `NodeTypeNotFound` error.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

- [ ] **Step 1: Add a registry test** at the bottom of `registry.rs` (in a new `#[cfg(test)] mod registry_secure_suspend_tests`):

```rust
#[cfg(test)]
mod registry_secure_suspend_tests {
    use super::*;
    use crate::dag_engine::application::secure_value_service::SecureValueService;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::secure_value_repository::SecureValueRepository;
    use async_trait::async_trait;

    struct NoopRepo;

    #[async_trait]
    impl SecureValueRepository for NoopRepo {
        async fn persist(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<(), DagError> {
            Ok(())
        }
        async fn decrypt(&self, _: &str, _: &str) -> Result<Option<String>, DagError> {
            Ok(None)
        }
        async fn exists(&self, _: &str, _: &str) -> Result<bool, DagError> {
            Ok(false)
        }
        async fn cleanup(&self, _: &str) -> Result<(), DagError> {
            Ok(())
        }
        async fn cleanup_expired(&self) -> Result<u64, DagError> {
            Ok(0)
        }
    }

    fn build_registry_with_secure_values() -> Arc<HashMapNodeRegistry> {
        let pool_registry = Arc::new(
            crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry::new(
                crate::dag_engine::infrastructure::pool_registry::PoolConfig::defaults(),
            ),
        );
        let repo_factory =
            Arc::new(crate::llm::infrastructure::ConversationRepositoryFactory::new(
                pool_registry.clone(),
            ));
        let sql_factory = Arc::new(
            crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory::new(
                pool_registry,
            ),
        );
        let task_memory: Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository> =
            Arc::new(super::registry_tavily_tests::StubTaskMemory);
        let svc = Arc::new(SecureValueService::new(Arc::new(NoopRepo) as Arc<_>));
        HashMapNodeRegistry::new_with_secure_values(
            repo_factory,
            sql_factory,
            Some(task_memory),
            Some(svc),
        )
    }

    #[test]
    fn secure_suspend_registered_when_secure_value_service_present() {
        let reg = build_registry_with_secure_values();
        assert!(
            reg.get_node("secure_suspend").is_some(),
            "secure_suspend must be registered when SecureValueService is wired"
        );
    }

    #[test]
    fn secure_suspend_not_registered_when_secure_value_service_absent() {
        let reg = super::registry_tavily_tests::build_registry();
        assert!(
            reg.get_node("secure_suspend").is_none(),
            "secure_suspend must NOT be registered without SecureValueService"
        );
    }
}
```

Note: the existing `registry_tavily_tests::StubTaskMemory` is `pub(super)` because it's used within the same `mod`. If it's not, change its visibility from `struct StubTaskMemory;` to `pub(super) struct StubTaskMemory;` in the file. Same for `build_registry`.

- [ ] **Step 2: Run the test, confirm failure**

Run: `cargo test --lib -p colmena_dag_engine registry_secure_suspend_tests`
Expected: failure on `secure_suspend_registered_when_secure_value_service_present`.

- [ ] **Step 3: Register the node.** In `registry.rs`, immediately after the existing `// --- Registrar Mock de Suspension ---` block (around line 119-122), add:

```rust
// --- Registrar secure_suspend (solo si hay SecureValueService) ---
if let Some(svc) = secure_value_service.clone() {
    nodes.insert(
        "secure_suspend".to_string(),
        Arc::new(
            crate::dag_engine::infrastructure::nodes::secure_suspend::SecureSuspendNode::new(
                svc,
            ),
        ),
    );
}
```

- [ ] **Step 4: Run the registry tests**

Run: `cargo test --lib -p colmena_dag_engine registry`
Expected: all tests pass, including the two new ones.

- [ ] **Step 5: Run the full unit test suite**

Run: `cargo test --lib -p colmena_dag_engine`
Expected: no regressions.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(secure-suspend): register node conditionally on SecureValueService"
```

---

## Task 11: Integration smoke test against real Postgres

End-to-end test: `secure_suspend` pauses, gets resumed via the standard CLI mechanism, and the resulting handle is correctly resolved by `inject_secrets` when an `http_request` (or any other non-LLM node) downstream consumes it.

**Files:**
- Create: `tests/graphs/basic/secure_suspend_smoke.json`
- Create: `tests/secure_suspend_integration.rs`

- [ ] **Step 1: Write the smoke graph** at `tests/graphs/basic/secure_suspend_smoke.json`:

```json
{
  "comment": "Smoke test: secure_suspend collects two secrets, log node downstream sees only handles.",
  "metadata": {
    "category": "basic",
    "features": ["secure_suspend", "secure_values"],
    "requires_env": ["DATABASE_URL"]
  },
  "nodes": {
    "ask_creds": {
      "type": "secure_suspend",
      "config": {
        "secrets": [
          { "question": "Q1?", "name": "smoke_secret_a" },
          { "question": "Q2?", "name": "smoke_secret_b" }
        ]
      }
    },
    "log_handles": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "ask_creds.handles", "to": "log_handles" }
  ]
}
```

- [ ] **Step 2: Inspect existing integration tests for the load-and-run pattern**

Run: `ls tests/ && head -80 tests/$(ls tests/ | grep -v graphs | head -1)`
Identify the pattern they use to instantiate the engine, run a graph, and resume from a suspended state. The CLI binary `dag_engine` exposes `run --session-id <id> --answer <text>` for resume — preferred over re-invoking the use-case directly.

- [ ] **Step 3: Write the integration test** at `tests/secure_suspend_integration.rs`. The test invokes the CLI binary twice (suspend, then resume) using `assert_cmd` (already a dev-dep in this kind of project — verify with `grep assert_cmd src/libs/colmena/Cargo.toml`; if missing, fall back to `std::process::Command`):

```rust
//! Integration test for secure_suspend against a live Postgres.
//! Run with: `source .env && cargo test --test secure_suspend_integration -- --ignored`.

use std::process::Command;

#[test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
fn secure_suspend_smoke_round_trip() {
    let session_id = format!(
        "secure_suspend_smoke_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let graph = "tests/graphs/basic/secure_suspend_smoke.json";

    // First invocation: graph runs, secure_suspend emits SUSPENDED.
    let out1 = Command::new(env!("CARGO_BIN_EXE_dag_engine"))
        .args(["run", graph, "--session-id", &session_id])
        .output()
        .expect("first run must execute");
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        stdout1.contains("SUSPENDED") || stdout1.contains("\"questions\""),
        "expected SUSPENDED status in first run output: {stdout1}"
    );

    // Second invocation: resume with the answer string in the canonical format.
    let answer = "Q1?\nsmoke-val-a\nQ2?\nsmoke-val-b";
    let out2 = Command::new(env!("CARGO_BIN_EXE_dag_engine"))
        .args([
            "run",
            graph,
            "--session-id",
            &session_id,
            "--answer",
            answer,
        ])
        .output()
        .expect("resume run must execute");
    let stdout2 = String::from_utf8_lossy(&out2.stdout);

    // Output downstream must contain the handles, NOT the real values.
    assert!(
        stdout2.contains("<sv_smoke_secret_a>"),
        "expected handle for smoke_secret_a: {stdout2}"
    );
    assert!(
        stdout2.contains("<sv_smoke_secret_b>"),
        "expected handle for smoke_secret_b: {stdout2}"
    );
    assert!(
        !stdout2.contains("smoke-val-a"),
        "real value smoke-val-a leaked into stdout: {stdout2}"
    );
    assert!(
        !stdout2.contains("smoke-val-b"),
        "real value smoke-val-b leaked into stdout: {stdout2}"
    );
}
```

- [ ] **Step 4: Confirm the binary name is correct**

Run: `grep -n '\[\[bin\]\]' src/libs/colmena/Cargo.toml`
Confirm there is `name = "dag_engine"` so `CARGO_BIN_EXE_dag_engine` resolves at compile time. If the bin name differs, adjust the env var accordingly.

- [ ] **Step 5: Run the integration test against a live DB**

Run: `source .env && cargo test --test secure_suspend_integration -- --ignored`
Expected: pass.

If the test fails because the CLI does not echo the downstream output to stdout in resume-mode, capture how `dag_engine run` prints final node outputs. Adjust the assertions to match the binary's actual output (e.g. include `--include-extra-info` flag if needed; reference CLAUDE.md "Opciones adicionales del subcomando `run`").

- [ ] **Step 6: Commit**

```bash
git add tests/graphs/basic/secure_suspend_smoke.json tests/secure_suspend_integration.rs
git commit -m "test(secure-suspend): integration smoke test with real Postgres"
```

---

## Task 12: Documentation updates

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/agent_context/node_ports_reference.md`

- [ ] **Step 1: Inspect the existing entry for `suspend`** to mirror its shape

Run: `grep -A 30 '"suspend"' docs/node_configurations.json`
Note the structure (description, config fields, types, examples).

- [ ] **Step 2: Add the `secure_suspend` entry** to `docs/node_configurations.json`. Use the same shape as `suspend`. Insert in alphabetical order — that means just before `socketio_request` (or wherever `s*` keys live):

```json
"secure_suspend": {
  "description": "Pausa el DAG y pide al usuario uno o más secretos en una sola pausa. Persiste cada valor cifrado en la tabla de secure values y devuelve solo handles `<sv_<name>>`. El valor real nunca aparece en outputs ni en logs ni en el contexto del LLM.",
  "config": {
    "secrets": {
      "type": "array",
      "required": true,
      "description": "Lista de objetos { question, name }. Mínimo 1, máximo 8. Cada `name` debe matchear ^[a-z][a-z0-9_]{2,63}$ y ser único dentro de la lista; cada `question` debe ser único dentro de la lista (anclas del parser de respuesta)."
    },
    "id": {
      "type": "string",
      "required": false,
      "description": "ID estable del bloque de preguntas. Default: el __node_id del nodo. Las questions emitidas tienen IDs de la forma `<id>__1`, `<id>__2`, ..."
    }
  },
  "outputs": {
    "suspend_path": {
      "__colmena_status": "SUSPENDED",
      "questions": "[{ id, question, type:'secret', options:null }]"
    },
    "resume_path": {
      "status": "resumed",
      "handles": "{ <name>: '<sv_<name>>' }"
    }
  },
  "example": {
    "type": "secure_suspend",
    "config": {
      "secrets": [
        { "question": "Cuál es tu Amadeus client_id?", "name": "amadeus_client_id" },
        { "question": "Cuál es tu Amadeus client_secret?", "name": "amadeus_client_secret" }
      ]
    }
  }
}
```

- [ ] **Step 3: Validate JSON**

Run: `python3 -c "import json; json.load(open('docs/node_configurations.json'))"` (or `jq . docs/node_configurations.json > /dev/null`)
Expected: no error.

- [ ] **Step 4: Update `docs/agent_context/node_ports_reference.md`**

Run: `grep -n "suspend" docs/agent_context/node_ports_reference.md` to find where `suspend` is documented, then add a `secure_suspend` block alongside it. Use this content:

```markdown
### secure_suspend

Pausa el DAG para recolectar uno o más secretos del usuario. El valor real
nunca aparece en outputs.

**Inputs (engine-injected):**
- `__node_id` — usado como base ID si `config.id` no está.
- `__colmena_session_id` — scope de los handles persistidos.
- `__colmena_resume_answer` — string formato `pregunta\nvalor\npregunta\nvalor` al reanudar.

**Outputs (suspend-path):**
- `__colmena_status: "SUSPENDED"`
- `questions: [{ id, question, type: "secret", options: null }, ...]` — una entrada por ítem en `config.secrets`, IDs `<base>__1`, `<base>__2`, ...

**Outputs (resume-path):**
- `status: "resumed"`
- `handles: { <name>: "<sv_<name>>", ... }` — mapa con un handle por secreto, indexado por el `name` del config.

**Default input:** `secrets`. **Default output:** `handles`.
```

- [ ] **Step 5: Confirm both docs render reasonably**

Run: `cat docs/agent_context/node_ports_reference.md | head -200`
Visual check.

- [ ] **Step 6: Commit**

```bash
git add docs/node_configurations.json docs/agent_context/node_ports_reference.md
git commit -m "docs(secure-suspend): document config schema, ports, and example"
```

---

## Final Verification

- [ ] **Step 1: Run the full CI-equivalent test sweep**

Run: `cargo test --verbose -p colmena_dag_engine 2>&1 | tail -40`
Expected: 0 failures across unit + integration + doc tests (excluding `#[ignore]`d ones).

- [ ] **Step 2: Run the ignored integration tests against live DB**

Run: `source .env && cargo test -- --ignored 2>&1 | tail -40`
Expected: 0 failures.

- [ ] **Step 3: Confirm no warnings**

Run: `cargo check --all-targets`
Expected: clean exit, no output beyond the standard cargo compile lines.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no clippy errors.

- [ ] **Step 5: Confirm git history is sequential and clean**

Run: `git log --oneline develop..HEAD`
Expected: ~12 commits, each tied to one task.

---

## Self-Review Notes

**Spec coverage check:** every section of `2026-05-07-secure-suspend-node-design.md` maps to at least one task —

| Spec section | Task(s) |
|---|---|
| Arquitectura — reuse SuspendNode + SecureValueService | Tasks 4, 6 |
| Config schema (`secrets[]`, `id`) | Task 5 |
| Suspend-path output | Task 6 |
| Resume-path output (handles map) | Task 8 |
| Protocolo de respuesta del UI (anchored parser) | Task 7 |
| Errores (8 specific messages) | Tasks 5, 7, 8 |
| Uso como tool LLM (description) | Documented in spec; no code task — the canvas-builder graph (Spec 2) consumes this verbatim. |
| Patrones de uso (HubSpot, Amadeus, DB) | Test cases in Tasks 6 & 8 mirror the patterns. |
| Modos de falla / no-leak | Task 8 (output assertions), Task 9 (tracing) |
| Plan de testing — unit | Tasks 5-9 cover all 11 listed unit tests |
| Plan de testing — integration | Task 11 |
| Cambios concretos al repo (table) | Tasks 1-4, 10, 12 |

**Type consistency:** `Secret { question, name }`, `parse_answers(&str, &[Secret]) -> Result<Vec<String>, String>`, `SecureValueService::persist_secret`/`handle_exists` — names used identically across all tasks.

**Placeholder scan:** none. Each step contains the actual code or command.
