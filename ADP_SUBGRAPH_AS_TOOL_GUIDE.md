# Subgrafos / LLMs como Tools (agents-as-tools) — Guía de implementación para ADP

> **Audiencia:** equipo ADP que arma grafos de agentes (canvas / JSON) y quiere que
> un agente delegue trabajo a **otro agente o sub-flujo** como si fuera una herramienta.
> **Self-contained:** todo lo necesario está aquí; no necesitas leer el código de Colmena.
> **Disponible desde:** 2026-06-19 (Colmena `develop`).

---

## 1. Qué es y cuándo usarlo

Un nodo `llm_call` puede registrar **otro grafo** (o un `llm_call` embebido) como una
**herramienta** más en su lista de tools. El LLM **decide en su propio loop** cuándo
invocarla, igual que decide llamar a `http_request` o a una búsqueda web.

Sirve para: **tomar una capability ya construida y dársela a otro agente** sin
reescribirla — p.ej. un agente "coordinador" que delega a un "investigador web", a un
"experto legal", o a un sub-flujo de varios pasos.

### ¿En qué se diferencia del Orchestrator?

| | **subgraph como tool** (esta guía) | **orchestrator** |
|---|---|---|
| Quién decide invocar | **El LLM**, en su loop de tools, cuando quiere | Un **Planner** que planifica todas las tareas por adelantado |
| Dónde se declara | `tool_configurations` de un `llm_call` | `config.agents` de un nodo `orchestrator` |
| Fuente del grafo hijo | archivo **o** inline | solo archivo |

Si quieres delegación "bajo demanda decidida por el modelo" → **subgraph como tool**.
Si quieres un plan multi-tarea orquestado → **orchestrator**.

---

## 2. Forma A — Reutilizar un grafo existente (por ruta)

```json
"tool_configurations": {
  "consultar_especialista": {
    "name": "consultar_especialista",
    "description": "Delega una tarea a un sub-agente especialista que devuelve una respuesta concisa.",
    "node_type": "subgraph",
    "fixed_config": {
      "child_graph_path": "./agents/especialista.json"
    }
  }
}
```

- `node_type: "subgraph"` es lo que habilita esta función.
- `description` es lo que el LLM padre usa para decidir cuándo llamar la tool. Sé claro.
- `child_graph_path` apunta al JSON del grafo hijo.
- Por defecto el LLM ve **un solo parámetro: `task` (string)**.

---

## 3. Forma B — Agente inline (sin archivo)

El grafo hijo va **embebido** dentro de `child_graph_inline` (mismo formato que cualquier
grafo: `nodes` + `edges`). Ejemplo real: un investigador web con búsqueda Tavily.

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
        "edges": [ { "from": "agent", "to": "out" } ]
      }
    }
  }
}
```

Reglas del grafo inline:
- Debe tener un nodo **`output`** — su valor es lo que vuelve al padre.
- El hijo puede tener **sus propias `tool_configurations`** (es un agente completo, no un LLM pelado).
- El hijo recibe la entrada del padre vía plantillas `{{...}}` (ver §5).

---

## 4. La entrada por defecto: `task`

Si **no** declaras `node_schema`, el LLM padre ve una tool con un único campo:

```json
{ "task": "string — la tarea o instrucción para el sub-agente" }
```

Ese `task` se inyecta en el estado del hijo y se usa así dentro del hijo:

```json
"system_message": "Eres un asistente conciso. Resuelve esta tarea: {{task}}"
```

> **Importante:** el `llm_call` usa el input `task` como su *prompt implícito*. Por eso un
> hijo con solo `task` funciona aunque no tenga campo `prompt`.

---

## 5. Entrada estructurada (opcional) — y sus 2 trampas

Si quieres que el LLM pase **varios campos tipados** (no un solo `task`), usa `node_schema`:

```json
"tool_configurations": {
  "consultar_clima": {
    "name": "consultar_clima",
    "description": "Consulta el clima de una ciudad en una fecha dada.",
    "node_type": "subgraph",
    "node_schema": {
      "child_graph_path": { "fixed": "./agents/clima.json" },
      "ciudad": { "type": "string", "required": true, "description": "Ciudad a consultar" },
      "fecha":  { "type": "string", "required": true, "pattern": "^\\d{4}-\\d{2}-\\d{2}$", "description": "Fecha YYYY-MM-DD" }
    }
  }
}
```

El hijo recibe `{{ciudad}}` y `{{fecha}}`.

### ⚠️ Trampa 1 — `child_graph_path`/`inline` va DENTRO de `node_schema` como `fixed`

Cuando usas `node_schema`, el motor **ignora `fixed_config`**. Si pones `child_graph_path`
en `fixed_config` junto a un `node_schema`, **se descarta** y la tool falla con
*"requires child_graph_inline or child_graph_path"*. Solución: declararlo como campo
`fixed` dentro de `node_schema` (como en el ejemplo de arriba).

| Caso | Dónde poner `child_graph_path`/`inline` |
|------|------------------------------------------|
| Sin `node_schema` (solo `task`) | en `fixed_config` |
| Con `node_schema` (estructurado) | como campo `fixed` **dentro de** `node_schema` |

### ⚠️ Trampa 2 — el hijo estructurado necesita un `prompt` explícito

Con entrada estructurada **no hay `task`**, así que el prompt implícito está vacío. El
grafo hijo debe tener un `prompt` explícito que use las variables:

```json
"agent": {
  "type": "llm_call",
  "config": {
    "system_message": "Eres un meteorólogo conciso. Responde en una frase.",
    "prompt": "Reporta el clima para {{ciudad}} el {{fecha}}."
  }
}
```

---

## 6. Comportamiento clave

| Tema | Comportamiento |
|------|----------------|
| **Aislamiento (memoria)** | **Stateless por llamada.** Cada invocación del tool arranca sin memoria de llamadas previas. Dos llamadas al mismo tool en el mismo turno **no** comparten contexto. |
| **Streaming** | **Transparente.** Los pasos internos del hijo se emiten al stream del padre con el prefijo **`subgraph-*`** (el frontend ADP ya los renderiza). Ver §8. |
| **Valor de retorno** | El valor del nodo `output` del hijo (debe existir). Vuelve al padre como el resultado del tool. |
| **`enabled_tools`** | No hace falta listar el tool ahí: todo lo declarado en `tool_configurations` queda **auto-habilitado**. |
| **Anidamiento** | Soportado, con guard de profundidad **máx 5 niveles** (un subgraph-tool que se referencia en ciclo corta con error claro en vez de recursar infinito). |

---

## 7. HITL — el sub-agente puede preguntarle al usuario (suspend/resume)

El hijo puede **pausar** para pedir un dato al usuario. La pausa "sube" automáticamente
por el agente padre hasta el cliente, y al recibir la respuesta el hijo **reanuda donde
se quedó**.

### 7.1 Cómo lo hace el grafo hijo

Dale al hijo una tool `suspend`. `id` es **obligatorio** (identifica la pregunta en el
resume); `question` es el texto mostrado.

```json
"agent": {
  "type": "llm_call",
  "config": {
    "provider": "google", "model": "gemini-2.5-flash", "api_key": "${GEMINI_API_KEY}",
    "system_message": "Eres un agente de reservas. Antes de confirmar, pregunta cuántas personas asistirán usando la tool `preguntar_usuario`. Tarea: {{task}}",
    "tool_configurations": {
      "preguntar_usuario": {
        "name": "preguntar_usuario",
        "node_type": "suspend",
        "description": "Pausa y le hace una pregunta aclaratoria al usuario.",
        "fixed_config": {
          "id": "reserva_num_personas",
          "question": "¿Cuántas personas asistirán?"
        }
      }
    }
  }
}
```

### 7.2 El ciclo de ejecución

1. **Run 1:** el usuario pide algo → el padre llama el tool → el hijo llama `suspend` →
   el run termina con estado **`SUSPENDED`** y una lista `questions: [{ id, question, type, options }]`.
2. **Run 2 (resume):** se re-ejecuta el **mismo** flujo con la respuesta del usuario. El
   hijo reanuda en el punto exacto y completa.

> **Requisito de producción:** usa un **`agent_session_id` estable** entre el run 1 y el
> run 2 (es el handle de la conversación). Sin él, el resume no encuentra el estado
> suspendido. ADP ya maneja esto por conversación.

### 7.3 Formato de la respuesta (resume)

La respuesta del usuario se entrega en formato **ID-keyed**, usando el `id` del suspend:

```
Q[reserva_num_personas]: ¿Cuántas personas asistirán?
A[reserva_num_personas]: 4 personas
```

- El `id` debe coincidir con el `id` del nodo `suspend`.
- Es orden-independiente y multilínea (la respuesta puede ocupar varias líneas).

---

## 8. Qué ve el frontend en el SSE (eventos del hijo)

Los eventos internos del hijo son **idénticos** a los de nivel padre, solo con prefijo
`subgraph-`. El frontend los renderiza igual. Tipos principales:

| Evento | Payload (campos clave) |
|--------|------------------------|
| `subgraph-node-start` | `node_id`, `node_type`, `config`, `inputs` |
| `subgraph-tool-input-available` | `toolCallId`, `toolName`, `input` |
| `subgraph-tool-output-available` | `toolCallId`, `output` |
| `subgraph-node-end` | `node_id`, `node_type`, `output: { result, extra_info: { usage, tool_calls } }` |
| `subgraph-usage-summary` | `nodes: [{ model, provider, prompt_tokens, completion_tokens, total_tokens }]` |
| `subgraph-text-delta` *(si `stream: true`)* | tokens del LLM hijo en vivo |
| `subgraph-reasoning-*`, `subgraph-skill-loaded`, `subgraph-error` | razonamiento / skills / errores del hijo |

El **valor de retorno** del subgraph es el `output` del nodo marcado internamente como
nodo de salida (el `subgraph-node-end` del nodo `output`). Eso es lo que el padre recibe
como `tool-output-available`.

En **HITL**, cuando el hijo suspende, el run termina con `finish` que incluye
`status: SUSPENDED` y `questions: [...]` (no se emite el `subgraph-node-end` final del
hijo hasta el resume).

---

## 9. Checklist para implementar en ADP

- [ ] El nodo padre es un `llm_call` con un `tool_configurations` que incluye una entrada con `node_type: "subgraph"`.
- [ ] `description` del tool es clara (el LLM decide cuándo llamarlo a partir de ella).
- [ ] El grafo hijo (archivo o inline) tiene un nodo **`output`**.
- [ ] **Sin `node_schema`** → `child_graph_path`/`inline` en `fixed_config`; el hijo usa `{{task}}`.
- [ ] **Con `node_schema`** → `child_graph_path`/`inline` como campo `fixed` dentro de `node_schema`; el hijo usa un `prompt` explícito que template las variables.
- [ ] Variables de entorno disponibles para el hijo (`GEMINI_API_KEY`, `TAVILY_API_KEY`, etc.).
- [ ] Si el hijo usa HITL: tool `suspend` con `id` (obligatorio) y `question`; conversación con `agent_session_id` estable; resume en formato `Q[id]:` / `A[id]:`.
- [ ] El frontend escucha eventos `subgraph-*` (ya soportado).

---

## 10. Errores comunes y su causa

| Síntoma | Causa | Arreglo |
|---------|-------|---------|
| `requires child_graph_inline or child_graph_path` | Usaste `node_schema` y dejaste `child_graph_path` en `fixed_config` | Mueve `child_graph_path` a un campo `fixed` dentro de `node_schema` (§5, trampa 1) |
| El hijo responde vacío / `null` con entrada estructurada | No hay `task` y el hijo no tiene `prompt` | Agrega `prompt` explícito que template las variables (§5, trampa 2) |
| El sub-agente "no recuerda" entre dos llamadas | Es el comportamiento esperado: **stateless por llamada** | Pasa todo el contexto necesario en los argumentos del tool |
| `suspend: config.id is required` | La tool `suspend` no tiene `id` | Define `id` en `fixed_config` (o como `fixed` en `node_schema`) |
| El resume no reanuda / "no suspended child" | `agent_session_id` distinto entre run 1 y run 2 | Usa el mismo `agent_session_id` estable de la conversación |
| Recursión infinita | Un subgraph-tool se referencia en ciclo | El motor corta a profundidad 5 con error claro; revisa la cadena de delegación |

---

## 11. Compatibilidad

Esta función es **puramente aditiva** en Colmena: no cambia ninguna API pública ni firma
de trait, y el worker de ADP no requiere cambios para soportarla. El frontend ya renderiza
los eventos `subgraph-*`. Solo necesitas declarar los grafos como se describe arriba.
