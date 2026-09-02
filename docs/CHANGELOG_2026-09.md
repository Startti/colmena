# Cambios recientes — 2026-09

> **Alcance:** Commits sobre `develop` desde el cierre de `2026-08`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. `sql_query`: eliminado el flag fantasma `guardrail_enabled`

**Qué cambió.** El nodo `sql_query` anunciaba en su `schema()` un campo de config
`guardrail_enabled` ("enables static validation rules") que **ningún código leía
jamás**. No existía ni existió nunca un `config.get("guardrail_enabled")` en
`sql.rs`. Un operador que ponía `guardrail_enabled: false` esperando desactivar la
validación estática no obtenía ningún efecto, y tampoco ninguna advertencia.

El campo se **eliminó** en lugar de cablearse. La validación estática es lo que
bloquea `DROP`, `TRUNCATE` y `DELETE`/`UPDATE` sin `WHERE`: hacerla apagable
habría sido un downgrade de seguridad, no un arreglo. Ahora es explícitamente
incondicional, con una nota en `sql.rs` para que el flag no se reintroduzca.

`guardrail_llm` **no se tocó** — ese guardrail sí es real y sigue siendo opcional
(`guardrail_llm.enabled`, default `false`).

**Sin cambio de comportamiento.** El campo vivía en el bloque `config` del
`schema()`, que es puramente descriptivo: el motor solo consume el bloque `inputs`
para construir la tool definition del LLM, y nunca valida claves desconocidas.
Por eso:

- Los grafos persistidos (incluidos los de ADP) que aún pasan
  `guardrail_enabled` como campo `fixed` **siguen funcionando sin cambios** — la
  clave sobrante se ignora, exactamente igual que antes.
- No hay cambio de API pública → **ADP no afectado**.

Se limpiaron además los lugares que propagaban el campo: la guía 23, el
`node_configurations.json` canónico, cuatro grafos de `tests/graphs/agents/` y la
skill `capability-data-sql`, que le enseñaba a los operadores a declararlo.

**Verificación.**

| Chequeo | Resultado |
|---|---|
| `cargo test --lib sql` | 182 passed, 0 failed |
| `cargo test --lib static_validator` | 27 passed, 0 failed — los bloqueos siguen intactos |
| E2E real vía DAG engine (`sql_query_readonly_test.json`, OpenAI + Postgres) | exit 0, el tool consultó la BD y devolvió tablas y `row_count` reales |

**Documentación de referencia.**
- [`docs/developer_guide/23_sql_node.md`](developer_guide/23_sql_node.md) — tabla de configuración.
- [`docs/qa/nodes/sql_query.md`](qa/nodes/sql_query.md) — hallazgo A1, marcado como resuelto.
- [`docs/qa/nodes/RESUMEN_GAPS.md`](qa/nodes/RESUMEN_GAPS.md) — resumen priorizado del audit.

**Origen.** Hallazgo de severidad Alta A1 del audit doc-vs-código por nodo (PR #226).

**Estado.** done.

---

## 2. Loop de grafo: guardia contra ejecución sin fin

**Qué cambió.** Un `loop_status` mal escrito podía dejar un loop de serve-mode
girando indefinidamente. `loop_controller` propagaba el valor tal cual, y el único
consumidor real (`api.rs`) solo detiene el loop cuando lee exactamente
`"FINISHED"` (o una suspensión, o un nodo de output). Un `"FINISHEDD"` no coincide
con nada, así que el motor tomaba otro turno. Para siempre.

**Los límites por nodo no cubrían este caso.** `max_total_calls` y
`max_calls_from` viven dentro de `RunUseCase`, y cada turno del loop es un
`run_dag` nuevo: sus contadores se reconstruyen desde cero en cada iteración. El
`turn_count` de `api.rs` existía, pero solo se imprimía — nunca se comparaba
contra nada. (`COLMENA_HARD_TURN_CAP` es de otra capa: acota los turnos del
agente LLM dentro de `AgentService`, no las iteraciones del grafo.)

Dos cambios, en dos capas distintas:

1. **`loop_controller` coacciona los valores desconocidos.** Valida contra
   `KNOWN_LOOP_STATUSES` = `NEXT_TURN`, `FINISHED`, `SUSPENDED`, `FINISHED_PHASE`,
   y convierte cualquier otro valor a `FINISHED` emitiendo un `warn`. Parar
   temprano es un fallo visible y depurable; un loop sin fin no lo es.

2. **Techo de turnos en `api.rs`** (`COLMENA_MAX_GRAPH_TURNS`, default `50`,
   `0` = sin techo), aplicado a los **dos** loops — el de JSON y el de streaming.
   Ataca la causa raíz: protege también cuando el runaway no viene de un typo
   (un orquestador que nunca emite `FINISHED`, un grafo sin nodo de output).

**Por qué NO se hizo fail-closed estricto.** Era la opción obvia y es la
equivocada: el enum documentado estaba **incompleto**. `orchestrator.rs:585` emite
`FINISHED_PHASE`, que no aparecía en `valid_values`. Rechazar los valores fuera de
la lista habría roto el orquestador en producción. Por eso `FINISHED_PHASE` es
ahora un valor válido de primera clase, con un test que verifica explícitamente
que **no** se colapsa a `FINISHED` (colapsarlo cortaría el loop una fase antes).

**Al alcanzar el techo la ejecución falla de forma ruidosa,** nunca devuelve la
última salida parcial como si el grafo hubiera terminado bien:

- **JSON**: HTTP 500 con `{ error, turns, last_output }`.
- **SSE**: un frame `{"type":"error","error":"Loop stopped after N turns..."}`.

**Compatibilidad.** Aditivo. Los cuatro estados válidos se comportan igual que
antes; solo cambian los valores que ya estaban rotos. El techo por defecto (50)
solo afecta a peticiones `?loop=true` que hoy no terminan — es decir, a las que ya
estaban colgadas. Sin cambio de API pública → **ADP no afectado**.

**Verificación.**

| Chequeo | Resultado |
|---|---|
| `cargo test --lib loop_controller` | 6 passed, 0 failed |
| Prueba de mutación (corrección desactivada a propósito) | `unrecognized_status_is_coerced_to_finished` **falla** — el test detecta el defecto real, no pasa por construcción |
| `cargo test --verbose` | ver PR |

**Documentación de referencia.**
- [`docs/developer_guide/12_dag_engine_guide.md`](developer_guide/12_dag_engine_guide.md) — "Techo de turnos del loop".
- [`docs/node_configurations.json`](node_configurations.json) — `loop_controller.loop_status`, con `FINISHED_PHASE` y la coerción.
- [`docs/agent_context/node_ports_reference.md`](agent_context/node_ports_reference.md) — puertos y salida del nodo.
- [`docs/qa/nodes/loop_controller.md`](qa/nodes/loop_controller.md) — hallazgo A2, marcado como resuelto.

**Origen.** Hallazgo de severidad Alta A2 del audit doc-vs-código por nodo (PR #226).

**Estado.** done.

---

## 3. Catálogo de nodos: cerrados los huecos y la contradicción interna

**Qué cambió.** `docs/node_configurations.json` describía **32** tipos de nodo
mientras declaraba **37** como válidos en `common_node_properties.type.valid_values`
y los referenciaba en `categories`. Faltaban las entradas de `tavily_client`,
`api_explorer`, `image_generation`, `image_edit` y `tts`. Nada detectaba esa
contradicción: el archivo se mantenía a mano, sin generador ni check en CI.

Las cinco entradas ahora existen, auditadas campo por campo contra la
implementación de cada nodo. Además:

- **Clave `required` duplicada** en `llm_call.crdt_documents`: el objeto tenía dos
  (`false` del campo, y una lista `["artifact_id"]` estilo JSON-Schema mal
  ubicada). `jq` la absorbía en silencio con last-wins; un parser tipado la
  rechaza. Se eliminó la segunda, redundante con `properties.artifact_id.required`.
- **Campos que el código lee y el catálogo no documentaba**:
  `llm_call.max_tool_result_bytes`, `orchestrator.api_key` y `orchestrator.plan`.
- **Nueva sección `common_config_fields`** para las claves que lee el *motor* del
  `config` de cualquier nodo, sin pertenecer a ningún tipo. Hoy contiene
  `include_extra_info`, que `DagRunUseCase` consulta al armar la salida final.
- **`api_explorer` documentado con `config_fields` vacío** y una nota: se
  construye una sola vez con valores por defecto y su `execute()` recibe
  `_config` sin usar, así que cualquier clave puesta ahí es inerte. Su `schema()`
  anuncia diez campos que el nodo nunca lee — drift del `.rs`, no del catálogo.
- **Correcciones de datos**: `tts.format` acepta también `mpeg` y `ogg` y es
  case-insensitive; `quality` de los nodos de imagen dejó de declarar
  `valid_values` porque el nodo no valida nada y reenvía el string al proveedor
  (con `dall-e-3` el vocabulario es `standard`/`hd`, no `low`/`medium`/`high`);
  y `provider` de `image_generation`/`tts` NO es case-insensitive — el match es
  exacto y `"OpenAI"` falla en runtime.

**Solo documentación.** No cambia ningún comportamiento del motor.

**Documentación de referencia.** [`docs/node_configurations.json`](../docs/node_configurations.json).

**Estado.** done.

