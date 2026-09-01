# QA — Nodo `subgraph`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (node_types.subgraph)
- `docs/node_as_tools_reference.json` (node_types_as_tools.subgraph)
- `docs/agent_context/node_ports_reference.md` (línea 52, línea 118)
- `docs/developer_guide/19_nested_agents_and_subgraphs.md`

---

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. Todos los campos documentados (`child_graph_path`, `child_graph_inline`, `__agent_name`) son implementados:
- `child_graph_path`: resuelto en `resolve_child_graph_source()` (línea 49-60)
- `child_graph_inline`: resuelto en `resolve_child_graph_source()` (línea 49-60)
- `__agent_name`: leído en `execute()` (línea 212-215) para emitir boundary events

---

## 2) Código NO documentado

### Entradas internas (__colmena_* keys) no en node_configurations.json

El código filtra y procesa 7 keys internas **no documentadas** en `node_configurations.json`:

| Key | Ubicación en código | Propósito |
|-----|-------------------|----------|
| `__colmena_resume_answer` | línea 276-316 | Detecta resume HITL; enruta a `resume_subgraph()` |
| `__colmena_agent_session_id` | línea 191-195 | Heredado del padre; pasado a ejecutor de hijo |
| `__colmena_node_id_path` | línea 198-208 | Path del nodo en la jerarquía (usado para lineage anidado) |
| `__colmena_tool_name` | línea 230 | Nombre del tool cuando se dispara como tool (crucial para boundary) |
| `__colmena_session_id` | línea 185-189 | Session ID del padre (fallback "unknown_parent") |
| `__colmena_subgraph_depth` | línea 97-100, 142-147 | Profundidad de anidación actual (auto-incrementado) |
| `__colmena_is_output_node` | línea 391-394 | Flag en `extra_info` del hijo para extraer su salida |

**Impacto para QA**: El conjunto completo de keys internas debe ser probado (filtrado IN, propagado/transformado, NOT a child state).

### Validaciones y errores fail-closed

| Validación | Línea | Behavior |
|-------------|-------|----------|
| `child_graph_source` falta | 321-324 | Error: "requires 'child_graph_inline' or 'child_graph_path'" |
| `child_graph_path` file no existe | 330-331 | Error: "child_graph_path not found: {path}" |
| Depth ceiling excedido | 261-270 | Error con prefijo `SUBGRAPH_DEPTH_EXCEEDED:` (solo si `COLMENA_MAX_SUBGRAPH_DEPTH` > 0 en env) |
| `SubGraphExecutorPort` no inicializado | 281-283, 366-368 | Error: "SubGraphExecutorPort not initialized" |
| Child suspendido sin sesión resumible | 285-293 | Error: "No suspended child found under parent {}/path {}" |
| Grafo inline/path JSON inválido | 334 | Error: JSON parse error |

### Eventos SSE no documentados en node_configurations.json

El código emite 2 eventos de boundary **no mencionados** en la doc de configuración (línea 344-410):

| Evento | Línea | Cuándo |
|--------|-------|--------|
| `DagExecutionEvent::NodeStart { node_type: "subgraph" }` | 346-353 | Al iniciar subgraph (si `boundary_name` presente) |
| `DagExecutionEvent::SubgraphNodeFinish` | 404-410 | Al terminar subgraph (si `boundary_name` presente) |

Estos eventos solo se emiten si existe un `boundary_name` (fallback: `__agent_name` → `__node_id` → `__colmena_tool_name`), que es correcto pero no documentado en el schema de config.

### Estado del hijo (child state building)

El código ejecuta un mapeo IN sofisticado (**no documentado en node_configurations.json**):
- `build_child_state()` (línea 90-102) filtra claves internas usando `is_excluded_from_child_state()` (línea 72-76)
- La constante `CHILD_GRAPH_SOURCE_KEYS` (línea 24) define qué se excluye: `["child_graph_inline", "child_graph_path"]`
- Re-inserta `__colmena_subgraph_depth` (línea 97-100) como la ÚNICA key `__colmena_*` que survives al hijo

**Impacto para QA**: Cualquier key que NO comience con `__colmena_` y NO esté en `CHILD_GRAPH_SOURCE_KEYS` alcanza el estado global del hijo (línea 91-95). Esto incluye `files`, `task`, y argumentos arbitrarios del modelo. El plumbing estático (`child_graph_inline`, `child_graph_path`) está deliberadamente excluido por seguridad (contiene secrets resueltos).

### Profundidad de anidación (unbounded by default)

El código implementa un techo OPTIONAL (línea 120-162):

| Mecanismo | Línea | Detalle |
|-----------|-------|--------|
| `depth_ceiling()` | 120-128 | Lee env `COLMENA_MAX_SUBGRAPH_DEPTH` una vez por proceso (cached en `OnceLock`) |
| Parsing | 125-126 | Solo valores `> 0` se respetan; `0`, vacío, no-parseable = "sin límite" |
| `exceeds_ceiling()` | 153-155 | Comparación pura: `depth >= ceiling` |
| `depth_exceeded()` | 159-161 | Solo falla si ceiling configurado Y depth lo alcanza; default es unbounded |

**Impacto para QA**: El default es "sin límite". Nesting profundo (50+ niveles) es permitido sin ceiling env var (verificado en código: línea 705-710, línea 714-717). Con `COLMENA_MAX_SUBGRAPH_DEPTH=3`, profundidad >= 3 falla con error `SUBGRAPH_DEPTH_EXCEEDED:`.

### Memory_mode (documentado en guide pero NO en tool_configurations.schema)

El desarrollador guide (doc línea 197-264) detalla 3 modes (`stateless`, `persistent`, `dynamic`) pero:
- El código de `subgraph.rs` **no** maneja `memory_mode` directamente
- El `memory_mode` se aplica en el `llm_call` dentro del hijo (hereda desde tool_configurations)
- Validación de `connection_url` en modos con memoria está en `graph.rs` y `llm.rs`, no en subgraph.rs

**Hallazgo**: La documentación en developer_guide es correcta pero el `tool_configuration_schema` en `node_configurations.json` no incluye `memory_mode` como campo permitido en `tool_configurations.<tool>`. Esto es una omisión de doc schema, no un bug de código.

### Resume path sin re-cargar graph

El código (línea 276-316) implementa un camino fast-path para resume:
- Si `__colmena_resume_answer` está presente, busca la sesión del hijo (línea 285-293)
- Llama `resume_subgraph()` directamente sin re-cargar el graph JSON (línea 300-308)
- No re-emite NodeStart/NodeEnd boundary events en resume

**Hallazgo**: No documentado que el resume es un camino separado que omite `build_child_state()` y carga de graph. Esto es correcto (la sesión del hijo ya tiene estado) pero la absence en docs da lugar a preguntas sobre qué datos están disponibles al resumir.

---

## 3) Plan de pruebas QA

### Caso S1.1: Subgraph con child_graph_path

**Objetivo**: Verificar que un grafo hijo en archivo se carga correctamente y sus inputs se pasan.

**Grafo mínimo** (`test_subgraph_path.json`):
```json
{
  "nodes": {
    "parent_in": { "type": "input", "config": {} },
    "sub": { 
      "type": "subgraph", 
      "config": { "child_graph_path": "./test_child_simple.json" }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "parent_in", "to": "sub" },
    { "from": "sub", "to": "out" }
  ]
}
```

**Child** (`test_child_simple.json`):
```json
{
  "nodes": {
    "child_in": { "type": "input", "config": {} },
    "add_one": { "type": "add", "config": { "a": 1, "b": "{{input_val}}" } },
    "child_out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "child_in", "to": "add_one" },
    { "from": "add_one", "to": "child_out" }
  ]
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run test_subgraph_path.json --agent-session-id s1_1_001
```

**Entrada**: `{ "input_val": 5 }`  
**Resultado esperado**: Salida del child `{ output: 6 }`  
**Pass/Fail**: Child recibió `input_val` en global state, lo template en `b: "{{input_val}}"`, y calculó correctamente.

---

### Caso S1.2: Subgraph con child_graph_inline

**Objetivo**: Verificar que un grafo embebido inline funciona igual que path.

**Grafo**:
```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "sub": { 
      "type": "subgraph", 
      "config": { 
        "child_graph_inline": {
          "nodes": {
            "calc": { "type": "multiply", "config": { "a": 2, "b": "{{value}}" } },
            "out": { "type": "output", "config": {} }
          },
          "edges": [ { "from": "calc", "to": "out" } ]
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "sub" },
    { "from": "sub", "to": "out" }
  ]
}
```

**Entrada**: `{ "value": 7 }`  
**Resultado esperado**: `{ output: 14 }`  
**Pass/Fail**: Child recibió variable templada y operó correctamente.

---

### Caso S1.3: child_graph_path NO EXISTE

**Objetivo**: Verificar error fail-closed cuando archivo no existe.

**Grafo**:
```json
{
  "nodes": {
    "sub": { "type": "subgraph", "config": { "child_graph_path": "./nonexistent.json" } }
  },
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run test_fail_path.json 2>&1 | grep -i "not found"
```

**Resultado esperado**: Error contiene "child_graph_path not found: ./nonexistent.json"  
**Pass/Fail**: Error message preciso, ejecución no continúa.

---

### Caso S1.4: Falta child_graph_path Y child_graph_inline

**Objetivo**: Verificar error cuando ambos campos faltan.

**Grafo**:
```json
{
  "nodes": {
    "sub": { "type": "subgraph", "config": {} }
  },
  "edges": []
}
```

**Resultado esperado**: Error contiene "requires 'child_graph_inline' or 'child_graph_path'"  
**Pass/Fail**: Error is caught at execute time.

---

### Caso S2.1: Filtrado de claves internas NO alcanza child state

**Objetivo**: Verificar que `__colmena_*` y `child_graph_*` no se filtran al estado hijo.

**Setup**: Grafo con child que emite todo lo que recibe en estado global.

**Child** (emite estado):
```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "log_state": { "type": "log", "config": {} },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in", "to": "log_state" },
    { "from": "log_state", "to": "out" }
  ]
}
```

**Parent inyecta secreto accidentalmente**:
```json
{
  "nodes": {
    "in": { "type": "mock_input", "config": {
      "api_key": "FAKE_SECRET_KEY_12345",
      "__colmena_session_id": "sess_abc",
      "legitimate_input": "data"
    } },
    "sub": { "type": "subgraph", "config": { "child_graph_inline": { ... } } }
  },
  "edges": [ { "from": "in", "to": "sub" } ]
}
```

**Ejecución** y capturar stdout:
```bash
cargo run --bin dag_engine -- run test_filter_keys.json 2>&1 | tee /tmp/q2_1.log
```

**Pass/Fail**: 
- ✓ `__colmena_session_id` NO aparece en output del child (filtrado)
- ✓ `FAKE_SECRET_KEY_12345` NO aparece (no debe estar en child state)
- ✓ `legitimate_input` SÍ aparece (datos legítimos pasan)

---

### Caso S2.2: Profundidad de anidación SIN COLMENA_MAX_SUBGRAPH_DEPTH

**Objetivo**: Verificar que nesting profundo (50 niveles) es permitido sin env var.

**Grafo**: Chain de 50 subgraphs inline anidados (cada uno contiene un `multiply` + salida).

**Ejecución**:
```bash
COLMENA_MAX_SUBGRAPH_DEPTH="" cargo run --bin dag_engine -- run test_deep_nesting_50.json --agent-session-id depth_test
```

**Resultado esperado**: Grafo ejecuta sin error `SUBGRAPH_DEPTH_EXCEEDED`.  
**Pass/Fail**: Ejecución completa; profundidad 50 permitida por defecto.

---

### Caso S2.3: Profundidad de anidación CON COLMENA_MAX_SUBGRAPH_DEPTH=3

**Objetivo**: Verificar que techo env var rechaza profundidad >= 3.

**Grafo**: Mismo grafo de 50 niveles.

**Ejecución**:
```bash
COLMENA_MAX_SUBGRAPH_DEPTH=3 cargo run --bin dag_engine -- run test_deep_nesting_50.json 2>&1 | grep -i "SUBGRAPH_DEPTH_EXCEEDED"
```

**Resultado esperado**: Error output contiene `SUBGRAPH_DEPTH_EXCEEDED:` (línea 264).  
**Pass/Fail**: Ejecución falla a profundidad 3; message es estable.

---

### Caso S3.1: HITL suspend/resume en subgraph (nodo subgraph clásico)

**Objetivo**: Verificar que suspensión del hijo hace bubble-up y resume reanuda correctamente.

**Child con suspend**:
```json
{
  "nodes": {
    "ask": { "type": "suspend", "config": { "id": "q1", "question": "¿Cuántos?" } },
    "respond": { "type": "log", "config": {} }
  },
  "edges": [ { "from": "ask", "to": "respond" } ]
}
```

**Parent**:
```json
{
  "nodes": {
    "sub": { "type": "subgraph", "config": { "child_graph_inline": { ... } } }
  }
}
```

**Run 1 (suspend)**:
```bash
cargo run --bin dag_engine -- run test_hitl_subgraph.json --agent-session-id hitl_001
```

**Resultado esperado**: Output contiene `"__colmena_status": "SUSPENDED"` y `"questions": [{ "id": "q1" }]`.

**Run 2 (resume)**:
```bash
cargo run --bin dag_engine -- run test_hitl_subgraph.json --agent-session-id hitl_001 \
  --answer $'Q[q1]: ¿Cuántos?\nA[q1]: 42'
```

**Resultado esperado**: Ejecución continúa; child recibe "42" y continúa (verify en log).  
**Pass/Fail**: Resume sin error; respuesta propagada al hijo correctamente.

---

### Caso S3.2: Output extraction vía __colmena_is_output_node

**Objetivo**: Verificar que salida del hijo se extrae por flag, no por passthrough completo.

**Child** (múltiples nodos, uno marcado output):
```json
{
  "nodes": {
    "compute": { "type": "add", "config": { "a": 1, "b": 1 } },
    "out": { "type": "output", "config": {} },
    "debug_log": { "type": "log", "config": {} }
  },
  "edges": [
    { "from": "compute", "to": "out" },
    { "from": "compute", "to": "debug_log" }
  ]
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run test_output_extract.json --agent-session-id out_extract_001
```

**Resultado esperado**: Parent salida es el valor del nodo `out` (que tiene `__colmena_is_output_node: true` en su `extra_info`), NO el estado completo del child.  
**Pass/Fail**: Verify SSE o log que output es limpio (línea 388-400).

---

### Caso S3.3: Memory mode STATELESS (default, subgraph como tool)

**Objetivo**: Verificar que cada llamada al subgraph-tool es aislada (stateless por defecto).

**LLM call con subgraph tool (sin memory_mode)**:
```json
{
  "nodes": {
    "llm": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "tool_configurations": {
          "archive_query": {
            "name": "archive_query",
            "node_type": "subgraph",
            "fixed_config": { "child_graph_inline": { "nodes": { "mem": { "type": "llm_call", "config": { "provider": "google", "model": "gemini-2.5-flash", "connection_url": "${DATABASE_URL}" } }, "out": { "type": "output" } }, "edges": [ { "from": "mem", "to": "out" } ] } }
          }
        }
      }
    }
  }
}
```

**Prompt**: "Llama archive_query con tarea 1, luego vuelve a llamarla con tarea 2. ¿Recuerda tarea 1?"

**Resultado esperado**: Dos llamadas a `archive_query` reciben path qualifiers distintos (derivados de tool_call_id diferentes), cada una es aislada (no recuerda la anterior).  
**Pass/Fail**: LLM reporta que memory es vacía; segundo call no conoce el primero.

---

### Caso S3.4: Memory mode PERSISTENT (subgraph-tool con memoria compartida)

**Objetivo**: Verificar que `memory_mode: "persistent"` usa un único hilo compartido.

**Tool config**:
```json
{
  "archive_keeper": {
    "name": "archive_keeper",
    "node_type": "subgraph",
    "memory_mode": "persistent",
    "fixed_config": { "child_graph_inline": { "nodes": { "keeper": { "type": "llm_call", "config": { "connection_url": "${DATABASE_URL}", "prompt": "{{task}}" } }, "out": { "type": "output" } }, "edges": [ { "from": "keeper", "to": "out" } ] } }
  }
}
```

**Prompt**: "Llama keeper con '¿Cuál es mi nombre?', luego dime: '¿Lo recuerdas?'"

**Resultado esperado**: Ambas llamadas comparten el mismo `node_id` (`tool/archive_keeper`), el model interactúa con un único hilo conversacional (pregunta1, respuesta, pregunta2, respuesta).  
**Pass/Fail**: Second call conoce la conversation anterior; model reporta "sí, recuerdo".

---

### Caso S3.5: Memory mode DYNAMIC (subgraph-tool con thread_id del modelo)

**Objetivo**: Verificar que `memory_mode: "dynamic"` requiere `thread_id` y crea hilos distintos por id.

**Tool config**:
```json
{
  "multi_thread_keeper": {
    "name": "multi_thread_keeper",
    "node_type": "subgraph",
    "memory_mode": "dynamic",
    "node_schema": {
      "child_graph_inline": { "fixed": { "nodes": { "keeper": { "type": "llm_call", "config": { "connection_url": "${DATABASE_URL}" } }, "out": { "type": "output" } }, "edges": [ { "from": "keeper", "to": "out" } ] } },
      "task": { "type": "string", "required": true }
    }
  }
}
```

**Prompt**: "Llama keeper(task='Hi', thread_id='proyecto_a'), luego keeper(task='Recuerdas?', thread_id='proyecto_a'), luego keeper(task='Hi', thread_id='proyecto_b'). ¿Cuántos hilos?"

**Resultado esperado**: 
- Calls 1+2 usan `thread_id="proyecto_a"` → comparten memoria, model recuerda.
- Call 3 usa `thread_id="proyecto_b"` → aislado, memory vacío.
- Cada respuesta echo-devuelve el thread_id (`[hilo: proyecto_a]`, `[hilo: proyecto_b]`).

**Pass/Fail**: Model reporta "dos hilos distintos"; second call within proyecto_a recuerda; proyecto_b es aislado.

---

### Caso S3.6: Omisión de thread_id en dynamic mode

**Objetivo**: Verificar que model omitir `thread_id` en dynamic devuelve error corregible.

**Prompt**: "Llama keeper(task='Test') sin thread_id y maneja el error."

**Resultado esperado**: Tool error indica que `thread_id` es requerido (no es silencio).  
**Pass/Fail**: Model puede leer error y reintentar con thread_id.

---

### Caso S3.7: Eventos SSE boundary (subgraph node + as-tool)

**Objetivo**: Verificar que boundary events `subgraph-node-start` y `subgraph-node-finish` se emiten.

**Setup**: Grafo con subgraph (edge-based) y capture SSE.

**Ejecución**:
```bash
cargo run --bin dag_engine -- serve test_subgraph_sse.json &
PID=$!
curl -s http://localhost:3000/chat -d '...' > /tmp/sse_output.sse
kill $PID
grep "subgraph-node-start\|subgraph-node-finish" /tmp/sse_output.sse
```

**Resultado esperado**: 
- Evento `SubgraphNodeStart` emitido con `node_type: "subgraph"` (línea 346-353)
- Evento `SubgraphNodeFinish` emitido con output final (línea 404-410)
- Boundary name es `__agent_name` (orquestrador) o `__node_id` (edge path) o `__colmena_tool_name` (tool path)

**Pass/Fail**: Ambos eventos presentes; lineage correcto.

---

### Caso S3.8: Child event lineage nesting (Fase F)

**Objetivo**: Verificar que eventos del child están anidados bajo la boundary (no siblings).

**Grafo**: Subgraph-as-tool que emite eventos internos.

**Parse SSE**:
```bash
python3 -c "
import json, sys
events = [json.loads(line.split(': ', 1)[1]) for line in open('/tmp/sse_output.sse').readlines() if line.startswith('data: ')]
for e in events:
    if e.get('type') in ['SubgraphWrapped', 'DagExecutionEvent']:
        print(f\"path={e.get('path')} depth={e.get('depth')}\")
"
```

**Resultado esperado**: Internal child events have `path` containing boundary name (e.g., `"tool_name>child_node"`), `depth` > 0 (línea 987).  
**Pass/Fail**: Events nesting is correct; tree structure visible.

---

### Caso S3.9: Config vs Inputs precedence (tool path vs edge path)

**Objetivo**: Verificar que `config` toma precedencia sobre `inputs` para child_graph_source.

**Setup**: Subgraph-as-tool con ambos sources presentes en inputs.

**Inputs al subgraph**:
```json
{
  "child_graph_inline": { "from": "inputs_key" },
  "child_graph_path": "./from_inputs.json"
}
```

**Config**:
```json
{
  "child_graph_inline": { "from": "config_key" },
  "child_graph_path": "./from_config.json"
}
```

**Resultado esperado**: Config wins (línea 51-52); child loads `{ "from": "config_key" }`.  
**Pass/Fail**: Verificar que child usa config source (e.g., by checking its execution or logs).

