# QA — Nodo `loop_controller`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (entrada `"loop_controller"`)
- `docs/node_as_tools_reference.json` (no existe entrada específica)
- `docs/agent_context/node_ports_reference.md` (tabla de puertos)
- Registry: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

## 1) Config documentada NO soportada por el código

**Hallazgo 1:** Validación de `loop_status` ausente — valores inválidos se aceptan sin rechazo. — ✅ **RESUELTO**
- **Qué decía la doc:** `docs/node_configurations.json` declaraba `"valid_values": ["NEXT_TURN", "FINISHED", "SUSPENDED"]` para el campo `loop_status`.
- **Qué hacía el código:** resolvía `loop_status` como cadena plana sin validación y lo propagaba tal cual en `__colmena_loop_status`. El consumidor real (`api.rs`) solo detiene el loop en `FINISHED`, así que un typo dejaba el loop de serve-mode girando; el loop de grafo tampoco tenía tope de iteraciones.
- **Resolución:** se descartó el fail-closed estricto porque el enum era incompleto — `orchestrator.rs` emite además `FINISHED_PHASE`, y rechazarlo habría roto el orquestador. En su lugar:
  1. `loop_controller` valida contra `KNOWN_LOOP_STATUSES` (`NEXT_TURN`, `FINISHED`, `SUSPENDED`, `FINISHED_PHASE`) y **coacciona** cualquier valor no reconocido a `FINISHED` con un `warn`. Parar temprano es un fallo visible y depurable; un loop sin fin no lo es.
  2. Se añadió el techo `COLMENA_MAX_GRAPH_TURNS` (default 50, `0` = sin techo) al loop de `api.rs`, que ataca la causa raíz: protege también cuando el runaway no viene de un typo. Al alcanzarlo, responde con un error explícito, nunca con la salida parcial.
- **Qué probar ahora:** que los 4 valores válidos pasan intactos (en especial que `FINISHED_PHASE` **no** se colapsa a `FINISHED`), que un typo produce `FINISHED` + warning, y que `suspend_flag: true` sigue ganando sobre cualquier valor. Cubierto por 6 tests unitarios en `loop_controller.rs`.

**Hallazgo 2:** Tipo de `all_tasks` nunca validado — documentado como "any".
- **Qué dice la doc:** `docs/node_configurations.json` especifica `"type": "any"` y describe `all_tasks` como "El payload final a emitir si FINISHED".
- **Qué hace el código:** `loop_controller.rs:61-67` toma `all_tasks` de inputs/config (si existe) y lo inserta en el output como `final_result` sin validación de estructura ni tipo.
- **Impacto QA:** No hay restricción en qué puede ser `all_tasks` (string, null, array, objeto). QA debe verificar que cualquier valor se propaga intacto.

## 2) Código NO documentado

**Hallazgo 1:** Resolución de inputs con fallback a config no explícitamente documentada.
- **Ubicación código:** `loop_controller.rs:30-35` (para `loop_status`) y `loop_controller.rs:38-42` (para `suspend_flag`).
- **Qué dice la doc:** `docs/node_configurations.json` describe campos `loop_status`, `suspend_flag`, etc. como campos de config, sin mencionar que inputs pueden sobrescribir config.
- **Qué hace el código:** Usa el patrón `inputs.get("field").or_else(|| config.get("field")).unwrap_or(default)`. Los inputs tienen precedencia sobre config.
- **Impacto QA:** Un edge que enrutea un valor a `loop_controller.loop_status` (como input) anula completamente el `config.loop_status`, un comportamiento no documentado en la referencia de puertos.

**Hallazgo 2:** El campo `suspend_flag` sobrescribe `loop_status` de forma incondicional.
- **Ubicación código:** `loop_controller.rs:38-46`. Si `suspend_flag` es `true`, la línea 45 fuerza `loop_status = "SUSPENDED"` sin importar el valor anterior.
- **Qué dice la doc:** `docs/node_configurations.json` describe `suspend_flag` como "a convenience override", pero no especifica que **SIEMPRE sobrescribe** el status anterior cuando es `true`.
- **Qué hace el código:** La lógica es: resolver `loop_status` primero (línea 30-35), luego si `suspend_flag == true`, sobrescribir a "SUSPENDED" (línea 44-46).
- **Impacto QA:** QA debe verificar que `suspend_flag: true` siempre produce `__colmena_loop_status: "SUSPENDED"` incluso si `loop_status: "NEXT_TURN"`.

**Hallazgo 3:** Comportamiento de `question` solo en estado SUSPENDED no documentado en node_as_tools_reference.
- **Ubicación código:** `loop_controller.rs:53-59`. El campo `question` es extraído e insertado en output **solo si** `loop_status == "SUSPENDED"`.
- **Qué dice la doc:** `docs/node_configurations.json` describe el campo `question` sin mencionar que es ignorado/excluido cuando el status no es SUSPENDED. `docs/agent_context/node_ports_reference.md` sí menciona "Conditionally adds `question` (when `SUSPENDED`...)" pero no en docs/node_as_tools_reference.json.
- **Qué hace el código:** Las líneas 53-59 usan `if loop_status == "SUSPENDED"` para condicionar la inclusión.
- **Impacto QA:** Un grafo que pasa `question` pero `loop_status: "FINISHED"` resulta en un output sin `question` (se ignora silenciosamente).

**Hallazgo 4:** Comportamiento de `all_tasks` solo en estado FINISHED no documentado.
- **Ubicación código:** `loop_controller.rs:60-68`. El campo `all_tasks` es renombrado a `final_result` e insertado en output **solo si** `loop_status == "FINISHED"`.
- **Qué dice la doc:** Similar a Hallazgo 3, `docs/node_configurations.json` describe `all_tasks`, pero `docs/agent_context/node_ports_reference.md` especifica "Conditionally adds ... `final_result` (when `FINISHED` and `all_tasks` is present)".
- **Qué hace el código:** Las líneas 60-68 usan `if loop_status == "FINISHED"`.
- **Impacto QA:** Un grafo con `loop_status: "NEXT_TURN"` e `all_tasks: {...}` resulta en un output sin `final_result`.

**Hallazgo 5:** El output siempre está dentro de una clave `"output"` según el patrón de nodo estándar.
- **Ubicación código:** `loop_controller.rs:70-72`. La salida es `{ "output": { __colmena_loop_status, ... } }`.
- **Qué dice la doc:** `docs/agent_context/node_ports_reference.md` especifica output port como `output` (correcto), pero el contexto de cómo el output es envuelto en `{ output: ... }` no se documenta explícitamente en `node_as_tools_reference.json`.
- **Qué hace el código:** El patrón es consistente con otros nodos (p.ej., `add`, `current_time`).
- **Impacto QA:** Documentación en `node_as_tools_reference.json` está incompleta para este nodo.

## 3) Plan de pruebas QA

### Prueba 3.1: Happy path — loop_status = "NEXT_TURN" desde config
**Objetivo:** Verificar que el nodo emite correctamente `__colmena_loop_status: "NEXT_TURN"` cuando se configura.

**Grafo mínimo:** `tests/graphs/qa/loop_controller_next_turn.json`
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "NEXT_TURN"
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna (config solo).  
**Resultado esperado:** SSE contiene nodo `controller` con output `{ "output": { "__colmena_loop_status": "NEXT_TURN" } }`.  
**Verificación:** Buscar en SSE el frame del nodo `controller` y verificar que `__colmena_loop_status` es exactamente la cadena `"NEXT_TURN"`.

---

### Prueba 3.2: Happy path — loop_status = "FINISHED" con all_tasks
**Objetivo:** Verificar que `all_tasks` aparece en output como `final_result` cuando status es FINISHED.

**Grafo:** 
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "FINISHED",
        "all_tasks": ["tarea1", "tarea2", "tarea3"]
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna.  
**Resultado esperado:** Output contiene `{ "output": { "__colmena_loop_status": "FINISHED", "final_result": ["tarea1", "tarea2", "tarea3"] } }`.  
**Verificación:** SSE debe contener el array completo bajo la clave `final_result`, no bajo `all_tasks`.

---

### Prueba 3.3: Happy path — loop_status = "SUSPENDED" con question
**Objetivo:** Verificar que `question` aparece en output cuando status es SUSPENDED.

**Grafo:**
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "SUSPENDED",
        "question": "¿Estás satisfecho con los resultados?"
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna.  
**Resultado esperado:** Output contiene `{ "output": { "__colmena_loop_status": "SUSPENDED", "question": "¿Estás satisfecho con los resultados?" } }`.  
**Verificación:** SSE debe contener la pregunta exacta bajo clave `question` cuando status es SUSPENDED.

---

### Prueba 3.4: suspend_flag = true override
**Objetivo:** Verificar que `suspend_flag: true` sobrescribe `loop_status` a "SUSPENDED" incondicionalmente.

**Grafo:**
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "NEXT_TURN",
        "suspend_flag": true,
        "question": "Necesitamos tu entrada"
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna.  
**Resultado esperado:** Aunque `loop_status: "NEXT_TURN"`, el output debe ser `{ "output": { "__colmena_loop_status": "SUSPENDED", "question": "Necesitamos tu entrada" } }`.  
**Verificación:** Verificar que `__colmena_loop_status` es exactamente `"SUSPENDED"` (no `"NEXT_TURN"`), incluso cuando la config especificaba lo contrario.

---

### Prueba 3.5: Inputs precedence over config
**Objetivo:** Verificar que un valor en inputs sobrescribe el valor en config.

**Grafo:**
```json
{
  "nodes": {
    "input_node": {
      "type": "mock_input",
      "config": {
        "output": {
          "loop_status": "FINISHED",
          "all_tasks": ["resultado_final"]
        }
      }
    },
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "NEXT_TURN"
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "input_node.output.loop_status", "to": "controller.loop_status" },
    { "from": "input_node.output.all_tasks", "to": "controller.all_tasks" },
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Via mock_input, `loop_status: "FINISHED"` y `all_tasks: ["resultado_final"]`.  
**Resultado esperado:** Output debe contener `__colmena_loop_status: "FINISHED"` y `final_result: ["resultado_final"]` (los valores del input, no del config).  
**Verificación:** Confirmar que inputs sobrescribieron el `loop_status: "NEXT_TURN"` del config.

---

### Prueba 3.6: Invalid loop_status (no es validado)
**Objetivo:** Verificar que un `loop_status` con un valor no reconocido es aceptado sin error (hallazgo S2.1).

**Grafo:**
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "INVALID_STATUS"
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna.  
**Resultado esperado:** El nodo NO lanza error. Output contiene `__colmena_loop_status: "INVALID_STATUS"`.  
**Verificación:** Verificar que la ejecución completa sin error y que el status inválido aparece tal cual en el output (sin sanitizar ni rechazar).

---

### Prueba 3.7: question ignorado cuando status != SUSPENDED
**Objetivo:** Verificar que `question` es excluido del output cuando `loop_status != "SUSPENDED"` (hallazgo S2.3).

**Grafo:**
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "FINISHED",
        "question": "Esta pregunta debería ignorarse",
        "all_tasks": {"resultado": "ok"}
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna.  
**Resultado esperado:** Output contiene `{ "output": { "__colmena_loop_status": "FINISHED", "final_result": {"resultado": "ok"} } }`. NO contiene clave `question`.  
**Verificación:** Escanear SSE del nodo `controller` y confirmar que no existe la clave `question` en el output.

---

### Prueba 3.8: all_tasks ignorado cuando status != FINISHED
**Objetivo:** Verificar que `all_tasks` es excluido del output cuando `loop_status != "FINISHED"` (hallazgo S2.4).

**Grafo:**
```json
{
  "nodes": {
    "controller": {
      "type": "loop_controller",
      "config": {
        "loop_status": "NEXT_TURN",
        "all_tasks": ["esto_se_ignora"],
        "question": "¿Continuar?"
      }
    },
    "logger": { "type": "log" }
  },
  "edges": [
    { "from": "controller", "to": "logger" }
  ]
}
```

**Entrada:** Ninguna.  
**Resultado esperado:** Output es `{ "output": { "__colmena_loop_status": "NEXT_TURN" } }`. NO contiene `final_result` ni `question`.  
**Verificación:** Confirmar que en NEXT_TURN, ni `all_tasks` ni `question` aparecen en output.
