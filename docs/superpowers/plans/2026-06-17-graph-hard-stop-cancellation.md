# Implementation Plan: Hard Stop / Cancellation de un grafo en ejecución

## Estado de implementación
- **Fase 1 (Colmena: estado + evento): ✅ HECHA**
- **Fase 2 (Colmena: cancelación cooperativa en el loop): ✅ HECHA Y VERIFICADA E2E**
- **Fase 3 (ADP: endpoint /cancel + Redis + wiring del token): ⏳ PENDIENTE**

### Desviaciones respecto al plan (mejoras durante impl)
1. **API aditiva en vez de breaking change.** `engine.execute_stream` se mantiene en 6 args
   (pasa `None` internamente). Se agregó `engine.execute_stream_cancellable(...)` (7 args,
   recibe el `CancellationToken`). Resultado: **cero call sites rotos** (ni los ~19 tests
   in-repo ni el worker ADP). ADP solo cambia a la variante nueva cuando implemente Fase 3.
   El `cancel_token: Option<...>` SÍ se agregó al `execute_stream` interno del use case.
2. **`cancel_running_descendants` con default no-op en el trait** `DagStateRepository`, así
   repos in-memory/test no requieren impl. Impl real (CTE recursivo) solo en Postgres.
3. **Sin migración** — confirmado `status VARCHAR(50)` sin CHECK.
4. **Snapshot de `global_shared_state`** antes del borrow mutable del nodo (solo cuando hay
   token) para persistir estado coherente en cancelación mid-node.
5. **`?` no propaga dentro de brazos de `tokio::select!`** → el save en cancelación mid-node
   es best-effort con log (no debe impedir el corte).

### Archivos tocados (Fase 1+2)
- `domain/state.rs` — `DagRunStatus::Cancelled` + `cancel_running_descendants` (trait, default).
- `domain/events.rs` — `DagExecutionEvent::Cancelled { reason, partial_output }`.
- `application/run_use_case.rs` — param `cancel_token`; check entre nodos (Punto A) + brazo
  `select!` mid-node (Punto B); recursión subgrafo pasa `None`.
- `infrastructure/persistence/postgres_dag_state_repository.rs` — CTE recursivo.
- `dag_engine/engine.rs` — `execute_stream` (6-arg, aditivo) + `execute_stream_cancellable`.
- `dag_engine/sse_mapper.rs` — mapeo `Cancelled` → frames `cancelled` + `finish`.
- `dag_engine/api.rs`, `dag_engine/main.rs` — sin cambios de firma (siguen 6-arg).
- `Cargo.toml` — `tokio-util = "0.7"`.

### Tests (Fase 1+2)
- Unit (CI-safe, 6 nuevos, verdes): status Display/FromStr; evento serde+roundtrip; mapper.
- Integración `tests/cancellation_integration.rs` (4, `#[ignore]`, **pasaron vs DB real**):
  1. pre-cancel → 0 nodos, CANCELLED.
  2. cancel entre nodos (Punto A) → exactamente 1 nodo, CANCELLED.
  3. sin token → COMPLETED (path inerte).
  4. **mid-node real (Punto B)**: `http_request` real contra wiremock con delay de 10s,
     cancel a ~800ms → **tiempo total 1.47s** (no 10s) → reqwest abortado a media petición;
     SSE mapeado real `node-start → cancelled → finish(finishReason=cancelled)` guardado en
     `/tmp/colmena_e2e/hard_stop_midnode.sse`; DB CANCELLED.
- Suite completa: 1777 unit tests verdes, clippy (`--lib` y `--tests`) limpio, fmt aplicado,
  todos los tests compilan.

## Summary
Permitir cancelar un grafo en ejecución de forma **cooperativa y limpia**: la cancelación
se observa **entre nodos** (con drop del nodo en vuelo), persiste un estado terminal
`CANCELLED` con outputs parciales, y emite un evento explícito. La señal fluye de punta a
punta: **frontend → API ADP → Redis → worker → Colmena**, usando un único
`tokio_util::sync::CancellationToken`.

## Motivation
- Hoy no existe forma de parar un grafo en curso. El SSE solo muere por timeout/error y el
  run queda como `RUNNING` para siempre en Postgres (runs zombie).
- Un usuario que arrancó un workflow largo (o un agente en loop) no puede abortarlo; gasta
  tokens y bloquea el worker (que procesa secuencial, 1 job a la vez por réplica).
- Necesitamos distinguir "cancelado por el usuario" de "se cayó", con un estado terminal
  coherente que no rompa el `resume`.

## Decisiones de diseño (acordadas en brainstorm)
1. **Semántica:** cancelación cooperativa limpia (no abort-por-drop pelado).
2. **Granularidad:** **entre nodos**. El nodo en vuelo se dropea vía `select!` (ops async
   como reqwest/sqlx abortan en el próximo await; `python_script`/pyo3 corre hasta terminar
   por límite de pyo3 — fuera de alcance matarlo a media ejecución).
3. **Routing distribuido:** vía **Redis** (no mapa en memoria con service discovery). Key
   durable `cancel:{job_id}` como fuente de verdad + canal pub/sub para latencia baja.
4. **Sin tocar el trait `ExecutableNode`.** El token es un parámetro local de
   `execute_stream` (no necesita vivir como campo). Los subgrafos NO reciben el token (su
   executor es un singleton de boot, ver paso 5): se interrumpen por **drop-propagation** y
   su estado se limpia con un UPDATE recursivo.

## Architectural Impact

### Colmena (motor Rust)
- **Layers affected**: domain (nuevo status + evento), application (loop + propagación),
  infrastructure (engine signature; opcional: endpoint `/cancel` en serve).
- **New traits/ports**: ninguno. Se reusa `SubGraphExecutorPort`.
- **Trait `ExecutableNode`**: **sin cambios** (clave de la decisión "entre nodos").
- **Modified files**:
  - `src/libs/colmena/src/dag_engine/domain/state.rs` — `DagRunStatus::Cancelled`
  - `src/libs/colmena/src/dag_engine/domain/events.rs` — `DagExecutionEvent::Cancelled`
  - `src/libs/colmena/src/dag_engine/application/run_use_case.rs` — token + loop + forward a subgrafo
  - `src/libs/colmena/src/dag_engine/engine.rs` — nuevo param en `execute_stream`
  - `src/libs/colmena/src/dag_engine/api.rs` — pasar `None` al `execute_stream` (sweep). El
    endpoint `POST /cancel` de serve-mode (paso 8) queda **diferido** post-v1.
  - `src/libs/colmena/src/dag_engine/main.rs` — pasar `None` al `execute_stream` (CLI sweep).
  - `Cargo.toml` — dependencia `tokio-util` (feature `rt`) si falta.
- **Binding impact**: Python **no**, TypeScript **no** — las bindings no invocan
  `execute_stream` (verificado por grep). Solo cambia 1 call site externo (worker ADP).
- **Breaking change controlado**: `engine.execute_stream` gana un parámetro → sweep del
  worker ADP ANTES de pushear a `develop` (regla de breaking-changes del CLAUDE.md).

### ADP (solo `apps/service/ia/platform/`)
- **API**: nueva ruta `POST /api/v1/executions/:job_id/cancel`.
- **Worker**: token por job + suscriptor Redis + `select!` en el loop de consumo del stream.
- **Shared**: `SseMapper` mapea el evento `Cancelled`.
- **NO tocar** frontend (`apps/chat`), backend (`apps/api`), ni `packages/database`.

---

## Detailed Steps

### Fase 1 — Colmena: estado y evento terminal

1. **`DagRunStatus::Cancelled`**
   - File: `src/libs/colmena/src/dag_engine/domain/state.rs`
   - What: agregar variante `Cancelled`; en `Display` → `"CANCELLED"`; actualizar cualquier
     `FromStr`/match de parseo de status (buscar `"COMPLETED"` / `"SUSPENDED"` para encontrar
     los call sites simétricos).
   - Why: estado terminal honesto, distinguible de `Failed` y `Running`.

2. **`DagExecutionEvent::Cancelled`**
   - File: `src/libs/colmena/src/dag_engine/domain/events.rs`
   - What:
     ```rust
     #[serde(rename = "cancelled")]
     Cancelled { reason: Option<String>, partial_output: Value },
     ```
   - Why: el frontend necesita un evento explícito que termine el stream limpio (no un error).

### Fase 2 — Colmena: cancelación cooperativa en el loop

3. **Param `cancel_token` en el use case**
   - File: `src/libs/colmena/src/dag_engine/application/run_use_case.rs`
   - What: `execute_stream(self, ..., cancel_token: Option<tokio_util::sync::CancellationToken>)`.
     Es un **parámetro local** del stream (no campo de `self`). La recursión interna a
     subgrafos (`run_subgraph` → `self.clone().execute_stream(...)`, líneas ~966/1023) pasa
     `None` — los hijos se manejan por drop-propagation + CTE (paso 5).
   - Dependencia: agregar `tokio-util` (feature `rt`) a `Cargo.toml` si no está.
   - Why: un único token raíz que el loop observa.

4. **Check entre nodos + `select!` del nodo en vuelo**
   - File: `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (loop ~291–548)
   - What:
     - Al inicio de cada iteración (antes de ejecutar el siguiente nodo):
       `if token.as_ref().is_some_and(|t| t.is_cancelled()) { <persist+yield+return> }`
     - Envolver el future de ejecución del nodo (el `tokio::select!` existente en ~484) con
       una rama adicional `_ = token_cancelled_fut => { <persist+yield+return> }`. Al
       dispararse, el future del nodo se dropea → aborta el await async en vuelo.
     - `<persist+yield+return>`: construir `DagRunState` con `status = Cancelled`,
       `all_outputs` parciales y `active_queue` restante; `repo.save(&state).await`;
       `yield DagExecutionEvent::Cancelled { reason, partial_output }`; `return`.
   - Why: granularidad "entre nodos" sin tocar el trait; estado parcial preservado.

5. **Propagación a subgrafos** (mecanismo corregido tras inspección)
   - Contexto: el subgrafo NO recibe el token por parámetro — llama a un executor
     **singleton fijado en boot** (`Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>>`,
     `subgraph.rs:12`; `registry.rs::set_subgraph_executor`), no a un clon per-run. Por eso
     guardar el token como campo del use case y confiar en `self.clone()` **no funciona**.
   - What (v1):
     - **Detener el trabajo**: vía **drop-propagation**. Cuando el loop raíz dispara la rama
       de cancelación, retorna y se dropea todo el árbol de futures → el `execute()` del nodo
       subgrafo y su `execute_stream` interno se dropean → el `await` en vuelo del hijo aborta.
     - **Dejar estado limpio**: al persistir `Cancelled` en la raíz, ejecutar UN update
       recursivo que marca la descendencia `RUNNING` como `CANCELLED`:
       ```sql
       WITH RECURSIVE descendants AS (
         SELECT session_id FROM dag_runs WHERE parent_session_id = $root
         UNION ALL
         SELECT d.session_id FROM dag_runs d
           JOIN descendants x ON d.parent_session_id = x.session_id
       )
       UPDATE dag_runs SET status = 'CANCELLED'
        WHERE session_id IN (SELECT session_id FROM descendants) AND status = 'RUNNING';
       ```
       (`parent_session_id` ya está indexado — migración `20260428000001`.)
     - Exponer como método nuevo en `DagStateRepository`, p. ej.
       `cancel_running_descendants(root_session_id) -> Result<u64, DagError>`.
   - Why: detiene el trabajo de los hijos sin tocar el trait `ExecutableNode`, y evita runs
     hijos zombie sin necesidad de threadear el token por toda la recursión.

6. **Engine forwarding + actualizar call sites internos**
   - File: `src/libs/colmena/src/dag_engine/engine.rs` (~343)
   - What: `execute_stream(..., cancel_token: Option<CancellationToken>)` que reenvía al use
     case. La firma cambia (breaking externo — solo worker ADP).
   - **Call sites internos de Colmena que deben pasar `None`** (sweep obligatorio para que
     compile): `engine.rs` wrapper, `api.rs` handlers de serve, `main.rs` (CLI bin), y la
     recursión en `run_use_case.rs` (`run_subgraph`). Buscar con
     `grep -rn "execute_stream" src/libs/colmena/src/`.
   - Why: exponer el token al consumidor in-process (ADP) sin romper el build interno.

7. **Persistencia — RESUELTO (sin migración)**
   - Verificado: `dag_runs.status` es `VARCHAR(50) NOT NULL` **sin CHECK constraint**
     (`migrations/postgres/20240101000000_initial_schema.sql:20`). `'CANCELLED'` persiste
     directo → **no se necesita migración** para el status.
   - File: `PostgresDagStateRepository` — agregar el método `cancel_running_descendants`
     (CTE recursivo del paso 5) al impl + a la firma del trait `DagStateRepository`.

8. **(Opcional, serve-mode) Endpoint `POST /cancel`**
   - File: `src/libs/colmena/src/dag_engine/api.rs`
   - What: registro en memoria `Arc<DashMap<session_id, CancellationToken>>` poblado al
     arrancar cada run webhook; handler `POST /cancel { session_id }` que dispara el token.
   - Why: paridad para quien use Colmena en modo `serve` directo. **ADP NO lo usa**
     (consume in-process) — marcar como nice-to-have, no bloqueante para el objetivo ADP.

### Fase 3 — ADP: señal de cancel vía Redis (refinada contra el código real)

**Dependencia de secuencia (BLOQUEANTE):** el worker importa colmena vía
`git branch = "develop"`. `execute_stream_cancellable` **aún no está en develop**. Para
construir/probar Fase 3: (a) merge colmena additivo a develop primero (el worker actual sigue
compilando porque `execute_stream` 6-arg no cambió), o (b) usar el `[patch]` local en
`platform/Cargo.toml` apuntando al worktree para E2E local. Orden de merge real: **colmena→develop, luego ADP**.

9. **Endpoint de cancel en la API** — `api/src/handlers.rs` + `api/src/main.rs`
   - Nueva ruta en `main.rs`: `.route("/api/v1/executions/:job_id/cancel", post(cancel_execution))`.
   - `cancel_execution(State<Arc<AppState>>, Path<String>)` → usando `state.redis_client`:
     ```rust
     let mut conn = state.redis_client.get_async_connection().await?;
     let _: () = conn.set_ex(format!("cancel:{job_id}"), 1, 3600).await?; // fuente de verdad
     let _: () = conn.publish("cancel", &job_id).await?;                  // wakeup baja latencia
     (StatusCode::ACCEPTED, Json(json!({ "job_id": job_id, "status": "cancelling" })))
     ```
   - Idempotente; si el job ya terminó es no-op seguro.

10. **Worker: dep + pre-check + token + suscriptor** — `worker/Cargo.toml` + `worker/src/main.rs`
    - `worker/Cargo.toml`: añadir `tokio-util = "0.7"`.
    - `process_job` gana un parámetro `redis_client: &Arc<redis::Client>` (para que el
      suscriptor abra su propia conexión pub/sub). `process_jobs_inline` ya tiene `state.redis`.
    - **Pre-check (job cancelado en cola)**: al inicio de `process_job`, antes de ejecutar:
      ```rust
      let cancelled_pre: bool = redis_con.exists(format!("cancel:{}", job.job_id)).await.unwrap_or(false);
      if cancelled_pre {
          publish!(json!({"type":"cancelled","reason":null}));
          publish!(json!({"type":"finish","finishReason":"cancelled"}));
          return Ok(());
      }
      ```
    - **Token + suscriptor pub/sub** (routing correcto con N réplicas):
      ```rust
      let token = tokio_util::sync::CancellationToken::new();
      let sub = { let c = redis_client.clone(); let t = token.clone(); let jid = job.job_id.clone();
        tokio::spawn(async move {
          if let Ok(conn) = c.get_async_connection().await {
            let mut ps = conn.into_pubsub();
            if ps.subscribe("cancel").await.is_ok() {
              use futures::StreamExt;
              let mut on = ps.on_message();
              while let Some(m) = on.next().await {
                if m.get_payload::<String>().ok().as_deref() == Some(jid.as_str()) { t.cancel(); break; }
              }
            }
          }
        }) };
      ```

11. **Worker: usar `execute_stream_cancellable`** — `worker/src/main.rs`
    - Cambiar la llamada `engine.execute_stream(...)` por
      `engine.execute_stream_cancellable(graph, job.session_id.clone(), job.resume_answer.clone(),
      true, None, job.agent_session_id.clone(), token.clone())`.
    - El loop de consumo **no cambia**: colmena emite `Cancelled`, el `SseMapper` (de colmena, ya
      actualizado) lo mapea a `cancelled`+`finish`, el worker los publica a `events:{job_id}`.
    - Al terminar (normal o cancelado): `sub.abort();` y opcional `DEL cancel:{job_id}` (TTL igual lo limpia).

12. **Frontend / SSE** — sin cambios de código en ADP
    - El `SseMapper` de colmena ya produce `cancelled`+`finish`. El frontend (`apps/chat`) adopta
      el frame `cancelled` siguiendo `docs/streaming/FRONTEND_CANCEL_CONTRACT.md` (ya entregado).
    - Opcional (platform, modificable): en `api/src/stream.rs` añadir
      `payload.contains("\"type\":\"cancelled\"")` como terminador adicional. No requerido porque
      `finish` ya termina el stream.

### Verificación E2E de Fase 3 (obligatoria antes de "done")
- Local con `[patch]` → colmena worktree. Levantar redis + api + worker. `POST /executions`
  con un grafo lento (http a endpoint con delay), abrir el `/stream`, `POST /executions/:id/cancel`,
  verificar frames `cancelled`+`finish` en el SSE y status `CANCELLED` en `dag_runs`.
  Guardar SSE en `/tmp/colmena_e2e/adp_hard_stop.sse` + reporte.
- Caso job-en-cola: cancelar antes de disparar `/process` → worker lo salta con cancelled+finish.
      pero habilita UX correcta cuando lo adopte.

---

## Testing Strategy

- **Unit (Colmena)**:
  - `run_use_case`: con `MockAdapter` y un grafo de 3 nodos, disparar el token tras el primer
    `NodeFinish` → assert que emite `Cancelled`, NO ejecuta el 3.º nodo, y persiste
    `status = Cancelled` con `all_outputs` parciales.
  - Cancel ANTES de iniciar → emite `Cancelled` inmediato, 0 nodos ejecutados.
  - Subgrafo: token raíz dispara → hijo también guarda `CANCELLED`.
  - Serde roundtrip del evento `Cancelled` y del status `"CANCELLED"`.
- **Integration (Colmena, `tests/graphs/`)**: grafo con un nodo lento real (`http_request` a
  endpoint con delay) + cancel a media ejecución → verificar corte y estado.
- **E2E real (regla del proyecto: siempre E2E real antes de "done")**:
  - ADP local: arrancar un job largo, `POST .../cancel`, verificar evento `cancelled` en el
    SSE y `cancel:{job_id}` consumido. Guardar SSE en `/tmp/colmena_e2e/hard_stop.sse` y
    presentar reporte amistoso.
  - Caso job-en-cola: cancelar antes del pickup → worker lo salta.
- **Manual**: doble cancel (idempotente); cancel de job ya terminado (no-op).

## Documentation Updates
- `docs/developer_guide/12_dag_engine_guide.md` — sección "Cancelación / hard stop"
  (incluir las limitaciones: pyo3/spawn_blocking corre hasta su timeout; writers LLM
  desprendidos terminan solos).
- `docs/dds/DAG_ENGINE_DISEÑO.md` — decisión de diseño (entre nodos, token, drop-propagation
  + CTE de descendientes, Redis routing key+pubsub).
- `docs/node_ports_reference` / eventos — documentar el evento `cancelled`.
- **Frontend (ENTREGADO):** `apps/service/ia/platform/docs/streaming/FRONTEND_CANCEL_CONTRACT.md`
  — endpoint `/cancel`, evento SSE `cancelled`+`finish`, comportamiento entre-nodos. Para
  el equipo de `apps/chat`.
- ADP: nota en el doc de la platform API sobre el endpoint `/cancel` y el contrato Redis
  (key durable `cancel:{job_id}` + canal pub/sub `cancel`).
- `docs/BACKLOG.md` — entrada "Colmena serve-mode `POST /cancel`" (diferido post-v1).
- `CLAUDE.md` "Current Status" — entrada al shipear.

## Risks & Mitigations (con resolución)
| Risk | Impact | Estado / Acción concreta |
|------|--------|--------------------------|
| `python_script`/pyo3 (`spawn_blocking`) no se interrumpe a media ejecución | Medio | **Aceptado y acotado.** Verificado: `python_node`, `gsheets_run_python`, `crdt_doc_run_python`, `attachment_run_python` corren en `spawn_blocking` **con timeout wall-clock propio**. Al cancelar, dropeamos el `await`; el thread corre hasta su propio timeout y su resultado se descarta. No bloquea el corte del grafo (el siguiente nodo no arranca). Documentar en dev guide. |
| Writers desprendidos del nodo LLM siguen tras el cancel | Bajo | **Aceptado.** `llm.rs:3284` usa `tokio::spawn` fire-and-forget para persistir historial. Son idempotentes/benignos: terminan solos y dejan historial consistente. No requieren acción. Documentar. |
| Breaking change en `execute_stream` rompe build del worker ADP | Alto | **Acción:** actualizar el único call site (`worker/src/main.rs:259`) en el MISMO PR/coordinación; correr build del worker antes de pushear Colmena a `develop`. Bindings NO afectadas (verificado: no llaman `execute_stream`). |
| Pub/sub se pierde si el worker se suscribe tarde / job aún en cola | Medio | **Resuelto en diseño.** Key durable `cancel:{job_id}` (EX 3600) es la fuente de verdad: chequeada al pickup y re-leída por el suscriptor; pub/sub es solo para latencia baja. |
| Drop del future deja recursos a medias (conexión, fichero) | Medio | **Mitigado.** `reqwest`/`sqlx` liberan por RAII al dropear. Los nodos `spawn_blocking` completan su propia transacción en su thread (no se dropean a media tx). Auditar solo esos 4 nodos en impl. |
| CHECK constraint en `dag_runs.status` rechaza `CANCELLED` | — | **Resuelto.** `status VARCHAR(50)` sin constraint → no aplica, sin migración. |
| Cancel con grafos anidados deja hijos `RUNNING` (zombie) | Medio | **Resuelto.** Drop-propagation detiene el trabajo + `cancel_running_descendants` (CTE recursivo sobre `parent_session_id` indexado) marca la descendencia como `CANCELLED` (paso 5). |
| Frontend no reconoce el evento y muestra "completado" en vez de "detenido" | Medio | **Resuelto vía contrato.** El stream emite un frame `cancelled` (UX) seguido de `finish` (terminador que el frontend ya respeta). Documento de contrato para `apps/chat` (ver Documentation Updates). |

## Open Questions — RESUELTAS
- **Frontend / UI del botón stop** → **Resuelto:** se entrega documento de contrato
  `apps/service/ia/platform/docs/FRONTEND_CANCEL_CONTRACT.md` (endpoint + evento SSE). La UI
  en `apps/chat` la implementa su equipo siguiendo ese contrato (fuera de alcance de este plan).
- **`/cancel` de serve-mode en Colmena (paso 8)** → **Resuelto: diferir.** ADP consume
  in-process; no usa el HTTP de Colmena. Se deja como nice-to-have post-v1 en BACKLOG.
- **Propagación de token a subgrafos** → **Resuelto:** NO vía campo del use case (el executor
  es singleton de boot por OnceLock, no un clon per-run). Se usa drop-propagation +
  `cancel_running_descendants` (paso 5 corregido). Sin tocar el trait `ExecutableNode`.

## Execution
- Colmena (Rust): usar `/rust_dev`. Correr `cargo test --verbose` (no solo `--lib`) antes de push.
- ADP worker/api (Rust): confinar a `apps/service/ia/platform/`. E2E real contra servicios vivos.
- Orden sugerido: Fase 1 → 2 (Colmena, mergeable y testeable solo) → sweep ADP → Fase 3.
