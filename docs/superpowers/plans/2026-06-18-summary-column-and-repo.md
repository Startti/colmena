# Columna `summary` + extensión del `ConversationRepository` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persistir un resumen semántico por mensaje en una columna `summary` de `llm_node_history`, y extender el `ConversationRepository` con `get_with_summaries` (leer mensajes + su summary) y `set_summary` (cachear el summary de un mensaje por ordinal), sin cambiar el comportamiento actual.

**Architecture:** Es la **Fase 3** del spec `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`. Migración aditiva (`summary TEXT` nullable) en Postgres y SQLite. El trait gana dos métodos con **default impls** (no rompe impls externos): `get_with_summaries` (default → `get_by_id` con summaries `None`) y `set_summary` (default → no-op). Los tres adapters (pg/sqlite/in-memory) los implementan de verdad. El ordinal = posición 0-based en `ORDER BY created_at` (estable por append-only). Sin cambio de comportamiento: nadie llama los métodos nuevos todavía (eso es Fase 4).

**Tech Stack:** Rust, `sqlx` (Postgres + SQLite), `async_trait`, migraciones en `src/libs/colmena/migrations/{postgres,sqlite}/`.

---

## File Structure

- `src/libs/colmena/migrations/postgres/20260618000000_llm_history_summary.sql` — **nuevo**.
- `src/libs/colmena/migrations/sqlite/20260618000000_llm_history_summary.sql` — **nuevo**.
- `src/libs/colmena/src/llm/domain/memory.rs` — **modificar**: struct `StoredMessage` + 2 métodos default en el trait.
- `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs` — **modificar**: impl de los 2 métodos.
- `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs` — **modificar**: impl de los 2 métodos.
- `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs` — **modificar**: impl de los 2 métodos.

---

### Task 1: Migraciones (Postgres + SQLite)

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260618000000_llm_history_summary.sql`
- Create: `src/libs/colmena/migrations/sqlite/20260618000000_llm_history_summary.sql`

- [ ] **Step 1: Migración Postgres**

```sql
-- Cache de resumen semántico por mensaje (Fase 3, conversation summary).
-- NULL = aún no resumido (o < umbral → verbatim). Ver spec 2026-06-18.
ALTER TABLE llm_node_history ADD COLUMN IF NOT EXISTS summary TEXT;
```

- [ ] **Step 2: Migración SQLite**

```sql
-- Cache de resumen semántico por mensaje (Fase 3, conversation summary).
-- SQLite no soporta IF NOT EXISTS en ADD COLUMN; la migración corre una sola
-- vez (sqlx trackea las aplicadas), así que es seguro.
ALTER TABLE llm_node_history ADD COLUMN summary TEXT;
```

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/migrations/postgres/20260618000000_llm_history_summary.sql \
        src/libs/colmena/migrations/sqlite/20260618000000_llm_history_summary.sql
git commit -m "feat(memory): add summary column to llm_node_history (pg + sqlite)"
```

---

### Task 2: `StoredMessage` + métodos default en el trait

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/memory.rs`

- [ ] **Step 1: Agregar el struct `StoredMessage`**

Después de la definición de `Conversation` en `memory.rs`:

```rust
/// Un mensaje persistido junto con su resumen cacheado (si existe).
/// `summary == None` → aún no resumido (o por debajo del umbral → verbatim).
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub message: LlmMessage,
    pub summary: Option<String>,
}
```

- [ ] **Step 2: Agregar los 2 métodos default al trait `ConversationRepository`**

Dentro de `pub trait ConversationRepository`, después de `delete`:

```rust
    /// Como `get_by_id`, pero devuelve cada mensaje junto a su `summary` cacheado.
    /// Default: delega en `get_by_id` con summaries en `None` (impls de DB lo overridean).
    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<StoredMessage>, LlmError> {
        let conv = self.get_by_id(key).await?;
        Ok(conv
            .messages
            .into_iter()
            .map(|message| StoredMessage {
                message,
                summary: None,
            })
            .collect())
    }

    /// Persiste el `summary` del mensaje en la posición `ordinal` (0-based en
    /// orden `created_at`). Default: no-op (impls de DB lo overridean).
    async fn set_summary(
        &self,
        _key: &ConversationKey,
        _ordinal: usize,
        _summary: &str,
    ) -> Result<(), LlmError> {
        Ok(())
    }
```

- [ ] **Step 3: Build (compila con los defaults, sin cambios en impls aún)**

Run: `cargo build -p colmena_dag_engine`
Expected: compila. `StoredMessage` debe estar re-exportado donde se re-exporta `Conversation`
(verificar el `pub use` del módulo `memory` en `src/libs/colmena/src/llm/domain/mod.rs`;
si `Conversation` se re-exporta ahí, agregar `StoredMessage` a la misma línea `pub use`).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/domain/memory.rs src/libs/colmena/src/llm/domain/mod.rs
git commit -m "feat(memory): StoredMessage + get_with_summaries/set_summary trait methods"
```

---

### Task 3: Implementación en Postgres

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs`

- [ ] **Step 1: Escribir el test que falla**

Agregar (o crear) un módulo de tests `#[ignore]`-gated al final del archivo (requiere
`DATABASE_URL`):

```rust
#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::llm::domain::{AgentSessionId, NodeIdPath, SessionId};

    fn key(agent: &str) -> ConversationKey {
        ConversationKey {
            session_id: SessionId(format!("sess_{agent}")),
            agent_session_id: Some(AgentSessionId(agent.to_string())),
            node_id: NodeIdPath("n".to_string()),
        }
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn set_and_get_summary_roundtrip() {
        let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let repo = PostgresConversationRepository::new(pool);
        let k = key("pg_summary_test_001");
        repo.delete(&k).await.unwrap();

        repo.add_message(&k, LlmMessage::user("first".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::assistant("second".into()).unwrap())
            .await
            .unwrap();

        // Antes de set: ambos summaries None.
        let before = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(before.len(), 2);
        assert!(before[0].summary.is_none());

        // Set summary en el ordinal 1 (segundo mensaje).
        repo.set_summary(&k, 1, "summary of second").await.unwrap();

        let after = repo.get_with_summaries(&k).await.unwrap();
        assert!(after[0].summary.is_none());
        assert_eq!(after[1].summary.as_deref(), Some("summary of second"));
        assert_eq!(after[1].message.content(), "second");

        repo.delete(&k).await.unwrap();
    }
}
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `set -a && source /Users/danielgarcia/startti/colmena/.env && set +a && cargo test -p colmena_dag_engine --lib summary_tests -- --ignored`
Expected: FAIL — `get_with_summaries`/`set_summary` usan el default (summary siempre None / no-op),
así que el assert de `Some("summary of second")` falla.

- [ ] **Step 3: Implementar `get_with_summaries` + `set_summary` en el impl de Postgres**

Dentro de `impl ConversationRepository for PostgresConversationRepository`, agregar (la
construcción de `LlmMessage` por rol es idéntica a la de `get_by_id`, ahora envuelta en
`StoredMessage` con la columna `summary`):

```rust
    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<crate::llm::domain::StoredMessage>, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, summary, created_at \
                 FROM llm_node_history \
                 WHERE agent_session_id = $1 AND node_id = $2 \
                 ORDER BY created_at ASC",
            )
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, summary, created_at \
                 FROM llm_node_history \
                 WHERE session_id = $1 AND node_id = $2 \
                 ORDER BY created_at ASC",
            )
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        let out = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_json: Option<serde_json::Value> = row.get("tool_calls");
                let summary: Option<String> = row.get("summary");

                let message = match role_str.as_str() {
                    "system" => LlmMessage::system(content).unwrap(),
                    "assistant" => {
                        if let Some(tc_json) = tool_calls_json {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_value(tc_json).unwrap_or_default();
                            LlmMessage::assistant_with_tool_calls(content, tool_calls).unwrap()
                        } else {
                            LlmMessage::assistant(content).unwrap()
                        }
                    }
                    "tool" => LlmMessage::tool(
                        tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                        content,
                    )
                    .unwrap(),
                    _ => LlmMessage::user(content).unwrap(),
                };

                crate::llm::domain::StoredMessage { message, summary }
            })
            .collect();

        Ok(out)
    }

    async fn set_summary(
        &self,
        key: &ConversationKey,
        ordinal: usize,
        summary: &str,
    ) -> Result<(), LlmError> {
        let ord = ordinal as i64;
        let res = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "UPDATE llm_node_history SET summary = $1 WHERE id = ( \
                   SELECT id FROM llm_node_history \
                   WHERE agent_session_id = $2 AND node_id = $3 \
                   ORDER BY created_at ASC OFFSET $4 LIMIT 1 )",
            )
            .bind(summary)
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .bind(ord)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE llm_node_history SET summary = $1 WHERE id = ( \
                   SELECT id FROM llm_node_history \
                   WHERE session_id = $2 AND node_id = $3 \
                   ORDER BY created_at ASC OFFSET $4 LIMIT 1 )",
            )
            .bind(summary)
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .bind(ord)
            .execute(&self.pool)
            .await
        };
        res.map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;
        Ok(())
    }
```

- [ ] **Step 4: Correr el test y verificar que pasa**

Run: `set -a && source /Users/danielgarcia/startti/colmena/.env && set +a && cargo test -p colmena_dag_engine --lib summary_tests -- --ignored`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs
git commit -m "feat(memory): postgres get_with_summaries + set_summary by ordinal"
```

---

### Task 4: Implementación en SQLite

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs`

- [ ] **Step 1: Escribir el test que falla**

Agregar al final del archivo (usa una DB SQLite en memoria, no requiere env):

```rust
#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::llm::domain::{AgentSessionId, NodeIdPath, SessionId};

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Esquema mínimo equivalente al de producción + columna summary.
        sqlx::query(
            "CREATE TABLE llm_node_history (\
                id TEXT PRIMARY KEY, session_id TEXT, agent_session_id TEXT, node_id TEXT, \
                role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, \
                summary TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn key() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("s".into()),
            agent_session_id: Some(AgentSessionId("a".into())),
            node_id: NodeIdPath("n".into()),
        }
    }

    #[tokio::test]
    async fn set_and_get_summary_roundtrip_sqlite() {
        let repo = SqliteConversationRepository::new(pool().await);
        let k = key();
        repo.add_message(&k, LlmMessage::user("first".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::assistant("second".into()).unwrap())
            .await
            .unwrap();

        repo.set_summary(&k, 1, "summary of second").await.unwrap();

        let after = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(after.len(), 2);
        assert!(after[0].summary.is_none());
        assert_eq!(after[1].summary.as_deref(), Some("summary of second"));
    }
}
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test -p colmena_dag_engine --lib sqlite_conversation_repository::summary_tests`
Expected: FAIL (default no-op → summary None).

- [ ] **Step 3: Implementar los 2 métodos en el impl de SQLite**

Dentro de `impl ConversationRepository for SqliteConversationRepository`:

```rust
    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<crate::llm::domain::StoredMessage>, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, summary, created_at \
                 FROM llm_node_history \
                 WHERE agent_session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC",
            )
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, summary, created_at \
                 FROM llm_node_history \
                 WHERE session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC",
            )
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        let out = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_str: Option<String> = row.get("tool_calls");
                let summary: Option<String> = row.get("summary");

                let message = match role_str.as_str() {
                    "system" => LlmMessage::system(content).unwrap(),
                    "assistant" => {
                        if let Some(tc_str) = tool_calls_str {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_str(&tc_str).unwrap_or_default();
                            LlmMessage::assistant_with_tool_calls(content, tool_calls).unwrap()
                        } else {
                            LlmMessage::assistant(content).unwrap()
                        }
                    }
                    "tool" => LlmMessage::tool(
                        tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                        content,
                    )
                    .unwrap(),
                    _ => LlmMessage::user(content).unwrap(),
                };

                crate::llm::domain::StoredMessage { message, summary }
            })
            .collect();

        Ok(out)
    }

    async fn set_summary(
        &self,
        key: &ConversationKey,
        ordinal: usize,
        summary: &str,
    ) -> Result<(), LlmError> {
        let ord = ordinal as i64;
        let res = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "UPDATE llm_node_history SET summary = ? WHERE id = ( \
                   SELECT id FROM llm_node_history \
                   WHERE agent_session_id = ? AND node_id = ? \
                   ORDER BY created_at ASC LIMIT 1 OFFSET ? )",
            )
            .bind(summary)
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .bind(ord)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE llm_node_history SET summary = ? WHERE id = ( \
                   SELECT id FROM llm_node_history \
                   WHERE session_id = ? AND node_id = ? \
                   ORDER BY created_at ASC LIMIT 1 OFFSET ? )",
            )
            .bind(summary)
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .bind(ord)
            .execute(&self.pool)
            .await
        };
        res.map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;
        Ok(())
    }
```

- [ ] **Step 4: Correr el test y verificar que pasa**

Run: `cargo test -p colmena_dag_engine --lib sqlite_conversation_repository::summary_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs
git commit -m "feat(memory): sqlite get_with_summaries + set_summary by ordinal"
```

---

### Task 5: Implementación en InMemory

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs`

- [ ] **Step 1: Escribir el test que falla**

Agregar al módulo de tests existente:

```rust
    #[tokio::test]
    async fn in_memory_summary_roundtrip() {
        let repo = InMemoryConversationRepository::new();
        let key = k(Some("chat_x"), "run_1", "router");
        repo.add_message(&key, LlmMessage::user("first".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&key, LlmMessage::assistant("second".into()).unwrap())
            .await
            .unwrap();

        repo.set_summary(&key, 1, "sum2").await.unwrap();

        let out = repo.get_with_summaries(&key).await.unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].summary.is_none());
        assert_eq!(out[1].summary.as_deref(), Some("sum2"));
        // get_by_id sigue devolviendo solo mensajes, sin romperse.
        assert_eq!(repo.get_by_id(&key).await.unwrap().messages.len(), 2);
    }
```

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test -p colmena_dag_engine --lib in_memory_conversation_repository::tests::in_memory_summary_roundtrip`
Expected: FAIL (default no-op).

- [ ] **Step 3: Cambiar el almacenamiento a `StoredMessage` e implementar los métodos**

Reemplazar el tipo del `inner` y los métodos. El `HashMap` ahora guarda `Vec<StoredMessage>`:

```rust
use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, StoredMessage,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct InMemoryConversationRepository {
    inner: Mutex<HashMap<(String, String), Vec<StoredMessage>>>,
}
```

`new` y `lookup_key` quedan igual. Los métodos del trait:

```rust
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let map = self.inner.lock().unwrap();
        let messages = map
            .get(&Self::lookup_key(key))
            .map(|v| v.iter().map(|sm| sm.message.clone()).collect())
            .unwrap_or_default();
        Ok(Conversation {
            key: key.clone(),
            messages,
        })
    }

    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        map.entry(Self::lookup_key(key))
            .or_default()
            .push(StoredMessage {
                message,
                summary: None,
            });
        Ok(())
    }

    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<StoredMessage>, LlmError> {
        let map = self.inner.lock().unwrap();
        Ok(map.get(&Self::lookup_key(key)).cloned().unwrap_or_default())
    }

    async fn set_summary(
        &self,
        key: &ConversationKey,
        ordinal: usize,
        summary: &str,
    ) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        if let Some(v) = map.get_mut(&Self::lookup_key(key)) {
            if let Some(sm) = v.get_mut(ordinal) {
                sm.summary = Some(summary.to_string());
            }
        }
        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        map.remove(&Self::lookup_key(key));
        Ok(())
    }
```

- [ ] **Step 4: Correr todos los tests del repo in-memory y verificar que pasan**

Run: `cargo test -p colmena_dag_engine --lib in_memory_conversation_repository`
Expected: PASS — el nuevo test + los 3 existentes (`agent_keying_isolates...`,
`legacy_keying...`, `node_id_isolates...`) siguen verdes.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs
git commit -m "feat(memory): in-memory get_with_summaries + set_summary"
```

---

### Task 6: Verificación de la fase

**Files:** ninguno

- [ ] **Step 1: Suite completa + clippy + fmt (incluye `--ignored` con DB)**

Run:
```bash
set -a && source /Users/danielgarcia/startti/colmena/.env && set +a && \
cargo test -p colmena_dag_engine --lib conversation_repository && \
cargo test -p colmena_dag_engine --lib summary_tests -- --ignored && \
cargo clippy -p colmena_dag_engine --all-targets -- -D warnings && \
cargo fmt --check
```
Expected: PASS / sin warnings / sin diffs.

- [ ] **Step 2: Smoke de migración E2E real**

Correr cualquier grafo `llm_call` con Postgres y confirmar que la columna existe:
```bash
cd /Users/danielgarcia/startti/colmena && set -a && source .env && set +a && \
cargo run --bin dag_engine -- run tests/graphs/memory/agent_chat_say.json --agent-session-id summary_col_smoke && \
psql "$DATABASE_URL" -P pager=off -c \
  "SELECT column_name FROM information_schema.columns WHERE table_name='llm_node_history' AND column_name='summary';"
```
Expected: la query devuelve la fila `summary`. Limpiar luego:
`psql "$DATABASE_URL" -c "DELETE FROM llm_node_history WHERE agent_session_id='summary_col_smoke';"`

---

## Self-Review

- **Spec coverage:** cubre §Arquitectura 2 del spec (columna `summary` + port extendido).
  Sin cambio de comportamiento: los métodos nuevos existen pero nadie los llama hasta la
  Fase 4. `get_by_id` queda intacto (back-compat).
- **Placeholder scan:** sin TODOs; todo el SQL y Rust está completo. La única verificación
  manual ("re-exportar `StoredMessage` en `domain/mod.rs`") trae el valor exacto a chequear.
- **Type consistency:** `StoredMessage { message, summary }`, `get_with_summaries -> Vec<StoredMessage>`
  y `set_summary(key, ordinal: usize, summary: &str)` son idénticos en trait, los 3 adapters
  y los tests. El ordinal es 0-based sobre `ORDER BY created_at` en los tres backends
  (pg `OFFSET $4`, sqlite `LIMIT 1 OFFSET ?`, in-memory `v.get_mut(ordinal)`).
