# QA — Nodo `router`

**Fuente de código:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/`

**Fuentes de doc revisadas:**
- `docs/node_configurations.json` (entrada `router`)
- `docs/node_as_tools_reference.json` (entrada `router`)
- `docs/agent_context/node_ports_reference.md` (sección router)
- `docs/developer_guide/37_router.md`
- `src/libs/colmena/text/prompts/routing_classifier_system.md`

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias críticas detectadas.** La configuración documentada en `node_configurations.json` es completamente soportada por el código. Sin embargo, hay una discrepancia de claridad menor:

- **Campo `description` en modo B:** `node_configurations.json` no menciona explícitamente que el campo `description` está **prohibido** en las ramas de modo `extract_and_route`. El código valida esto en `config.rs:129-131` (rechaza ramas con `description` cuando `mode == "extract_and_route"`), pero la documentación no lo advierte. Impacto: operador que intente copiar una rama de modo A a modo B puede recibir un error de validación inesperado.

---

## 2) Código NO documentado

### A. Temperatura fija a 0.1

**Hallazgo:** Ambos modos (A y B) fijan `temperature: 0.1` hardcoded en el prompt de sistema.

- **Modo A (llm_direct.rs:72):** `temperature_override: Some(0.1)`
- **Modo B (extract_and_route.rs:49):** `temperature_override: Some(0.1)`

**No está documentado** en `node_configurations.json`. Impacto: un operador no puede cambiar la temperatura del clasificador/extractor, y si intenta, descubrirá que la configuración se ignora silenciosamente. Recomendación: documentar esta restricción o hacerla configurable.

### B. Campo `reason` en modo A — conversión silenciosa a string vacío

**Hallazgo:** En modo A (`llm_direct`), el campo `reason` en la rama schema de LLM es **optional** (`required: false`, llm_direct.rs:49), pero cuando el LLM no lo proporciona, se convierte silenciosamente a un string vacío en lugar de `null` en el output (llm_direct.rs:82-86).

```rust
// Línea 82-86: si reason es None, se usa ""
"reason": reason.as_deref().unwrap_or(""),
```

**No está documentado** en `node_ports_reference.md`. Dice que `reason` puede aparecer en `__decision`, pero no que siempre será un string (nunca `null`). Impacto: un cliente JSON esperando `reason: null` cuando no hay razón debe estar preparado para encontrar `reason: ""` en su lugar.

### C. Forwarding de observer a subgrafo

**Hallazgo:** Cuando una rama tiene un subgrafo, el observer es reenviado al `SubGraphNode` (node.rs:153):

```rust
let mut subgraph_inputs = node_inputs.clone();
subgraph_node.execute(&subgraph_inputs, observer).await
```

El observer (SSE stream) no aparece explícitamente en la documentación de node_ports_reference.md como parámetro implícito reenviado. Impacto: bajo; es el comportamiento esperado, pero no está documentado que los eventos SSE del subgrafo se propagarán bajo la rama como contenedor.

### D. Validación de nombre de rama — regex no documentado

**Hallazgo:** Los nombres de rama deben coincidir con `^[a-z][a-z0-9_]{0,63}$` (config.rs:73). La documentación no especifica este patrón.

- Debe comenzar con letra minúscula
- Máximo 64 caracteres
- Solo alphanumeric + guion bajo

**No está documentado** en `node_configurations.json`. Impacto: operador puede proponer un nombre de rama como "Check-Payment" o "Check_Payment_v2" y descubrir que falla la validación sin explicación clara en el documento.

### E. Conversión silenciosa de entrada a JSON

**Hallazgo:** Si la entrada no es un string (node.rs:71-74), se convierte a JSON mediante `serde_json::to_string_pretty()` antes de enrutarla. No está documentado.

```rust
let input_str = match input {
    serde_json::Value::String(s) => s.clone(),
    other => serde_json::to_string_pretty(&other).unwrap(),
};
```

Impacto: bajo; es una comodidad de conversión. Pero significa que un usuario que pase un número como entrada lo verá reformateado como JSON en el prompt del LLM.

---

## 3) Plan de pruebas QA

### Caso 1: Modo A (llm_direct) — routing básico con 2 ramas

**Objetivo:** Verificar que el modo `llm_direct` clasifica la entrada usando LLM y selecciona una rama por nombre exacto.

**Grafo JSON mínimo:**
```json
{
  "nodes": [
    {
      "node_type": "current_time",
      "config": { "id": "time" },
      "position": { "x": 0, "y": 0 }
    },
    {
      "node_type": "router",
      "config": {
        "id": "route_weather",
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "sunny",
            "description": "User is asking about sunny weather or warm climate"
          },
          {
            "name": "rainy",
            "description": "User is asking about rainy weather or precipitation"
          }
        ]
      },
      "position": { "x": 200, "y": 0 },
      "inputs": {
        "input": { "from_output": "time", "output": "output" }
      }
    }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run <graph.json> --agent-session-id qa_router_mode_a_001
```

**Entrada / prompt:** (Pasar una pregunta clara sobre clima)
```bash
--answer $'Q[route_weather]: What is the weather like today?\nA[route_weather]: It has been raining all morning'
```

**Resultado esperado:**
- `__decision.selected_branch == "rainy"`
- `__decision.reason` es un string (puede ser vacío)
- `rainy` port tiene output `{ "input": "<original_input>" }`
- `sunny` port es `null`
- No hay campo `extracted` en el output

**Verificación:** Revisar SSE o salida JSON; `__decision.selected_branch` debe ser `"rainy"`.

---

### Caso 2: Modo B (extract_and_route) — routing por extracción y DSL

**Objetivo:** Verificar que `extract_and_route` extrae campos según schema y aplica `when` DSL para seleccionar rama.

**Grafo JSON mínimo:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "extract_priority",
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "type": "object",
          "properties": {
            "priority": {
              "type": "string",
              "enum": ["high", "medium", "low"]
            },
            "category": {
              "type": "string"
            }
          },
          "required": ["priority", "category"]
        },
        "branches": [
          {
            "name": "urgent",
            "when": { "equals": { "field": "priority", "value": "high" } }
          },
          {
            "name": "backlog",
            "when": { "in": { "field": "priority", "values": ["medium", "low"] } }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run <graph.json> --agent-session-id qa_router_mode_b_001
```

**Entrada:**
```bash
--answer $'Q[extract_priority]: This is a critical bug fix needed immediately\nA[extract_priority]: extracted'
```

**Resultado esperado:**
- `__decision.selected_branch == "urgent"`
- `urgent` port tiene `{ "input": "<original>", "extracted": { "priority": "high", "category": "bug_fix" } }`
- `backlog` port es `null`
- `__decision.extracted` contiene el objeto extraído

**Verificación:** Validar que `extracted.priority == "high"` y rama seleccionada es `urgent`.

---

### Caso 3: Validación de nombre de rama — regex

**Objetivo:** Verificar que nombres de rama que violen `^[a-z][a-z0-9_]{0,63}$` son rechazados al cargar la config.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "bad_names",
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "Check-Payment",
            "description": "Payment branch"
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run <graph.json>
```

**Resultado esperado:** Error de validación durante la carga del grafo, rechazando el nombre `"Check-Payment"` (contiene mayúsculas y guion). Mensaje de error debe mencionar el patrón de nombre válido o al menos rechazar la rama.

**Verificación:** Verificar que el proceso falla antes de ejecutar cualquier nodo.

---

### Caso 4: Entrada vacía — fail-closed

**Objetivo:** Verificar que entrada `null`, string vacío, array vacío u objeto vacío causan error.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "route_empty",
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "fallback",
            "description": "Fallback branch"
          }
        ]
      },
      "position": { "x": 0, "y": 0 },
      "inputs": {
        "input": { "value": "" }
      }
    }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run <graph.json>
```

**Resultado esperado:** Error `RouterRuntimeError: missing input — nothing to route` sin intentar llamar al LLM.

**Verificación:** SSE debe mostrar evento de error; no debe haber llamada de LLM registrada.

---

### Caso 5: Modo B — ninguna rama coincide (`when` DSL false)

**Objetivo:** Verificar que cuando ninguna rama cumple su condición `when`, el router falla con un error que incluye el JSON extraído.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "no_match",
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "type": "object",
          "properties": {
            "status": { "type": "string", "enum": ["new", "open", "closed"] }
          },
          "required": ["status"]
        },
        "branches": [
          {
            "name": "new_tickets",
            "when": { "equals": { "field": "status", "value": "new" } }
          },
          {
            "name": "open_tickets",
            "when": { "equals": { "field": "status", "value": "open" } }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Entrada:**
```bash
--answer $'Q[no_match]: All my tickets are completed\nA[no_match]: extracted'
```

**Resultado esperado:** Error que contiene:
- `"RouterRuntimeError: no branch matched"`
- JSON extraído visible en el mensaje (p.ej., `"status": "closed"`)

**Verificación:** Error message debe incluir el objeto extraído para debugging.

---

### Caso 6: Modo A — LLM elige rama desconocida

**Objetivo:** Verificar que si el LLM elige un nombre de rama que no existe, el router falla.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "llm_bad_choice",
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "branch_a",
            "description": "Branch A"
          },
          {
            "name": "branch_b",
            "description": "Branch B"
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Entrada:** (Diseñada para llevar al LLM a elegir mal, si es posible; de lo contrario, usar mock o aceptar que es difícil de reproducir)

**Resultado esperado:** Si el LLM elige `"unknown_branch"`, error `RouterRuntimeError: llm picked unknown branch 'unknown_branch'`.

**Verificación:** Revisar SSE para mensaje de error.

---

### Caso 7: When DSL — dotted paths (campos anidados)

**Objetivo:** Verificar que `when` DSL soporta rutas de puntos como `"user.profile.tier"`.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "dotted_path",
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "type": "object",
          "properties": {
            "user": {
              "type": "object",
              "properties": {
                "profile": {
                  "type": "object",
                  "properties": {
                    "tier": { "type": "string", "enum": ["gold", "silver", "bronze"] }
                  }
                }
              }
            }
          }
        },
        "branches": [
          {
            "name": "vip",
            "when": { "equals": { "field": "user.profile.tier", "value": "gold" } }
          },
          {
            "name": "standard",
            "when": { "in": { "field": "user.profile.tier", "values": ["silver", "bronze"] } }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Entrada:** (Extraerá `user.profile.tier = "gold"`)

**Resultado esperado:** Rama `vip` seleccionada; dotted path evaluado correctamente.

**Verificación:** `__decision.selected_branch == "vip"`.

---

### Caso 8: When DSL — operadores múltiples (`all`, `any`, `not`)

**Objetivo:** Verificar que combinadores DSL funcionan: `all` (AND), `any` (OR), `not` (negación).

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "combinators",
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "type": "object",
          "properties": {
            "age": { "type": "integer" },
            "vip": { "type": "boolean" },
            "country": { "type": "string" }
          }
        },
        "branches": [
          {
            "name": "premium",
            "when": {
              "all": [
                { "gte": { "field": "age", "value": 18 } },
                { "equals": { "field": "vip", "value": true } }
              ]
            }
          },
          {
            "name": "restricted",
            "when": {
              "not": { "in": { "field": "country", "values": ["US", "CA", "UK"] } }
            }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Entrada:** (Extraerá `age=25, vip=true, country="FR"`)

**Resultado esperado:**
- Primera evaluación: `all` → age >= 18 (true) AND vip == true (true) → `premium` seleccionado
- Segunda evaluación: `not(country in [...])` → true (FR no está en lista) → `restricted` también cumple; según lógica de router, se elige la primera que coincida

**Verificación:** Al menos una rama debe seleccionarse; comportamiento con múltiples coincidencias debe ser determinista (típicamente: primera en la lista).

---

### Caso 9: When DSL — operador `exists`

**Objetivo:** Verificar que `exists: true` valida que un campo esté presente y no sea null.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "exists_check",
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "type": "object",
          "properties": {
            "optional_field": { "type": "string" }
          }
        },
        "branches": [
          {
            "name": "has_value",
            "when": { "exists": { "field": "optional_field", "value": true } }
          },
          {
            "name": "no_value",
            "when": { "not": { "exists": { "field": "optional_field", "value": true } } }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Entrada:** (Extraerá con o sin `optional_field`)

**Resultado esperado:**
- Si presente: rama `has_value`
- Si ausente o null: rama `no_value`

**Verificación:** Validar rama seleccionada según presencia del campo.

---

### Caso 10: Subgrafo como rama en modo A

**Objetivo:** Verificar que una rama puede ejecutar un subgrafo como destino, y SSE/eventos del subgrafo se propagan correctamente.

**Grafo padre (grafo principal):**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "route_to_subgraph",
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "process_order",
            "description": "Process an order",
            "subgraph": {
              "child_graph_path": "tests/graphs/qa/subgraph_simple.json"
            }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Grafo hijo (`tests/graphs/qa/subgraph_simple.json`):**
```json
{
  "nodes": [
    {
      "node_type": "add",
      "config": { "id": "add_numbers", "a": 10, "b": 20 },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run <padre.json> --agent-session-id qa_router_subgraph_001
```

**Resultado esperado:**
- Rama `process_order` seleccionada
- Subgrafo ejecutado
- Output del subgrafo (`add_numbers` → 30) propagado como payload de `process_order`
- SSE contiene eventos del subgrafo con path anidado

**Verificación:** `process_order` port tiene `{ "input": "...", "add_numbers": { "output": 30 } }` (u estructura similar del subgrafo).

---

### Caso 11: Validación de mutual exclusivity en subgrafo

**Objetivo:** Verificar que una rama con `child_graph_path` no puede tener también `child_graph_inline`, y vice versa.

**Grafo JSON (inválido):**
```json
{
  "nodes": [
    {
      "node_type": "router",
      "config": {
        "id": "bad_subgraph",
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "both_paths",
            "description": "Invalid: has both paths",
            "subgraph": {
              "child_graph_path": "path.json",
              "child_graph_inline": { "nodes": [] }
            }
          }
        ]
      },
      "position": { "x": 0, "y": 0 }
    }
  ]
}
```

**Resultado esperado:** Error de validación durante carga: "subgraph debe tener exactamente uno de child_graph_path o child_graph_inline".

**Verificación:** Proceso falla antes de ejecutar el nodo.

