# Backlog — Future work / parked items

> **Propósito:** Listar features identificados, especificados o solicitados que **no están en el roadmap activo**. Cada entrada tiene un trigger explícito ("¿cuándo retomamos esto?") para evitar que algo se quede olvidado o se construya prematuramente.

Si vas a empezar a trabajar en algo de acá, sacalo de esta lista y agregalo al changelog del mes correspondiente. Si descartás definitivamente un item, marcalo `~~tachado~~` y dejá una nota explicando por qué.

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

## Items resueltos recientemente

El último item — `data:` (base64 inline) auto-summary v2 — se resolvió el 2026-05-18 (ver `docs/CHANGELOG_2026-05.md` → "Inline data: auto-summary (v2)"). Los detalles de la resolución viven en la git history (commits `cc924a3`, `a3053cd`).

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
