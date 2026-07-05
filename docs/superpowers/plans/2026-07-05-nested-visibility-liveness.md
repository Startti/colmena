# Implementation Plan: Visibilidad total anidada + `level`/`path` + red de seguridad de liveness

## Summary
El `dag_engine` debe **forwardear al stream SSE la actividad de TODOS los niveles de anidamiento** (texto + razonamiento + tool-calls de sub-agentes en subgrafos/orchestrators embebidos, a cualquier profundidad), **etiquetando cada frame con `level` (profundidad) y `path` (linaje)**, y mantener un heartbeat de liveness que solo late en silencio genuino. Hoy los eventos de nivel ≥2 se **descartan** (`_ => None` tras un solo desenvuelto de `SubgraphWrapped`) y los eventos de borde suprimen el heartbeat sin mantener vivo el stream.

## Motivation
Reproducido empíricamente (réplica del "agente creador" contra `colmena-api` dev, revisión desplegada):
- **22 s de stream mudo** mientras corría un orchestrator embebido; en el creador real supera 60 s → *falso* `Stream timeout` del watchdog del API.
- Niveles internos invisibles: `orch` (L2, 1 wrap) visible; `calc_a`/`write_a` (L3, 2 wraps) descartados; tools python (L4, 3 wraps) totalmente invisibles.
- Causa: `run_use_case` re-envuelve cada evento hijo en `SubgraphWrapped` (anidándolos), pero `sse_mapper` solo desenvuelve **un** nivel → los profundos caen en `_ => None`. En paralelo, `LlmMessageStart/Finish/LlmUsage/TurnStart` resetean `last_activity` (suprimen heartbeat) pero mapean a `None` (no XADDean → no resetean el watchdog de 60s).

Objetivo de producto (decidido con ADP): visibilidad **total** (texto real + razonamiento cuando el modelo lo expone), `stream:true` por defecto salvo que se especifique `false`, y cada frame etiquetado con `level` + `path`. `type`s existentes intactos; **campos nuevos aditivos** (contrato en `SPEC_NESTED_VISIBILITY_SSE_FIELDS.md` del repo ADP).

## Architectural Impact
- **Layers affected**: `application` (`run_use_case.rs`, liveness), `infrastructure` (`sse_mapper.rs`, `nodes/subgraph.rs`, `nodes/llm.rs`).
- **New traits/ports**: ninguno.
- **New adapters**: ninguno.
- **Modified files**:
  - `src/dag_engine/domain/events.rs` — `SubgraphWrapped` lleva `depth`/`path` (o variante que preserve profundidad).
  - `src/dag_engine/application/run_use_case.rs` — no re-anidar `SubgraphWrapped`; propagar depth/path; **dos relojes** de liveness.
  - `src/dag_engine/sse_mapper.rs` — desenvolver `SubgraphWrapped` a **cualquier** profundidad; emitir `level`/`path` en todo frame; forwardear los eventos de borde hoy `None`.
  - `src/dag_engine/infrastructure/nodes/subgraph.rs` — emitir fronteras también en el path subgraph-as-tool (`agent_name` None); propagar `path_prefix` como `path`.
  - `src/dag_engine/infrastructure/nodes/llm.rs` — auditar/consolidar `stream` default = `true` en TODOS los paths (directo + agent_service).
- **Binding impact**: Python **no**; TypeScript **no** (cambio interno del stream JSON, no de firmas napi/pyo3). Verificar que ningún test de bindings assertee la ausencia de campos.

---

## Detailed Steps

### Fase A — Arreglar el drop de niveles profundos (núcleo de la visibilidad)

1. **Aplanar el anidamiento de `SubgraphWrapped` (no re-envolver).**
   - File: `src/dag_engine/application/run_use_case.rs` (~L706-737, arm `SubgraphChildEvent`)
   - What: al recibir un `SubgraphChildEvent` cuyo `inner` ya es `SubgraphWrapped`, **no** crear `SubgraphWrapped { SubgraphWrapped }`; en su lugar propagar un único `SubgraphWrapped` incrementando un contador `depth` y prefijando `path`. Requiere que `SubgraphWrapped` cargue `depth: u32` y `path: String` (Fase C, step 6).
   - Why: hoy el doble-wrap es lo que el mapper no sabe desenvolver.

2. **Desenvolver `SubgraphWrapped` a cualquier profundidad en el mapper.**
   - File: `src/dag_engine/sse_mapper.rs` (bloques `SubgraphWrapped` en `map`, ~L89 y ~L356)
   - What: en vez de `match inner.as_ref()` de un solo nivel, **desreferenciar recursivamente** hasta el evento base (o leer `depth`/`path` si se aplanó en step 1), y mapear ese evento base a su frame `subgraph-*`. Eliminar el `_ => None` que traga los anidados.
   - Why: hace visibles L3, L4, … con el mismo esquema `subgraph-*` que hoy usa L1.
   - Test: `mapper.map(SubgraphWrapped{SubgraphWrapped{LlmToken}})` debe producir un `subgraph-text-delta` (hoy produce `[]`).

### Fase B — Etiquetar `level` + `path`

3. **Propagar `path` desde `path_prefix`.**
   - File: `src/dag_engine/application/run_use_case.rs` (`path_prefix` ~L161/L458; `run_subgraph`/`resume_subgraph` ~L1136/L1201)
   - What: construir el `path` del nodo (`padre>...>nodo`) a partir del `path_prefix` existente y adjuntarlo al `SubgraphWrapped` (y a los eventos de nivel 0).
   - Why: `path_prefix` ya se inyecta por nivel; solo hay que exponerlo.

4. **Emitir `level` y `path` en cada frame del mapper.**
   - File: `src/dag_engine/sse_mapper.rs`
   - What: agregar `"level": <depth>, "path": <path>` a cada `json!` de frame (subgraph-* con su depth real; frames de nivel 0 con `level:0` y `path` = id raíz).
   - Why: contrato del SPEC ADP.

### Fase C — Visibilidad de bordes + soporte de depth en el tipo

5. **Forwardear los eventos de borde hoy `None`.**
   - File: `src/dag_engine/sse_mapper.rs` (`LlmMessageStart` L286, `LlmMessageFinish` L287, `LlmUsage` L198, `TurnStart` L149, y sus equivalentes en el bloque wrapped)
   - What: mapearlos a un frame real forwardeado (mínimo: un `agent-turn` / `message-boundary` ligero, o plegarlos en un `status` de progreso con `level`/`path`). DEBEN mapear a `Some` para que XADDeen (mantener vivo el stream) y ser visibles.
   - Why: cierra la mitad de liveness del bug y da señal de turno al cliente. (El texto/razonamiento reales ya salen vía tokens con `stream:true`.)
   - Nota: no usar `type:"finish"`/`"error"` (el API cierra el stream con esos strings).

6. **`SubgraphWrapped` carga `depth` + `path`.**
   - File: `src/dag_engine/domain/events.rs`
   - What: `SubgraphWrapped { inner: Box<DagExecutionEvent>, depth: u32, path: String }` (o struct equivalente). Ajustar serde + los sitios de construcción/match.
   - Why: soporta Fases A/B sin re-anidar.

### Fase D — Default de streaming consistente

7. **`stream` default = `true` en todos los paths.**
   - File: `src/dag_engine/infrastructure/nodes/llm.rs` (path directo ya usa `.unwrap_or(true)` en ~L3108) + `src/llm/application/agent_service.rs` (`should_stream = on_token.is_some()`, ~L237/L693)
   - What: garantizar que un `llm_call`/agente **sin** campo `stream` **streamee** (wire `on_token`), y que orchestrator-internos (planner/critic/reactor) sigan la misma regla; solo `stream:false` explícito desactiva.
   - Why: decisión de producto — visibilidad total por defecto. Auditar que no haya un path que default a `false`.

### Fase E — Red de seguridad de liveness (dos relojes)

8. **Separar `last_activity` en dos relojes.**
   - File: `src/dag_engine/application/run_use_case.rs` (~L549, L677, L609, L628)
   - What:
     - `last_forwarded` — avanza **solo** cuando el evento produce un part que se XADDea (Some). Gobierna el heartbeat: `sleep_until(max(last_forwarded, last_beat) + hb)`.
     - `last_any` — avanza con **cualquier** `NodeEvent`. Gobierna el idle-abort: `sleep_until(last_any + idle_timeout)`.
   - Why: hoy un solo `last_activity` (reseteado por todo evento, incluidos los `None`) suprime el heartbeat sin mantener vivo el stream. Separarlos alinea el heartbeat con la señal que realmente resetea el watchdog del API, **sin** romper la detección de cuelgue real.
   - Nota: con Fases A/C/D casi todo trabajo activo produce eventos forwardeados → el heartbeat pasa a ser el respaldo para silencio genuino (nodo hoja mudo). El heartbeat sigue llevando `level`/`path` del nodo activo.

### Fase F — Fronteras de subgraph-as-tool

9. **Emitir `NodeStart`/`SubgraphNodeFinish` también sin `agent_name`.**
   - File: `src/dag_engine/infrastructure/nodes/subgraph.rs` (~L218, L277 — gate `if let (Some(agent_name), Some(obs))`)
   - What: emitir fronteras de subgraph también cuando el subgrafo se invoca como **tool** (agent_name None), con su `level`/`path`.
   - Why: hoy los roles del creador (subgraph-as-tool) no muestran frontera → el árbol no se puede delimitar en la UI.

---

## Testing Strategy
- **Unit (`sse_mapper`)**: `SubgraphWrapped` doblemente anidado → produce el frame `subgraph-*` correcto con `level:2`/`path` (hoy: `[]`). Frames de borde → ahora `Some`. `level`/`path` presentes en cada frame.
- **Unit (liveness, `run_use_case`)**: inyectar en el `select!` una secuencia de `NodeEvent::SubgraphChildEvent` que envuelvan `LlmMessageStart/Finish/LlmUsage` cada 5 s por 90 s **sin** eventos `Some` → assertar que **sí** se emite `Progress` (~cada 20 s) y que el idle-abort NO dispara (porque `last_any` avanza). Variante plana (agent loop no anidado) + variante anidada (creador).
- **Integration**: grafo top `llm_call` → subgraph-tool → orchestrator → sub-agente con python tool (réplica en `apps/service/ia/platform/` del ADP, sin deps externas) → assertar que aparecen frames de `level:3`/`level:4` y que no hay hueco >`hb_interval` sin evento.
- **Manual E2E**: correr el creador real desde el frontend ADP; confirmar en el stream de `colmena-api` (no en logs del worker) que los niveles anidados streamean y no hay `Stream timeout`.

## Documentation Updates
- `SPEC_STREAM_MIDRUN_LIVENESS.md` (ADP) — anexar la corrección de los dos relojes.
- `SPEC_NESTED_VISIBILITY_SSE_FIELDS.md` (ADP) — contrato de `level`/`path` (ya creado).
- Changelog de colmena — visibilidad anidada + campos nuevos.

## Risks & Mitigations
| Risk | Impact | Mitigation |
|------|--------|------------|
| Volumen de eventos crece mucho (niveles profundos × turnos) | Ancho de banda SSE, ruido en UI | `level`/`path` permiten colapsar por rama en ADP; considerar throttle opcional de deltas por nivel; medir en el creador real |
| Cambiar la semántica de `last_activity` rompe el idle-abort | Cuelgue real deja de detectarse | Dos relojes explícitos: `last_any` conserva la detección de cuelgue; test dedicado |
| `stream:true` por defecto cambia comportamiento de grafos existentes | Más tokens/costos; structured-output que asumía no-stream | Solo afecta nodos sin `stream` explícito; `stream:false` sigue disponible; auditar nodos que consumen output no-stream |
| Recursión del desenvuelto en el mapper con profundidad grande | Stack/costo | Iterativo (loop) en vez de recursivo; el límite `MAX_SUBGRAPH_TOOL_DEPTH` (5) acota la profundidad |
| Forwardear bordes cambia el stream que ADP ya consume | Regresión en el reducer | Aditivo + `type`s nuevos caen en `default: return state`; coordinar con el SPEC |

## Open Questions
- ¿Los eventos de borde (`LlmMessageStart/Finish/Usage`) se forwardean como un `type` nuevo dedicado (`agent-turn`) o plegados en `status`? (No bloqueante; recomendación: `type` nuevo ligero para no confundir con el heartbeat.) 
- ¿`level` de los frames de nivel 0 se emite explícito (`0`) o se omite? (Recomendado: explícito, para uniformidad del consumidor.)

## Execution
Usar el skill `/rust_dev` para implementar fase por fase (TDD: test que falla → cambio → verde). Orden sugerido: A → B → C → F (visibilidad), luego E (liveness), luego D (default stream, con auditoría). `cargo test --verbose`, `cargo clippy`, `cargo fmt` antes de push.
