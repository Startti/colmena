# QA — Nodo `sql_query`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
Fuentes de doc revisadas: 
- `docs/node_configurations.json`
- `docs/node_as_tools_reference.json`
- `docs/agent_context/node_ports_reference.md`
- `docs/developer_guide/23_sql_node.md`

## 1) Config documentada NO soportada por el código

### Hallazgo 1: Campo `guardrail_enabled` ignorado por el código — ✅ RESUELTO

> **Estado: cerrado.** El campo fantasma fue eliminado del `schema()` del nodo y de
> toda la documentación canónica. La validación estática es incondicional por
> diseño y ya no se anuncia ningún interruptor para apagarla. Este hallazgo se
> conserva como registro de la auditoría; **ya no es reproducible**.

**Qué decía la doc (antes del fix):**
- `docs/developer_guide/23_sql_node.md` listaba `guardrail_enabled | boolean | No | true | Enable static validation rules`.
- El `schema()` del nodo lo anunciaba en su bloque `config` (`sql.rs`).
- *Corrección a la ficha original:* también se afirmaba que aparecía en
  `docs/node_as_tools_reference.json`. **Eso era falso** — ese archivo nunca lo
  contuvo. Verificado al aplicar el fix.

**Qué hacía el código:**
- Ningún `.get("guardrail_enabled")` existía en `sql.rs`. Solo se leía
  `guardrail_llm.enabled`, que activa el crítico LLM (ese sí es opcional y real).
- La validación estática siempre estuvo activa: es lo que bloquea `DROP`,
  `TRUNCATE` y `DELETE`/`UPDATE` sin `WHERE`.

**Por qué se eliminó en vez de implementarse:**
Cablear el flag habría permitido apagar la validación que impide operaciones
destructivas — un downgrade de seguridad. Se quitó el flag y se dejó una nota en
`sql.rs` para que no se reintroduzca.

**Qué verificar en QA:**
- Pasar `guardrail_enabled` (en config o como campo `fixed`) es inofensivo: la
  clave sobrante se ignora, igual que antes. Los grafos persistidos que aún lo
  llevan siguen funcionando sin cambios.
- La validación estática debe seguir bloqueando `DROP`/`TRUNCATE`/`DELETE` sin
  `WHERE` en todos los casos (ver pruebas 4-6 de la sección 3).

---

## 2) Código NO documentado

### Hallazgo 1: Constantes de límite de esquema no documentadas

**En el código:**
- `sql.rs` línea 42-43: `const MAX_SCHEMA_TABLES: usize = 40;` y `const MAX_SCHEMA_CHARS: usize = 8000;`
- Comportamiento: si el número de tablas supera 40 O el rendered schema supera 8000 caracteres, el nodo degrada la descripción a una "lista simple de nombres calificados" + recomendación de introspección (`information_schema`).

**En la documentación:**
- No aparece en `node_configurations.json` (son constantes internas, no configurables).
- No mencionado en `docs/developer_guide/23_sql_node.md`.
- No visible al usuario.

**Impacto para QA:**
- La descripción del tool que ve el LLM cambia drasticamente si el database tiene >40 tablas. Esto es silencioso — el test debe validar ambos casos (database pequeño y database grande) para verificar el modo degradado.

---

### Hallazgo 2: Comportamiento de fail-fast en `guardrail_llm.api_key` vacío

**En el código:**
- `sql.rs` línea 557-563: Si `guardrail_llm.enabled = true` pero `api_key` resuelve a string vacío (o no se resuelve ${VAR}), el nodo retorna `Err(...)` bloqueando la inicialización entera.
- Mensaje de error descriptivo: "sql node: guardrail_llm.enabled = true but api_key resolved to an empty string"

**En la documentación:**
- `docs/developer_guide/23_sql_node.md` línea 130 describe `api_key` como "API key for the critic LLM. Supports `${ENV_VAR}`" pero no aclara qué pasa si la variable de entorno no existe.
- No está mencionado que esto **falla en la inicialización** del grafo (no en tiempo de query).

**Impacto para QA:**
- Si un grafo está configurado con `guardrail_llm.enabled: true` y `api_key: "${OPENAI_API_KEY}"` pero `OPENAI_API_KEY` no está seteada, el grafo **nunca inicia** — error en tiempo de carga.
- La prueba debe validar tanto el caso de happy path (var seteada) como el error (var faltante), y confirmar que el error es **en tiempo de inicialización**, no de ejecución.

---

### Hallazgo 3: Campos de gobernanza no pueden ser sobrescritos por inputs (parcialmente documentado)

**En el código:**
- `sql.rs` línea 59-65: Define `GOVERNANCE_KEYS = ["connection_url", "permissions", "runtime_limits", "guardrail_llm", "setup_sql"]`
- `sql.rs` línea 76-88: `build_effective_config()` implementa el merge: **governance fields** autorizados en `config` NO pueden ser sobrescritos por inputs.
- Ejemplo: si `config.permissions = { preset: "read_only" }` y un input llega con `permissions: { preset: "full" }`, el input es ignorado y `read_only` prevalece.

**En la documentación:**
- `docs/developer_guide/23_sql_node.md` línea 71-73 menciona de pasada: "When used as an LLM tool via `tool_configurations`, all `node_schema` fixed values arrive in inputs (not config)."
- `node_as_tools_reference.json` describe "fixed" fields pero no explica que algunos campos SON governance aunque no sean "fixed".
- **Falta documentación clara** sobre qué sucede si un upstream node emite `permissions` en su output y el SQL node recibe eso vía input.

**Impacto para QA:**
- Pruebas deben cubrir: (1) governance override intentado (input.permissions sobrescribe config.permissions?) → NO, config.permissions gana. (2) data override permitido (input.query sobrescribe config.query?) → SÍ, input.query gana.

---

### Hallazgo 4: Resolución de `${ENV_VAR}` en `tenant_user_id` falla silenciosamente

**En el código:**
- `sql.rs` línea 500-509: Resuelve `tenant_user_id` con `Self::resolve_env_vars(raw)`, pero si falla retorna `warn!` y usa el string original (no resuelto).
- No bloquea la query, solo traza un warning.

**En la documentación:**
- `docs/developer_guide/23_sql_node.md` línea 93 dice: "User ID for RLS isolation. Supports `${ENV_VAR}`" pero no aclara qué pasa si la variable no existe (¿error? ¿fallback?).

**Impacto para QA:**
- Query que usa `tenant_user_id` con un ${VAR} inexistente silenciosamente falla a usar el string literal (e.g., "$ENV_VAR" literalmente en la WHERE clause de RLS).
- La prueba debe validar: (1) ${VAR} seteada → se resuelve correctamente. (2) ${VAR} NO seteada → warning en logs + fallback al string literal (¿es ese el comportamiento deseado?).

---

### Hallazgo 5: Descripción de multi-statement behavior en tool description, no en outputs schema

**En el código:**
- `sql.rs` línea 338-348: El nodo agrega texto a la descripción del tool explicando: "Multi-statement queries: separá statements con `;`. Se ejecutan TODOS en una transacción atómica... El output devuelto es el resultado del ÚLTIMO statement."
- **Esto es información de comportamiento, no de esquema.**

**En la documentación:**
- `docs/developer_guide/23_sql_node.md` línea 248-292 describe el pipeline de ejecución multi-statement y dice: "Multi-statement queries are validated per-statement, so `SELECT 1; DROP TABLE x;` is rejected for the DROP and does not execute." ✓
- `docs/node_configurations.json` NO menciona multi-statement.
- `node_as_tools_reference.json` NO menciona multi-statement.
- `node_ports_reference.md` NO menciona multi-statement.

**Impacto para QA:**
- Si el LLM llama a la query con dos statements (SELECT + INSERT), el operador/usuario espera que ambos se ejecuten pero solo el INSERT sea retornado. Esto está documentado en la guía, pero el usuario que apenas mira `node_configurations.json` no lo ve.
- Prueba: (1) SELECT + INSERT → ambos ejecutan, output es resultado de INSERT. (2) SELECT + DROP → DROP bloqueado, SELECT executa ¿y se retorna?, ¿o se retorna error de DROP?.

---

### Hallazgo 6: Auto-RLS post-CREATE TABLE no está claramente documentado como side-effect

**En el código:**
- `sql.rs` línea 624-655: Si `permissions.auto_rls() = true` Y la query contiene CREATE TABLE, el nodo automáticamente ejecuta `setup_rls_for_new_table()` DESPUÉS de que la query se ejecute.

**En la documentación:**
- `docs/developer_guide/23_sql_node.md` línea 17 menciona "Auto-create RLS policies during init and after CREATE TABLE" pero este es un side-effect silencioso.
- No hay documentación de "WARNING: si tu query crea una tabla, RLS se activará automáticamente".

**Impacto para QA:**
- Prueba: CREATE TABLE newt (id INT); sin RLS → el nodo crea la tabla + automáticamente setup RLS si auto_rls=true. Verificar que la tabla queda con políticas RLS.

---

### Hallazgo 7: Error handling strategy (nunca lanza, siempre retorna JSON con error)

**En el código:**
- `sql.rs` línea 672-693: En caso de error, retorna `Ok(json!({ "error": ..., "source": "..." }))` — NUNCA `Err(...)`.
- El nodo es "fail-open" en el sentido de que no detiene el DAG; simplemente emite un JSON con error adentro.

**En la documentación:**
- `docs/developer_guide/23_sql_node.md` línea 219-230 describe "Error Envelope" pero lo presenta como "On failure, the node returns an error envelope **without throwing**. This allows downstream nodes to handle errors gracefully."
- ✓ Está documentado, pero de forma pasiva ("allows downstream nodes...").

**Impacto para QA:**
- Toda prueba de error debe verificar: (1) el nodo NO falla (retorna Ok), (2) el JSON contiene `"error"` y `"source"`, (3) los downstream nodes reciben JSON, no un error de ejecución.

---

## 3) Plan de pruebas QA

### Prueba 1: Permisos — preset read_only

**Objetivo:** SELECT permitido; INSERT/UPDATE/DELETE bloqueados.

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "sql_select",
      "node_type": "sql_query",
      "config": {
        "connection_url": "${DATABASE_URL}",
        "permissions": { "preset": "read_only" },
        "query": "SELECT 1 AS result"
      }
    }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run <graph.json>
```

**Resultado esperado:**
- Retorna `{ "output": [{ "result": 1 }], "row_count": 1, "truncated": false }`
- Sin error.

**Verificación de fail (INSERT):**
- Grafo con `query: "INSERT INTO t VALUES (1)"` 
- Retorna `{ "error": "BLOCKED by static validator...", "source": "static_validator" }`

---

### Prueba 2: Permisos — preset read_write

**Objetivo:** SELECT, INSERT, UPDATE permitidos; DELETE y ALTER bloqueados.

**Grafo:** schema con tabla `users (id INT, name TEXT)` + permisos read_write.

**Caso 1 — INSERT:**
```bash
query: "INSERT INTO users (id, name) VALUES (1, 'Alice')"
```
**Esperado:** `{ "output": { "rows_affected": 1 }, "row_count": 1, "truncated": false }`

**Caso 2 — UPDATE:**
```bash
query: "UPDATE users SET name = 'Bob' WHERE id = 1"
```
**Esperado:** `{ "output": { "rows_affected": 1 }, "row_count": 1, "truncated": false }`

**Caso 3 — DELETE bloqueado:**
```bash
query: "DELETE FROM users WHERE id = 1"
```
**Esperado:** `{ "error": "BLOCKED...", "source": "static_validator" }`

---

### Prueba 3: Permisos — preset read_write_delete

**Objetivo:** SELECT, INSERT, UPDATE, DELETE, ALTER TABLE ADD COLUMN permitidos.

**Grafo:** read_write_delete preset.

**Caso 1 — DELETE con WHERE:**
```bash
query: "DELETE FROM users WHERE id = 1"
```
**Esperado:** Éxito. `{ "output": { "rows_affected": 1 }, ... }`

**Caso 2 — ALTER TABLE ADD COLUMN:**
```bash
query: "ALTER TABLE users ADD COLUMN email VARCHAR(255)"
```
**Esperado:** Éxito (si permitido en preset).

**Caso 3 — DROP bloqueado:**
```bash
query: "DROP TABLE users"
```
**Esperado:** `{ "error": "BLOCKED...", "source": "static_validator" }`

---

### Prueba 4: Validación — DELETE sin WHERE

**Objetivo:** Validación bloqueada.

**Grafo:** any preset.

```bash
query: "DELETE FROM users"
```

**Esperado:** `{ "error": "BLOCKED by static validator...: DELETE without a WHERE clause...", "source": "static_validator" }`

---

### Prueba 5: Validación — UPDATE sin WHERE

**Objetivo:** Validación bloqueada.

```bash
query: "UPDATE users SET name = 'X'"
```

**Esperado:** Bloqueado con error de static_validator.

---

### Prueba 6: Validación — TRUNCATE bloqueado

**Objetivo:** TRUNCATE siempre bloqueado.

```bash
query: "TRUNCATE users"
```

**Esperado:** `{ "error": "BLOCKED...", "source": "static_validator" }`

---

### Prueba 7: Validación — CREATE FUNCTION sin COMMENT ON

**Objetivo:** Función sin COMMENT ON bloqueada.

```bash
query: "CREATE FUNCTION add_one(x INT) RETURNS INT AS $$ SELECT x + 1; $$ LANGUAGE SQL"
```

**Esperado:** `{ "error": "BLOCKED...: CREATE FUNCTION must be accompanied by COMMENT ON...", "source": "static_validator" }`

---

### Prueba 8: Validación — CREATE FUNCTION con COMMENT ON

**Objetivo:** Función con COMMENT ON permitida (si preset = full).

```bash
query: "CREATE FUNCTION add_one(x INT) RETURNS INT AS $$ SELECT x + 1; $$ LANGUAGE SQL; COMMENT ON FUNCTION add_one(INT) IS 'Add one to input';"
```

**Esperado:** `{ "output": { "created": true }, ... }` (si full preset y ambos statements ejecutan).

---

### Prueba 9: Multi-statement — SELECT + INSERT

**Objetivo:** Ambos statements ejecutan en una transacción; output es el del último (INSERT).

**Grafo:**
```bash
query: "SELECT COUNT(*) AS cnt FROM users; INSERT INTO users (id, name) VALUES (99, 'Test');"
```

**Esperado:**
- Ambos statements se ejecutan.
- Output es resultado del INSERT: `{ "output": { "rows_affected": 1 }, "row_count": 1, "truncated": false }`
- SELECT anterior se ejecuta pero su resultado se descarta.

**Verificación:** Ver logs; SELECT debe haber sido ejecutado.

---

### Prueba 10: Multi-statement — SELECT + DROP

**Objetivo:** Drop bloqueado; ¿qué pasa con el primer SELECT?

**Grafo:**
```bash
query: "SELECT COUNT(*) FROM users; DROP TABLE users;"
```

**Esperado (hipótesis basada en código):**
- Validación ocurre per-statement.
- Statement 1 (SELECT) pasa validación.
- Statement 2 (DROP) falla validación.
- Transacción **no se inicia** (porque una validación falló) → SELECT no se ejecuta.
- Retorna: `{ "error": "BLOCKED...: DROP...", "source": "static_validator" }`

**Verificación:** Confirmar que la tabla no fue modificada.

---

### Prueba 11: Multi-row INSERT

**Objetivo:** Multi-row VALUES se permite sin límite (solo statement_timeout_ms).

**Grafo:**
```bash
query: "INSERT INTO users (id, name) VALUES (1, 'A'), (2, 'B'), (3, 'C')"
```

**Esperado:** `{ "output": { "rows_affected": 3 }, "row_count": 3, "truncated": false }`

---

### Prueba 12: Output — SELECT retorna array

**Objetivo:** SELECT returns array of row objects.

**Grafo:**
```bash
query: "SELECT id, name FROM users LIMIT 2"
```

**Esperado:**
```json
{
  "output": [
    { "id": 1, "name": "Alice" },
    { "id": 2, "name": "Bob" }
  ],
  "row_count": 2,
  "truncated": false
}
```

---

### Prueba 13: Output — CREATE TABLE

**Objetivo:** CREATE TABLE output shape.

**Grafo:**
```bash
query: "CREATE TABLE new_table (id INT, val TEXT)"
```

**Esperado:**
```json
{
  "output": { "created": true, "type": "table" },
  "row_count": 0,
  "truncated": false
}
```

---

### Prueba 14: Output — CREATE FUNCTION

**Objetivo:** CREATE FUNCTION output shape.

**Grafo:**
```bash
query: "CREATE FUNCTION my_func() RETURNS INT AS $$ SELECT 42 $$ LANGUAGE SQL; COMMENT ON FUNCTION my_func() IS 'Test';"
```

**Esperado:**
```json
{
  "output": { "created": true },
  "row_count": 0,
  "truncated": false
}
```

---

### Prueba 15: Runtime limit — max_rows truncation

**Objetivo:** SELECT que retorna > max_rows se trunca.

**Grafo:** 100 filas, max_rows = 10.

```bash
query: "SELECT * FROM large_table"
```

**Esperado:**
```json
{
  "output": [... 10 rows ...],
  "row_count": 100,
  "truncated": true
}
```

**Verificación:** `truncated: true` si rows > max_rows; `row_count` = total real.

---

### Prueba 16: Runtime limit — statement_timeout_ms

**Objetivo:** Query que tarda > statement_timeout_ms se cancela.

**Grafo:** statement_timeout_ms = 100ms; query that takes >100ms.

```bash
query: "SELECT pg_sleep(1)"
```

**Esperado:** Error de timeout. `{ "error": "...", "source": "execution" }`

---

### Prueba 17: Guardrail LLM — api_key vacío falla en init

**Objetivo:** `guardrail_llm.enabled = true` + api_key vacío/no resuelto → error de inicialización.

**Grafo:**
```json
{
  "connection_url": "${DATABASE_URL}",
  "guardrail_llm": {
    "enabled": true,
    "api_key": "${NONEXISTENT_VAR}"
  }
}
```

**Esperado:** El nodo NO se inicializa. Error durante la carga del grafo: "guardrail_llm.enabled = true but api_key resolved to an empty string".

---

### Prueba 18: Guardrail LLM — enabled=true, api_key válido

**Objetivo:** LLM critic se ejecuta; query se revisa antes de ejecutar.

**Grafo:** OPENAI_API_KEY seteada; query = "SELECT * FROM users".

**Esperado:** Query ejecuta después de LLM critic approval. Output normal.

---

### Prueba 19: Schema provisioning — create_schemas_if_missing=true

**Objetivo:** Schemas en allowed_schemas se crean automáticamente si no existen.

**Grafo:**
```json
{
  "permissions": {
    "allowed_schemas": ["my_new_schema"],
    "create_schemas_if_missing": true
  }
}
```

**Esperado:** Al inicializar, el nodo ejecuta `CREATE SCHEMA IF NOT EXISTS my_new_schema`. Schema ahora existe en la BD.

---

### Prueba 20: Schema provisioning — create_schemas_if_missing=false

**Objetivo:** Schemas NO se crean; solo se validan.

**Grafo:**
```json
{
  "permissions": {
    "allowed_schemas": ["nonexistent_schema"],
    "create_schemas_if_missing": false
  }
}
```

**Esperado:** Si schema no existe, init puede fallar O puede ignorar (dependiendo de código). Verificar qué sucede.

---

### Prueba 21: RLS — auto_rls + tenant_user_id

**Objetivo:** RLS policies se crean automáticamente en init.

**Grafo:**
```json
{
  "permissions": {
    "auto_rls": true,
    "tenant_user_id": "user_123",
    "tenant_column": "user_id"
  }
}
```

**Esperado:** Tablas en allowed_schemas reciben RLS policies que filtran por user_id = 'user_123'.

---

### Prueba 22: RLS — auto_rls post-CREATE TABLE

**Objetivo:** Cuando query crea una tabla, RLS se aplica automáticamente después.

**Grafo:** auto_rls=true.

```bash
query: "CREATE TABLE my_new_table (id INT, user_id VARCHAR, data TEXT)"
```

**Esperado:** Tabla se crea + RLS policy se agrega automáticamente.

**Verificación:** `SELECT * FROM pg_policies WHERE tablename = 'my_new_table'` retorna filas.

---

### Prueba 23: Governance lock — permissions no puede ser sobrescrito por input

**Objetivo:** Si config.permissions existe, input.permissions es ignorado.

**Grafo:**
```json
{
  "config": {
    "permissions": { "preset": "read_only" }
  },
  "upstream_output": {
    "permissions": { "preset": "full" }
  }
}
```

**Esperado:** Aunque upstream emita `preset: full`, el nodo usa `preset: read_only`. Las queries están bloqueadas a SELECT.

---

### Prueba 24: Governance lock — connection_url no puede ser sobrescrito

**Objetivo:** connection_url en config no puede ser overridden por input.

**Grafo:** config.connection_url = postgres://main; input.connection_url = postgres://attacker.

**Esperado:** El nodo se conecta a postgres://main, no attacker.

---

### Prueba 25: Error source — static_validator

**Objetivo:** Errores de validación reportan source="static_validator".

```bash
query: "DELETE FROM users"
```

**Esperado:** `{ "error": "...", "source": "static_validator" }`

---

### Prueba 26: Error source — llm_critic

**Objetivo:** Errores de LLM critic reportan source="llm_critic".

**Grafo:** guardrail_llm enabled; LLM critic rechaza query.

**Esperado:** `{ "error": "Critic rejected: ...", "source": "llm_critic" }`

---

### Prueba 27: Error source — execution

**Objetivo:** Errores de DB reportan source="execution".

```bash
query: "SELECT * FROM nonexistent_table"
```

**Esperado:** `{ "error": "...: table not found", "source": "execution" }`

---

### Prueba 28: Schema metadata — small database

**Objetivo:** Tool description incluye schema render.

**Grafo:** Database con ≤40 tablas; render ≤8000 chars.

**Esperado:** Descripción del tool contiene tabla con columns + tipos + keys.

---

### Prueba 29: Schema metadata — large database

**Objetivo:** Tool description degrada a lista simple.

**Grafo:** Database con >40 tablas O render >8000 chars.

**Esperado:** Descripción degrada a lista simple: "Tablas (schema: X): - schema.table1 - schema.table2 ... (Schema grande: usá introspección...)"

---

### Prueba 30: Env var resolution — connection_url

**Objetivo:** `${DATABASE_URL}` se resuelve.

**Grafo:**
```bash
connection_url: "${DATABASE_URL}"
```

**Esperado:** Si DATABASE_URL está seteada, se usa ese valor. Query se ejecuta contra la BD correcta.

---
