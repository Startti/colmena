# Backlog — Future work / parked items

> **Propósito:** Listar features identificados, especificados o solicitados que **no están en el roadmap activo**. Cada entrada tiene un trigger explícito ("¿cuándo retomamos esto?") para evitar que algo se quede olvidado o se construya prematuramente.

Si vas a empezar a trabajar en algo de acá, sacalo de esta lista y agregalo al changelog del mes correspondiente. Si descartás definitivamente un item, marcalo `~~tachado~~` y dejá una nota explicando por qué.

---

## ⭐ Cola priorizada — daniel@startti.co (2026-06-09)

> Items elevados por el owner el 2026-06-09 tras shippear Subsystem G v1.1.
> Mezclados de varios subsistemas; agrupados acá para visibilidad.
> Cada item tiene su entrada de detalle más abajo (cross-referenced).

| # | Item | Sección detallada | Esfuerzo |
|---|---|---|---|
| **11** | ✅ Cache nativo de provider habilitado por default en Anthropic + Gemini (shipped 2026-06-09) | [Provider-level prompt caching](#crdt-documents-v11--provider-level-prompt-caching-anthropic--gemini) | done |
| **12** | ✅ SQL node — `INSERT` multi-statement (shipped 2026-06-09) | [SQL node — INSERT multi-line bug](#sql-node--insert-multi-line-bug-2026-06-09--shipped-2026-06-09) | done |
| **13** | ✅ SQL node — bulk insert desde CSV adjunto (shipped 2026-06-09, Postgres + CSV only) | [SQL node — bulk insert desde attachment](#sql-node--bulk-insert-desde-attachment-2026-06-09) | done |
| **14** | 🧠 Filtrar fields que el LLM ve de outputs de nodos upstream | [Output filtering para LLM](#output-filtering-para-llm--qué-campos-ve-el-modelo-2026-06-09) | requiere brainstorming dedicado |

**Estado:** items 11-14 fueron agregados al backlog el 2026-06-09. Item 11
ya existía bajo la sección "CRDT Documents v1.1 — Provider-level prompt
caching" (estaba mal categorizado — es plataforma-wide, no CRDT-specific);
queda referenciado en su sección original con bump de prioridad. Items
12-14 son nuevos y tienen sus propias secciones abajo.

**🧊 CRDT freeze (2026-06-09):** todo el bloque "CRDT Documents v1.1" más
abajo queda **despriorizado** por decisión del owner (el subsistema no se
va a llevar a develop/prod en ADP por ahora). **Excepción explícita:**
item 11 (prompt caching) sigue activo porque es plataforma-wide, no
CRDT-specific. Ver banner detallado al inicio del bloque CRDT.

---

## ~~SQL node — INSERT multi-line bug (2026-06-09)~~ — SHIPPED 2026-06-09

- **Origen:** Reporte del owner (2026-06-09).
- **Root cause confirmado:** el bug NO era multi-líneas — era
  **multi-statement** (varios `;` en la misma query). `sqlx::query().execute()`
  usa el extended protocol de Postgres (PREPARE+BIND+EXECUTE) que solo acepta
  UN comando SQL por mensaje. Cuando el LLM escribía 2+ INSERTs separados
  por `;\n`, Postgres rechazaba con `cannot insert multiple commands into a
  prepared statement`.
- **Fix shipped:** Política C — refactor de `execute_query` en
  `sql_pool_adapter.rs` para iterar `Vec<Statement>` ejecutando uno por
  vez dentro de una transacción atómica. Output del último statement;
  rollback total en cualquier fallo. + UTF-8 panic fix en log preview.
- **Docs LLM-facing:** description_supplement con anti-patterns visuales
  + nueva skill built-in `sql-query-best-practices` con 6 references
  opt-in.
- **Verificación:** 7 integration tests `#[ignore]`-gated en
  `sql_pool_adapter::tests::pc_*`. Live verification via
  `tests/graphs/agents/sql_multistatement_repro.json`.
- **CHANGELOG:** §18.

---

## SQL node — bulk insert desde attachment (2026-06-09)

- **Origen:** Reporte del owner (2026-06-09). Use case: usuario sube
  CSV/Excel de 1000+ filas. Hoy si el agente quiere meterlo en DB
  tiene que leerlo entero (token waste + alucinaciones).
- **Goal:** que el LLM vea solo un sample (primeras N filas + header
  + count + tipos inferidos) y decida si la column mapping es correcta;
  el bulk insert lo hace el backend leyendo el adjunto desde
  `OutputStorageRepository` y emitiendo INSERTs o `COPY` (Postgres).
- **Diseño v1 propuesto:**
  - Nuevo tool `sql_bulk_insert_from_attachment(attachment_id, table, column_mapping?, on_error?)`.
  - El LLM previamente llama `sql_inspect_attachment(attachment_id, sample_rows?)` que devuelve `{columns: [...], inferred_types: {...}, sample: [row1, row2, ...], total_rows, encoding, delimiter}` sin cargar todo.
  - El `sql_bulk_insert` streamea el archivo (CSV via `csv` crate; xlsx via `calamine`) → batched INSERT o `COPY FROM STDIN` (Postgres only).
  - Reusa el SQL sandbox + permissions de v1 (`sql_ast` valida la table está en `allowed_schemas`).
  - On-error policy: `fail_fast` (default — rollback al primer error), `skip_rows` (continúa, reporta fallas), `partial_commit` (commit lo que pase).
- **Files:**
  - Nuevo: `dag_engine/infrastructure/nodes/llm_synthetic_tools/sql_bulk_tools.rs` (~400 LOC)
  - Modificado: `sql_node.rs` (exponer port para `OutputStorageRepository::read_stream`)
  - Tests: ~100 LOC con CSV/xlsx fixtures.
- **Esfuerzo:** **3-5 días**.
- **Acceptance:**
  - CSV de 1k filas se inserta con LLM viendo solo 5 rows de sample.
  - xlsx con headers en row 1 funciona idem.
  - `on_error: fail_fast` rollbackea atomic.
  - Postgres `COPY` se usa cuando se detecta backend Postgres (perf).
- **Cuándo retomar:** alta prioridad — habilita workflow concreto que
  ya pidieron usuarios. Después de items 11 y 12.
- **Decisiones abiertas:**
  - ¿Solo Postgres con `COPY`, o también MySQL/SQLite con batched INSERT? Recomendado v1 = solo Postgres; otros vienen después.
  - ¿`column_mapping` opcional con auto-inference, o siempre requerido? Recomendado: opcional con echo del mapping inferido para que el LLM lo confirme.

---

## Google SA — alias presentable via Workspace Group + revisión del leak del project ID (2026-06-09)

- **Origen:** Brainstorming con owner (2026-06-09) — el SA email actual
  expuesto a usuarios finales para que compartan docs (`colmena-agent@startti-dev.iam.gserviceaccount.com`)
  es feo, revela el GCP project ID, y rotar la SA implica avisarle a todos.
- **Solución estándar Google:** crear un Google Group (`agente@startti.co`
  o similar) en Google Workspace, agregar la SA como miembro. Los usuarios
  comparten docs con la dirección del grupo; la SA hereda el acceso. Bonus:
  rotar SA sin avisarle a nadie, oculta el project ID, audit log limpio.
- **Riesgo de seguridad de exponer el email solo:** bajo (comparable a
  publicar `soporte@`). Sin la JSON key del SA no se puede autenticar.
  Vectores residuales: phishing a admins, share-bombing (vector latente
  de prompt-injection indirecta SI en el futuro el agente lee "todos los
  docs compartidos" — hoy no aplica porque siempre requiere `doc_id`
  explícito).
- **Bloqueado por info de operador:** quién es Workspace admin de Startti,
  qué dominios están disponibles (`startti.co`, `startti.ai`, ...).
- **Scope cuando se retome:**
  - Crear el Group en Workspace.
  - Agregar SAs de dev/prod como miembros.
  - Actualizar `text/` y prompts auto-inyectados para usar el group email
    en vez del email del SA.
  - Documentar el setup en `docs/developer_guide/`.
- **NO BLOQUEA** el feature de auto-prompt de doc-ID al primer turno
  (BACKLOG/topic siguiente) — ese feature usa el SA email vigente, sea
  cual sea.

---

## Output filtering para LLM — qué campos ve el modelo (2026-06-09)

- **Origen:** Reporte del owner (2026-06-09). Use case: nodo upstream
  emite `{param1, param2, param3, param4, param5}` pero el LLM solo
  necesita ver `param1` y `param4`. Hoy ve todo → desperdicio de
  tokens + ruido.
- **🧠 Requiere brainstorming dedicado antes de scope.** El owner
  ya lo flagueó así. Este item es estratégico — toca el contrato
  fundamental entre nodos y el LLM en el DAG.
- **Bocetos posibles (ninguno decidido):**
  - **(A) Per-edge `select`** — JsonPath / JMESPath en la edge:
    `{ "from": "api_call", "to": "llm", "select": [".result.data.param1", ".result.data.param4"] }`.
    Pro: localizado al uso. Con: cada llm_call repite el select.
  - **(B) Per-node `llm_visible_fields`** — el nodo emisor declara qué expone al LLM:
    `api_call: { ..., llm_visible_fields: ["result.param1", "result.param4"] }`.
    Pro: una sola fuente de verdad. Con: el nodo emisor decide por el consumidor.
  - **(C) Per-tool-config en `llm_call`** — el `llm_call` config declara qué fields ver de cada upstream:
    `llm_call: { ..., upstream_filters: { api_call: ["param1", "param4"] } }`.
    Pro: máximo control del consumidor. Con: el llm_call queda acoplado a sus upstreams.
  - **(D) Lazy field access** — tool devuelve metadata `{available_fields: [...]}` + el LLM hace `read_field(node, field)` a demanda. Pro: cero waste, máxima flexibilidad. Con: 1+ turn extra, más latencia.
  - **(E) Output schema declarativo** — outputs traen anotaciones `@llm_hidden` en JSON schema; engine las strippea antes del LLM. Pro: cleanest. Con: invasivo (schema requerido en todo nodo emisor).
- **Trade-offs serios a discutir:**
  - **Ownership:** ¿quién decide qué se filtra? (operador del grafo / autor del nodo / agente)
  - **Retrocompat:** sin filter default = mismo comportamiento de hoy.
  - **Debuggability:** ¿el `extra_info` sigue mostrando el output completo aunque el LLM no lo vea? (sí — para que el operador pueda debuggear sin perder data).
  - **Observabilidad:** SSE debe loguear qué se filtró.
  - **Performance:** la transformación debe ser barata (filter inline, no nueva pasada).
- **Próximo paso:** sesión de brainstorming con `superpowers:brainstorming` skill.
  - **Input requerido del owner:** pintar el use case real (qué nodo, qué fields, qué pasó). Después se discuten las opciones A-E con escenarios concretos.
  - **Output esperado:** propuesta formal en `docs/proposals/2026-06-XX-output-filtering-for-llm.md` con la opción ganadora + spec + plan.
- **Estimación post-brainstorming:** indeterminada (depende de la opción ganadora). Sospecha: opción (A) o (C) ~3-5 días. Opción (B) o (E) ~5-8 días.
- **Cuándo retomar:** después de items 11-13. Brainstorming primero,
  implementación después.

---

## 🧊 CRDT Documents v1.1 — TODO EL BLOQUE DESPRIORIZADO (2026-06-09)

> **Decisión del owner (daniel@startti.co, 2026-06-09):** todos los items
> de CRDT Documents v1.1 quedan **despriorizados** hasta nuevo aviso.
> El subsistema CRDT no se va a llevar a producción ni a develop en ADP
> por ahora — el equipo confirmó que la tabla CRDT vive solo en memoria
> y no se va a integrar al frontend en el horizonte cercano. Mientras
> tanto, ningún item de esta sección debe consumir tiempo de dev.
>
> **Excepción — item 11 de la cola priorizada (prompt caching).** El item
> "Provider-level prompt caching (Anthropic + Gemini)" sigue activo
> porque es **plataforma-wide**, no CRDT-specific. Su título quedó así
> por contexto histórico (se descubrió durante F-T14). Tratar esa única
> entrada como independiente del freeze de CRDT.
>
> **Trigger para descongelar el resto:** cuando ADP retome el roadmap de
> documents colaborativos en frontend, o cuando un cliente concreto pida
> el feature. Hasta entonces, NO atacar ninguno de estos items.

---

## CRDT Documents v1.1 — formato visual en xlsx (fills, fonts, merges)

- **Origen:** validación manual de v1 (2026-06-02). El operador importó `spike/fixtures/test.xlsx`, vio que merges (A1:D1) y fills de header (amarillo) desaparecen tanto en el browser (Univer) como en el `.xlsx` exportado. Spec v1 explícitamente difirió formato — esta entrada deja el path para retomarlo.
- **Problema:** el IR projection solo modela `{v, t}` por celda. Toda info de formato (fills, fonts, borders, alignment, number-format) y estructura visual (merged ranges, row heights, column widths) se descarta en import y queda imposible de escribir en export. Para un usuario que sube un reporte real, el doc convergido + exportado es "datos crudos sin estilo" — el LLM puede operar sobre los valores, pero el deliverable final no preserva la apariencia.
- **Workaround actual:** el usuario re-aplica el formato a mano sobre el `.xlsx` exportado, o usa Univer en browser para re-formatear antes de descargar. Para flujos donde el visual matters (reportes ejecutivos, dashboards) el round-trip de v1 no es suficiente — el usuario debe mantener el original aparte y mergear manualmente los valores que cambió el LLM.
- **Por qué está parqueado:** v1 priorizó CRDT correcto + flujo end-to-end (import → LLM/Python → export). Formato es independiente del core de CRDT — agregarlo no valida ningún riesgo nuevo, solo mejora fidelidad visual. Diferido para que v1 ship en plazo.
- **Fix propuesto:** cinco tareas en orden:
  1. **Extender el IR cell shape** de `{v, t}` a `{v, t, fmt?: FormatRef}`, donde `FormatRef` apunta a un `named_styles` Y.Map a nivel workbook (`workbook.named_styles[styleId] = {fill, font, border, alignment, number_format}`). Reusar el patrón del existing `documents/` module (§3 de `27_documents_library.md`).
  2. **Añadir merges al workbook IR**: `sheet.merges: Y.Array<{start, end}>` con ranges A1-style (ej. `{start: "A1", end: "D1"}`).
  3. **Importer (`xlsx_import.rs`)**: leer `Cell.style()` de calamine, deduplicar formats, escribirlos como `named_styles` + referencias desde celdas. Leer `worksheet.merged_regions()` → escribir al Y.Array de merges.
  4. **Exporter (`xlsx_export.rs`)**: por cada `named_style` construir un `rust_xlsxwriter::Format`; aplicar via `worksheet.write_with_format()`. Por cada merge llamar `worksheet.merge_range()`.
  5. **Browser bridge (`crdt_documents/static/index.html`)**: extender el inbound observer para detectar cambios en `fmt` y `merges` → dispatchar comandos Univer `SetStyle`, `MergeCells`. Outbound: subscribirse al command bus para los mismos comandos y traducirlos al Y.Doc.
- **Acceptance criteria:**
  - Import del fixture preserva A1:D1 mergeado y el fill #FFD966 en row 2 (visible en browser + en `.xlsx` exportado).
  - Round-trip integration test: import → export → re-import → assertion sobre styles deduplicados.
  - Un humano editando formato en Univer (cambia fill, fuente, hace un merge) → propaga al Y.Doc → otro tab ve el cambio.
  - LLM tool `crdt_doc_set_cell_format(sheet_id, addr, fmt)` opcional pero deseable para que el agente pueda aplicar formato programáticamente.
- **Estimación:** ~5 tareas, ~3-5 días con un dev full-time. Sumar ~250 LoC al IR, ~300 al importer/exporter combinados, ~150 al frontend bridge. No requiere deps nuevas (rust_xlsxwriter ya soporta todo el formato). Riesgo: el modelo de styles dedup en CRDT bajo concurrencia (dos peers crean el mismo style simultáneamente) — probable solución: idempotent style id derivado del hash de sus propiedades.
- **Cuándo retomar:** cuando v1 esté shippeado a producción Y ADP integre la primera vista del workbook colaborativo. Si el primer cliente real reporta que pierde apariencia es trigger inmediato. Si nadie se queja en 2-4 semanas, queda en backlog hasta nuevo trigger.
- **Referencias:**
  - Spec v1 que explícitamente difiere esto: [`docs/superpowers/specs/2026-06-01-documents-crdt-v1-design.md`](superpowers/specs/2026-06-01-documents-crdt-v1-design.md) §3 "Fuera de v1".
  - Existing `documents/` module que ya tiene `named_styles` en el IR Excel: [`docs/developer_guide/27_documents_library.md`](developer_guide/27_documents_library.md) §2 "IR".
  - calamine cells with style: <https://docs.rs/calamine/latest/calamine/struct.Range.html>.
  - rust_xlsxwriter formats: <https://docs.rs/rust_xlsxwriter/latest/rust_xlsxwriter/struct.Format.html>.

---

## CRDT Documents v1.1 — WS-peer auto-reconnect

- **Origen:** decisión de scoping al implementar el modo `ws_peer` (V2-T1 a V2-T5, 2026-06-02). La política para v1 es fail-fast: si la WS muere mid-call, las tool calls subsiguientes devuelven `artifact_not_found`, el LLM ve el error y reporta al usuario. No hay retry.
- **Problema:** un blip de red transitorio (5-10s) entre el worker y el CRDT documents service hace que el graph entero falle. El usuario tiene que reintentar manualmente.
- **Workaround actual:** la mayoría de los blips son cortos. Si el deploy es intra-Cloud-Run la frecuencia debería ser baja (<1/1000 calls). Si se vuelve significativo, el usuario re-ejecuta el graph (la operación es generalmente idempotente desde la perspectiva del LLM — `set_range` no duplica datos).
- **Por qué está parqueado:** auto-reconnect agrega ~150-300 LoC en `ws_peer.rs` (state machine de retry + backoff + dedup de updates ya enviados pre-disconnect). v1 prioriza shipping con el flow visible, no resiliencia operacional.
- **Fix propuesto:** en `WsPeerArtifact`, agregar:
  1. Política de reintento configurable (`max_retries`, `initial_backoff_ms`, `max_backoff_ms`).
  2. State machine en el background task: `Healthy → Reconnecting → Failed` con counter de intentos.
  3. Re-handshake completo (sync_step1 → sync_step2) post-reconnect — los updates locales no aplicados aún se quedan en la mpsc y se envían post-handshake. Yjs sync es idempotente, los re-applies son no-ops.
  4. `is_alive()` queda `true` durante `Reconnecting`, `false` en `Failed`. Tool dispatchers no ven la diferencia.
- **Acceptance criteria:**
  - Integration test que mata el server mid-call, lo levanta de nuevo en <2s, y el agente termina el graph sin error.
  - Logging estructurado de cada reconnect attempt.
  - Backoff respeta `max_retries` antes de fallar.
- **Estimación:** ~2 días dev. Riesgo: bugs sutiles en el sync v1 re-handshake si el state vector quedó desalineado pre-disconnect.
- **Cuándo retomar:** cuando el deploy productivo muestre >1% de tool calls fallando por WS blips, o cuando ADP reporte UX afectado por reintentos manuales.
- **Referencias:** [`src/libs/colmena/src/crdt_documents/ws_peer.rs`](../src/libs/colmena/src/crdt_documents/ws_peer.rs) — comment "Failure mode (v1 policy: fail-fast)".

---

## CRDT Documents v1.1 — TTL/eviction de Y.Docs idle en RAM

- **Origen:** scope-cut al diseñar V2-T2 (2026-06-02). El `DocRegistry` actual nunca evicciona artifacts: cada `get_or_create` carga el Y.Doc en RAM y queda ahí hasta que el server reinicie.
- **Problema:** memoria del CRDT documents service crece sin techo con # artifacts accedidos durante la vida del proceso. Un workbook real de Excel ocupa 1-10MB en RAM. Después de 1000 artifacts accedidos en 24h, el proceso usa 1-10GB.
- **Workaround actual:** reiniciar el proceso cada N horas (Cloud Run `min_instances=1` + rolling restarts). Aceptable a baja escala (<100 artifacts/día) — el reinicio recarga snapshots desde disco al primer acceso.
- **Por qué está parqueado:** v1 lanza con <100 artifacts esperados. Mejora cuantitativa, no cualitativa.
- **Fix propuesto:** agregar TTL configurable al `DocRegistry`:
  1. `RegisteredArtifact::last_accessed: AtomicI64` actualizado en cada `get()`.
  2. Background task que cada N min escanea el registry, evicciona entradas con `last_accessed < now - TTL` Y `no peers WS conectados`.
  3. Evict = drop del `Doc` en RAM. La próxima `get_or_create` lo relee del snapshot en disco.
  4. Métricas: gauge `crdt_documents_in_ram`, counter `crdt_documents_evicted_total`.
- **Acceptance criteria:**
  - Integration test que crea 100 artifacts, espera TTL, verifica que <10 quedan en RAM.
  - Performance: TTL eviction no bloquea WS upgrades (lock fino vs registry-wide lock).
  - Snapshot reload < 200ms para un workbook de 1MB.
- **Estimación:** ~1 día.
- **Cuándo retomar:** cuando observemos memoria > 2GB o frecuencia de OOM > 0 en producción.

---

## CRDT Documents v1.1 — Redis pub/sub para broadcast cross-instancia

- **Origen:** discusión arquitectónica 2026-06-02 sobre el modelo "RAM autoritativo en el server".
- **Problema:** el CRDT documents server escala verticalmente pero no horizontal. Si scaleamos a 2+ instancias del WS server, un peer conectado a instancia A no ve mutaciones de peers conectados a instancia B. Sticky LB routing por `artifact_id` evita el problema pero limita la resiliencia (caída de un nodo pierde todas sus sessiones).
- **Workaround actual:** `min_instances=1, max_instances=1` para el CRDT service. Acepta el single point of failure.
- **Por qué está parqueado:** no hay carga aún. v1 con una instancia maneja > 1000 artifacts activos sin sweat.
- **Fix propuesto:** agregar Redis pub/sub:
  1. Cada server, al recibir un update vía WS, lo publica también a `redis://...:6379/channels/crdt:<artifact_id>`.
  2. Cada server se suscribe a esos channels para los artifacts que tiene en RAM.
  3. Al recibir un message del channel (de otra instancia), aplica el update a su Y.Doc local y lo broadcastea a sus propios peers WS.
  4. Redis NO es source-of-truth — es solo el bus. Source of truth sigue siendo el snapshot en GCS.
- **Acceptance criteria:**
  - 2 instancias del CRDT server detrás de un LB random-routing.
  - Peer P1 a instancia A, peer P2 a instancia B sobre el mismo artifact.
  - Mutación de P1 visible en P2 con < 100ms latencia.
- **Estimación:** ~2-3 días dev + Redis ops setup.
- **Cuándo retomar:** cuando la carga justifique > 1 instancia del CRDT service.

---

## CRDT Documents v1.1 — Per-cell attribution para peer:browser events

- **Origen:** scope-cut al implementar subsistema B (2026-06-03). El server recibe updates Yjs binarios opacos de browsers; no puede saber qué sheet/celda cambió sin inferencia activa.
- **Problema:** los eventos de `peer:browser` quedan con `sheet_id: NULL` y summary "peer update (N bytes)". En el auto-summary aparecen como "Workbook (sheet unknown): N changes by peer:browser", lo cual es menos informativo que "Inventory: N changes by peer:browser".
- **Workaround actual:** acepta granularidad coarse. Si el agente necesita saber qué sheet cambió, debe leer el doc directamente vía `crdt_doc_list_sheets` + `crdt_doc_read`.
- **Por qué está parqueado:** el v1 prioriza el flow end-to-end. La inferencia per-cell requiere un diff de projection antes/después del apply_update, lo cual es trabajo no trivial.
- **Fix propuesto:**
  1. En `handle_socket`, antes de cada `apply_update`, capturar la projection actual del Y.Doc.
  2. Aplicar el update.
  3. Diffear la projection nueva contra la vieja.
  4. Por cada celda cambiada, registrar un event con sheet_id, addr, value (antes/después).
- **Acceptance criteria:**
  - peer:browser events tienen sheet_id + addr poblados.
  - Auto-summary muestra "Inventory: 3 changes by peer:browser" en vez de "Workbook (sheet unknown)".
  - Performance: el diff per-update < 5ms para workbooks <1MB.
- **Estimación:** ~1-2 días. Medir impacto perf con benchmark.
- **Cuándo retomar:** cuando UX feedback indique que la atribución coarse es limitante (probable para flows colaborativos browser+agente).

---

## CRDT Documents v1.1 — Paginación de list_my_artifacts

- **Origen:** scope-cut subsistema B (2026-06-03).
- **Problema:** sesiones con >50 artifacts solo ven los 50 más recientes via `crdt_doc_list_my_artifacts`. No hay cursor de paginación.
- **Workaround actual:** los 50 más recientes alcanzan para la mayoría de flows. Cliente puede pasar `limit` mayor (sin tope técnico, pero impacta performance/payload del response).
- **Fix propuesto:** agregar `offset` o `cursor: Option<String>` (timestamp-based). Devolver `next_cursor` cuando hay más.
- **Cuándo retomar:** cuando reportemos sesiones con >50 artifacts.

---

## CRDT Documents v1.1 — Retención TTL en `crdt_doc_events`

- **Origen:** decisión durante diseño B (2026-06-03).
- **Problema:** la tabla crece sin límite. Para una sesión de uso intenso (1 evento/min × 100 días) son 144k rows. Manejable, pero crece.
- **Fix propuesto:** scheduled job (Cloud Scheduler) que ejecuta `DELETE FROM crdt_doc_events WHERE created_at < now() - INTERVAL '90 days'`. Configurable. Patrón ya establecido en colmena: `attachment_gc` binary corre como Cloud Run Job + Cloud Scheduler.
- **Cuándo retomar:** cuando la tabla supere 1M rows en producción.

---

## CRDT Documents v1.1 — Configurable limits para `crdt_doc_run_python`

- **Origen:** scope-cut al implementar subsistema C (2026-06-03/04). Los límites de tamaño/timeout viven hardcoded en `crdt_doc_run_python.rs`.
- **Problema:** workbooks específicos pueden necesitar más memoria (datasets analíticos >100MB) o más tiempo de cómputo (joins complejos, statistical tests caros). El default conservador no acomoda casos legítimos.
- **Workaround actual:** el agente puede dividir el análisis en múltiples calls más chicos. Para casos genuinamente grandes (10M+ rows), no hay path.
- **Fix propuesto:**
  1. Estructurar limits como `RunPythonLimits` struct con defaults match v1.
  2. Cargar desde `crdt_documents.run_python_limits.*` (config del nodo) o env vars (`COLMENA_CRDT_PY_MAX_LOAD_MB`, `COLMENA_CRDT_PY_TIMEOUT_SECS`, etc).
  3. Mantener ceiling absoluto hardcoded para prevenir abuse (ej. nunca permitir >1GB load aunque config diga).
  4. Telemetry: counter por tipo de cap-hit.
  5. Para `output_sheet` > 100K rows: chunked transact_mut (escribir en lotes de 10K para no bloquear el CRDT subscription).
- **Acceptance criteria:**
  - Operator puede subir el cap de 100MB → 500MB vía env var.
  - Cap absoluto (1GB) sigue activo aunque config pida más.
  - Métrica de cap-hits visible en logs/metrics.
- **Estimación:** ~1 día dev + tests.
- **Cuándo retomar:** cuando observemos usuarios chocando caps regularmente, o un cliente concreto pida specifically.

---

## CRDT Documents v1.1 — Dynamic Univer grid sizing desde Y.Doc

- **Origen:** durante el C smoke (2026-06-04), imports xlsx >100 filas se renderizaban truncados en el canvas porque `rowCount` era literal `100`. Bumped a 50000 como interim (ver `static/index.html`), pero sigue siendo hardcode.
- **Problema:** sheets más grandes que el cap (`>50K filas` post-fix, `>5K columnas`) son invisibles parcialmente para el usuario, aunque el Y.Doc las tenga completas y el agente las analice OK. Workaround manual: bumpear el literal y rebuild.
- **Fix propuesto:**
  1. Al mount, calcular `max_row` y `max_col` por sheet recorriendo `cells` del Y.Doc (parsing de A1 addresses → row/col index).
  2. Pasar a Univer `rowCount: max_row + 50` (buffer para edición), `columnCount: max(26, max_col + 5)`.
  3. En el observer de `cells.observe`, si una nueva celda excede el `rowCount` actual, dispatch `SetRowCountMutation` o equivalente para crecer la grid dinámicamente.
  4. Alternativa más simple: exponer `?rows=N&cols=M` query string como override sin tocar el observer.
- **Acceptance criteria:**
  - Import de un xlsx con 100k filas se renderiza completo sin tocar config.
  - El agente puede escribir una sheet con 30k rows y aparece en el viewer.
- **Estimación:** ~2-4 horas (depende de cuán bien Univer expone mutaciones de grid bounds).
- **Cuándo retomar:** cuando aparezca un workbook real >50k filas, o cuando F necesite renderizar diffs/joins grandes.

---

## CRDT Documents v1.1 — Auto-detect title row en `df_records`

- **Origen:** xlsx importados a menudo tienen una "title row" en A1 (`Reporte Q3 2026`) con B1/C1/D1 vacíos, y los headers reales en row 2. La C smoke gastó 8 retries hasta que el agente entendió el shape vía el SKILL.md.
- **Problema:** el contrato actual es "row 1 SIEMPRE es headers". Es predecible pero requiere skill + boilerplate (`df.columns = df.iloc[0].tolist(); df = df.iloc[1:]…`) para todo xlsx con título.
- **Fix propuesto:**
  1. Heurística en `df_records::build_sheet_records`: si row 1 tiene ≤25% de celdas no-nulas Y row 2 tiene ≥75% no-nulas, asumir que row 1 es title y row 2 son los headers reales. Devolver un nuevo campo `title_row: Some("...")` en `SheetRecords`.
  2. El dispatcher de `run_python` lo pasa al agente vía el output JSON como hint.
  3. Opcional v2: parámetro `header_row` en `RunPythonArgs` (default `auto`, alternativa `1`, `2`, `none`) para que el agente fuerce el comportamiento.
- **Acceptance criteria:**
  - Importar el xlsx de la C smoke (título "Reporte Q3 2026" + headers Producto/Cantidad/Precio/Total) y hacer `df['Precio'].sum()` directo, sin promotion boilerplate.
  - Tests con todos los 4 layouts: (a) solo headers row 1, (b) title + headers, (c) sin headers (col_A/B/…), (d) title sin headers reales.
- **Estimación:** ~3-5 horas (heurística + tests + actualizar SKILL.md).
- **Cuándo retomar:** cuando F lo necesite, o si vemos múltiples agentes loopeando por el mismo motivo.

---

## CRDT Documents v1.1 — Multi-session workspace visibility

- **Origen:** restricción explícita en F (subsistema 3, 2026-06-04): hoy `crdt_doc_list_my_artifacts` filtra por session_id, así que un agente solo descubre artifacts creados en su misma sesión. El owner pidió específicamente que "diferentes agentes en diferentes turnos incluso con diferentes agent session id puedan crear artefactos que otros agentes modifiquen lean o comparen".
- **Problema:** sin esto, F funciona dentro de una sesión pero no entre sesiones. El usuario tiene que pasar el `artifact_id` explícito en el prompt para cruzar sesiones, lo cual no escala a flujos colaborativos reales.
- **Fix propuesto:**
  1. Introducir concepto de "workspace" (= organización, team, project) en `crdt_doc_session_artifacts`: relación N:N en vez de "owned by one session".
  2. Nuevo tool `crdt_doc_list_workspace_artifacts({workspace_id?})` que devuelve los artifacts del workspace del caller. Default workspace = el del session id.
  3. Modelo de permisos opcional por artifact (`read | read_write | owner`) gateado por workspace membership.
  4. Mecanismo de share/link entre artifacts con auditoría (quién compartió con quién cuándo).
- **Acceptance criteria:**
  - Agente A (session_id=s1) crea artifact art_X. Agente B (session_id=s2, mismo workspace) lo descubre vía `list_workspace_artifacts` y lo importa vía `import_sheet`.
  - Agente C (otro workspace) NO ve art_X.
  - Owner puede revocar acceso de un workspace a un artifact.
- **Estimación:** ~2-3 días dev (incluyendo migrations + tools + tests).
- **Cuándo retomar:** bloqueante para subsistema A (microservice deploy multi-tenant). Antes de subir a prod multi-usuario.

---

## CRDT Documents v1.1 — Live linking de sheets clonadas

- **Origen:** decisión explícita en F: el clonado de `crdt_doc_import_sheet` es snapshot only — cambios posteriores en el source no se propagan al clone.
- **Problema:** para análisis "vivos" (ej: dashboard que compara Q3 con Q4 en tiempo real mientras Q4 se actualiza), el agente o el usuario tienen que re-importar manualmente.
- **Fix propuesto:**
  1. Nuevo flag `live: true` en `import_sheet` que registra una subscripción del clone al source.
  2. Cuando el source cambia (vía `cells_map.observe`), aplicar el delta al clone con conflict resolution (last-write-wins por celda).
  3. Manejo de borrado del source: el clone se "freezes" en el último estado y se marca con flag visible.
  4. Cleanup automático cuando el artifact destino se borra.
- **Acceptance criteria:**
  - Edito una celda en el source → se refleja en el clone dentro de 1s.
  - Borro la sheet source → el clone queda en el estado final con flag "source deleted".
  - Borro el clone → no afecta el source.
- **Estimación:** ~2 días (subscription management + conflict resolution + cleanup paths + tests).
- **Cuándo retomar:** cuando aparezca un caso de uso real de "dashboard cross-artifact" (compare/enrich que necesita seguir cambios upstream).

---

## CRDT Documents v1.1 — Eliminar sheets

- **Origen:** F clona sheets y no provee mecanismo para limpiar. El cap de 100 sheets/artifact protege contra runaway pero no permite mantener el workbook ordenado.
- **Problema:** después de un análisis, el agente o el usuario quieren eliminar las sheets clonadas temporales. Hoy no hay tool ni acción en el canvas.
- **Fix propuesto:**
  1. Nuevo tool `crdt_doc_delete_sheet({sheet_id, confirm?})` que elimina la sheet del Y.Doc en una transacción. Requiere `confirm: true` explícito para prevenir borrado accidental por el LLM.
  2. UI button en Univer para que el usuario también pueda borrar (probablemente ya existe en Univer — solo wirearlo al delta del Y.Doc).
  3. Audit event con el nombre y resumen del contenido borrado (para soft-undo manual si fuera necesario).
- **Acceptance criteria:**
  - Borrar una sheet decrementa `MAX_SHEETS_PER_ARTIFACT` counter; siguiente import vuelve a entrar.
  - El borrado se propaga vía WS a todos los peers.
  - Event log conserva el resumen para auditoría.
- **Estimación:** ~4-6 horas dev.
- **Cuándo retomar:** post-merge de F, cuando el feedback real de usuarios muestre que el clutter de sheets clonadas es molesto.

---

## CRDT Documents v1.1 — Consolidar parser A1 (parse_a1 / parse_a1_to_rc)

- **Origen:** code review de F-T1 (2026-06-04). Hay 3 copias del parser de direcciones A1 → (row, col) en el codebase: `crdt_documents/df_records.rs:124` (`parse_a1`), `crdt_documents/xlsx_export.rs:58` (`parse_a1`), `dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs:212` (`parse_a1_to_rc`, agregado por F-T1).
- **Problema:** las 3 copias difieren en detalles (la nueva acepta lowercase via `to_ascii_uppercase`, las otras dos requieren uppercase). Cualquier futura mejora (overflow handling, validación, soporte para R1C1) tiene que tocarse en 3 lugares.
- **Fix propuesto:**
  1. Crear `src/libs/colmena/src/crdt_documents/addr.rs` con la función canónica (e.g. `pub fn parse_a1(addr: &str) -> Option<(u32, u32)>` con doc + tests).
  2. Migrar los 3 call-sites a importarla.
  3. Eliminar las versiones locales.
- **Acceptance criteria:**
  - Solo 1 implementación de A1 parsing en todo el crate (verificado por `grep -rn "fn parse_a1" src/`).
  - Comportamiento idéntico en los 3 call-sites (tests cubren A1, AA12, Z99, edge cases con lowercase/digits-only/empty).
- **Estimación:** ~2 horas.
- **Cuándo retomar:** próxima vez que F o cualquier subsistema toque address parsing, o como tarea de cleanup standalone.

---

## CRDT Documents v1.1 — Reutilizar projection::project en list_sheets_of

- **Origen:** code review de F-T1 (2026-06-04). `execute_list_sheets_of` walking del Y.Doc (~70 líneas) replica lógica que `crdt_documents::projection::project` ya hace. La projection actual no incluye `n_rows`/`n_cols` por sheet, lo que forzó la duplicación.
- **Problema:** mantener dos caminos paralelos que recorren la misma estructura del Y.Doc invita drift. Si la estructura cambia (e.g. agregamos `revision` por sheet), hay que actualizar ambos lugares.
- **Fix propuesto:**
  1. Extender `projection::project` (o agregar `project_summary` companion) que incluya opcionalmente `n_rows` y `n_cols` por sheet (computados desde max addr en cells map).
  2. Reescribir `execute_list_sheets_of` para usar `project_summary` + filtrar a los campos LLM-relevantes.
  3. Beneficio adicional: si v1.1 incorpora más metadata por sheet (e.g. `created_by_origin`, `last_modified_at`), aparece automáticamente para el LLM via projection.
- **Acceptance criteria:**
  - `execute_list_sheets_of` deja de hacer raw Y.Doc walking.
  - Tests de F-T1 (`crdt_doc_list_sheets_of_test`) siguen pasando sin cambios.
  - Output schema es bit-exact al actual.
- **Estimación:** ~3 horas.
- **Cuándo retomar:** junto con la consolidación del parser A1 (mismo touchpoint conceptual: cleanup del module crdt_documents).

---

## CRDT Documents v1.1 — Provider-level prompt caching (Anthropic + Gemini)

> ⭐ **Priority bumped 2026-06-09 por daniel@startti.co** — listado como
> item 11 en la "cola priorizada" al inicio del backlog. El item no es
> CRDT-specific (es plataforma-wide); el título quedó así por contexto
> histórico (se descubrió durante F-T14).

- **Origen:** F-T14 step A4 / análisis de tokens (2026-06-04). Las 3 optimizaciones de Plan GAMMA bajaron tokens enviados de ~95K → ~75K (-22% por run). Pero hay otra capa de ahorro disponible que no se atacó: caching nativo del provider, que reduce el COSTO de los tokens que igual se mandan.
- **Problema:** OpenAI tiene caching automático para prefixes ≥1024 tokens (ya leemos `cached_tokens` en el adapter — funciona out of the box). Anthropic requiere markers `cache_control: ephemeral` en system_message + tools[] (nuestro adapter LEE `cache_read_input_tokens` pero NO los SETEA — caching nunca se activa). Gemini tiene Cached Content API explícita (nuestro adapter no tiene nada). Resultado: 2 de 3 providers no aprovechan caching aunque el infra del adapter ya sabe leer las stats.
- **Fix propuesto:**
  1. Anthropic adapter: en cada `LlmRequest`, agregar `cache_control: {"type": "ephemeral"}` al system_message + al último tool de la lista de tools. Eso marca el prefix como cacheable; calls subsecuentes con el mismo prefix dentro de 5 min se billan al 10% del precio normal. Cero impacto en latencia (solo marker).
  2. Gemini adapter: implementar Cached Content API. Al primer request, crear un cached content con system + tools (TTL 5 min). Subsecuentes requests pasan el ID del cached content y reciben billing al ~10%.
  3. Test: assertion en integration tests que `cache_read_input_tokens > 0` después del 2do request en una conversación multi-turn.
- **Acceptance criteria:**
  - Smoke F con Anthropic Claude muestra `cache_read_input_tokens > 0` en iter ≥2.
  - Smoke F con Gemini Pro muestra ahorros billing similares (medibles via `cachedContentTokenCount`).
  - Adapter de OpenAI sigue funcionando idéntico.
- **Estimación:** ~1h Anthropic, ~3h Gemini (su API es más rara), ~1h tests. Total ~5h.
- **Cuándo retomar:** cuando ADP empiece a procesar volume real y los costos de tokens importen. Para dev local, el costo por run es <$0.05 — no es prioridad inmediata.

---

## CRDT Documents v1.1 — Formulas (subsystem D follow-ups)

Items derivados de la implementación de Subsystem D (formulas v1, 2026-06-04 → 2026-06-05). El core ya está shippeado en develop; estos son refinamientos diferidos.

- [ ] **Univer ↔ yrs formula round-trip — frontend integration gap (top priority for ADP frontend team).**
  El backend (`apply_set_cell_in_proc`) persiste correctamente `{v, t, f, fs}`
  en cada escritura de fórmula. El inbound observer del demo estático
  (`src/libs/colmena/src/crdt_documents/static/index.html`, D-T14) reenvía
  `f`/`fs` a Univer vía `SET_RANGE_VALUES_MUTATION`. Sin embargo, el
  `UniverFormulaEnginePlugin` re-procesa la celda después de aplicar nuestra
  mutación y emite una **segunda mutación** cuyo `cellPayload` NO preserva
  `f`. El outbound handler (D-T14) escribe esa segunda mutación de vuelta al
  yrs Doc como `{v, t}` solamente, **sobrescribiendo `f`/`fs` originales en
  milisegundos**.

  **Síntomas cuando hay un browser conectado:**
  - `f`/`fs` desaparecen del yrs Doc poco después de que el backend los escribe.
  - `crdt_doc_read(include_formulas=true)` devuelve sólo `{v}` para esas celdas.
  - Escrituras literales del agente en celdas de input no cascadean
    (`recalc_chain` no encuentra fórmulas que recorrer).
  - Ediciones del usuario en Univer tampoco disparan el cascade del backend
    (D-T15) porque el dep-graph está vacío.

  **Repro:**
  ```bash
  cargo run --release --bin dag_engine crdt-yws-graph \
    tests/graphs/crdt_documents/d_formulas_interactive_demo.json \
    --seed-artifact-id <ULID> --wait-before-graph 30 ...
  ```
  Conectar un browser dentro de los 30s. Tras correr el grafo:
  `curl http://127.0.0.1:8090/documents/<ULID>/projection.json` muestra las
  celdas sin `f`. El mismo test sin browser conectado pasa 100% verde
  (D-T15 unit tests + smoke en `feature/docs` HEAD).

  **Para el equipo de frontend del ADP** (la fix real vive ahí, no acá):
  - **Path 1 (recomendado)**: cambiar el inbound de
    `SET_RANGE_VALUES_MUTATION` a `SET_RANGE_VALUES_COMMAND` (el command de
    Univer de nivel superior). El path de comando pasa por el command
    service, al que el formula engine se subscribe para indexar — esto le
    "enseña" al engine las dependencias de la fórmula y evita el ciclo
    re-process-and-strip. Riesgo: el command dispara `onCommandExecuted`
    como echo outbound; requiere manejo cuidadoso del flag
    `applyingFromYDoc`.
  - **Path 2**: después de aplicar la mutación, llamar explícitamente
    `formulaEngine.registerFormula(unitId, sheetId, addr, fText)` (o
    equivalente de la API 0.x de Univer) para que el engine indexe la
    fórmula. Menos invasivo que Path 1 pero depende de que la API interna de
    Univer sea estable.
  - **Path 3 (fallback)**: mantener la arquitectura actual y endurecer el
    outbound handler — cuando llegue una `SET_RANGE_VALUES_MUTATION` desde
    Univer sin `f`, chequear el yrs Doc por un `f` existente en esa celda y
    preservarlo (no sobrescribir). Limita el daño pero no soluciona que
    el engine de Univer no conozca las fórmulas escritas por el backend.

  **Contrato forward-compat para cualquier frontend** (independiente del
  path elegido): el schema de celda en yrs es
  `workbook.sheets[i].cells.<A1> = {v, t, f?, fs?}` donde
  `fs ∈ {"be", "fe", "needs_browser"}`. Los frontends DEBEN preservar
  `f`/`fs` en cualquier escritura de celda que no sea un reemplazo literal
  explícito (por el usuario) de la fórmula.

  D-T14 dejó el plumbing de `f`/`fs` por inbound + outbound. D-T15 dejó el
  observer server-side que cascadea fórmulas cuando llegan ediciones del
  browser — funciona correctamente cuando `f` está intacta, pero hoy no se
  puede ejercitar porque `f` ya se perdió cuando dispararía. Una vez que
  cualquiera de los 3 paths aterrice y `f` sobreviva el round-trip, el
  observer de D-T15 + el cascade client-side de Univer deberían funcionar
  sin cambios adicionales.

  **Referencias:**
  - D-T14: commits `25633fa`, `6a14571` (plumbing inbound/outbound).
  - D-T15: commit `19bb419` (server-side recalc observer).
  - Demo: `src/libs/colmena/src/crdt_documents/static/index.html` (sección
    inbound observer + outbound `onCellChanged`).
  - Schema de celdas: `crdt_documents::tool_executor::apply_set_cell_in_proc`.

- [ ] **Cross-sheet eager recalc** — cuando `Sheet2!A1` cambia, los
  dependientes en `Sheet1` que la referencian deben auto-actualizarse.
  Hoy quedan stale hasta que alguien los re-toca. Spec §11.

- [ ] **`crdt_doc_recalc(sheet?, all=true)` tool** — refresh explícito,
  necesario para el caso cross-sheet stale y para escenarios post-import.

- [ ] **Cross-artifact references** `='[OtherWB.xlsx]Sheet1'!A1`.

- [ ] **Array formulas** `{=SUM(A1:A10*B1:B10)}` — validar semántica de
  formualizer, diseñar UI de spill.

- [ ] **Defined names** `=SalesTotal`.

- [ ] **AST caching** por celda para evitar re-parse en cada recalc.

- [ ] **Univer-side `fs:"fe"` hook** — patch chico en el cliente (~30
  líneas) para que las fórmulas tipeadas por el usuario carguen
  `fs:"fe"` en vez de `fs:undefined`.

- [ ] **Anti-divergence Playwright bridge** — terminar el evaluador
  Univer-side en `tests/formula_divergence.rs` (el test
  `univer_matches_formualizer` está stub con `unimplemented!()`). El
  bridge spawnaría headless Chromium via Playwright, cargaría Univer,
  evaluaría cada fixture en el browser y compararía contra formualizer.
  Crecer el corpus hacia el target original de 80 fixtures cuando el
  bridge exista.

- [ ] **Structured `kind` field on CRDT events** — hoy
  `formula_replaced_by_literal` está codificado en el string `summary`
  con un prefix. Un enum `kind` tipado en el event store sería más
  robusto para parsing downstream.

- [ ] **Textual-fallback recalc for `needs_browser` cells** — hoy
  `dependents_of` silenciosamente excluye celdas con `fs:"needs_browser"`
  del grafo de dependencias (porque `parse()` retorna `NeedsBrowser` para
  ellas). Un scan textual atraparía refs a A1 dentro de una fórmula
  no-parseable y al menos notificaría al usuario que la celda está stale.

- [ ] **`CellResolver::get_formula(sheet, addr)`** método del trait —
  hoy el lookup walkea `iter_formulas_in_sheet` linealmente. Lookup
  directo mejoraría performance de recalc en sheets grandes.

**Referencias:**
- Spec: [`docs/superpowers/specs/2026-06-04-crdt-formulas-design.md`](superpowers/specs/2026-06-04-crdt-formulas-design.md).
- Plan: [`docs/superpowers/plans/2026-06-04-crdt-formulas.md`](superpowers/plans/2026-06-04-crdt-formulas.md).
- Cambios shippeados: ver `docs/CHANGELOG_2026-06.md` → entry D.

---

## Subsystem E v1.1 (Google Sheets)

- [ ] **E-T7b — xlsx attachment plumbing** (deferred from E-T7). The 2 xlsx
  dispatchers (`gsheets_create_from_xlsx`, `gsheets_export_xlsx`) need
  attachment-byte fetcher AND register-bytes-as-attachment path. Neither
  exists today as a tool-dispatcher-accessible helper. Either thread an
  attachment fetcher/registrar into `DagToolExecutor`, OR add a new
  sentinel mechanism so the LLM loop handles attachment I/O on the
  dispatcher's behalf. Tool definitions are already published in
  `enabled_tools` so once the plumbing exists, agents see no API change.
- [ ] **`gsheets_list_spreadsheets()`** — Drive discovery scoped to a
  shared folder. Needs `drive.metadata.readonly` scope and a
  folder-filter mechanism so the agent doesn't see the whole Drive.
- [ ] **OAuth user-scoped auth** — act on behalf of a specific human
  user. Likely a new `SheetsAuthProvider` trait so ADP (or any
  downstream consumer) can plug in its OAuth flow.
- [ ] **Cell formatting** — colors, borders, column widths via
  `batchUpdate` + `repeatCell`/`updateBorders`.
- [ ] **Charts** via `batchUpdate.addChart`.
- [ ] **Conditional formatting** via `batchUpdate.addConditionalFormatRule`.
- [ ] **Data validation (dropdowns)** via `batchUpdate.setDataValidation`.
- [ ] **Permissions / sharing tools** via `drive.permissions.*`.
- [ ] **Revisions / undo** via Drive Revisions API.
- [ ] **Webhook subscriptions** for push notifications on sheet changes.
- [ ] **Per-call credential overrides** — a graph could provide a different
  SA for different spreadsheets, useful for multi-tenant scenarios.
- [ ] **Apps Script execution** from colmena (a single new tool calling
  `scripts.run`).

---

## Subsystem G v1.1 (Google Docs)

> Refreshed 2026-06-09 post live verification. Los items abajo reflejan
> el shape real de v1 shipped (ver §17 de la
> [spec](superpowers/specs/2026-06-08-google-docs-design.md)) y los
> hallazgos operacionales documentados en
> [`developer_guide/45_gdocs.md`](developer_guide/45_gdocs.md) §"Limitaciones
> en v1".

- [ ] **`apply_edits` ConfirmManyMatches threshold** — standalone
  `replace_text` aborta con `ConfirmManyMatches` cuando una find string
  matchea ≥5 veces (sin `confirm_many: true`). `apply_edits` hoy lo
  bypassa: ejecuta replace-all silenciosamente. Resultado: el LLM puede
  modificar 4 párrafos cuando creía estar tocando solo 1. Fix: aplicar
  el mismo threshold por sub-edit antes de la fase de resolve. Decisión
  pendiente: ¿romper compat (siempre exigir `confirm_many` cuando
  multi-hit) o solo emitir warning estructurado en el `EditResult`?
  Origen: análisis del bug en agent_session `cmq7kem1h003001s6mr36uwe8`
  (2026-06-10).

- [ ] **`apply_edits` skill auto-loaded para scope-discipline** — el
  LLM hoy llama `replace_text` con find strings ambiguos ("Ejercicios:"
  matchea Día 1 y Día 3) sin usar `scope` ni `anchor`. Educable vía
  skill markdown auto-cargada cuando `gdocs_*` está en el catálogo
  (mismo pattern que sql-query-best-practices). Incluye: "siempre
  scope-á por paragraph range cuando hay múltiples secciones",
  "preferí `anchor` sobre find strings genéricos", ejemplo del bug.
  Origen: misma sesión (2026-06-10).

- [ ] **OAuth user-scoped flow** — bloqueante para "cualquier Gmail user
  puede usar el agente sin Shared Drive". Hoy `create_*` solo funciona
  con Workspace Shared Drives o DWD por la quota cero del SA. Con
  OAuth user-scoped (consent screen + refresh_token storage + ciclo
  de refresh + `OAuthDocsClient` adapter alternativo), los archivos
  creados quedan owned directamente por el usuario. ~1-2 semanas dev.
  Misma raíz que la línea equivalente en Subsystem E v1.1.
- [x] ~~**Paragraph-level human-change diff en `human_changes_pending`**~~ —
  **shipped 2026-06-09** vía Camino A. Snapshot cache extendido sobre
  `gdocs_session_state` (no tabla nueva — additive ALTER TABLE con
  `last_snapshot_json` JSONB + `last_snapshot_size_bytes` INTEGER).
  Diff via Myers (crate `similar`) particionado por scope; cap 1 MB
  configurable via `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`. Instancias sin
  migración degradan a v1 behavior (`information_schema` check al
  boot). Spec:
  [`docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`](superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md).
  CHANGELOG §17. Residuales para v1.2: detección solo-estilo, diff
  intra-paragraph carácter-perfecto, atribución a usuario (sin Google
  per-edit log → permanente null).
- [ ] **`add_tab` markdown seeding** — hoy el arg `markdown` se acepta
  pero la response incluye `pending_markdown_seed: true` y el contenido
  no se aplica. Implementar: post-creación del tab, llamar el converter
  + `batch_update` con `tabId` del nuevo tab. ~30 LOC en el dispatcher.
- [ ] **`dispatch_create_from_docx` attachment plumbing** — el dispatcher
  devuelve `not_yet_wired`. Necesita attachment-byte fetcher
  threaded a `DagToolExecutor` (mismo bloqueo que E-T7b en gsheets).
  Una vez fixed, ambos features (gsheets + gdocs) se desbloquean
  simultáneamente.
- [ ] **`dispatch_export` attachment wrapping** — hoy devuelve
  `{format, byte_len}`. Falta envolver los bytes en un attachment
  registrado para que downstream pueda re-compartirlo. Mismo plumbing
  que E-T7b (register-bytes path).
- [ ] **`mode: "suggest"`** — `writeControl.suggestionsEnabled` (parámetro
  aceptado pero no-op en v1; el agente recibe un warning si lo pasa).
- [ ] **Surgical table-cell edits** (`gdocs_set_table_cell`,
  `gdocs_insert_table_row`). Hoy las tablas existen en el doc (Drive
  las convierte nativamente desde markdown en `create_from_markdown`)
  pero el agente no puede editar celdas individuales sin un round-trip
  manual.
- [ ] **`gdocs_insert_image_after_text`** (sabores `attachment_id` + URL).
  Hoy no existe — markdown images en inserts pasan como lossy. Requiere
  attachment fetcher (shared root con `create_from_docx`).
- [ ] **Drive Comments API** — mensajería humano ↔ agente in-doc
  (`gdocs_add_comment`, `gdocs_resolve_comment`, `gdocs_list_comments`).
  Útil para el flujo de revisión donde el humano deja TODO comments y
  el agente los procesa.
- [ ] **`gdocs_list_documents`** — descubrimiento scoped a folder via
  Drive (`drive.files.list?q=mimeType='application/vnd.google-apps.document'`).
  Hoy el agente solo opera sobre `doc_id` recibidos en el prompt o
  input del grafo.
- [ ] **Markdown tables en insert/replace** — requiere round-trip snapshot
  para computar índices de celda. Hoy `gdocs_insert_*`,
  `gdocs_replace_section`, `gdocs_append_markdown` y `gdocs_apply_edits`
  rechazan markdown con tablas (`invalid_args`).
- [ ] **Ejecución de Apps Script** desde colmena (`scripts.run`).
- [ ] **Drive Revisions restore** (rollback a una revisión previa).
- [ ] **Math expressions en markdown** — hoy pasan como `$…$` literal.

---

## Toolkit packages v1.1

- [ ] **Auto-inject package description** into agent system message when a
  package is enabled (one-paragraph orientation block, collapsed in
  `lazy_tool_loading` catalogs). Provides discoverable context on what the
  package is for + when to use it.
- [ ] **Unknown alias warning** — when `enabled_tools` contains a name that
  matches no tool, package, or `configured_alias`, surface a structured
  warning in `extra_info` instead of silently producing an empty filter.
  Catches typos early.
- [ ] **DAG-node summaries** — extend `ExecutableNode` trait with optional
  `summary()` method so DAG nodes used as tools (via `tool_configurations`)
  can declare a default; the user's
  `tool_configurations.<name>.summary` overrides it per agent. Enables lazy
  loading for user-created tools without duplication.
- [ ] **Cross-feature validation** — when `enabled_tools` contains both a
  toolkit package and individual tools from that package,
  detect conflicts and warn (e.g.
  `["gsheets", "gsheets_read"]` is redundant).

**Referencias:**
- Spec: `docs/superpowers/specs/2026-06-06-toolkit-packages-design.md`.
- Dev guide: `docs/developer_guide/40_toolkit_packages.md`.
- Implementación inicial: commits E-T16.

---

## Items resueltos recientemente

El último item — `data:` (base64 inline) auto-summary v2 — se resolvió el 2026-05-18 (ver `docs/CHANGELOG_2026-05.md` → "Inline data: auto-summary (v2)"). Los detalles de la resolución viven en la git history (commits `cc924a3`, `a3053cd`).

---

---

## Text centralization follow-ups

- **Auto-generated tools index** — replace the hand-written
  `41_builtin_tools_index.md` with a build step that reads
  `text/tools/*.yaml` and writes the markdown. The completeness test
  shipped in E-T18b would become redundant; the build step would be the
  single source of truth.
- **i18n support for tool text** — extend the YAML schema to allow
  language-keyed entries (`summary.en`, `summary.es`) and add a runtime
  language selector. Out of scope today; only English tool text ships.
- **Hot reload for `text/`** — watch the folder for changes and reparse
  YAMLs without restart. Useful for prompt iteration during development;
  complex because the binary embeds the YAML via `include_str!`.

---

---

## Auto-generate `42_builtin_skills_index.md`

- **Origen:** E-T19 shipped (2026-06-06). El index actual se curó manualmente. Oportunidad de automatizar.
- **Problema:** cada vez que se añade un nuevo built-in skill folder con `SKILL.md`, hay que actualizar `docs/developer_guide/42_builtin_skills_index.md` manualmente. El CI test (`index_doc_covers_all_registered_skills`) rechaza el ship si falta un entry, pero no auto-lo-genera.
- **Workaround actual:** developer edita el markdown a mano.
- **Por qué está parqueado:** E-T19 prioriza ship del index con contenido correcto. Auto-gen es optimización, no bloqueante.
- **Fix propuesto:** leer cada `SKILL.md` frontmatter (name + description) y emitir el markdown table. Mismo shape que la deferida auto-gen para `41_builtin_tools_index.md`.
- **Acceptance criteria:**
  - Build step genera `42_builtin_skills_index.md` desde `src/libs/colmena/skills/*/SKILL.md` (8 hoy).
  - CI test (`index_doc_covers_all_registered_skills`) se reduce a "tabla tiene todos los skills" (trivial post-auto-gen).
  - Index es la single source of truth — fallback a manual edit es imposible.
- **Estimación:** ~2-4 horas. Script en bash/python/rust que parsa frontmatter. Alterna como build.rs hook.
- **Cuándo retomar:** cuando se agregue el skill #9, o cuando el chore de update manuals sea frecuente (próx 2-3 meses).
- **Referencias:** E-T17 text centralization, E-T18 tools index, E-T19 skills index.

---

## E-T20 / E-T21 follow-ups (multi-sheet & exploration skills)

- **Format options per output_sheet** — opt-in `header_style`,
  `column_widths`, `freeze_top_row` accompanying each entry in
  `output_sheets`. Today the dispatcher writes raw values only.
- **Diff-aware sheet write** — opt-in arg to overwrite an existing tab
  instead of creating a suffixed one. The current "create-new-with-suffix"
  default is safe; the opt-in makes destructive writes explicit.
- **Auto-naming for unnamed DataFrames** — if a DataFrame appears in
  `output_sheets` under a `None` or numeric key, the dispatcher assigns
  `Untitled (1)`, `Untitled (2)`. Today the script must name every
  entry.
- **Direct hyperlink to new tab in response** — `wrote_sheets[i].url`
  pointing to `https://docs.google.com/spreadsheets/d/<id>/edit#gid=<sheet_id>`
  for quick user navigation.

---

## Cómo agregar un item a este backlog

Cada entrada debe tener:

- **Origen** — de dónde vino la idea (audit, conversación con stakeholder, bug report).
- **Problema** — qué duele actualmente.
- **Workaround actual** — qué tiene que hacer el usuario hoy en lugar de la solución.
- **Por qué está parqueado** — qué pesó más, prioridad o costo.
- **Fix propuesto** — boceto de la solución (1-2 párrafos), suficiente para retomar sin tener que re-pensar todo.
- **Acceptance criteria** — qué define que el fix está completo.
- **Estimación** — orden de magnitud (LOC, días, dependencias nuevas).
- **Cuándo retomar** — un trigger concreto, no "cuando haya tiempo".
- **Referencias** — links a docs/specs/plans existentes.

## gsheets_run_python — UX aliases for bindings + output_sheets

**Trigger**: when re-running the multi-sheet demo and the LLM still
hallucinates field names. Demo on 2026-06-06 showed Gemini-2.5-pro
calling `gsheets_run_python` with `binding_name` instead of `var` and
`sheet_name` instead of `sheet`, and passing `output_sheets` as a tool
arg instead of a code-level variable. Schema is correct, description
now has an explicit example, but Gemini paraphrases the schema and gets
field names wrong.

**Fix**: at the dispatcher, accept synonym field names BEFORE deserializing:
- `var` ↔ `binding_name` ↔ `name`
- `sheet` ↔ `sheet_name`
- Silently drop `output_sheets` if it appears as a tool arg (with a warning that
  it's a code-level variable, not an arg)

Same pattern as E-T6 (UX aliases on other gsheets tool args:
`address` ↔ `addr`, `start` ↔ `start_addr`, `values` ↔ `values_2d`,
`name` ↔ `sheet`).

~30 LOC in `gsheets_run_python.rs` dispatcher's args parser. Add 3-4
wiremock tests verifying each alias maps correctly.

Also: same UX aliases for `crdt_doc_run_python` bindings (currently
`sheet_ids: [String]` — could accept `sheets`, `sheet_names` as
synonyms). Less critical there because the legacy single-sheet path
guides the LLM more.

---

## Sheets write safety v1.1 — `overwrite` mode E2E coverage

- **Origen:** P1+P2 shipped 2026-06-07. Las 3 modos (`replace` /
  `update_in_place` / `overwrite`) tienen unit tests pero solo
  `replace` (collision `fail`) y `update_in_place` se verificaron
  contra Google Sheets real durante el sweep final. `overwrite`
  no se probó live porque haría falta destruir un tab existente
  con datos para verificar el path.
- **Problema:** un bug latente en el branch overwrite (e.g.
  `do_overwrite` con `fetch_tab_meta` error swallowing o
  schema-change guard mal computado) solo aparecería en producción
  cuando un operador explícitamente opt-in a `mode: "overwrite"`
  o `fixed_config.on_existing_sheet: "overwrite"`.
- **Workaround actual:** el unit test `do_overwrite` cubre el path.
  Operadores que activen overwrite deben hacer una prueba manual
  inicial contra un tab desechable antes de wire-up en producción.
- **Por qué está parqueado:** riesgo bajo — el path es estrechamente
  análogo a `replace` (mismo write_full_df helper). El schema-change
  guard tiene unit test específico.
- **Fix propuesto:** crear `tests/graphs/agents/gsheets_overwrite_e2e.json`
  que (a) crea un sheet nuevo via gsheets_create_spreadsheet (requiere
  fix de auth/scopes — ver siguiente entry), (b) seedea data, (c)
  ejecuta `output_sheets = {tab: {mode: "overwrite", df: df2}}` con
  schema compatible, (d) verifica que escribe correctamente, (e)
  ejecuta con schema cambiado sin `allow_schema_change` y verifica
  que devuelve `SchemaChange` error.
- **Cuándo retomar:** la próxima vez que un operador real configure
  `on_existing_sheet: "overwrite"` y reporte comportamiento raro. O
  como parte del próximo sprint de QA gsheets.
- **Referencias:**
  - [Spec](superpowers/specs/2026-06-06-sheets-write-safety-design.md) §1+§3
  - Plan T5 → `do_overwrite` impl: `gsheets_run_python.rs:537-585`

---

## Sheets write safety v1.1 — append / upsert / delete_where modes

- **Origen:** spec sheets-write-safety 2026-06-06 explícitamente excluyó
  estos modos del v1 (sección "Non-goals"). Sólo `replace` /
  `update_in_place` / `overwrite` shipped.
- **Trigger para retomar:** cuando un agente real encuentre un caso de
  uso donde los 3 modos actuales no alcanzan:
  - `append`: pegar N filas nuevas al final de una tab existente sin
    leer/regenerar las anteriores (use case: log incremental, time-series).
  - `upsert`: update si match por `key`, insert si no — un híbrido entre
    `update_in_place` y `append` (use case: sync periódico desde una
    fuente externa).
  - `delete_where`: borrar filas que cumplen una condición (use case:
    purge de records expirados/obsoletos).
- **Estado actual:** un agente que necesita `append` hoy debe (a) leer
  con `gsheets_read`, (b) construir el df nuevo con head appended, (c)
  `output_sheets = {tab: {mode: "overwrite", df: combined}}` con
  `allow_schema_change: false`. Funciona pero re-escribe la tab completa
  — el caso exacto que `append` resolvería con 1 round-trip.
- **Fix propuesto:** extender el postlude para que el spec dict acepte
  `mode: "append" | "upsert" | "delete_where"`, agregar 3 helpers en
  el dispatcher (`do_append` / `do_upsert` / `do_delete_where`) que
  use `batch_update_cells` (append: append rows at end-of-range;
  upsert: hybrid; delete_where: clear cells in matching rows).
  Mirror en `crdt_doc_run_python`.
- **Cuándo retomar:** cuando un caso de uso real lo justifique. Hasta
  entonces, el workaround vía overwrite es aceptable para volúmenes
  bajos.

---

## Sheets write safety v1.1 — surface `last_modified` in SheetExists error

- **Origen:** spec sheets-write-safety §1 mencionaba `last_modified:
  "2026-06-04T10:23:00Z"` en el ejemplo del envelope `SheetExists`.
  El shipped envelope incluye `n_rows`, `n_cols`, `columns` pero
  NO `last_modified` — se difirió porque requiere una llamada extra
  a la Drive API.
- **Problema:** sin `last_modified`, el LLM (y la persona leyendo
  su reporte) no puede distinguir entre "este tab tiene data fresca,
  cuidado" vs "este tab es del año pasado, probablemente sea seguro
  overwrite". Solo ve `n_rows: 4998` que no dice si esas filas son
  recientes.
- **Workaround actual:** el LLM puede llamar `gsheets_list_sheets`
  (que no devuelve `modifiedTime`) o leer el primer/último row para
  inferir la antigüedad de la data. Heurístico y caro.
- **Fix propuesto:** en `fetch_tab_meta`, agregar una llamada Drive
  API `files.get(<spreadsheet_id>, fields=modifiedTime)` y surfacearlo
  en `current_state.last_modified`. Requiere el scope
  `https://www.googleapis.com/auth/drive.metadata.readonly` (la SA ya
  lo tiene). Costo: 1 HTTPS adicional cuando hay colisión — ~80ms.
- **Cuándo retomar:** si los operadores reportan que el LLM toma
  decisiones erradas con frecuencia (e.g. sobrescribe data fresca
  pensando que era vieja).
- **Estimación:** ~30 LOC en `fetch_tab_meta` + 1 unit test mock
  Drive API + actualizar la doc del envelope en
  `sheet_collision.rs::build_sheet_exists_error`. ~15 minutos.

---

## Sheets write safety v1.1 — gsheets_create_spreadsheet permission scope

- **Origen:** durante E2E testing del feature shipped 2026-06-07
  descubrimos que la SA `colmena-sheets-tester@startti-dev.iam.
  gserviceaccount.com` no puede llamar a `gsheets_create_spreadsheet`
  — devuelve `permission_denied`. Tiene scope `sheets` pero no `drive`
  (que es lo que requiere `spreadsheets.create`).
- **Problema:** los E2E graphs que necesitan crear un sheet fresh
  (e.g. el deferred `overwrite` E2E) no son viables con esta SA.
  Tampoco lo es el patrón canónico "agente crea su propia hoja de
  trabajo desde cero".
- **Workaround actual:** el operador crea la planilla manualmente y
  la comparte con la SA antes de cualquier graph que la use. Los
  demos pre-existentes usan este patrón (Products + Sales sheets
  pre-creados por el operador).
- **Fix propuesto:** agregar el scope `drive.file` (no el full `drive`
  — solo crea archivos que la SA dueña) a la SA. O crear una SA
  separada para operaciones de Drive y rotar por feature flag. O,
  alternativa más limpia, documentar que `gsheets_create_spreadsheet`
  requiere un scope adicional y dejar que los operadores lo activen
  por SA.
- **Cuándo retomar:** cuando un usuario o agente real lo requiera.
  Bajo ASAP — bloquea ciertos workflows de auto-bootstrap.

---

## Sheets write safety v1.1 — diff_writer 26-column header limit

- **Origen:** durante E2E del feature shipped 2026-06-07,
  `fetch_tab_meta` y `do_update_in_place` leen el header row vía
  `read_range(..., Some("A1:Z1"), ...)` — hardcodea 26 columnas.
- **Problema:** sheets con más de 26 columnas (eso pasa — el demo
  prior tenía 12 pero algunas hojas reales superan los 50+) van a
  tener el header truncado. El diff funciona OK porque solo compara
  columnas que aparecen en ambos lados, pero la `current_state.columns`
  del envelope `SheetExists` mostrará solo las primeras 26 y el LLM
  decide con info incompleta.
- **Workaround actual:** el LLM puede llamar `gsheets_read` con un
  rango más amplio (e.g. `A1:ZZ1`) manualmente. No automático.
- **Fix propuesto:** cambiar el hardcode `A1:Z1` por un range
  computado: leer `list_sheets` → `meta.col_count` → construir el
  range A1-style hasta esa columna (e.g. col_count=50 → `A1:AX1`).
  Helper `col_index_to_a1(n)` ya existe (es el `a1_addr` con row=1).
- **Cuándo retomar:** la próxima vez que se observe truncado de header
  en logs de producción, o como parte de un sweep general de gsheets
  ergonomics.
- **Estimación:** ~10 LOC en `fetch_tab_meta` + 1 unit test que mockea
  un sheet con 30+ columnas. ~10 minutos.

---

## ~~Lazy tool loading — OpenAI message-order regression al cerrar el turn~~ — RESUELTO 2026-06-07

**Root cause real (encontrado 2026-06-07):** NO era un problema del OpenAI adapter ni del flujo synthetic-tool de colmena. El bug estaba en **`compact_history_to_summary`** en [`src/libs/colmena/src/llm/application/agent_service.rs:745`](../src/libs/colmena/src/llm/application/agent_service.rs:745). La compactación dividía la historia en `keep_first / middle / keep_recent` para optimizar tokens, pero la frontera entre `middle` (que se reemplaza por un summary) y `kept_recent` (que se mantiene verbatim) **podía caer dentro de una secuencia `{assistant.tool_calls, tool, tool, ...}`**. Como resultado, el `assistant` quedaba en `middle` (summarizado y descartado), pero sus `tool` responses quedaban en `kept_recent` — **tool messages huérfanos** sin assistant.tool_calls precedente, lo que OpenAI rechaza con `'messages with role 'tool' must be a response to a preceding message with 'tool_calls''`.

El issue es ÉL MISMO que motiva [el hallazgo #3 (OpenAI Responses API `input_text` invalid en synthetic-tool path)](#openai-responses-api--input_text-invalid-en-synthetic-tool-path). El shape malformado golpea ambos endpoints OpenAI (Chat Completions + Responses) con errores distintos pero misma raíz.

**Fix shipped:** en `compact_history_to_summary`, antes de slicear, walk `middle_end` backwards while `messages[middle_end].role() == Tool`. Esto pulls todas las tool messages contiguas Y su `assistant.tool_calls` precedente al `kept_recent`, preservando el par invariante requerido por OpenAI Chat Completions y Responses API.

```rust
// Before slicing, ensure the boundary doesn't fall mid-tool-sequence.
let mut middle_end = initial_middle_end;
while middle_end > keep_first && matches!(messages[middle_end].role(), MessageRole::Tool) {
    middle_end -= 1;
}
```

**Test regresivo agregado** (`summary_never_orphans_tool_message_after_compaction`): reproduce el escenario exacto del E2E Phase 1.2 — assistant con 5 parallel tool_calls + 5 tool responses, keep_first=2, keep_recent=5. Sin el fix, deja 5 tool messages huérfanos en kept_recent. Con el fix, el assistant se pulla al kept_recent y la invariante se mantiene. Test escanea cada Tool message en el output y verifica que tenga un Assistant con tool_calls precedente. Suite full pasa (1467 tests, 0 failures).

**Por qué la hipótesis original era incorrecta:** miré el código de `describe_tool` (synthetic tool en `dag_engine/infrastructure/dag_tool_executor.rs:633`) y confirmé que se ejecuta como tool normal — produce un par `{assistant.tool_calls, tool}` natural. El bug NO es en la inyección, sino en la compactación posterior. Misma compactación afecta a Gemini pero Gemini es más permisivo y no rechaza, solo degrada en calidad de respuesta (la pareja rota se ignora).

**Conservado para referencia histórica:**
- **Origen:** E2E verification del worker desplegado (Cloud Run dev) — Phase 1.2 del runbook `verifying_deployed_worker.md` (2026-06-07). El graph `lazy_tools` con `provider: openai`, `lazy_tool_loading: true`, 3 tools (`current_time` eager, `add` y `multiply` lazy) ejecutó las 5 tool calls correctamente: `current_time() → describe_tool(multiply) → describe_tool(add) → add(25,17)=42 → multiply(42,3)=126`. Cinco `tool-output-available` events sin error. Pero al cerrar el turn OpenAI rechazó la historia con `invalid_request_error: messages.[2].role`.
- **Síntoma observado:** error del provider `'messages with role 'tool' must be a response to a preceding message with 'tool_calls'. param: messages.[2].role'`.
- **Hipótesis original (descartada):** synthetic tools de colmena (`describe_tool`, `load_attachment`) inyectaban tool messages sin assistant.tool_calls precedente. Lectura del código confirmó que NO — el flujo es correcto. El bug emergía sólo después de compactación.
- **Referencias originales:** SSE `/tmp/colmena_e2e/1.2_lazy_tools.sse` (efímero); Runbook E2E Phase 1.2.

---

## ~~Conversational memory cross-node — verificar política y documentar~~ — RESUELTO 2026-06-07 (BY DESIGN)

**Verdict (2026-06-07):** El aislamiento por nodo es **intencional, no regresión**. La llave compuesta `(agent_session_id, node_id)` se introdujo deliberadamente en abril 2026 para fixear el **silent `llm_node_history` collision** que ocurría cuando 2+ `llm_call` nodes compartían un mismo agent y se pisaban las histories. La migración [`20260428000002_llm_history_agent_and_node.sql`](../src/libs/colmena/migrations/postgres/20260428000002_llm_history_agent_and_node.sql) agregó `node_id` como segunda mitad de la PK justamente para eliminar ese bug.

**Evidencia clave (Explore agent + lectura directa del código):**
- `ConversationKey { session_id, agent_session_id, node_id }` en [`src/libs/colmena/src/llm/domain/memory.rs:19-46`](../src/libs/colmena/src/llm/domain/memory.rs:19-46).
- `WHERE agent_session_id = $1 AND node_id = $2` en [`postgres_conversation_repository.rs:26`](../src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs:26).
- Spec del fix original (donde "collision" se nombra explícitamente como el bug que se arregla): [`docs/superpowers/plans/2026-04-28-agent-session-id.md`](superpowers/plans/2026-04-28-agent-session-id.md) §3.2.

**Resolución implementada:**
1. **Doc**: nueva sección "🧱 Aislamiento por nodo — memoria NO compartida entre llm_call distintos" en [`docs/developer_guide/15_memory_guide.md`](developer_guide/15_memory_guide.md) explicando:
   - Por qué la memoria está aislada (historia del collision bug)
   - El comportamiento observable (prompt_tokens bajo en step_2 confirma que no carga history de step_1)
   - 3 opciones para compartir información entre nodos: **edge data-flow** (recomendado), reutilizar el mismo `node_id` en runs sucesivos, o el patrón **orchestrator/planner**
   - 2 anti-patterns con sus consecuencias
2. **HTML playground**: comment del preset `gemini_orchestrator` actualizado para reflejar que NO testea memoria cross-node (porque no la puede testear) — el preset valida orquestación de step_1 → step_2 vía data-flow, no memoria persistente compartida.

**Conservado para referencia histórica:**
- **Origen:** E2E verification del worker desplegado — Phase 1.4 del runbook `verifying_deployed_worker.md` (2026-06-07). El graph `gemini_orchestrator` define dos `llm_call` nodes con el mismo `agent_session_id` y un edge `step_1 → step_2`. step_1 dijo "Hello Daniel! 123 + 456 = 579. I will remember your name." step_2 respondió "Hello! It's nice to meet you" con `prompt_tokens=311` — confirma que NO cargó la history de step_1.
- **Síntoma observable:** `step_2.extra_info.usage.prompt_tokens=311` (sólo system + prompt; con history cargada esperaríamos ~700+ tokens).
- **Referencias originales:** SSE `/tmp/colmena_e2e/1.4_gemini_orch_v2.sse` (efímero); Runbook E2E Phase 1.4.

---

## ~~OpenAI Responses API — `input_text` invalid en synthetic-tool path~~ — RESUELTO 2026-06-07 (bug independiente, fix en PR #92)

**Corrección importante (2026-06-07, post-redeploy verification):** la nota inicial de este entry decía que este bug compartía root cause con el de Chat Completions y se resolvía con el fix de compactación. **Eso fue incorrecto.** Al re-correr Phase 3.1 contra el worker desplegado con el fix de compaction (PR #91), el error `'Invalid value: input_text'` reapareció con sólo 5 mensajes en la historia — por debajo del threshold de compactación. El bug era **separado**, en `build_responses_request_body` de [`openai_adapter.rs`](../src/libs/colmena/src/llm/infrastructure/openai_adapter.rs), con 2 problemas distintos:

1. **Content type hardcoded `input_text` para TODO role.** OpenAI Responses API requiere `output_text` para mensajes de assistant; el adapter mandaba `input_text` y la API rechazaba.
2. **Tool calls y tool responses serializados como mensajes** `{role, content}`. La Responses API los espera como items top-level distintos: `{type: "function_call", call_id, name, arguments}` para tool calls del assistant, y `{type: "function_call_output", call_id, output}` para tool responses. El código viejo los aplanaba como mensajes y, además, perdía los `tool_calls` del assistant entirely.

**Fix shipped en PR [#92](https://github.com/Startti/colmena/pull/92):** rewrite del dispatch en `build_responses_request_body`:

| LlmMessage role | Output Responses API |
|---|---|
| System / User | `{role, content: [{type:"input_text",text}, ...files]}` |
| Assistant con tool_calls | (opcional `output_text` si hay texto) + un `{type:"function_call", call_id, name, arguments}` por cada tool call |
| Assistant solo con texto | `{role:"assistant", content: [{type:"output_text",text}]}` |
| Tool | `{type:"function_call_output", call_id, output}` (no role) |

**4 unit tests agregados** en `openai_adapter.rs::tests`:
- `responses_serializes_assistant_text_as_output_text`
- `responses_serializes_assistant_tool_calls_as_function_call_entries`
- `responses_serializes_tool_response_as_function_call_output`
- `responses_serializes_full_load_attachment_sequence_correctly` — **regression test** que reproduce el escenario exacto de Phase 3.1 (system + user con PDF inline → assistant llama `load_attachment` → tool ack → synthetic user con file content) y valida que el shape JSON enviado a OpenAI Responses API es correcto. Incluye los invariantes: **no item con `assistant+input_text`, y no Tool role escapando como mensaje top-level**.

**E2E verification (post-redeploy 2026-06-07):**
- Phase 3.1 `pdf_analyst_base64` con OpenAI Responses API + `load_attachment` + Plan B catalog-driven: **PASS contra el deployed worker**. Con un PDF real de Avianca (12 KB, "Delayed Baggage Report"), el LLM extrajo datos verbatim: Report Number `MTYAV40108581`, Customer Name `GARCIA, DANIEL GUILLERMO`, fecha `May 13, 2026, 14:08`, código de reserva `AJTQ6V`, teléfono `3153678626`. SSE evidencia: `/tmp/colmena_e2e/PDF_real.sse` (efímero).

**Conservado para referencia histórica:**
- **Origen:** E2E verification — Phase 3.1 (`pdf_analyst_base64`) y Phase 4.1 (`sql_products_readonly`) del runbook `verifying_deployed_worker.md` (2026-06-07).
- **Síntoma observado:**
  ```
  Invalid value: 'input_text'. Supported values are: 'output_text' and 'refusal'.
  param: input[2].content[0]
  ```
- **Hipótesis original (descartada):** la primera teoría fue que el bug compartía root cause con el de Chat Completions (compaction). Re-verificación post-redeploy desmintió esa hipótesis: el bug persistía incluso sin compaction. La causa real era el shape incorrecto del Responses API en el adapter.
- **Referencias originales:** SSE `/tmp/colmena_e2e/3.1_pdf.sse` y `/tmp/colmena_e2e/4.1_sql_readonly.sse` (efímeros); OpenAI Responses API docs: <https://platform.openai.com/docs/api-reference/responses>.

---

## ~~`document_id` collision entre image_generation providers~~ — RESUELTO 2026-06-07

**Root cause (encontrado 2026-06-07):** la implementación de `build_document_id` en [`src/libs/colmena/src/dag_engine/infrastructure/nodes/util/attachment_id.rs`](../src/libs/colmena/src/dag_engine/infrastructure/nodes/util/attachment_id.rs) usaba `storage_key_suffix(storage_key)` = "los últimos 6 chars alfanuméricos del storage_key" como suffix del ID. Cuando los storage_keys terminaban con la extensión del archivo (ej. `chat-attachments/sess_xyz/image_0.png`), los últimos 6 chars alfanuméricos eran dominados por la parte fija del filename (`image0png` → `ge0png`), no por la parte única del storage_key. Dos providers con mismo filename (`image_0.png`) producían siempre el mismo suffix `ge0png` → mismo `document_id` → segundo INSERT pisaba el primero en `conversation_attachments`.

**Fix shipped:** reemplazado el `storage_key_suffix(storage_key)` por `Uuid::new_v4().simple().to_string()[..8]` (8 chars hex). Garantiza unicidad incluso con argumentos idénticos (mismo filename + mismo storage_key + mismo prompt). El argumento `storage_key` queda en la signature por backward compat pero pasa a ser unused (`_storage_key`).

**Tests agregados:**
- `strips_known_extensions_and_appends_uuid_suffix` — valida shape correcto + suffix hex de 8 chars
- `avoids_collision_between_same_filename_and_same_storage_key` — **test regresivo** que reproduce el escenario exacto del bug y verifica que ahora produce IDs distintos
- `avoids_collision_across_many_rapid_calls` — stress test: 1000 generaciones con mismos args, todos los IDs deben ser únicos
- Tests originales (`avoids_collision_between_same_filename_different_keys`, `sanitize_trims_trailing_underscores`, `file_ext_handles_image_and_audio`) siguen pasando

**Decisión deliberada — NO se agregó unique constraint en Prisma:** el fix en colmena (IDs únicos por construcción) resuelve el problema en la fuente. Un unique constraint defensivo en `conversation_attachments(agent_session_id, document_id)` quedaría como belt-and-suspenders pero requiere migration en ADP. Si en el futuro alguien quiere esa capa extra de seguridad, abrir nueva entrada BACKLOG. Por ahora: trust the fix.

**Conservado para referencia histórica:**
- **Síntoma observado:** E2E Phase 2 (Vertex Imagen 4) + Phase 2.3 (OpenAI gpt-image-1) corrieron secuencialmente con el mismo `agent_session_id`. Ambos devolvieron `document_id: "img_image_0_ge0png"` — idénticos. El segundo upsert sobrescribió la fila del primero en `conversation_attachments`.
- **Impacto operacional medido:** rare en producción (usuarios típicamente esperan ver una imagen antes de pedir otra), pero data loss real cuando ocurre. El blob en GCS sigue existiendo, pero la fila DB pierde el link a la primera generación.
- **Referencias originales:** SSE `/tmp/colmena_e2e/2.1_image_gen_v2.sse` y `/tmp/colmena_e2e/2.3_image_gen_openai_v2.sse` (efímeros); Runbook E2E Phase 2.

---

## ~~HTML preset `image_gen_then_edit_openai_dev` roto post Plan B (2026-05-25)~~ — RESUELTO 2026-06-07

**Resolución (2026-06-07, ADP commit en `feat/fix-image-gen-edit-preset`):** Reemplazado en ADP `test_stream_cloud.html` por el preset `image_edit_from_web_url` que valida `image_edit` end-to-end usando una URL pública estable de Wikimedia como `source_url`, sin intentar chain desde `generate_image`. Adicionalmente las descripciones de `generate_image`, `speak_text` y `edit_image` del preset `multimedia_agent_dev` se corrigieron para reflejar el shape real post Plan B (solo `document_id`, sin `url` ni `attachment_id`) y advierten explícitamente que `edit_image.source_url` no acepta document_ids ni `$attachment:` placeholders.

**Trabajo futuro relacionado (no parqueado acá, abrir nueva entrada cuando se necesite):** si se requiere demostrar chain `generate_image → edit_image` end-to-end en una demo, hay 2 caminos:
- **Camino A — Extender `image_edit.fetch_image()`** en colmena para que acepte `$attachment:<document_id>` y resuelva via attachment registry. Requiere cambios en Rust + tests. ~2-4h.
- **Camino B — Preset HTML con `resolve_attachment_url` tool** vía `http_request` a `/api/attachments/<id>/url` con cookie auth. Solo cambia el playground, no toca Rust. ~1h. Requiere ADP_SESSION_TOKEN en el panel de credenciales.

Conservado para referencia histórica:
- **Origen:** E2E verification — Phase 2.4 del runbook `verifying_deployed_worker.md` (2026-06-07). El preset HTML definía un chain `gen → edit` con el edge `gen.output.images.0.url → edit.source_url`. Pero la migración Plan B shipped el 2026-05-25 eliminó el field `url` del payload de respuesta de `image_generation` y `image_edit` — solo queda `document_id`. El edge quedaba apuntando a un field inexistente.
- **Síntoma observado:** el trigger corría pero los nodos `gen` y `edit` NO se ejecutaban porque la resolución del edge daba `null` y el DAG bloqueaba silenciosamente. El run terminaba con un `finish` que solo contenía `trigger: {}` — sin error event, sin warning.
- **Referencias originales:** SSE `/tmp/colmena_e2e/2.4_gen_then_edit.sse` (efímero); Plan B migration: [`docs/superpowers/specs/2026-05-25-plan-b-adp-migration-notes.md`](superpowers/specs/2026-05-25-plan-b-adp-migration-notes.md).

---

## ~~CRÍTICO — `gsheets_run_python` y `crdt_doc_run_python` rotos en dev: pandas no instalado en worker image~~ — RESUELTO 2026-06-07

**Fix shipped:** ADP commit `ee08598e` ("fix(worker): install pandas/numpy/scipy
for gsheets_run_python runtime", 2026-06-07 16:58 -0500, mergeado en `develop`).
El `Dockerfile` del worker (`apps/service/ia/platform/worker/Dockerfile:31-43`)
ahora hace `apt-get install -y --no-install-recommends python3 libpython3.11
python3-pandas python3-numpy python3-scipy` con un bloque de comentario
explicando por qué (Debian Bookworm ships pandas 1.5.3 / numpy 1.24 / scipy 1.10,
compatibles con el contrato read-records / write-records del dispatcher).

**Verificación pendiente:** re-correr Phase 6.B1-B5 del runbook E2E contra el
worker dev re-deployado para confirmar que el fix llegó a Cloud Run. Si la
imagen actual en `colmena-worker` (us-central1) fue built post-`ee08598e`, todos
los tests deberían pasar verde.

**Conservado para referencia histórica:**


- **Origen:** E2E verification — Phase 6 (sheets-write-safety) del runbook `verifying_deployed_worker.md` (2026-06-07). Intento de ejecutar B1 (`gsheets_create_new`) contra el worker dev en Cloud Run. El dispatcher cargó las bindings correctamente desde el sheet real (1F7AsFx4yW4uVnJRaRWwpzQuSNvruqGohI2B2NRygT-Y), confirmó permisos del SA, pero falla en la ejecución del Python con `ModuleNotFoundError: No module named 'pandas'`.
- **Problema:** la imagen Docker del worker desplegado en Cloud Run dev **no tiene pandas instalado**. El sandbox lo lista como import permitido (`Allowed imports: collections, datetime, decimal, functools, itertools, json, math, numpy, pandas, re, scipy, statistics, string`) — pero el runtime real no lo tiene. La causa probable es que el dispatcher pre-importa `pandas as pd` antes del código del usuario (es uno de los globals que documenta como disponible: "Has access to `pandas as pd`, `numpy as np`, `scipy.stats as stats`"). Ese pre-import falla con ModuleNotFoundError, abortando el run antes de que el código del usuario ejecute. Confirmado: incluso un script que solo importa numpy falla porque la inicialización de pandas pasa primero.
- **Bloqueante:** sí. Bloquea TODOS los tests de Phase 6 (sheets-write-safety) y cualquier uso de `gsheets_run_python` o `crdt_doc_run_python` contra el worker dev. Es el feature más reciente shipped al worker (2026-06-07 / PR #86) y aparentemente la imagen Docker no incluye la dependencia.
- **Hipótesis técnica:**
  - El `Dockerfile.deps` del worker probablemente no instala pandas+scipy+numpy. Quizás el dispatcher requiere una imagen Python especializada distinta a la del worker normal.
  - O hay un `pip install pandas` que está en el Cargo.toml de colmena pero no se incluyó al construir la imagen Docker del worker.
  - O la imagen base cambió y el `apt-get install python3-pandas` quedó stale.
- **Verificación rápida:** correr cualquier graph que use `gsheets_run_python` o `crdt_doc_run_python`. Si el output tiene `error: "Python execution error: ModuleNotFoundError: No module named 'pandas'"`, el bug está activo.
- **Workaround actual:** ninguno desde el lado del agente — el dispatcher exige pandas. Los operadores que necesitan pandas-based gsheets deben usar `cargo run` local (donde el `.venv` sí tiene pandas) en vez del worker desplegado.
- **Por qué urgente:** sheets-write-safety es un feature recién shipped (2026-06-07 colmena develop merge PR #86). El BACKLOG ya tiene varias entradas de mejoras incrementales (v1.1) que asumen el feature funciona — pero el feature está roto en dev y nadie se dio cuenta porque no hay E2E automatizado. Riesgo grande de que llegue así a prod si nadie lo testea antes.
- **Fix propuesto:** dos pasos:
  1. Identificar qué imagen Docker usa Cloud Run para `colmena-worker`. Probable: `apps/service/ia/platform/Dockerfile.deps` o un Dockerfile relacionado. Verificar si tiene `RUN pip install pandas numpy scipy` o `RUN apt-get install -y python3-pandas`. Si NO lo tiene, agregar.
  2. Rebuild + redeploy del worker. Re-correr Phase 6 (B1-B5) para confirmar que pandas ahora carga.
- **Acceptance criteria:**
  - Phase 6.B1 (`gsheets_create_new`) corre verde: nueva tab creada con 3 filas via pandas.
  - Phase 6.B2 (`gsheets_collision_fail`) devuelve el envelope `SheetExists` correctamente.
  - Phase 6.B3 (`gsheets_collision_auto_suffix`) crea `Sheet1 (N)`.
  - Phase 6.B4 (`gsheets_update_in_place`) reporta cells_changed > 0 y solo escribe celdas con diff.
  - Phase 6.B5 (`gsheets_multi_sheet`) crea 3 nuevas tabs en una llamada.
  - Un smoke test que importa pandas y reporta versión devuelve éxito desde el worker dev.
- **Estimación:** ~1-2 horas para encontrar el Dockerfile, agregar la dep, rebuild, redeploy, re-validar. El fix en sí es de 1-2 líneas.
- **Cuándo retomar:** ASAP. Bloquea un feature recién shipped y un capítulo entero del E2E runbook. Posible trigger: cuando alguien (operador o cliente) intente usar gsheets-pandas en dev y vea que rompe.
- **Referencias:**
  - SSE evidencia: `/tmp/colmena_e2e/B1.sse`, `/tmp/colmena_e2e/B2.sse`, `/tmp/colmena_e2e/B3.sse`, `/tmp/colmena_e2e/sanity_numpy.sse` (efímeros).
  - Tool output ejemplo:
    ```json
    {"output":null,"stdout":"","error":"Python execution error: ModuleNotFoundError: No module named 'pandas'","loaded_columns":{"products":["product_id","sku","name","category","cost","price","stock"]}}
    ```
    Nota: `loaded_columns` confirma que bindings + permisos del SA funcionan. La falla es 100% del runtime Python.
  - Spec del feature: [`docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`](superpowers/specs/2026-06-06-sheets-write-safety-design.md).
  - Documentación del dispatcher: [`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`](src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs) — el doc comment al inicio del struct `GsheetsRunPythonArgs.code` dice "Has access to `pandas as pd`, `numpy as np`, `scipy.stats as stats`".
  - Worker Dockerfile en ADP: `/Users/danielgarcia/startti/adp/apps/service/ia/platform/Dockerfile.deps` (a verificar).
  - Cloud Run service: `colmena-worker` en `us-central1`, SA `adp-backend-sa-develop@startti-dev.iam.gserviceaccount.com`.
  - Runbook E2E: Phase 6 en [`verifying_deployed_worker.md`](#phase-6).
