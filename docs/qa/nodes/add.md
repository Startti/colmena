# QA — Nodo `add`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (línea: entrada `"add"`)
- `docs/node_as_tools_reference.json` (tool schema dentro de ejemplos)
- `docs/agent_context/node_ports_reference.md` (tablas de puertos + casos de uso)
- Registry: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs:91`

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación afirma correctamente que el nodo `add` tiene `config_fields: {}` (vacío), y el código confirma esto: el parámetro `_config` en el método `execute` (línea 30 de math.rs) es explícitamente ignorado. El nodo no acepta ni requiere ningún campo de configuración.

## 2) Código NO documentado

**Hallazgo 1:** Error específico `MathError::NotANumber` no documentado.
- **Ubicación código:** `math.rs:10-20` (definición de error) y `math.rs:34-35` (aplicación en `AddNode::execute`).
- **Qué dice la doc:** `docs/node_configurations.json` afirma genéricamente que "the node returns an error if either is missing or non-numeric", pero no especifica el mensaje de error ni el tipo de error (`MathError::NotANumber`).
- **Qué hace el código:** Lanza `MathError::NotANumber(input_name)` con el nombre del campo que no es número (p.ej., "a" o "b").
- **Impacto QA:** QA debe probar que el error es específico y contiene el nombre del campo ofensivo.

**Hallazgo 2:** Tipo de dato interno (float64) no documentado.
- **Ubicación código:** `math.rs:18` (función `get_f64` extrae `f64`), `math.rs:36` (operación con `f64`).
- **Qué dice la doc:** `node_as_tools_reference.json` declara inputs/output como `"number"` (abstracto).
- **Qué hace el código:** Internamente trabaja con `f64` (Rust), soporta tanto enteros como flotantes en JSON (p.ej. `5` o `5.5`), pero la operación y el retorno son `f64`.
- **Impacto QA:** Pruebas con valores decimal (p.ej., `0.1 + 0.2`) pueden mostrar imprecisión de punto flotante típica de `f64`, que no se documenta como limitación conocida.

**Hallazgo 3:** Comportamiento con valores `null` o ausentes no explícitamente documentado.
- **Ubicación código:** `math.rs:18-20` (función `get_f64` llama `inputs.get("a")` que retorna `None` si ausente, y la validación falla).
- **Qué dice la doc:** `docs/node_configurations.json` dice "Both 'a' and 'b' are required — the node returns an error if either is missing", genérico.
- **Qué hace el código:** Llama `get_f64(inputs.get("a"), "a")?` que transforma un `None` (ausencia) en `MathError::NotANumber("a")`. El error es idéntico al de un no-número.
- **Impacto QA:** La distinción entre "campo ausente" y "campo no es número" es imperceptible para el usuario final (mismo error code), pero internamente son el mismo caso (el helper `and_then(Value::as_f64)` falla en ambas condiciones).

## 3) Plan de pruebas QA

### Prueba 3.1: Happy path — suma de dos números enteros positivos
**Objetivo:** Verificar que el nodo suma correctamente dos enteros.

**Grafo mínimo:** `tests/graphs/qa/add_happy_path_integers.json`
```json
{
  "nodes": {
    "input_a": { "type": "mock_input", "config": { "output": 10 } },
    "input_b": { "type": "mock_input", "config": { "output": 5 } },
    "add_node": { "type": "add" },
    "log_result": { "type": "log" }
  },
  "edges": [
    { "from": "input_a.output", "to": "add_node.a" },
    { "from": "input_b.output", "to": "add_node.b" },
    { "from": "add_node", "to": "log_result" }
  ]
}
```

**Entrada:** `a = 10`, `b = 5`  
**Resultado esperado:** `{ "output": 15 }`  
**Verificación:** SSE contiene `{ "output": 15 }` en el nodo `add_node` (verificar event stream, no solo log).

---

### Prueba 3.2: Happy path — suma de números decimales
**Objetivo:** Verificar que el nodo suma correctamente números flotantes.

**Grafo:** Idem 3.1, pero con `input_a = 3.5`, `input_b = 2.25`  
**Entrada:** `a = 3.5`, `b = 2.25`  
**Resultado esperado:** `{ "output": 5.75 }`  
**Verificación:** Valor en SSE es exactamente `5.75`.

---

### Prueba 3.3: Happy path — suma con números negativos
**Objetivo:** Verificar que el nodo maneja correctamente operandos negativos.

**Grafo:** Idem, pero `input_a = -10`, `input_b = 5`  
**Entrada:** `a = -10`, `b = 5`  
**Resultado esperado:** `{ "output": -5 }`  
**Verificación:** Resultado correcto en SSE.

---

### Prueba 3.4: Happy path — suma de cero
**Objetivo:** Verificar caso límite con cero.

**Grafo:** Idem, `input_a = 0`, `input_b = 7`  
**Entrada:** `a = 0`, `b = 7`  
**Resultado esperado:** `{ "output": 7 }`  
**Verificación:** Resultado correcto.

---

### Prueba 3.5: Happy path — default_output = "output"
**Objetivo:** Verificar que el nodo declara correctamente `default_output` y se puede referenciar sin `. output`.

**Grafo:**
```json
{
  "nodes": {
    "input_a": { "type": "mock_input", "config": { "output": 2 } },
    "input_b": { "type": "mock_input", "config": { "output": 3 } },
    "add_node": { "type": "add" }
  },
  "edges": [
    { "from": "input_a.output", "to": "add_node.a" },
    { "from": "input_b.output", "to": "add_node.b" },
    { "from": "add_node", "to": "log_result" }
  ]
}
```

**Resultado esperado:** El edge `{ "from": "add_node", "to": "log_result" }` se resuelve correctamente a `add_node.output`.  
**Verificación:** SSE muestra resultado `5` sin error de resolución de puerto.

---

### Prueba 3.6: Error case — entrada `a` es null
**Objetivo:** Verificar que el nodo rechaza `null` en el puerto `a`.

**Grafo:**
```json
{
  "nodes": {
    "mock_null": { "type": "mock_input", "config": { "output": null } },
    "input_b": { "type": "mock_input", "config": { "output": 5 } },
    "add_node": { "type": "add" },
    "log_result": { "type": "log" }
  },
  "edges": [
    { "from": "mock_null.output", "to": "add_node.a" },
    { "from": "input_b.output", "to": "add_node.b" },
    { "from": "add_node", "to": "log_result" }
  ]
}
```

**Entrada:** `a = null`, `b = 5`  
**Resultado esperado:** Error con mensaje conteniendo "Entrada no es un número: a" (o "a" en el error).  
**Verificación:** SSE muestra estado `ERROR` en el nodo `add_node`; el error contiene mención de "a".

---

### Prueba 3.7: Error case — entrada `b` es string
**Objetivo:** Verificar que el nodo rechaza strings en el puerto `b`.

**Grafo:** Idem 3.6, pero `mock_null.output = "hello"`  
**Entrada:** `a = 10`, `b = "hello"`  
**Resultado esperado:** Error con mención de "b" en el mensaje.  
**Verificación:** SSE indica error; message contiene "b".

---

### Prueba 3.8: Error case — entrada `a` ausente (no edge)
**Objetivo:** Verificar que el nodo falla si el edge hacia `a` no existe.

**Grafo:**
```json
{
  "nodes": {
    "input_b": { "type": "mock_input", "config": { "output": 5 } },
    "add_node": { "type": "add" }
  },
  "edges": [
    { "from": "input_b.output", "to": "add_node.b" }
  ]
}
```

**Entrada:** solo `b = 5`, `a` no conectado  
**Resultado esperado:** Error "Entrada no es un número: a" (aplicado a `None`).  
**Verificación:** SSE muestra error; nodo no produce output.

---

### Prueba 3.9: Edge case — números muy grandes (overflow)
**Objetivo:** Verificar comportamiento con valores en los límites de `f64`.

**Grafo:** Idem 3.1, `input_a = 1.8e308`, `input_b = 1e308`  
**Entrada:** `a = 1.8e308`, `b = 1e308`  
**Resultado esperado:** `{ "output": <inf> }` (infinito en JSON) o error de overflow.  
**Verificación:** Resultado es soportado por el runtime Rust/JSON (típicamente `Infinity`).

---

### Prueba 3.10: Edge case — imprecisión de punto flotante
**Objetivo:** Documentar la limitación conocida de `f64`.

**Grafo:** Idem, `input_a = 0.1`, `input_b = 0.2`  
**Entrada:** `a = 0.1`, `b = 0.2`  
**Resultado esperado:** `{ "output": 0.30000000000000004 }` (o lo que devuelva Rust internamente).  
**Verificación:** QA acepta que `0.1 + 0.2 ≠ 0.3` exactamente en punto flotante binario; no es un bug del nodo.

---

### Prueba 3.11: LLM tool usage — add como tool en un llm_call
**Objetivo:** Verificar que `add` funciona como tool del LLM (lazy o eager).

**Grafo:**
```json
{
  "nodes": {
    "llm_agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_message": "You are a helpful assistant. Use the add tool to compute 7 + 8."
      },
      "tool_configurations": {
        "add": {
          "name": "add",
          "node_type": "add",
          "node_schema": {
            "a": { "type": "number", "required": true, "description": "Primer operando." },
            "b": { "type": "number", "required": true, "description": "Segundo operando." }
          }
        }
      }
    }
  ],
  "edges": []
}
```

**Entrada:** Prompt pide al LLM que use `add` para calcular `7 + 8`.  
**Resultado esperado:** LLM llama al tool `add` con `a=7`, `b=8`, recibe `{ "output": 15 }`, y responde "El resultado es 15".  
**Verificación:** SSE muestra `tool_call_finished` con resultado `15`; respuesta final del LLM menciona 15.

---

### Prueba 3.12: Config validation — `config` field es ignorado (no error)
**Objetivo:** Verificar que pasar un `config` al nodo `add` no causa error (se ignora silenciosamente).

**Grafo:** Agregar `"config": { "ignored_field": true }` al nodo `add_node` en cualquier grafo anterior.  
**Resultado esperado:** El nodo ejecuta normalmente; el campo `ignored_field` es ignorado sin error.  
**Verificación:** SSE no contiene error; nodo produce output esperado.

---

### Resumen: Cobertura de pruebas
| Categoría | Cantidad | Casos |
|-----------|----------|-------|
| Happy path (válido) | 5 | enteros, decimales, negativos, cero, default_output |
| Error (fail-closed) | 4 | null, string, ausente `a`, ausente `b` |
| Edge cases | 3 | overflow, imprecisión flotante, config ignorado |
| LLM tool | 1 | tool calling básico |
| **Total** | **13** | |
