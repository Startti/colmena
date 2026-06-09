# Tablas pendientes en ADP Prisma para sincronizar con colmena develop

> Última verificación: 2026-06-09 (post v1.1 paragraph diff)
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

ADP **NO tiene** 4 tablas nuevas + 1 extensión a `gdocs_session_state`
que colmena necesita. Sin las 4 tablas el subsistema CRDT y el subsistema
G fallan al primer call. Sin la extensión v1.1 el guard de Google Docs
funciona pero **degrada a v1 behavior** (sin diff per-paragraph; warn
al boot). Hay que crear una nueva migration en ADP siguiendo el patrón
`migrate deploy`-only.

| Tabla / change | Migration de origen en colmena | Subsistema | Criticidad |
|---|---|---|---|
| `crdt_doc_events` | `20260603000000_crdt_doc_changes.sql` | CRDT documents | bloqueante |
| `crdt_doc_session_cursors` | `20260603000000_crdt_doc_changes.sql` | CRDT documents | bloqueante |
| `crdt_doc_session_artifacts` | `20260603000000_crdt_doc_changes.sql` | CRDT documents | bloqueante |
| `gdocs_session_state` ⭐ | `20260608000000_gdocs_session_state.sql` | G — Google Docs v1 | bloqueante |
| `gdocs_session_state` ADD COLUMNS (v1.1) | `20260609000000_gdocs_session_state_snapshot.sql` | G v1.1 paragraph diff | feature flag — graceful degrade |

Detalles de las 4 tablas en §1-§4 abajo; detalles del ADD COLUMNS v1.1
en §5.

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

**Opción A — combo single-PR (recomendado):**

1. Crear `packages/database/prisma/migrations/<YYYYMMDDHHMMSS>_colmena_crdt_gdocs_v11/migration.sql`
   con TODO el SQL en orden:
   - 3 CRDT tables (`crdt_doc_events`, `crdt_doc_session_cursors`,
     `crdt_doc_session_artifacts`)
   - `gdocs_session_state` (v1 base)
   - `ALTER TABLE gdocs_session_state ADD COLUMN IF NOT EXISTS …` (v1.1 extension — §5)
2. Agregar los 4 models al `schema.prisma`, después de
   `ProviderFileCache`. El modelo `GdocsSessionState` debe incluir los
   campos v1 + v1.1 desde el principio (`lastSnapshotJson` y
   `lastSnapshotSizeBytes` como `Json?` / `Int?` — ver §5).
3. Local: `pnpm prisma migrate deploy` contra una DB de dev.
4. Verificar que `apps/service/ia/platform/worker` compila y boot-checks
   pasan localmente contra esa DB **sin** el warn
   `gdocs.snapshot.column_missing`.
5. Cloud: el deploy hook
   (`apps/service/ia/platform/deploy_gcp.sh`) corre `migrate deploy`
   antes de levantar los workers.

**Opción B — split en dos PRs (si el v1.1 quiere shipear después):**

- PR-1: solo §1-§4 (las 4 tablas base). Subsystem G v1 funciona; v1.1
  funciona en modo degraded con warn al boot.
- PR-2: §5 (`ALTER TABLE`). Activa el diff per-paragraph; warn
  desaparece.

Ambas opciones son seguras — la migration de §5 es additive con
`IF NOT EXISTS` y colmena degrada grácilmente cuando las columnas no
existen.

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

---

## 5. `gdocs_session_state` — v1.1 extension (2026-06-09)

**Contexto.** Subsystem G v1.1 (paragraph-level human-change diff)
extiende `gdocs_session_state` con dos columnas nullable para persistir
el `DocumentSnapshot` post-write. Cuando el co-edit guard detecta drift,
diff-ea el snapshot prior vs current → lista paragraph-level con
`before_text`/`after_text` particionada por scope. Sin estas columnas,
colmena degrada grácilmente a v1 behavior (block conservador con listas
vacías), pero pierde la feature.

### Raw SQL (idempotente, additive)

```sql
ALTER TABLE gdocs_session_state
  ADD COLUMN IF NOT EXISTS last_snapshot_json       JSONB,
  ADD COLUMN IF NOT EXISTS last_snapshot_size_bytes INTEGER;
```

Archivo: `src/libs/colmena/migrations/postgres/20260609000000_gdocs_session_state_snapshot.sql`

### Update al schema Prisma

En el `model GdocsSessionState` creado en §4, agregar:

```prisma
  lastSnapshotJson      Json?    @map("last_snapshot_json")
  lastSnapshotSizeBytes Int?     @map("last_snapshot_size_bytes")
```

El modelo final queda:

```prisma
model GdocsSessionState {
  agentSessionId        String   @map("agent_session_id")
  documentId            String   @map("document_id")
  lastRevisionId        String   @map("last_revision_id")
  lastEditAt            DateTime @default(now()) @map("last_edit_at") @db.Timestamptz()
  lastSnapshotJson      Json?    @map("last_snapshot_json")              // v1.1
  lastSnapshotSizeBytes Int?     @map("last_snapshot_size_bytes")        // v1.1

  @@id([agentSessionId, documentId])
  @@index([lastEditAt], map: "gdocs_session_state_last_edit_at_idx")
  @@map("gdocs_session_state")
}
```

### Behavior cuando no está aplicada

Colmena detecta la ausencia de la columna `last_snapshot_json` al boot
via `information_schema.columns` y loguea **una sola vez**:

```
gdocs: last_snapshot_json column missing on gdocs_session_state;
co-edit guard degrades to v1 (revisionId equality only).
Apply migration 20260609000000_gdocs_session_state_snapshot.sql
```

No crash, no data loss, no breaking change para ADP. Las queries del
adapter ramifican vía un flag `has_snapshot_col` para que las inserts
no toquen las columnas inexistentes.

### Cap de tamaño

Constante `DEFAULT_MAX_SNAPSHOT_BYTES = 1_048_576` (1 MB). Override en
runtime via `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`. Si un snapshot
serializado supera el cap, se descarta (`NULL` en la columna JSONB) y
ese `(session, doc)` específico funciona en modo degraded con warn
`gdocs.snapshot.too_large`.

### Aplicación recomendada en ADP

Mismo flujo que §4 (`gdocs_session_state` base):

1. Crear nueva migration:
   `packages/database/prisma/migrations/<YYYYMMDDHHMMSS>_gdocs_session_state_snapshot/migration.sql`
   con el SQL idempotente de arriba.
2. Actualizar `model GdocsSessionState` en `schema.prisma`.
3. `pnpm prisma migrate deploy` contra dev.
4. Verificar que el worker recompila + boot-check confirma columnas
   presentes (sin el warn).
5. Cloud: `apps/service/ia/platform/deploy_gcp.sh` corre `migrate deploy`
   en cada deploy.

### Referencias

- Spec: `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`
- Plan: `docs/superpowers/plans/2026-06-09-gdocs-paragraph-diff.md`
- Dev guide: `docs/developer_guide/45_gdocs.md` §"Co-edit safety pipeline"
- CHANGELOG: `docs/CHANGELOG_2026-06.md` §17
