# Tablas pendientes en ADP Prisma para sincronizar con colmena develop

> Última verificación: 2026-06-09
> Verificado contra: `apps/service/ia/platform/{api,worker}` (Rust) + `packages/database/prisma/schema.prisma`.

## Resumen ejecutivo

El worker Rust de ADP (`apps/service/ia/platform/worker`) consume colmena
develop directamente vía Cargo. Cuando colmena agrega una migration de
Postgres bajo `src/libs/colmena/migrations/postgres/`, ADP debe re-escribirla
a mano en su propio dir de Prisma y aplicarla con `migrate deploy` (nunca
`migrate dev` / `reset` — ver memoria del operador).

ADP **ya tiene mirror** de estas 7 tablas de colmena:

1. `ConversationAttachment` → `conversation_attachments`
2. `LlmNodeHistory` → `llm_node_history`
3. `DagRun` → `dag_runs`
4. `DagTaskMemory` → `dag_task_memory`
5. `DagPhaseSummary` → `dag_phase_summary`
6. `SecureValueMapping` → `secure_value_mappings`
7. `ProviderFileCache` → `provider_file_cache`

ADP **NO tiene** 4 tablas que colmena necesita. Sin ellas, el subsistema CRDT
de documents y el subsistema G (Google Docs co-edit guard) fallan en el
primer call. Hay que crear una nueva migration en ADP siguiendo el patrón
`migrate deploy`-only.

| Tabla | Migration de origen en colmena | Subsistema |
|-------|--------------------------------|------------|
| `crdt_doc_events` | `20260603000000_crdt_doc_changes.sql` | CRDT documents |
| `crdt_doc_session_cursors` | `20260603000000_crdt_doc_changes.sql` | CRDT documents |
| `crdt_doc_session_artifacts` | `20260603000000_crdt_doc_changes.sql` | CRDT documents |
| `gdocs_session_state` ⭐ | `20260608000000_gdocs_session_state.sql` | G — Google Docs |

## Tablas faltantes

### 1. `crdt_doc_events`

**Por qué:** el subsistema CRDT de documents mantiene un event log append-only
con un resumen por mutación (qué celda/sheet cambió, quién la originó). Es la
fuente de verdad para reconstruir el estado de un artefacto y para que
cada sesión sepa qué eventos ya consumió (vía `crdt_doc_session_cursors`).

**SQL (verbatim desde colmena):**

```sql
CREATE TABLE IF NOT EXISTS crdt_doc_events (
    id          BIGSERIAL   PRIMARY KEY,
    artifact_id TEXT        NOT NULL,
    sheet_id    TEXT,
    origin      TEXT        NOT NULL,
    summary     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS crdt_doc_events_lookup
    ON crdt_doc_events(artifact_id, id);

CREATE INDEX IF NOT EXISTS crdt_doc_events_by_sheet
    ON crdt_doc_events(artifact_id, sheet_id, id);
```

**Columnas:**

- `id` — BIGSERIAL, PK monotónica. Los cursors la usan para `WHERE id > last_event_id`.
- `artifact_id` — id del documento CRDT (no del AgentSession). Múltiples sesiones pueden tocar el mismo artifact.
- `sheet_id` — opcional; relevante para artefactos con sub-páginas (gsheets-style). NULL para docs planos.
- `origin` — quién originó el cambio (e.g. `agent`, `human`, `tool:<name>`).
- `summary` — texto cortito describiendo la mutación (no es el diff completo).
- `created_at` — timestamp del append.

**Índices:**

- `crdt_doc_events_lookup` — barrido por artifact, ordenado por id. Es el camino caliente del cursor diff.
- `crdt_doc_events_by_sheet` — barrido filtrado por sheet dentro del artifact.

**Prisma model:**

```prisma
model CrdtDocEvent {
  id         BigInt   @id @default(autoincrement())
  artifactId String   @map("artifact_id")
  sheetId    String?  @map("sheet_id")
  origin     String
  summary    String
  createdAt  DateTime @default(now()) @map("created_at") @db.Timestamptz()

  @@index([artifactId, id], map: "crdt_doc_events_lookup")
  @@index([artifactId, sheetId, id], map: "crdt_doc_events_by_sheet")
  @@map("crdt_doc_events")
}
```

### 2. `crdt_doc_session_cursors`

**Por qué:** cada `(agent_session_id, artifact_id)` necesita recordar el
último `crdt_doc_events.id` que esa sesión ya consumió. Sin esta tabla,
el agente re-procesa el historial completo en cada turno o pierde
mutaciones que ocurrieron entre turnos.

**SQL (verbatim):**

```sql
CREATE TABLE IF NOT EXISTS crdt_doc_session_cursors (
    agent_session_id TEXT        NOT NULL,
    artifact_id      TEXT        NOT NULL,
    last_event_id    BIGINT      NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, artifact_id)
);
```

**Columnas:**

- `agent_session_id` + `artifact_id` — PK compuesta.
- `last_event_id` — referencia a `crdt_doc_events.id` (no FK explícita; FK haría el log append-only más caro y la tabla de eventos es eventually-purgeable).
- `updated_at` — para auditar staleness.

**Prisma model:**

```prisma
model CrdtDocSessionCursor {
  agentSessionId String   @map("agent_session_id")
  artifactId     String   @map("artifact_id")
  lastEventId    BigInt   @map("last_event_id")
  updatedAt      DateTime @default(now()) @map("updated_at") @db.Timestamptz()

  @@id([agentSessionId, artifactId])
  @@map("crdt_doc_session_cursors")
}
```

### 3. `crdt_doc_session_artifacts`

**Por qué:** índice inverso por sesión — "¿qué artefactos tocó este
AgentSession y cuándo fue el último acceso?". Permite poblar el catálogo
de artefactos en el system message del LLM y aplicar políticas de TTL
por sesión.

**SQL (verbatim):**

```sql
CREATE TABLE IF NOT EXISTS crdt_doc_session_artifacts (
    agent_session_id TEXT        NOT NULL,
    artifact_id      TEXT        NOT NULL,
    name             TEXT        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, artifact_id)
);

CREATE INDEX IF NOT EXISTS crdt_doc_session_artifacts_recent_idx
    ON crdt_doc_session_artifacts(agent_session_id, last_accessed_at DESC);
```

**Prisma model:**

```prisma
model CrdtDocSessionArtifact {
  agentSessionId String   @map("agent_session_id")
  artifactId     String   @map("artifact_id")
  name           String
  createdAt      DateTime @default(now()) @map("created_at") @db.Timestamptz()
  lastAccessedAt DateTime @default(now()) @map("last_accessed_at") @db.Timestamptz()

  @@id([agentSessionId, artifactId])
  @@index([agentSessionId, lastAccessedAt(sort: Desc)], map: "crdt_doc_session_artifacts_recent_idx")
  @@map("crdt_doc_session_artifacts")
}
```

### 4. `gdocs_session_state` ⭐ NUEVA — subsistema G shipped 2026-06-08

**Por qué:** el co-edit guard de Google Docs (subsystem G) compara la
`revisionId` actual del doc (consultada vía Drive API) contra la última
revisión que el agente vio. Si difieren, hubo edición humana entre
turnos y el agente debe re-leer antes de escribir. Sin esta tabla el
agente no puede detectar ediciones humanas y podría sobrescribir
trabajo del usuario.

**Crítico:** sin esta tabla el subsistema G arroja
`gdocs_not_configured: DATABASE_URL required for revision tracking` en el
primer call y bloquea toda actividad de gdocs en producción.

**SQL (verbatim):**

```sql
-- gdocs_session_state — last-known revision per (agent_session, doc).
-- Used by the co-edit guard to detect human edits between agent writes.

CREATE TABLE IF NOT EXISTS gdocs_session_state (
    agent_session_id TEXT        NOT NULL,
    document_id      TEXT        NOT NULL,
    last_revision_id TEXT        NOT NULL,
    last_edit_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_session_id, document_id)
);

CREATE INDEX IF NOT EXISTS gdocs_session_state_last_edit_at_idx
    ON gdocs_session_state (last_edit_at);
```

**Prisma model:**

```prisma
model GdocsSessionState {
  agentSessionId String   @map("agent_session_id")
  documentId     String   @map("document_id")
  lastRevisionId String   @map("last_revision_id")
  lastEditAt     DateTime @default(now()) @map("last_edit_at") @db.Timestamptz()

  @@id([agentSessionId, documentId])
  @@index([lastEditAt], map: "gdocs_session_state_last_edit_at_idx")
  @@map("gdocs_session_state")
}
```

**Relación a `AgentSession`:** el modelo `ConversationAttachment` (que
también es composite-keyed por `agent_session_id`) **no** declara una
relación Prisma a `AgentSession` — solo el `@@id` compuesto. Las 4
tablas nuevas siguen ese mismo patrón. Si en el futuro se quiere
cascada `onDelete`, agregar la FK en una migration aparte (las
migraciones de colmena no la declaran a propósito, para mantener el
event log y los cursors purgeables independientemente).

## Cómo aplicar

ADP NO ejecuta las migraciones de colmena directamente; las re-escribe
en su propio dir de Prisma siguiendo el modelo que ya usa para
`secure_value_mappings`, `llm_node_history`, etc. Per memoria del
operador: **solo `migrate deploy`. Nunca `migrate dev` ni `migrate reset`.**

1. Crear `packages/database/prisma/migrations/<YYYYMMDDHHMMSS>_colmena_crdt_and_gdocs/migration.sql` con el SQL combinado de las 4 tablas (en el orden de arriba: las 3 de CRDT en un bloque, luego `gdocs_session_state`).
2. Agregar los 4 models al `schema.prisma`, después de los modelos espejo existentes de colmena (después de `ProviderFileCache`).
3. Local: `pnpm prisma migrate deploy` contra una DB de dev.
4. Verificar que `apps/service/ia/platform/worker` compila y boot-checks pasan localmente contra esa DB.
5. Cloud: el deploy hook (`apps/service/ia/platform/deploy_gcp.sh`) corre `migrate deploy` antes de levantar los workers.

## Notas operativas

- Tipos: TEXT → `String`, TIMESTAMPTZ → `DateTime @db.Timestamptz()`, BIGSERIAL → `BigInt @id @default(autoincrement())`, BIGINT → `BigInt`.
- Nombres de campo en Prisma: camelCase + `@map("snake_case")` (sigue convención de `ConversationAttachment`, NO la de `LlmNodeHistory` que usa snake_case directo — ambas coexisten en ADP; la camelCase es la preferida para tablas nuevas).
- Nombres de tabla (`@@map`) en snake_case y singular como vienen de colmena.
- Nombres de índice (`map: "..."`) copiados verbatim desde la migration de colmena para que `prisma db pull` futuro sea idempotente.
- BIGSERIAL en Postgres no requiere `@db.BigInt` en Prisma — `BigInt` ya mapea a `bigint`. `@default(autoincrement())` produce `SERIAL`/`BIGSERIAL` según el tipo.

## Inconsistencias detectadas

1. **Convención de naming de columnas mixta en ADP.** `ConversationAttachment` usa camelCase + `@map()`. `LlmNodeHistory` y `SecureValueMapping` usan snake_case directo en los nombres de campo de Prisma. Esta doc recomienda camelCase + `@map()` (la más moderna) para las 4 tablas nuevas, pero el operador puede preferir snake_case por consistencia con `LlmNodeHistory` — ambas son válidas a nivel SQL. **Decidir antes de crear el PR.**
2. **Sin FK declarada a `AgentSession`.** Las migraciones de colmena no declaran FK, y `ConversationAttachment` tampoco la declara en Prisma. Si ADP quiere cascada `onDelete: Cascade` al borrar un AgentSession (consistente con `LlmNodeHistory.agentSession`), hay que agregarla manualmente — pero ojo: el event log `crdt_doc_events` NO tiene `agent_session_id`, solo `artifact_id`. Solo las 3 tablas restantes son candidatas a FK.
3. **No hay rollback en la migration de CRDT.** Solo `gdocs_session_state` viene con bloque rollback comentado. Si ADP necesita rollback para la migration combinada, escribirlo a mano (DROP INDEX + DROP TABLE en orden inverso).
