
# Cambios recientes — 2026-07

> **Alcance:** Commits sobre `feat/data-run-python` (y ramas siguientes) desde el cierre de `2026-06` hasta el merge eventual a `develop`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. `data_run_python` — soft-deprecation de `gsheets_run_python` + `attachment_run_python`

**Qué cambió.** `data_run_python` pasa a ser el **tool tabular primario** para leer, computar y escribir datos tabulares (attachments CSV/XLSX, Google Sheets, SQL SELECT, inline → pandas sandbox → write-back a `output_tables`/`output_sheets`/`output_attachments`). Los dos tools específicos previos, `gsheets_run_python` y `attachment_run_python`, quedan **soft-deprecados**:

- Ambos **siguen funcionando** y se mantienen registrados por compatibilidad con grafos persistidos (en la DB de ADP) que los nombran. `gsheets_run_python` permanece en el alias `gsheets` durante el bridge.
- Sus descripciones llevan ahora un prefijo `DEPRECATED — usá data_run_python`.
- Las 11 skills de gsheets instruyen al modelo a llamar `data_run_python`.
- El alias del toolkit `gsheets` ahora **incluye** `data_run_python` (`enabled_tools: ["gsheets"]` lo expone). Vía alias, la capacidad gsheets se auto-detecta; la capacidad SQL sigue requiriendo `fixed_config.sql`.
- `sql_inspect_attachment` y `sql_bulk_insert_from_attachment` **no** están deprecados (el volcado 1:1 crudo de un CSV vía COPY sigue siendo su dominio).

**Por qué importa.** Convierte lo que iba a ser un breaking-change-con-riesgo-externo (borrado duro que rompería skills, el alias, y grafos persistidos en ADP) en un cambio **aditivo y reversible, sin reroute de motor**. El código de los dos tools viejos no se toca (cero riesgo de comportamiento); solo se reorienta guía/skills/docs/alias hacia `data_run_python` y se marcan las viejas como deprecadas.

**Borrado real diferido (Fase 2, gated).** La eliminación del código de `gsheets_run_python`/`attachment_run_python` queda diferida a una Fase 2 gated en telemetría (~0 llamadas) + verificación de que no hay grafos persistidos en la DB de ADP usando esos `node_type` (o migración hecha). ADP no afectado en Fase 1 (sin cambio de API pública).

**Documentación de referencia.**
- Plan: [`docs/superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md`](superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md)
- Spec original: [`docs/superpowers/specs/2026-07-01-data-run-python-design.md`](superpowers/specs/2026-07-01-data-run-python-design.md)
- Dev guide: [`docs/developer_guide/48_data_run_python.md`](developer_guide/48_data_run_python.md) (primario), [`§39`](developer_guide/39_gsheets.md), [`§23`](developer_guide/23_sql_node.md)
- Índices: [`§41 builtin tools`](developer_guide/41_builtin_tools_index.md), [`§42 builtin skills`](developer_guide/42_builtin_skills_index.md)
- Schema: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json) (`gsheets_run_python` marcado `deprecated`)

**Estado.** done (Fase 1 — soft-deprecation). Fase 2 (borrado) pendiente y gated.

---

## 2. `socketio_request` — transporte default `websocket` + log de disconnect fallido

**Qué cambió.** El nodo `socketio_request` ahora conecta por **WebSocket por default** (`transport: "websocket"`); antes el default era `"any"` (HTTP long-polling primero + upgrade). Además, un `disconnect()` fallido ya no se descarta en silencio — se loggea `[SocketIoNode] ⚠ disconnect failed: …` (2 call sites: cierre normal y aborto por pre-event fallido).

**Por qué importa.** Origen: handoff de ADP 2026-07-04. El gateway `canvas` de ADP corre en Cloud Run con hasta 10 instancias **sin session affinity**; con polling-first, cada request HTTP de una misma sesión Engine.IO puede aterrizar en otra instancia que no la conoce → `⚠ server error event: EngineIO Error` recurrente (~cada 4-5 s) en los logs del worker. Con websocket-only hay una sola conexión persistente y el problema desaparece. Bonus: este cambio es prerequisito para que ADP pueda activar `CANVAS_TRANSPORT_WS_ONLY=true` (con polling-first activo, ese flag rechazaría el handshake). El log del disconnect da visibilidad a la conexión zombi que quedaba latiendo errores cuando el paquete DISCONNECT de polling aterrizaba en la instancia equivocada.

**Semántica sin cambio.** Emit + ack / wait_event / pre_events / timeouts / envelope de salida — idénticos en ambos transportes. Los operadores que necesiten polling-first pueden fijar `transport: "any"` explícitamente (opt-out de 1 línea). Ningún grafo del repo dependía del default `"any"`.

**Documentación de referencia.**
- Dev guide: [`docs/developer_guide/21_socketio_node.md`](developer_guide/21_socketio_node.md) (tabla de config + nueva sección de troubleshooting "Recurring EngineIO Error")
- Schema: [`docs/node_configurations.json`](node_configurations.json) (`transport.default: "websocket"`)
- Tools reference: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json) (`socketio_request.special_behaviors` + `transport` fijado en los ejemplos de node_schema)

**Estado.** done.

---

## 3. `socketio_request` — visibilidad de errores de transporte para el LLM + mute de conexiones zombi

**Qué cambió.** (1) Cuando una operación falla, el envelope de error ahora incluye `transport_errors` (errores de transporte capturados durante ESA operación, agregados — p.ej. `"EngineIO Error (x4)"`) y `advice` (guía accionable para el LLM: la conexión está inestable, reintentar no ayuda, informar al usuario). El envelope ya viajaba al LLM (tool message) y por SSE (`tool-output-available`), así que el modelo ahora puede distinguir "server lento" de "conexión rota" sin ningún cambio en el executor ni en ADP. (2) Todos los handlers de la conexión se gatean con un flag `active` que se apaga al desconectar: las conexiones zombi que `rust_socketio 0.6` filtra tras un disconnect incompleto (su task de fondo sigue vivo consumiendo el stream) ya no pueden imprimir `EngineIO Error` infinitos en los logs del worker.

**Por qué importa.** Follow-up del incidente ADP 2026-07-04 (PR #145): websocket-only eliminó la causa polling/stickiness, pero los logs del worker (revision 00083) mostraron que el ruido residual viene del task de fondo del crate que no muere tras `disconnect()`. En éxito el buffer se descarta (no se alarma al modelo por ruido irrelevante).

**Documentación de referencia.**
- Plan: [`docs/superpowers/plans/2026-07-05-socketio-transport-error-visibility.md`](superpowers/plans/2026-07-05-socketio-transport-error-visibility.md)
- Dev guide: [`docs/developer_guide/21_socketio_node.md`](developer_guide/21_socketio_node.md)
- Tools reference: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json)

**Estado.** done.

---

## 4. Visibilidad total anidada + campos `level`/`path` + red de seguridad de liveness (dos relojes)

**Qué cambió.** El stream SSE ahora forwardea la actividad de sub-agentes anidados a **cualquier profundidad** (subgraph-as-tool → orchestrator → sub-agentes hijos → sus tools), y cada frame lleva dos campos nuevos **aditivos**: `level` (profundidad de anidamiento; `0` = agente principal) y `path` (linaje `padre>…>nodo`).

- **Drop de niveles profundos (Fase A/B).** `run_use_case.rs` **aplana** el anidamiento: en vez de crear `SubgraphWrapped { SubgraphWrapped { … } }` (que el mapper solo desenvolvía un nivel → `_ => None` → **descartado**), propaga un único `SubgraphWrapped` incrementando `depth` y prefijando el `node_id` al `path`. `SubgraphWrapped` ahora lleva `depth: u32` + `path: String` (serde defaults 1/""). El `sse_mapper` desenvuelve a cualquier profundidad (`deep_base`), acumula `depth` → `level` (`level_and_path`) e inyecta `level`/`path` en **todo** frame (nivel 0 → `level:0`, `path = node_id`).
- **Fronteras subgraph-as-tool (Fase F).** `subgraph.rs` emite `subgraph-node-start`/`-end` también cuando el subgrafo se invoca como tool (sin `__agent_name`), usando el `__node_id` como nombre — antes solo los agentes de orchestrator delimitaban.
- **Bordes de turno (Fase C).** `LlmMessageStart`/`Finish` (antes `None`) → frame ligero `agent-turn` (Some, **nunca** `finish`/`error`). `ToolDescribed` anidado ahora visible.
- **Liveness dos relojes (Fase E).** Se separa el `last_activity` único en `last_forwarded` (avanza solo con eventos de contenido/progreso → gobierna el heartbeat) y `last_any` (avanza con cualquier evento → gobierna el idle-abort). Cierra el **falso `Stream timeout`**: un sub-agente que solo emitía bordes cada pocos segundos mantenía vivo el idle-abort pero seguía suprimiendo el heartbeat sin XADDear → el watchdog de 60 s del API mataba el stream. Clasificador `DagExecutionEvent::advances_heartbeat_clock()`.
- **Stream default (Fase D).** Auditoría: los tres paths (directo, agente, orchestrator) ya streamean por defecto; solo `stream:false` explícito desactiva. Extraído `LlmNode::resolve_stream_enabled` para dejarlo testeable.

**Por qué importa.** Reproducido empíricamente contra `colmena-api` dev: 22 s de stream mudo con un orchestrator embebido (en el creador real >60 s → falso `Stream timeout`), y niveles ≥2 completamente invisibles. Ahora ADP puede renderizar el árbol anidado (indentación/breadcrumbs por `level`/`path`).

**Contrato / ADP.** 100% **aditivo**: los `type` existentes NO cambian; `level`/`path` son campos nuevos opcionales. ADP viejo los ignora (`default: return state`); ADP nuevo los aprovecha. Sin cambio de API pública Rust → worker ADP no afectado. Bindings Python/TS no tocados (cambio interno del JSON del stream).

**Regresión.** mapper: `SubgraphWrapped` doblemente anidado → `subgraph-text-delta` con `level:2` (antes `[]`). liveness: node que solo emite bordes envueltos → **sí** heartbeats y **no** idle-abort.

**Documentación de referencia.**
- Plan: [`docs/superpowers/plans/2026-07-05-nested-visibility-liveness.md`](superpowers/plans/2026-07-05-nested-visibility-liveness.md)
- Contrato de campos: [`docs/SPEC_NESTED_VISIBILITY_SSE_FIELDS.md`](SPEC_NESTED_VISIBILITY_SSE_FIELDS.md)
- Liveness previo (#144): `SPEC_STREAM_MIDRUN_LIVENESS.md` (ADP)

**Estado.** done (Fases A/B/C/D/E/F). Pendiente: E2E manual contra el creador real desde el frontend ADP.

---

## 5. `for_each` — ejecución determinista de tools sobre una lista

**Qué cambió.** Nuevo nodo `for_each`: ejecuta un tool embebido (`target: { node_type, node_schema? }`) una vez por cada fila de una lista, de forma **determinista** (la iteración ocurre en Rust vía `ListToolExecutor`/`run_list`, no por el LLM re-llamando el mismo tool N veces dentro de su loop). Es **un solo nodo, dos formas de uso**: como nodo de grafo (`config` estático) o como tool de un `llm_call` (`tool_configurations` + `node_schema`, con `target`/políticas `fixed` y `items` LLM-visible).

- Lista de filas: `items` (array inline) → `items_from` (`source: "sheet"` en v1, con selección/renombre de columna vía `column`/`as`) → edge de entrada por defecto (solo nodo de grafo). `items_from: { source: "attachment" }` queda **diferido a v1.1** — un `ExecutableNode` no puede resolver `document_id → bytes` hoy sin inyección adicional.
- Políticas: `on_error` (`continue`/`abort`, default `continue`), `concurrency` (default 1), `max_items` (default 1000, trunca con warning).
- Cada fila se fusiona en el `node_schema` del target con el mismo `merge_args_into_schema` que usa un tool LLM normal; campos `required` del target se validan por fila antes de despachar (falla por fila, no aborta el batch salvo `on_error: "abort"`).
- Resultado: `{ output: { total, ok, err, results: [{ index, input, status, output|error }] } }`, un entry por fila en orden original.
- Eventos SSE: `batch-progress` (snapshot agregado, al inicio y al final) y `batch-item-finished` (uno por fila). Cuando `target: subgraph`, los eventos internos del sub-agente se propagan con `level`/`path` incrementados (reusa la visibilidad anidada del §4).
- HITL fail-closed: una suspensión dentro de una fila se reporta como error de esa fila — `for_each` no soporta pausar el fan-out a mitad de camino. Guard anti-recursión: `target.node_type == "for_each"` se rechaza.
- Config-first/inputs-fallback (`cfg_or_input`, mismo patrón que `suspend`) — el mismo código sirve sin cambios tanto al path de nodo-de-grafo (`config` estático) como al path de tool LLM (`DagToolExecutor` pliega todo en `inputs`, `config = {}`).

**Por qué importa.** Elimina el patrón "el LLM re-llama el mismo tool 20 veces en su loop" para operaciones tipo batch (actualizar N usuarios, procesar N filas de un sheet) — más rápido, más barato en tokens, y determinista (no depende de que el modelo no se salte/repita filas).

**Verificado en vivo (Gemini 2.5 Flash).** Grafo de nodo (`tests/graphs/basic/for_each_node.json`, target `add`, sin LLM): 2/2 filas ok. Tool con `target: http_request` (`tests/graphs/agents/for_each_http_tool.json`, echo endpoint): 3/3 filas ok, frames `batch-progress`/`batch-item-finished` presentes. Tool con `target: subgraph` (`tests/graphs/agents/for_each_subgraph_tool.json`, sub-agente inline): 3/3 filas ok, cada fila corrió un sub-agente aislado (Mode B) con `level`/`path` anidados correctos.

**Documentación de referencia.**
- Dev guide: [`docs/developer_guide/49_for_each.md`](developer_guide/49_for_each.md) — guía completa.
- Schema: [`docs/node_configurations.json`](node_configurations.json) (`node_configurations.for_each`).
- Tools reference: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json) (`node_types_as_tools.for_each`, ambos ejemplos).
- Ports: [`docs/agent_context/node_ports_reference.md`](agent_context/node_ports_reference.md).
- Índice de tools: [`docs/developer_guide/41_builtin_tools_index.md`](developer_guide/41_builtin_tools_index.md) → sección "Registered nodes usable as tools".
- Spec: [`docs/superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md`](superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md).

**Estado.** done (v1 — `items`/`items_from: sheet`/`target: any registered node incl. subgraph`). Diferido a v1.1: `items_from: attachment`, `items_from: tool_result`, checkpoint store durable, `results_to` sink de escritura.
