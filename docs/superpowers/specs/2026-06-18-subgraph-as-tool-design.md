# Subgrafos y LLMs como Tools (agents-as-tools) — Diseño

- **Fecha:** 2026-06-18
- **Estado:** Diseño aprobado (pendiente plan de implementación)
- **Autor:** daniel@startti.co
- **Enfoque elegido:** Levantar el whitelist de `tool_configurations` para permitir
  `node_type: "subgraph"`, reusando el nodo `subgraph` existente.

---

## 1. Problema

Hoy un `llm_call` puede registrar nodos como tools (`http_request`, `sql_query`,
`python_script`, `tavily_client`, etc.) vía `tool_configurations`, pero el conjunto
soportado **no incluye** `subgraph` ni `llm_call`. Eso deja un hueco entre los tres
mecanismos existentes:

- **Nodos como tools** — el LLM decide en su loop, pero solo sobre nodos "hoja".
- **Nodo `subgraph`** — ejecuta un grafo hijo aislado, pero se dispara por **edges**
  del DAG (determinista), no por decisión del LLM.
- **`orchestrator`** — delega a sub-agentes, pero la decisión la toma un **Planner**
  que planifica todas las tareas por adelantado, no el LLM en su loop de tool-calling.

El caso de uso objetivo (patrón **agents-as-tools**): tomar un grafo/agente ya
construido (con sus propias tools, RAG, memoria) y exponerlo como **una sola
capability** a otro agente, de modo que **el LLM padre decida en su loop** cuándo
invocarlo. Dos formas:

1. **Subgrafo-como-tool** — reusar un grafo existente vía `child_graph_path`.
2. **LLM-como-tool inline** — un `llm_call` anidado (p.ej. conectado a web search)
   expuesto como una sola tool, declarado inline vía `child_graph_inline`.

El segundo es un caso particular del primero (un grafo de un solo nodo), así que la
solución es **una sola** con dos formas de declaración.

---

## 2. Decisiones de diseño (acordadas)

| Tema | Decisión |
|------|----------|
| Mecanismo | Reusar el nodo `subgraph` permitiéndolo como `node_type` de tool (Enfoque 1). |
| Declaración | `node_type: "subgraph"` para ambos casos; `child_graph_path` o `child_graph_inline` en `fixed_config`. |
| Entrada del LLM | **Default: un único `task: string`**. Estructurado opcional vía `node_schema` (idéntico a las demás tools). |
| Estado / memoria | **Stateless por llamada (A)** — cada invocación arranca sin memoria de llamadas previas. |
| Observabilidad | **Streaming transparente (B)** — los pasos internos del sub-agente se emiten al stream del padre con prefijo `subgraph-*`. |
| HITL (suspend/resume) | **Soportado**. El sub-agente puede suspenderse para preguntar al usuario; reusa los rieles existentes. |

---

## 3. Arquitectura

### 3.1 Flujo de una llamada (sin HITL)

```
LLM padre emite tool_call "buscar_jurisprudencia" { task: "..." }
  │
  ▼
DagToolExecutor::execute_inner
  ├─ get_node("subgraph")                      (Arc con SubGraphExecutorPort ya cableado)
  ├─ mergea fixed_config → inputs              (child_graph_path entra como input)
  ├─ inyecta __colmena_session_id / __colmena_agent_session_id
  ├─ inyecta __colmena_node_id_path efímero    (derivado de tool_call.id)  ← NUEVO
  ├─ inyecta __colmena_subgraph_depth + 1      (guard de recursión)        ← NUEVO
  └─ node.execute(inputs, {}, state, observer) (observer enhebrado)        ← NUEVO
       │
       ▼
   SubGraphNode::execute
     ├─ lee child_graph_path/inline desde inputs (fallback config)         ← NUEVO
     ├─ mapeo IN: task + campos estructurados → global_shared_state del hijo
     ├─ run_subgraph (sesión aislada, UUID v4 nuevo, parent_session_id ligado)
     ├─ emite subgraph-* events vía observer
     └─ mapeo OUT: valor del nodo `output` del hijo
  │
  ▼
ToolResult { output: <valor del output node, stringificado> }
  │
  ▼
LLM padre continúa su loop con el tool_result
```

### 3.2 Flujo con HITL (suspend → resume)

Reusa **íntegramente** la maquinaria existente de `secure_suspend`:

**Suspensión:**
1. El hijo se suspende → `SubGraphNode` hace bubble-up
   `{ __colmena_status: "SUSPENDED", questions: [...] }`
   ([subgraph.rs:109-115](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)).
2. `DagToolExecutor` devuelve ese JSON como `ToolResult`.
3. El loop del `llm_call` padre detecta `__colmena_status: SUSPENDED`, **no persiste
   el tool result**, persiste el assistant-message con la tool call pendiente, y
   propaga SUSPENDED al DAG
   ([llm.rs:3123-3135](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).

**Resume:**
4. El padre se re-entra con `__colmena_resume_answer`.
5. `find_pending_tool_call` encuentra la tool call pendiente por `tool_call_id`
   ([llm.rs:2234](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).
6. `execute_with_resume_answer(pending, answer)` re-ejecuta la tool inyectando
   `__colmena_resume_answer` en los inputs
   ([dag_tool_executor.rs:1700](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)).
7. `SubGraphNode` lee `__colmena_resume_answer` y enruta a `resume_subgraph`
   ([subgraph.rs:75](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)).
8. **Multi-suspend** (el hijo vuelve a suspender) ya está contemplado
   ([llm.rs:2244-2253](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).

> **Invariante crítico:** el `__colmena_node_id_path` efímero (sección 3.3) debe
> derivarse **determinísticamente** de `tool_call.id`. Como en el resume se re-ejecuta
> la **misma** tool call pendiente (mismo `id`, ya persistido en el assistant-message),
> el path se reconstruye idéntico y el hijo reencuentra su propio estado/memoria. Un
> path aleatorio rompería el resume.

### 3.3 Aislamiento stateless

Hoy el `llm_call` interno de un subgraph keya su memoria por
`(agent_session_id, path_qualifier)`, donde el qualifier viene de
`__colmena_node_id_path`. Como todas las tool calls comparten `agent_session_id`,
sin intervención acumularían historia (comportamiento B).

Para forzar **stateless (A)**: el `DagToolExecutor` inyecta un
`__colmena_node_id_path` **efímero y único por invocación**, derivado del
`tool_call.id` (p.ej. `tool/<tool_call_id>`). Así:

- Cada llamada arranca con historia vacía (no contamina entre invocaciones ni
  acumula tokens).
- El `agent_session_id` se sigue heredando (para que el resume HITL encuentre el run
  hijo por leaf-finding).
- Al ser **determinista del `tool_call_id`**, el resume reconstruye el mismo scope.

> El descubrimiento del run hijo SUSPENDED en el resume es por `agent_session_id`
> (leaf-finding, independiente del path), así que el path efímero no rompe esa parte;
> solo aísla la **memoria del LLM interno**.

### 3.4 Streaming transparente

El nodo `subgraph` ya emite `subgraph-*` events vía
`observer.on_event(NodeEvent::SubgraphChildEvent(..))`
([subgraph.rs:159,216](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)),
pero **solo si recibe un observer**. Hoy el tool path pasa `None`
([dag_tool_executor.rs:645,1763](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)).

Arreglo: añadir `DagToolExecutor::with_observer(_observer)` y pasarlo a
`node.execute(...)` en ambos call sites. El patrón ya existe para
`with_skill_observer` / `with_describe_tool_observer`
([llm.rs:2134-2168](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).
El frontend ADP ya sabe renderizar el prefijo `subgraph-*`.

---

## 4. Forma de declaración (API JSON)

### 4.1 Forma A — Reusar un grafo existente

```json
"tool_configurations": {
  "buscar_jurisprudencia": {
    "name": "buscar_jurisprudencia",
    "description": "Busca y resume jurisprudencia relevante sobre un tema legal.",
    "node_type": "subgraph",
    "fixed_config": {
      "child_graph_path": "./agents/legal_research_agent.json"
    }
  }
}
```

El LLM ve `{ task: string }`. El hijo recibe `{{task}}` en su `global_shared_state`.

### 4.2 Forma B — LLM-como-tool inline (con web search)

```json
"tool_configurations": {
  "investigar_web": {
    "name": "investigar_web",
    "description": "Investiga un tema en la web y devuelve un resumen citado.",
    "node_type": "subgraph",
    "fixed_config": {
      "child_graph_inline": {
        "nodes": {
          "agent": {
            "type": "llm_call",
            "config": {
              "provider": "google",
              "model": "gemini-2.5-flash",
              "api_key": "${GEMINI_API_KEY}",
              "system_message": "Investiga el siguiente tema y devuelve un resumen citado: {{task}}",
              "tool_configurations": {
                "web": {
                  "name": "web",
                  "node_type": "tavily_client",
                  "node_config": { "api_key": "${TAVILY_API_KEY}" },
                  "expose_sub_tools": "all"
                }
              }
            }
          },
          "out": { "type": "output" }
        },
        "edges": [{ "from": "agent", "to": "out" }]
      }
    }
  }
}
```

### 4.3 Entrada estructurada (opcional)

```json
"tool_configurations": {
  "consultar_clima": {
    "name": "consultar_clima",
    "description": "Consulta el clima de una ciudad en una fecha dada.",
    "node_type": "subgraph",
    "node_schema": {
      "child_graph_path": { "fixed": "./agents/weather_agent.json" },
      "ciudad": { "type": "string", "required": true, "description": "Ciudad a consultar" },
      "fecha":  { "type": "string", "required": true, "pattern": "^\\d{4}-\\d{2}-\\d{2}$", "description": "Fecha YYYY-MM-DD" }
    }
  }
}
```

El hijo recibe `{{ciudad}}` y `{{fecha}}`. Las claves internas (`__colmena_*`,
`__node_id`) se filtran del mapeo IN (comportamiento existente del `subgraph`).

> **Regla (confirmada vía test E2E T3):** cuando usas `node_schema` (entrada
> estructurada), `child_graph_path` / `child_graph_inline` deben declararse DENTRO
> de `node_schema` como campo `fixed`, **no** en `fixed_config`. Con `node_schema`
> presente, el executor construye los inputs del tool SOLO a partir del `node_schema`
> parseado e **ignora `fixed_config`**; si pones el path ahí se descarta silenciosamente
> y el subgraph falla con `requires child_graph_inline or child_graph_path`. (Sin
> `node_schema`, con el `task` por defecto, el path sí va en `fixed_config`.)
>
> Además, un grafo hijo que recibe variables estructuradas (sin `task`) necesita un
> `prompt` explícito que las template (p.ej. `{{ciudad}}`), porque el `llm_call` usa
> el input `task` como prompt implícito y en entrada estructurada no existe ese `task`
> — sin `prompt` explícito el LLM hijo no recibe turno de usuario.

---

## 5. Vacíos a cerrar (gap analysis)

Lo confirmado contra el código: la mayoría del flujo ya existe y funciona. Los huecos
son acotados y mayormente aditivos.

| # | Vacío | Ubicación | Arreglo | Riesgo |
|---|-------|-----------|---------|--------|
| **G1** | El tool path pasa `node_exec_config = {}` y mergea `fixed_config` en `inputs`, pero `subgraph` lee `child_graph_path`/`child_graph_inline` desde `config`. | [dag_tool_executor.rs:1755](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs) + [subgraph.rs:117-128](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs) | En `SubGraphNode::execute`, leer ambos campos desde `inputs` con fallback a `config` (aditivo; no rompe el camino por-edges). | Bajo |
| **G2** | El tool path **no inyecta `__colmena_node_id_path`** → el hijo no recibe path qualifier → memoria del LLM interno se keya solo por `agent_session_id` → colisiones. | dag_tool_executor.rs (zona 1708-1722) | Inyectar `__colmena_node_id_path` por call. | Medio |
| **G3** | **Stateless (A)** requiere qualifier efímero por llamada, pero **determinista** para que el resume reconstruya el mismo scope. | mismo punto que G2 | Derivar el path de `tool_call.id` (estable suspend↔resume, ya persistido). **Nunca random.** | Medio (punto más sutil) |
| **G4** | El tool path pasa `None` como observer → los `subgraph-*` events no salen al stream (rompe streaming B). | [dag_tool_executor.rs:645 y :1763](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs) | Añadir `with_observer(_observer)` (patrón ya usado para skill/describe_tool observers) y pasarlo en ambos `.execute(...)`. | Bajo |
| **G5** | Sin `node_schema`, el schema que ve el LLM cae a `node.schema()`, y `SubGraphNode::schema()` devuelve `properties: {}` → el LLM vería una tool **sin argumentos**, no el `{ task: string }` default. | [subgraph.rs:31-36](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs) + [dag_tool_executor.rs:762](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs) | Hacer que `SubGraphNode::schema()` exponga `{ task: {type:string, required} }` por defecto. La forma estructurada vía `node_schema` mantiene prioridad (BRANCH 0). | Bajo |
| **G6** | Recursión: un subgraph-tool cuyo hijo tiene otro subgraph-tool → riesgo de recursión infinita por mala config. | nuevo | Guard de profundidad: propagar `__colmena_subgraph_depth` y cortar con error claro pasado un límite (default 5, configurable). | Bajo |
| **G7** | El payload SUSPENDED del padre espera `questions`; confirmar que el `subgraph` pasa ese campo verbatim al bubble. | [subgraph.rs:109-115](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs) + [llm.rs:3133](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs) | Verificar/asegurar que el resultado SUSPENDED del hijo se devuelve sin perder `questions`. | Bajo |

### Lo que ya funciona (reuso, cero código nuevo)

- Bubble-up de SUSPENDED desde una tool ([llm.rs:3123-3135](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).
- Resume vía `find_pending_tool_call` + `execute_with_resume_answer` ([dag_tool_executor.rs:1700](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs), [subgraph.rs:75](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)).
- Multi-suspend anidado ([llm.rs:2244-2253](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)).
- Inyección de `__colmena_session_id` / `__colmena_agent_session_id` ([dag_tool_executor.rs:1708-1722](../../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)).
- `SubGraphExecutorPort` ya cableado sobre el Arc de `get_node("subgraph")` ([registry.rs:357](../../../src/libs/colmena/src/dag_engine/infrastructure/registry.rs)).

---

## 6. Contrato (definition of done)

1. `node_type: "subgraph"` permitido en `tool_configurations` (el dispatch ya cae al
   camino genérico `get_node`; quitar del whitelist documental en
   `docs/node_as_tools_reference.json`).
2. Default: el LLM ve `{ task: string }` → `{{task}}` en el hijo (G5). Estructurado
   vía `node_schema` (ya funciona, BRANCH 0).
3. `child_graph_path` / `child_graph_inline` desde `fixed_config` (G1).
4. Stateless por call vía path efímero **determinista del `tool_call_id`** (G2+G3).
5. HITL suspend/resume: **0 código nuevo**, reusa los rieles existentes (solo
   verificar G7).
6. Streaming transparente: observer enhebrado (G4).
7. Guard de profundidad (G6).

---

## 7. Plan de pruebas

Todos los grafos van en `tests/graphs/agents/` y se corren con
`--agent-session-id <id_estable>` (regla de CLAUDE.md para flujos con estado).

| Test | Qué valida | HITL |
|------|------------|------|
| **T1 — capability sin HITL** | Caso B: un subgraph-tool que "hace algo y devuelve". El padre llama, recibe el output del hijo, continúa. | No |
| **T2 — LLM-as-tool inline + web search** | Forma B (sección 4.2): `child_graph_inline` de un `llm_call` con `tavily_client`. Requiere `TAVILY_API_KEY` (marcar `#[ignore]` si aplica). | No |
| **T3 — entrada estructurada** | `node_schema` con `{ ciudad, fecha }` → el hijo recibe `{{ciudad}}`/`{{fecha}}`. | No |
| **T4 — HITL suspend → resume** | El sub-agente suspende (pregunta al usuario), el padre propaga SUSPENDED, resume con `--answer`, el hijo reanuda y devuelve. Valida G3 (path determinista) y G7. | Sí |
| **T5 — multi-suspend anidado** | El sub-agente suspende dos veces seguidas. | Sí |
| **T6 — aislamiento stateless** | Dos llamadas consecutivas al mismo subgraph-tool en la misma conversación **no** comparten memoria (la segunda no "recuerda" la primera). | No |
| **T7 — guard de profundidad** | Un subgraph-tool que se referencia recursivamente corta con error claro pasado el límite. | No |

Además: unit tests para `SubGraphNode::schema()` (G5), lectura de
`child_graph_path`/`inline` desde inputs (G1), y derivación determinista del path
desde `tool_call_id` (G3).

> Verificación E2E real obligatoria antes de dar por cerrado: correr T1, T2 y T4
> contra servicios reales (no solo unit/wiremock), guardando el SSE en
> `/tmp/colmena_e2e/<name>.sse` y presentando un reporte amigable.

---

## 8. Impacto en ADP / breaking changes

- **No hay breaking change.** Es puramente aditivo: una capability nueva opt-in vía
  `tool_configurations`. Ningún grafo existente cambia de comportamiento.
- No toca la API pública de `EngineConfig` / `ColmenaEngine` / traits exportados, así
  que no requiere sweep del worker ADP. (Si el plan de implementación termina tocando
  una firma de trait, revisar `apps/service/ia/platform/{worker,api}/src/` antes de
  pushear a develop.)
- El frontend ADP ya renderiza `subgraph-*` events, así que el streaming transparente
  funciona sin cambios de frontend.

---

## 9. Fuera de alcance (v1)

- Memoria acumulada (comportamiento B) como opt-in — diferido; v1 es solo stateless.
- Exponer `node_type: "llm_call"` directo como tool (sin envoltura `subgraph`) — no
  necesario: el LLM-as-tool se cubre con `child_graph_inline` de un solo nodo, que
  hereda gratis aislamiento, streaming y HITL.
- Alias cosmético `node_type: "agent"` → `subgraph` — diferido; v1 usa `subgraph`.
- Auto-flow de outputs de nodos upstream hacia el tool call (sigue requiriendo que el
  dato viaje como argumento del LLM).
```

