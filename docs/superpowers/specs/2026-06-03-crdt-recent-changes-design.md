# CRDT Documents — Recent changes awareness + artifact discovery (B)

**Status:** approved 2026-06-03
**Subsystem:** B of the post-V2 MVP roadmap
**Predecessor:** [V2 — WS peer mode](2026-06-01-documents-crdt-v1-design.md)

## 1. Problema

Cuando un agente vuelve a operar sobre un workbook (mismo `agent_session_id`, mismo `artifact_id`), no tiene visibilidad de qué pasó entre su turno anterior y el actual. Hay tres gaps concretos:

1. **No sabe qué cambiaron otros peers**: el humano editó D4 en el browser, otro agente agregó una row, y el agente actual no se entera. Tiene que **adivinar o pedir contexto explícitamente** vía la tool `crdt_doc_get_recent_changes` (que hoy existe pero requiere acordarse de llamarla).
2. **No tiene memoria de qué workbooks creó o accedió previamente**: si la sesión tiene 5 artifacts, el agente solo conoce el `artifact_id` que viene pineado en el graph config. No puede listar, no puede ramificarse a otro.
3. **No puede crear workbooks nuevos desde adentro del turno**: si la conversación necesita "exportá esto a una hoja nueva", el agente no tiene tool para eso.

El estado actual del `ChangeTracker` (in-memory, per-process) además **no sobrevive restarts del WS server**, lo cual es un agravante para (1).

## 2. Resultado deseado

Un agente con `crdt_documents` configurado:
- Al inicio de cada turn, **ve automáticamente** un summary corto de los cambios desde su último turn (filtrados para excluir sus propias mutaciones).
- Puede pedir detalle célula-por-célula con un tool, filtrable por sheet.
- Puede listar sus workbooks accesibles en la sesión.
- Puede crear workbooks nuevos durante el turn.
- Todo durable: el cursor sobrevive restarts del worker Y del WS server.

## 3. Decisiones tomadas (durante brainstorming)

| # | Decisión | Resolución |
|---|---|---|
| 1 | Definición del cursor | Por `agent_session_id`, filtrando mutaciones propias (origin = "agent:llm"). Modelo: "qué cambiaron los demás mientras yo no estaba". |
| 2 | Persistencia | SQL: tres tablas (`crdt_doc_events`, `crdt_doc_session_cursors`, `crdt_doc_session_artifacts`). |
| 3 | Inyección del contexto | Append al `system_message` (mismo patrón que `temporal_geographic_context`). |
| 4 | Forma del summary | Una línea por sheet con autor+conteo. Sin detalles cell-level. Tope 10 sheets, overflow "...and N more". |
| 5 | Tool drill-down | Extender `crdt_doc_get_recent_changes` con filtros `since_event_id?`, `sheet_id?`, `limit?` (default 50). |
| 6 | Discovery | Nuevos tools `crdt_doc_create_artifact(name)` + `crdt_doc_list_my_artifacts()`. |
| 7 | Config | `crdt_documents.artifact_id` sigue siendo required (no breaking — graphs existentes funcionan igual). |
| 8 | Worker → server lookup | REST `GET /documents/:id/changes?since=N&sheet_id=X&limit=K` desde el worker. |
| 9 | Migrations | Dos archivos en `src/libs/colmena/migrations/{sqlite,postgres}/`. |
| 10 | DB en producción | `adp_db_develop` (mismo que tablas existentes — no requiere coordinar con repo de ADP, las migrations corren al startup). |

## 4. Arquitectura

### 4.1 Componentes nuevos / modificados

```
┌──────────────────────────────────────────────────────────────┐
│  CRDT documents server (proceso A, separado)                 │
│                                                              │
│  ┌──────────────┐    REST    ┌────────────────────────┐      │
│  │  ws_handler  │ ──record──>│   ChangeTrackerStore   │      │
│  │  (WS peer    │            │   (SQL adapter)        │      │
│  │   apply)     │            │                        │      │
│  └──────────────┘            │   ├ insert_event       │      │
│         │                    │   ├ since(cursor)      │──┐   │
│         │ tool_executor      │   ├ touch_artifact     │  │   │
│         │ apply_set_cell     │   └ update_cursor      │  │   │
│         ▼                    └────────────────────────┘  │   │
│  ┌──────────────────┐                                    │   │
│  │ Y.Doc registry   │                                    │   │
│  └──────────────────┘                                    │   │
│                                                          │   │
│  Endpoints:                                              │   │
│  POST /documents      (extended: takes agent_session_id) │   │
│  GET  /documents/by-session/:sid           (new)         │   │
│  GET  /documents/:id/changes?since=&sheet_id=&limit=     │◄──┘
│  GET  /documents/:id/projection.json                     │
│  POST /documents/:id/import                              │
│  GET  /documents/:id/export.xlsx                         │
│  WS   /yjs/:id                                           │
└──────────────────────────────────────────────────────────────┘
                            ▲  REST   ▲ REST   ▲ WS
                            │         │        │
                            │         │        │
┌──────────────────────────────────────────────────────────────┐
│  Worker (proceso B, stateless, ejecuta graphs)               │
│                                                              │
│  llm.rs execute() ──┬─── builds CrdtDocsContext              │
│                     │                                        │
│                     ├─── BEFORE LLM call:                    │
│                     │    REST GET /changes?since=<cursor>    │
│                     │      → narration block                 │
│                     │    APPEND to system_message            │
│                     │                                        │
│                     ├─── DURING LLM call (tool dispatchers): │
│                     │    crdt_doc_set_cell → WS peer mut     │
│                     │      → server records event in SQL     │
│                     │    crdt_doc_get_recent_changes →       │
│                     │      REST GET /changes?...             │
│                     │    crdt_doc_list_my_artifacts →        │
│                     │      REST GET /documents/by-session/.. │
│                     │    crdt_doc_create_artifact →          │
│                     │      REST POST /documents              │
│                     │                                        │
│                     └─── AFTER LLM call:                     │
│                          REST POST cursor update             │
│                          (last_event_id seen by this agent)  │
└──────────────────────────────────────────────────────────────┘
```

### 4.2 Tablas SQL

```sql
-- Append-only log de cambios. Source of truth para "what happened".
CREATE TABLE IF NOT EXISTS crdt_doc_events (
  id BIGSERIAL PRIMARY KEY,                  -- AUTOINCREMENT en sqlite
  artifact_id TEXT NOT NULL,
  sheet_id TEXT,                             -- nullable: ops workbook-level
  origin TEXT NOT NULL,                      -- "agent:llm", "peer:browser", etc.
  summary TEXT NOT NULL,                     -- texto humano
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crdt_doc_events_lookup
  ON crdt_doc_events(artifact_id, id);
CREATE INDEX crdt_doc_events_by_sheet
  ON crdt_doc_events(artifact_id, sheet_id, id);

-- Cursor por (sesión, artifact). Marca hasta qué evento ya vio el agente.
CREATE TABLE IF NOT EXISTS crdt_doc_session_cursors (
  agent_session_id TEXT NOT NULL,
  artifact_id TEXT NOT NULL,
  last_event_id BIGINT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (agent_session_id, artifact_id)
);

-- Ownership: qué artifacts pertenecen a cada sesión. Spine para discovery.
CREATE TABLE IF NOT EXISTS crdt_doc_session_artifacts (
  agent_session_id TEXT NOT NULL,
  artifact_id TEXT NOT NULL,
  name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (agent_session_id, artifact_id)
);
CREATE INDEX crdt_doc_session_artifacts_recent_idx
  ON crdt_doc_session_artifacts(agent_session_id, last_accessed_at DESC);
```

**Variantes por dialect**: `BIGSERIAL` en Postgres, `INTEGER PRIMARY KEY AUTOINCREMENT` en SQLite. `TIMESTAMPTZ DEFAULT now()` en Postgres, `TEXT DEFAULT CURRENT_TIMESTAMP` en SQLite. Patrón existente del proyecto (ver `conversation_attachments` migration).

### 4.3 Auto-injected summary

Bloque agregado al `system_message` cuando hay `crdt_documents.artifact_id` Y hay eventos pendientes:

```
---
Workbook changes since your last turn (5 events, 2 peers):
- Inventory: 3 changes by peer:browser
- Pricing: 2 changes by peer:agent_orchestrator
Use `crdt_doc_get_recent_changes(sheet_id?)` for cell-level detail.
---
```

**Reglas:**
- Solo se agrega si hay >0 eventos relevantes (después de filtrar `origin != "agent:llm"` para esta sesión).
- Cap 10 sheets en el listado; si hay más, agrega `...and N more sheets changed`.
- El cursor del agente NO se actualiza acá — se actualiza después de la LLM call (ver §4.5).
- Si el `agent_session_id` está ausente del execute(), el block se omite (no hay forma de identificar al agente).

### 4.4 Tools

#### `crdt_doc_get_recent_changes` (extendido)

```rust
struct GetRecentChangesArgs {
    /// Cursor — only events after this id. Default: agent's own cursor.
    since_event_id: Option<u64>,
    /// Filter to one sheet. Default: all sheets.
    sheet_id: Option<String>,
    /// Cap result count. Default: 50.
    limit: Option<u32>,
}
```

Response:
```json
{
  "current_event_id": 47,
  "events": [
    { "id": 45, "origin": "peer:browser", "sheet_id": "sh_inv", "summary": "set Inventory!D4 = 42", "created_at": "..." }
  ],
  "truncated": false
}
```

Implementación: REST `GET /documents/:id/changes?since=N&sheet_id=X&limit=K`. El `since` default vienedel cursor en SQL (vía `ChangeTrackerStore::cursor_for(session_id, artifact_id)`).

#### `crdt_doc_list_my_artifacts` (nuevo)

```rust
struct ListMyArtifactsArgs {} // sin params
```

Response:
```json
{
  "artifacts": [
    {
      "artifact_id": "art_...",
      "name": "Inventory Q3",
      "created_at": "...",
      "last_accessed_at": "..."
    }
  ]
}
```

Implementación: REST `GET /documents/by-session/:sid`. Filtra por `agent_session_id` del context, ordena por `last_accessed_at DESC`. Cap default 50.

#### `crdt_doc_create_artifact` (nuevo)

```rust
struct CreateArtifactArgs {
    name: String,
}
```

Response:
```json
{
  "artifact_id": "art_...",
  "name": "..."
}
```

Implementación: REST `POST /documents` con body `{name, agent_session_id}`. El server crea el Y.Doc en el registry + inserta row en `crdt_doc_session_artifacts`. Retorna el nuevo artifact_id.

**Importante**: el nuevo artifact_id NO reemplaza al `artifact_id` principal del context — el agente puede seguir mutando el original y opcionalmente operar sobre el nuevo (pero hoy no podemos: el context apunta a uno solo). En este spec dejamos el create como "ahora existe, lo podés mencionar al usuario, pero para mutarlo tenés que volver con otro graph turn que lo pinee". El **multi-artifact write access** lo resuelve subsistema **F**.

### 4.5 Lifecycle: cuándo se actualiza el cursor

El cursor (`last_event_id` para esta `agent_session_id` + `artifact_id`) se actualiza **al final del execute()** del llm_call:

```
execute() {
  1. Build CrdtDocsContext (existing).
  2. Query GET /changes?since=<cursor> + format summary block.
  3. Append summary to system_message.
  4. (existing) Run LLM loop with tools.
  5. NEW: After loop ends successfully, POST cursor update with
     the highest event_id observed during this turn.
}
```

Por qué al final y no al principio:
- Si el agente falla mid-turn, NO movemos el cursor → el próximo turn ve los mismos cambios sin perderse nada.
- El highest event_id observado incluye los eventos generados durante el turn (mutaciones propias del agente + mutaciones de peers que llegaron por WS mientras el turn corría). Es lo correcto porque "lo que vi" es todo eso.

### 4.6 Quién registra los eventos (dual-write con adapter por modo)

El registro de eventos es responsabilidad **de quien hace la mutación**, pero el camino al SQL depende del modo del `CrdtDocsContext`:

| Modo | Quién muta | Camino al SQL |
|---|---|---|
| **Local** (autónomo, `from_config`) | Tool dispatcher in-process | Directo: `ChangeTracker::record` → `ChangeTrackerStore::insert_event` (mismo proceso tiene DB pool) |
| **Local + singleton (shared)** | Tool dispatcher in-process | Directo, igual que local |
| **WsPeer** (worker remoto) | Tool dispatcher in-process del worker | REST POST `/documents/:id/events` al server (el worker NO toca el DB del server directamente) |
| **Browser (humano)** | Univer dispatcher → WS update | El **server** captura el update via `handle_socket::post_update` callback y graba con origin `peer:browser` |

Esto da **dual-write en WsPeer mode**: cada tool call hace una mutación al Y.Doc local (que se propaga vía WS) PLUS un REST POST al events endpoint. Tradeoff aceptado:

- **Costo**: +1 round-trip REST por tool call (~5-50ms intra-region). En un turno con 10 tool calls: +50-500ms total. Aceptable.
- **Beneficio**: eventos del agente quedan estructurados (sheet_id, summary detallado). Si grabaramos solo via WS, el server solo vería bytes binarios del update y la summary sería coarse ("agent wrote 30 bytes").

Implementación: trait `EventSink` con dos impls:
```rust
#[async_trait]
trait EventSink: Send + Sync {
    async fn record(&self, ev: NewEvent) -> Result<u64, SinkError>;
}

// Modo local/shared
struct DirectStoreSink { store: Arc<dyn ChangeTrackerStore> }

// Modo ws_peer  
struct RestSink { client: reqwest::Client, base_url: String }
```

`CrdtDocsContext` expone un método `sink() -> &dyn EventSink`. Tool dispatchers llaman `ctx.sink().record(NewEvent { artifact_id, sheet_id, origin, summary })`.

### 4.6.1 Limitación conocida: peer:browser sin sheet_id

El server, observando updates WS de browsers, no tiene forma trivial de saber qué sheet cambió — los updates Yjs son deltas CRDT opacos. Para v1 los eventos de origin `peer:browser` tienen:
- `sheet_id: NULL`
- `summary: "peer update (N bytes)"` (mismo que v1 hace hoy)

**Consecuencia en el auto-summary**: cambios de browser aparecen como un bucket "Workbook-level" sin atribución de sheet:

```
Workbook changes since your last turn (5 events, 2 peers):
- Inventory: 3 changes by agent:other_orchestrator
- Workbook (sheet unknown): 2 changes by peer:browser
Use `crdt_doc_get_recent_changes(sheet_id?)` for cell-level detail.
```

Mejora deferida a v1.1: hacer un projection diff antes/después del apply_update en el server para derivar sheet_id (y eventualmente addr). Documentado en BACKLOG como item "Per-cell attribution para peer:browser events".

### 4.7 Filtrado de mutaciones propias

El filtro "no me muestres lo que yo hice" se aplica a NIVEL QUERY, no a nivel storage:

```sql
SELECT * FROM crdt_doc_events
WHERE artifact_id = $1 AND id > $2
  AND NOT (origin = 'agent:llm' AND ...)  -- ???
ORDER BY id ASC
LIMIT $3
```

**Problema**: el origin `agent:llm` es genérico. No distingue "este agente" de "otro agente que también es LLM". Necesitamos un origin más específico.

**Solución**: cuando un tool dispatcher registra un evento (via `ctx.sink().record(...)` — ver §4.6), el origin incluye el `agent_session_id`:

```rust
let origin = format!("agent:{}", ctx.session_id());
// luego pasado como campo de NewEvent al sink
```

Y el query del summary filtra ese origin específico:
```sql
AND origin != $4  -- $4 = format!("agent:{}", current_session_id)
```

Eventos de otros agentes LLM, de browsers, o de Python helpers siguen siendo visibles. Solo se filtran las mutaciones de ESTE agente en ESTA sesión.

## 5. Cambios al código

### 5.1 Nuevo módulo `change_tracker_store`

`src/libs/colmena/src/crdt_documents/change_tracker_store.rs` — adapter SQL para los `ChangeTracker` ops. Trait + dos impls (sqlite, postgres):

```rust
#[async_trait]
pub trait ChangeTrackerStore: Send + Sync {
    async fn insert_event(&self, ev: NewEvent) -> Result<u64, StoreError>;
    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, StoreError>;
    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, StoreError>;
    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), StoreError>;
    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), StoreError>;
    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, StoreError>;
}
```

### 5.2 `ChangeTracker` se transforma en wrapper

El `ChangeTracker` actual (in-memory `VecDeque`) se reemplaza por un wrapper sobre `ChangeTrackerStore`. El módulo `change_tracker.rs` mantiene la API pública (`record`, `since`) pero internamente llama a SQL.

Para tests sin DB: implementación in-memory de `ChangeTrackerStore` (`InMemoryChangeTrackerStore`).

### 5.3 Modificaciones al `CrdtDocumentsRuntime`

```rust
pub struct CrdtDocumentsRuntime {
    pub registry: Arc<DocRegistry>,
    pub storage: Arc<dyn ArtifactStorage>,
    pub tracker: Arc<ChangeTracker>,        // ← ahora wrapper sobre store
    pub store: Arc<dyn ChangeTrackerStore>, // ← NEW
}
```

`from_config` lee `DATABASE_URL` (o `database_url` del config) y construye el store apropiado. Si no hay DB → `InMemoryChangeTrackerStore` (modo dev sin DB).

### 5.4 Nuevos endpoints REST

`src/libs/colmena/src/crdt_documents/server.rs` — agregar:

- `GET /documents/:id/changes` con query params `since`, `sheet_id`, `limit`. Header `X-Agent-Session-Id` requerido para resolver cursor + filtro de origin propio.
- `GET /documents/by-session/:sid`. Lista artifacts.
- `POST /documents` extendido — body opcional `{name, agent_session_id}`. Si hay `agent_session_id`, llama `store.touch_artifact()`.
- `POST /documents/:id/cursor` con body `{agent_session_id, last_event_id}`. Endpoint específico para que el worker actualice cursor post-turn.

### 5.5 Modificaciones a `llm.rs`

Dos lugares:

**(1)** Al construir el `system_message` (después del temporal_geographic_context block, antes de devolverlo al LlmRequest):

```rust
if let Some(ctx) = crdt_docs_context.as_ref() {
    if let Some(session_id) = agent_session_id_str.as_ref() {
        let block = build_recent_changes_block(ctx, session_id).await?;
        if !block.is_empty() {
            system_message.push_str(&block);
        }
    }
}
```

`build_recent_changes_block` hace el query y formatea el bloque. Es función nueva en un módulo `crdt_summary` adentro de llm_synthetic_tools (vive cerca de los tools que la complementan).

**(2)** Después del LLM loop, antes del cleanup actual:

```rust
if let Some(ctx) = crdt_docs_context.as_ref() {
    if let Some(session_id) = agent_session_id_str.as_ref() {
        update_cursor_for_session(ctx, session_id).await?;
    }
}
```

`update_cursor_for_session` consulta cuál fue el `current_event_id` máximo durante este turn (lo trackea el `CrdtDocsContext` via un `AtomicU64` actualizado en cada tool call) y postea el cursor.

### 5.6 Modificaciones a los tool dispatchers

Cada tool dispatcher que registra un evento (`set_cell`, `set_range`, `add_sheet`, etc.) usa la nueva interfaz `EventSink` que abstrae el camino al SQL (directo en local mode, REST POST en ws_peer):

```rust
// ANTES:
ctx.tracker().record(ctx.artifact_id(), "agent:llm", ...);

// DESPUÉS:
let origin = format!("agent:{}", ctx.session_id());
ctx.sink().record(NewEvent {
    artifact_id: ctx.artifact_id().clone(),
    sheet_id: Some(sheet_id.to_string()),
    origin,
    summary: format!("set {}!{} = {}", sheet_id, addr, value),
}).await.ok();
```

`CrdtDocsContext` se extiende para conocer `agent_session_id` (se lo pasamos en `new_local(runtime, artifact_id, session_id)` / `new_ws_peer(peer, session_id, base_url)`) y para construir el `EventSink` apropiado al modo.

Tradeoff aceptado: el tool dispatcher pasa a ser `async` (era sync). `dispatch_*` ya es async así que el cambio es contenido.

**Manejo de errores**: `.ok()` swallow del error. Si el REST POST falla, la mutación al Y.Doc YA aconteció — perder el event log es un degraded path documentado (§6). En logs queda registro para debugging; no impacta funcionalidad para el usuario.

### 5.7 Lifecycle del cursor en `llm.rs`

El context trackea el `max_event_id` visto durante el turn via `AtomicU64`. Cada `sink().record()` actualiza si el returned event_id es mayor. Al final del execute():

```rust
if let (Some(ctx), Some(sid)) = (&crdt_docs_context, &agent_session_id_str) {
    let max_id = ctx.max_event_id_observed();
    if max_id > 0 {
        ctx.update_cursor(sid, max_id).await.ok();
    }
}
```

`update_cursor` también usa el mismo split: local → directo a `ChangeTrackerStore::upsert_cursor`; ws_peer → REST POST `/documents/:id/cursor`.

## 6. Edge cases y semánticas degradadas

| Situación | Comportamiento |
|---|---|
| Agent_session_id ausente (CLI sin flag) | Auto-summary se omite. Tools de discovery devuelven `{error: "session_required"}`. Tools de mutación siguen funcionando. |
| DB unreachable al startup | Runtime falla a construirse. llm.rs propaga el error. |
| DB unreachable mid-call | Tool dispatcher loggea + sigue (mutación al Y.Doc OK, event no registrado). Auto-summary del próximo turn no verá esa mutación. Mejora-futuro: retry queue. |
| Server WS unreachable | Mismo fail-fast de V2 (no auto-reconnect en v1). |
| Cursor > max_event_id (post-restart del server con events nuevos) | Devolvemos 0 eventos (no error). Es el estado correcto: "todo lo que pasó ya lo viste". |
| Cursor en SQL pero events table vacía (degradación rara) | Devuelve 0 eventos. Documentamos en logs. |
| Agente crea 100 artifacts en una sesión | `list_my_artifacts` cap 50, retorna los más recientes. Si necesita más, parámetro `limit` o paginación (v1.1). |

## 7. Plan de testing

- **Unit**: cada method de `ChangeTrackerStore` con la impl in-memory. ~10 tests.
- **Unit**: formatter del summary (varios shapes de events, cap, overflow). ~5 tests.
- **Integration**: ciclo end-to-end con server real + agent + DB sqlite local. ~3 tests:
  - Mutación por agent → event en SQL → otro agent ve summary en next turn.
  - Cursor avanza correctamente entre turns.
  - List/create tools end-to-end.
- **Manual browser smoke**: humano edita en browser, agente en otro proceso ve el bloque auto-inyectado en el siguiente turn.

## 8. Estimación

| Pieza | LoC aprox | Días |
|---|---|---|
| Migrations (sqlite + postgres) | ~60 | 0.25 |
| `ChangeTrackerStore` trait + impls + tests | ~400 | 0.75 |
| `ChangeTracker` refactor a wrapper | ~80 | 0.25 |
| REST endpoints nuevos/extendidos | ~150 | 0.5 |
| 2 tools nuevos + 1 extendido | ~150 | 0.5 |
| `llm.rs` integration (system_message + cursor update) | ~120 | 0.5 |
| Origin filter por session_id en context propagation | ~60 | 0.25 |
| Integration tests | ~250 | 0.75 |
| Doc updates (38_crdt_documents.md, node_configurations.json) | ~60 | 0.25 |
| **Total** | **~1300 LoC** | **~4 días dev** |

## 9. Fuera de scope (deferred)

- **Auto-reconnect del WS peer si la conexión muere** → ya en BACKLOG como v1.1.
- **Multi-artifact write access en un mismo turn** → subsistema F (compare two excels).
- **Auditoría de cambios desde la UI** → la tabla existe, surfacing es trabajo de ADP frontend.
- **TTL/retención de events table** → eventualmente `DELETE WHERE created_at < now() - INTERVAL '90 days'`. Trivial agregar después.
- **Paginación de `list_my_artifacts` para sesiones con 100+ artifacts** → v1.1 si la UX lo amerita.
- **Tracking de "qué peer cerró"** (presence) → tabla aparte si se necesita, no afecta este spec.
- **ADP Prisma schema mirror** → ADP tendrá que agregar las 3 tablas al schema Prisma para CI/migrations en su CD pipeline. Cero coordinación necesaria de nuestra parte mientras tanto: las migrations de colmena corren al startup del worker y crean las tablas en `adp_db_develop` directamente (mismo flujo que `conversation_attachments` siguió en su día). Documentar en CHANGELOG.

## 10. Cómo retomar

Spec listo + aprobado. Próximo paso: invocar el skill `writing-plans` para generar el plan de implementación con tasks numeradas, dependencies, y acceptance criteria por task.
