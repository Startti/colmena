# Diseño: alinear `secure_values` con `agent_session_id`-first lookup

**Estado:** propuesta aprobada
**Fecha:** 2026-05-08
**Autor:** Daniel García (Startti)

## Contexto

Validamos que `secure_suspend` + Gap 1 + Gap 2 funcionan end-to-end **cuando suspend y resume comparten el mismo `--session-id`**. Pero un caso real de producción en ADP es:

- El meta-agente (canvas-builder) corre en una sesión y persiste un secreto.
- El usuario invoca el agente nuevo en otra sesión (distinto `session_id` ephemeral, mismo `agent_session_id` estable).
- El handle queda literal porque el lookup busca por `session_id` que ya cambió.

El motor de Colmena **ya tiene un patrón resuelto para este caso**, usado por la memoria conversacional y el DAG state:

- `postgres_conversation_repository.rs:22-44`: `WHERE agent_session_id = $1 AND node_id = $2` cuando agent_session_id está set, fallback a `WHERE session_id = $1 AND node_id = $2`.
- `postgres_dag_state_repository.rs:218`: `find_resume_entry(agent_session_id)` para encontrar suspended chains a través de runs.

Mi spec original de `secure_suspend` flagueó esto como "fuera de alcance — promoción a scope mayor". El usuario observó correctamente que es la pieza que falta para cerrar el caso real, y que **el patrón a seguir ya existe en el repo**. No es trabajo de plataforma nuevo, es alineación.

## Objetivo

Que `secure_value_mappings` y todas las operaciones de lookup/persist sigan exactamente el mismo patrón que `llm_node_history`:

- Persiste siempre `session_id`. Persiste `agent_session_id` adicionalmente cuando está disponible.
- Lookup (`decrypt`, `exists`): si `agent_session_id` está set, busca por él; fallback a `session_id` cuando no.
- Cleanup: se mantiene como hoy (por `session_id`) — los registros ephemeral se cubren ahí, y los ligados a `agent_session_id` viven hasta que el agente o el `cleanup_expired` los retire.

## Diseño

### 1. Migration aditiva

`src/libs/colmena/migrations/postgres/20260508000001_secure_values_agent_session_id.sql`:

```sql
-- Spec: docs/superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md
ALTER TABLE secure_value_mappings
    ADD COLUMN IF NOT EXISTS agent_session_id TEXT;

CREATE INDEX IF NOT EXISTS idx_secure_values_agent_hash
    ON secure_value_mappings(agent_session_id, hash_key);
```

No drop, no rename, no migración de data — filas existentes tienen `agent_session_id = NULL` y siguen siendo accesibles por el path de fallback `session_id`.

El UNIQUE constraint actual `(session_id, hash_key)` se mantiene como está. **No se necesita un constraint sobre `(agent_session_id, hash_key)`** porque dos runs distintos pueden persistir bajo el mismo `agent_session_id` y el ON CONFLICT por `(session_id, hash_key)` ya cubre las re-invocaciones del MISMO run.

### 2. Trait `SecureValueRepository` — extender firmas

Tres métodos cambian:

```rust
async fn persist(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
    source_node_id: &str,
    hash_key: &str,
    real_value: &str,
    field_name: &str,
) -> Result<(), DagError>;

async fn decrypt(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
    hash_key: &str,
) -> Result<Option<String>, DagError>;

async fn exists(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
    hash_key: &str,
) -> Result<bool, DagError>;
```

`cleanup` y `cleanup_expired` no cambian (cleanup borra por `session_id` cuando termina un run; expired borra todo lo vencido).

### 3. `PostgresSecureValueRepository` — implementación

**`persist`** — siempre escribe `session_id`, escribe `agent_session_id` si está set:

```rust
sqlx::query(r#"
    INSERT INTO secure_value_mappings
        (session_id, agent_session_id, source_node_id, hash_key, encrypted_value, field_name)
    VALUES ($1, $2, $3, $4, pgp_sym_encrypt($5::text, $6), $7)
    ON CONFLICT (session_id, hash_key) DO UPDATE SET
        encrypted_value = EXCLUDED.encrypted_value,
        agent_session_id = EXCLUDED.agent_session_id,
        expires_at = NOW() + INTERVAL '1 hour'
"#)
.bind(session_id)
.bind(agent_session_id)
.bind(source_node_id)
.bind(hash_key)
.bind(real_value)
.bind(&encryption_key)
.bind(field_name)
.execute(&self.pool).await?;
```

**`decrypt`** — agent-first, session fallback (mismo patrón que conversation repo):

```rust
let row = if let Some(agent) = agent_session_id {
    sqlx::query(r#"
        SELECT pgp_sym_decrypt(encrypted_value, $1) as decrypted
        FROM secure_value_mappings
        WHERE agent_session_id = $2 AND hash_key = $3
    "#).bind(&encryption_key).bind(agent).bind(hash_key).fetch_optional(&self.pool).await?
} else {
    sqlx::query(r#"
        SELECT pgp_sym_decrypt(encrypted_value, $1) as decrypted
        FROM secure_value_mappings
        WHERE session_id = $2 AND hash_key = $3
    "#).bind(&encryption_key).bind(session_id).bind(hash_key).fetch_optional(&self.pool).await?
};
```

**`exists`** — análogo:

```rust
let exists: bool = if let Some(agent) = agent_session_id {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE agent_session_id = $1 AND hash_key = $2)"
    ).bind(agent).bind(hash_key).fetch_one(&self.pool).await?
} else {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE session_id = $1 AND hash_key = $2)"
    ).bind(session_id).bind(hash_key).fetch_one(&self.pool).await?
};
```

El default impl del trait (introducido en Spec 1) sigue funcionando: llama `decrypt(...)` con la nueva firma y verifica `is_some()`.

Mock inline (`MockSecureValueRepository` en `secure_value_service.rs::tests`) se actualiza al nuevo trait. Su `persist` puede ignorar `agent_session_id` o guardarlo en una segunda tabla en memoria — para tests basta con seguir indexando por `hash_key` (no nos preocupa el lookup por agent en mocks), o agregar tests dedicados al fallback.

### 4. `SecureValueService` — propagar `agent_session_id`

Tres métodos cambian — todos aceptan `Option<&str>` para agent:

```rust
pub async fn hash_output(
    &self,
    output: &Value,
    config: &Value,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
    source_node_id: &str,
) -> Result<Value, DagError>;

pub async fn inject_secrets(
    &self,
    inputs: &mut Value,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
) -> Result<(), DagError>;

pub async fn persist_secret(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
    source_node_id: &str,
    name: &str,
    real_value: &str,
) -> Result<String, DagError>;

pub async fn handle_exists(
    &self,
    session_id: &str,
    agent_session_id: Option<&str>,   // NUEVO
    handle: &str,
) -> Result<bool, DagError>;
```

Todas las llamadas internas a `self.repo.persist/decrypt/exists` propagan `agent_session_id`.

### 5. Call sites

Los puntos donde se invoca el service / repo necesitan pasar `agent_session_id`. Cuatro sitios:

| Archivo | Variable disponible | Acción |
|---|---|---|
| `dag_engine/application/run_use_case.rs` (~378-400) | `active_agent_session_id: Option<String>` | Pasar `as_deref()` a las dos llamadas inject (inputs y config) |
| `dag_engine/infrastructure/dag_tool_executor.rs` (~810) | nuevo campo `agent_session_id: Option<String>` (siguiendo el patrón de `session_id`) + builder `with_agent_session_id` | Inyectar `__colmena_agent_session_id` en inputs y pasarlo a inject_secrets |
| `dag_engine/infrastructure/nodes/secure_suspend.rs` (resume-path) | leer `__colmena_agent_session_id` de inputs | Pasarlo a `persist_secret` y `handle_exists` |
| `dag_engine/infrastructure/nodes/http.rs` o donde se invoque `hash_output` | similar — leer agent_session_id de inputs | Pasarlo a `hash_output` |

El motor ya tiene `active_agent_session_id` desde Spec de DAG state (commit anterior). Solo lo enrutamos.

### 6. Inyección de inputs reservados

`run_use_case.rs` ya inyecta `__colmena_session_id` en inputs (~línea 398). **Añadir** `__colmena_agent_session_id`:

```rust
inputs.insert("__colmena_session_id".to_string(), Value::String(session_id.clone()));
if let Some(asid) = active_agent_session_id.as_deref() {
    inputs.insert("__colmena_agent_session_id".to_string(), Value::String(asid.to_string()));
}
```

`secure_suspend` lee ambos en su resume-path. `dag_tool_executor` también — para que cuando `secure_suspend` se invoque como LLM tool, tenga ambos disponibles.

`http.rs` añade `__colmena_agent_session_id` a su lista de reserved_keys (no se manda como query param a APIs externas).

## Tests

### Unit

1. `Postgres::persist_with_agent_then_decrypt_with_only_session_returns_none` — `#[ignore]` integration. Persiste con `session_id=A, agent_session_id=X`. Decrypt con `session_id=A, agent_session_id=None` → encuentra (fallback path). Decrypt con `session_id=B, agent_session_id=None` → None.
2. `Postgres::persist_with_agent_then_decrypt_with_same_agent_different_session` — persiste con `S=A, A=X`. Decrypt con `S=B (distinto), A=X` → **encuentra**. **Esta es la prueba clave** — confirma el caso canvas-builder.
3. `Postgres::persist_without_agent_then_decrypt_with_session_works` — caso legacy puramente ephemeral.
4. Service-level: `inject_secrets_uses_agent_session_id_when_provided`.
5. `secure_suspend::resume_path_persists_with_agent_session_id_from_inputs`.

### Integration end-to-end

`tests/secure_values_cross_session_integration.rs` (`#[ignore]`):

1. Setup: persist `<sv_smoke>` con `session=run1, agent=A1`.
2. Run a graph in a NEW process / different session_id (`run2`) but same agent (`A1`), graph has a node whose config contains `<sv_smoke>`.
3. Verify the node received the real value at injection time.

### Re-validación end-to-end (manual / smoke)

Re-correr `tests/graphs/advanced/secure_suspend_login_e2e.json` con dos invocaciones CLI separadas usando `--agent-session-id` para validar el caso real.

## Pre-requisitos / fuera de alcance

**Pre-requisitos:**
- Spec 1 (inject in config) — cerrado.
- Spec 2 (llm_call propaga SUSPENDED) — cerrado.
- Migrations corren al startup del motor (ya está cableado).

**Fuera de alcance:**
- Workspace-level scope (más amplio que agent_session_id). Cuando aparezca, mismo patrón de columna nullable + lookup multi-key.
- Migración de data existente — no hay (filas ephemeral ya no se necesitan).
- Cambios a `cleanup_expired` (ya cubre todos los registros por TTL).

## Cambios concretos al repo

| Archivo | Acción |
|---|---|
| `migrations/postgres/20260508000001_secure_values_agent_session_id.sql` | NUEVO — ALTER + INDEX. |
| `src/libs/colmena/src/dag_engine/domain/secure_value_repository.rs` | Trait: agregar `agent_session_id: Option<&str>` a 3 métodos; actualizar default impl de `exists`. |
| `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_secure_value_repository.rs` | Implementar nueva firma con if-let-Some agent / else session. |
| `src/libs/colmena/src/dag_engine/application/secure_value_service.rs` | Service métodos propagan `agent_session_id`. Mock inline actualizado. Tests existentes actualizados. |
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Inyectar `__colmena_agent_session_id` en inputs. Pasar `agent_session_id` a 2 llamadas `inject_secrets`. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Nuevo campo `agent_session_id` + builder; inyectar `__colmena_agent_session_id` en inputs; propagar a inject_secrets. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` | Resume-path lee `__colmena_agent_session_id` de inputs y lo pasa a `persist_secret`/`handle_exists`. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` | Añadir `__colmena_agent_session_id` a reserved_keys; pasarlo a `hash_output` cuando el nodo es secure. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Pasar agent_session_id al construir DagToolExecutor (vía `with_agent_session_id`). |
| `src/libs/colmena/tests/secure_values_cross_session_integration.rs` | NUEVO — integration test que prueba persist en S1 + A, lookup en S2 + A. |

Estimado: ~150-200 LoC + 1 migration + tests.
