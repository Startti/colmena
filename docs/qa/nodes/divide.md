# QA — Nodo `divide`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs:95-121`  
Fuentes de doc revisadas:
- `docs/node_configurations.json:824-849`
- `docs/node_as_tools_reference.json` (no tiene entrada específica)
- `docs/agent_context/node_ports_reference.md` (entrada en tabla)

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. El código y la documentación coinciden:
- `config_fields: {}` — el nodo no acepta parámetros de configuración.
- Inputs `a` (dividend) y `b` (divisor) son ambos requeridos y tipo `number`.
- Output `output` es tipo `number` (float64).
- Error en división por cero está documentado y es soportado por el código (línea 109-111).

## 2) Código NO documentado

### Hallazgo 2.1: `node_ports_reference.md` oculta el error de división por cero

**Fuente:** `docs/agent_context/node_ports_reference.md` (tabla de nodos Math)

**Qué dice la doc:**
```
| **divide** | Division | `output = a / b` (requires explicit `.a`, `.b` fields) |
```

**Qué hace el código:**
```
// math.rs:109-111
if b == 0.0 {
    return Err(Box::new(MathError::DivisionByZero));
}
```

**Impacto QA:** Un tester consultando `node_ports_reference.md` no sabe que `b=0` produce un error fail-closed con el nombre `DivisionByZero`. Solo `node_configurations.json` lo documenta (línea 836). Una tabla de puertos debe listar errores posibles, no solo outputs felices.

### Hallazgo 2.2: Inputs no están marcados como "required" en el schema

**Fuente:** `math.rs:119` (método `schema()`)

**Qué dice el código:**
```json
{"type": "divide", "inputs": {"a": "number", "b": "number"}, "outputs": {"output": "number"}}
```

**Implementación real (`math.rs:106-107`):**
```rust
let a = get_f64(inputs.get("a"), "a")?;  // Falla si "a" no existe o no es número
let b = get_f64(inputs.get("b"), "b")?;  // Falla si "b" no existe o no es número
```

**Impacto QA:** El schema `schema()` no marca `a` ni `b` como `"required": true`, pero el método `execute` retorna error si falta alguno. Si un grafo se construye sin conectar `a` o `b` al nodo `divide`, el ejecutor dará error `NotANumber: "a"` en lugar de una validación estática. `node_configurations.json` tampoco marca estos campos explícitamente como required, aunque el texto lo implica ("Both operands must be provided").

### Hallazgo 2.3: `node_as_tools_reference.json` tiene brecha en ejemplos

**Fuente:** `docs/node_as_tools_reference.json` (sección "Math Nodes")

**Estado:** Existe ejemplo para `add_tool` pero no para `divide` (ni `subtract`, `multiply`, `exponential`). Aunque `divide` está listado en `node_types: ["add", "subtract", "multiply", "divide", "exponential"]`, no hay ejemplo práctico de cómo usarlo como LLM tool con `tool_configurations`.

**Impacto QA:** Un agente LLM que intenta usar `divide` como tool no tiene documentación clara de la sintaxis esperada (`node_schema` con campos `a` y `b`, salida vía `output`, etc.).

## 3) Plan de pruebas QA

### TC 1: División normal (happy path)
- **Objetivo:** Verificar que división básica con números enteros funciona.
- **Grafo mínimo:**
  ```json
  {
    "name": "divide_happy_path",
    "nodes": [
      {
        "id": "input_a",
        "type": "input",
        "config": {"data": 10}
      },
      {
        "id": "input_b",
        "type": "input",
        "config": {"data": 2}
      },
      {
        "id": "divide_node",
        "type": "divide"
      },
      {
        "id": "output",
        "type": "output"
      }
    ],
    "edges": [
      {"from": "input_a", "to": "divide_node", "to_input": "a"},
      {"from": "input_b", "to": "divide_node", "to_input": "b"},
      {"from": "divide_node", "to_output", "from_output": "output"}
    ]
  }
  ```
- **Comando:** `cargo run --bin dag_engine -- run <grafo.json>`
- **Entrada:** a=10, b=2
- **Resultado esperado:** output=5.0 en `divide_node`
- **Verificación:** output.result.output === 5.0

### TC 2: División con decimales
- **Objetivo:** Verificar precisión con números fraccionarios.
- **Entrada:** a=7.5, b=2.5
- **Resultado esperado:** output=3.0
- **Verificación:** output.result.output === 3.0

### TC 3: División por cero (fail-closed)
- **Objetivo:** Verificar que división por b=0 produce error `DivisionByZero`.
- **Entrada:** a=10, b=0
- **Resultado esperado:** Nodo `divide_node` retorna error con mensaje "División por cero"
- **Verificación:** 
  - Evento SSE con `"error": "División por cero"` en el resultado del nodo
  - El error es de tipo `MathError::DivisionByZero` (interno)
  - El DAG se detiene (no intenta ejecutar nodos posteriores)

### TC 4: Input `a` no es número
- **Objetivo:** Verificar validación de tipo en el input `a`.
- **Entrada:** a="texto", b=2
- **Resultado esperado:** Error `NotANumber: "a"`
- **Verificación:**
  - Evento SSE con `"error": "Entrada no es un número: a"` en `divide_node`
  - El DAG se detiene

### TC 5: Input `b` no es número
- **Objetivo:** Verificar validación de tipo en el input `b`.
- **Entrada:** a=10, b="texto"
- **Resultado esperado:** Error `NotANumber: "b"`
- **Verificación:**
  - Evento SSE con `"error": "Entrada no es un número: b"` en `divide_node`

### TC 6: Resultado negativo
- **Objetivo:** Verificar división con resultado negativo.
- **Entrada:** a=-10, b=2
- **Resultado esperado:** output=-5.0
- **Verificación:** output.result.output === -5.0

### TC 7: Ambos operandos negativos
- **Objetivo:** Verificar división con ambos operandos negativos.
- **Entrada:** a=-10, b=-2
- **Resultado esperado:** output=5.0 (negativo ÷ negativo = positivo)
- **Verificación:** output.result.output === 5.0

### TC 8: Magnitudes muy pequeñas
- **Objetivo:** Verificar precisión en float64 con números muy pequeños.
- **Entrada:** a=0.0001, b=0.01
- **Resultado esperado:** output≈0.01 (dentro de tolerancia float64)
- **Verificación:** Comparar con tolerancia: abs(output.result.output - 0.01) < 1e-9

### TC 9: Input `a` falta (null o ausente)
- **Objetivo:** Verificar comportamiento cuando falta el input `a`.
- **Entrada:** a=null (o arista no conectada), b=2
- **Resultado esperado:** Error `NotANumber: "a"`
- **Verificación:** Evento SSE con error en `divide_node`

### TC 10: Input `b` falta (null o ausente)
- **Objetivo:** Verificar comportamiento cuando falta el input `b`.
- **Entrada:** a=10, b=null (o arista no conectada)
- **Resultado esperado:** Error `NotANumber: "b"`
- **Verificación:** Evento SSE con error en `divide_node`

### TC 11: Uso como LLM tool
- **Objetivo:** Verificar que `divide` funciona cuando se usa en `tool_configurations` de un `llm_call`.
- **Grafo mínimo:**
  ```json
  {
    "nodes": [
      {
        "id": "llm",
        "type": "llm_call",
        "config": {
          "provider": "google",
          "model": "gemini-2.5-flash"
        },
        "tool_configurations": {
          "my_divide": {
            "name": "my_divide",
            "description": "Divide two numbers",
            "node_type": "divide",
            "node_schema": {
              "a": {"type": "number", "required": true, "description": "Dividend"},
              "b": {"type": "number", "required": true, "description": "Divisor"}
            }
          }
        }
      }
    ]
  }
  ```
- **Comando:** `cargo run --bin dag_engine -- run <grafo.json>`
- **Entrada (prompt):** "Divide 15 by 3"
- **Resultado esperado:** 
  - LLM invoca `my_divide` tool con a=15, b=3
  - Tool retorna output=5.0
  - LLM incluye el resultado en su respuesta final
- **Verificación:** 
  - Evento SSE con `tool-use` mostrando el tool call
  - Evento SSE con `tool-result` mostrando output=5.0
  - LLM procesa correctamente y responde

