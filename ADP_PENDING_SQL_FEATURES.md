# ADP — Cambios pendientes por features de `sql_query` (Colmena `develop`)

> **Para el equipo de ADP.** Tres features de Colmena se mergearon a `develop`
> (2026-06-21). El worker (`apps/service/ia/platform/{worker,api}/`) consume
> `colmena develop` vía Cargo, así que el próximo Cloud Build las trae
> automáticamente. **Ninguna rompe compilación** (todo es aditivo). Este doc
> lista lo que ADP puede/debe revisar para (a) confirmar que sigue compilando y
> (b) exponer las features nuevas en el canvas.
>
> Commits en develop: `e3451b8d` (#117), `ffe67dff` (#116), `668570dc` (#118).

---

## TL;DR — ¿ADP tiene que hacer algo obligatorio?

**No para compilar.** Las 3 features son aditivas:
- Nuevos métodos en traits internos (`ExecutableNode::as_initializable` con default `None`; `SqlConnectionPort::execute_setup_sql` / `load_table_schemas`), todos con un único impl in-repo en Colmena → el worker de ADP **no implementa esos traits**, así que no necesita cambios.
- Nuevo valor de preset y nuevos campos de config **opcionales**.

**Sí, recomendado** para que los usuarios puedan usar las features: exponer el preset y el campo nuevos en el canvas (ver "Acciones recomendadas").

**No hay migraciones de Prisma/DB** para estas features (a diferencia del gdocs paragraph-diff). `setup_sql` provisiona schemas en runtime del lado del operador, no vía Prisma.

---

## Feature 1 — Preset `read_write_delete` + `ALTER TABLE ADD COLUMN` + inyección de schema/capacidades (PR #118)

### Qué cambió en el motor
- **Nuevo preset `read_write_delete`** = `SELECT, INSERT, UPDATE, DELETE, ALTER TABLE ADD COLUMN`. (Hueco que faltaba entre `read_write` —sin DELETE— y `full` —con CREATE TABLE—.)
- **`full` ahora también permite `ALTER TABLE ADD COLUMN`** (antes lo bloqueaba). Solo `ADD COLUMN`; cualquier otro `ALTER` (DROP COLUMN, ALTER TYPE, RENAME) sigue bloqueado en **todos** los presets, junto con DROP/TRUNCATE/CREATE SCHEMA.
- **Nuevo valor de `deny`: `add_column`**.
- La descripción de la tool `sql_query` ahora se **auto-enriquece** en el init con: (1) un bloque de capacidades en lenguaje natural derivado del preset, y (2) el schema completo (columnas + tipos + PK/UNIQUE/FK), con cap a 40 tablas / ~8000 chars.
- **Wiring**: `ExecutableNode::as_initializable()` (default `None`) + `DagToolExecutor::available_tools()` ahora llama `initialize()` para nodos que lo soportan y agrega el supplement a la descripción.

### Impacto de compilación en ADP
**Ninguno.** Si el worker implementa nodos custom con `ExecutableNode`, el método nuevo tiene default `None` → no requiere cambios.

### ⚠️ Nota de comportamiento (importante)
`available_tools()` ahora, para tools `sql_query`, **conecta a la base e introspecciona el schema en el momento de listar las tools** (cacheado por `OnceCell`, una vez por instancia de nodo). Implicancias para el worker:
- El listado de tools de un agente con tool SQL ahora hace un connect a Postgres la primera vez.
- Si el `connection_url` es un **secure handle `<sv_*>` sin resolver** al momento del listado, el connect falla silenciosamente y la tool se lista **sin** el schema inyectado (no crashea). Si ADP arma agentes SQL con credenciales vía secure values, tener esto en cuenta: el schema solo se inyecta cuando `connection_url` resuelve en tiempo de listado (ej. `${DATABASE_URL}` por env).

### Acciones recomendadas en ADP (canvas / config — las hace el equipo de ADP)
- [ ] **Ofrecer `read_write_delete` como preset seleccionable** en la UI de permisos de la tool SQL.
- [ ] Si hay un **enum/validador del valor `preset`** (frontend o algún validador de config), **agregar `read_write_delete`** para que no se rechace.
- [ ] (Opcional) Ofrecer `add_column` como opción de `deny`.
- [ ] (Opcional) Documentar para el usuario que `read_write_delete`/`full` pueden agregar columnas (no borrarlas ni cambiar tipos).

---

## Feature 2 — `setup_sql`: bootstrapping de entorno (PR #116)

### Qué cambió en el motor
- Nuevo campo **opcional** `setup_sql` (string SQL multi-statement) en la config del nodo `sql_query`. Corre DDL + seed **idempotente** en el init del nodo (nivel-operador, no pasa por el validador del LLM), en una transacción atómica. Permite que un grafo publicado auto-provisione su schema/tablas/seed en el primer uso.

### Impacto de compilación en ADP
**Ninguno** (campo opcional nuevo; `execute_setup_sql` es método de trait interno con un solo impl).

### Acciones recomendadas en ADP
- [ ] **Permitir definir `setup_sql`** en la config de la tool `sql_query` del canvas (textarea multilínea).
- [ ] Documentar para autores el **contrato de idempotencia**: usar `CREATE ... IF NOT EXISTS`, `INSERT ... ON CONFLICT DO NOTHING`, `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`. No usar `BEGIN/COMMIT` explícito ni `CREATE INDEX CONCURRENTLY`/`VACUUM` (corre dentro de una transacción implícita).
- [ ] Si ADP valida/serializa el shape de config de `sql_query`, agregar `setup_sql` como string opcional.

---

## Feature 3 — Fix: campos `fixed` del node_schema son autoritativos (PR #117)

### Qué cambió en el motor
- En `DagToolExecutor`, un argumento del LLM ya **no puede sobrescribir** un campo `fixed` declarado por el operador en `node_schema`. Antes, un modelo que emitía una clave igual a un campo fixed (ej. `connection_url`, `permissions`, `setup_sql`) lo pisaba. Ahora se ignora (con warning logueado). Cierra un gap de seguridad (especialmente relevante con `setup_sql`, exento del validador).

### Impacto en ADP
**Ninguno** de compilación. Nota: si algún flujo de ADP dependía de que el LLM pudiera sobreescribir un campo `fixed` (no debería — era un bug), ese override ahora se ignora. Altamente improbable que algo dependa de eso.

---

## Checklist de verificación para ADP

- [ ] Rebuild del worker contra `colmena develop` (`668570dc`) → confirmar que compila limpio.
- [ ] Smoke test de un agente con tool `sql_query` existente → confirmar que sigue funcionando (la inyección de schema es transparente; el listado de tools ahora hace un connect a DB).
- [ ] (Si aplica) Exponer `read_write_delete` y `setup_sql` en el canvas.
- [ ] (Si aplica) Actualizar cualquier enum/validador de `preset` para incluir `read_write_delete`.

## Referencias en el repo Colmena
- Specs: `docs/superpowers/specs/2026-06-21-sql-setup-block-design.md`, `docs/superpowers/specs/2026-06-21-sql-schema-context-and-crud-preset-design.md`.
- Guía: `docs/developer_guide/23_sql_node.md` (presets, `setup_sql`, contexto inyectado).
- Schema canónico: `docs/node_configurations.json` (`sql_query`).
