# QA — Nodo `subtract`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs:47-69`
Fuentes de doc revisadas:
- `docs/node_configurations.json:770-795`
- `docs/node_as_tools_reference.json` (líneas 23, 2191 — listas válidas solamente)
- `docs/agent_context/node_ports_reference.md:36, 105`

---

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La doc declara `config_fields: {}` (vacío) y el código ignora el parámetro `config` (línea 54: `_config: &Value`), coherente con la especificación.

---

## 2) Código NO documentado

### Hallazgo S2.1: Validación de tipos numéricos

- **Dónde ocurre**: `math.rs:18-21` en `get_f64()`
- **Qué hace el código**:
  ```rust
  fn get_f64(val: Option<&Value>, input_name: &str) -> Result<f64, MathError> {
      val.and_then(Value::as_f64)
          .ok_or_else(|| MathError::NotANumber(input_name.to_string()))
  }
  ```
  Retorna error `MathError::NotANumber` si la entrada no es un número válido (NaN, string, bool, null, etc.)

- **Qué dice la doc**:
  - `node_configurations.json`: "Must be a valid JSON number" (p/ ambas entradas), pero NO especifica qué sucede si no lo es.
  - `node_ports_reference.md`: no menciona comportamiento de error.

- **Impacto QA**: Sin documentación del error `NotANumber`, QA no sabe si pasar `"a": "hello"` debe fallar silenciosamente, ignorar la entrada, o lanzar un error específico.

### Hallazgo S2.2: Tipo de número específico (f64)

- **Dónde ocurre**: `math.rs:19` con `Value::as_f64`
- **Qué hace el código**: La sustracción opera internamente con `f64` (punto flotante de 64 bits). Esto implica comportamiento específico (precisión, rango, infinito, -0.0).

- **Qué dice la doc**:
  - `node_configurations.json`: "type": "number" (JSON Schema genérico, no especifica f64).
  - Ninguna doc menciona que el tipo es de punto flotante ni sus límites.

- **Impacto QA**: QA no sabe si valores enteros muy grandes (> 2^53) se redondean silenciosamente o si hay garantías de exactitud.

---

## 3) Plan de pruebas QA

### Prueba S3.1: Happy path — números positivos

**Objetivo**: Verificar que resta simple con números positivos funciona.

**Grafo mínimo** (tests/graphs/qa/subtract_simple_positive.json):
```json
{
  "nodes": {
    "inputs": { "type": "input", "config": { "a": 10, "b": 3 } },
    "math": { "type": "subtract" },
    "output": { "type": "output" }
  },
  "edges": [
    { "from": "inputs.a", "to": "math.a" },
    { "from": "inputs.b", "to": "math.b" },
    { "from": "math.output", "to": "output.input" }
  ]
}
```

**Comando**: `cargo run --bin dag_engine -- run tests/graphs/qa/subtract_simple_positive.json`

**Entrada**: a=10, b=3

**Resultado esperado**: `{ "result": { "output": 7 }, "extra_info": { "__colmena_is_output_node": true } }`

**Verificación**: Parse JSON, assert result.output == 7

---

### Prueba S3.2: Happy path — números negativos

**Objetivo**: Verificar resta con operandos negativos.

**Grafo**: idéntico, inputs: `{"a": -5, "b": 3}`

**Resultado esperado**: output == -8

**Verificación**: output == -8

---

### Prueba S3.3: Happy path — decimales

**Objetivo**: Verificar que números de punto flotante se restan correctamente.

**Grafo**: idéntico, inputs: `{"a": 10.5, "b": 2.3}`

**Resultado esperado**: output ≈ 8.2 (permitir epsilon de 1e-10)

**Verificación**: |output - 8.2| < 1e-10

---

### Prueba S3.4: Caso límite — minuendo es cero

**Objetivo**: Verificar que 0 - b funciona.

**Grafo**: idéntico, inputs: `{"a": 0, "b": 5}`

**Resultado esperado**: output == -5

**Verificación**: output == -5

---

### Prueba S3.5: Caso límite — sustraendo es cero

**Objetivo**: Verificar que a - 0 funciona.

**Grafo**: idéntico, inputs: `{"a": 10, "b": 0}`

**Resultado esperado**: output == 10

**Verificación**: output == 10

---

### Prueba S3.6: Caso límite — ambos ceros

**Objetivo**: Verificar que 0 - 0 = 0.

**Grafo**: idéntico, inputs: `{"a": 0, "b": 0}`

**Resultado esperado**: output == 0

**Verificación**: output == 0

---

### Prueba S3.7: Error — entrada 'a' es string

**Objetivo**: Verificar que se rechaza entrada no numérica en 'a'.

**Grafo**: idéntico, inputs: `{"a": "hello", "b": 3}`

**Resultado esperado**: Error con mensaje que contenga "Entrada no es un número: a" (de MathError::NotANumber, math.rs:11)

**Verificación**: SSE contiene frame tipo `execution-error` con error text, o salida de JSON contiene `"error"` key.

---

### Prueba S3.8: Error — entrada 'b' es string

**Objetivo**: Verificar que se rechaza entrada no numérica en 'b'.

**Grafo**: idéntico, inputs: `{"a": 10, "b": "world"}`

**Resultado esperado**: Error "Entrada no es un número: b"

**Verificación**: SSE contiene error.

---

### Prueba S3.9: Error — falta entrada 'a'

**Objetivo**: Verificar que se rechaza si 'a' no viene en inputs.

**Grafo**: como S3.1 pero sin edge from inputs.a

**Resultado esperado**: Error "Entrada no es un número: a" (porque `inputs.get("a")` retorna None, get_f64 lo interpreta como no número)

**Verificación**: SSE contiene error.

---

### Prueba S3.10: Error — falta entrada 'b'

**Objetivo**: Verificar que se rechaza si 'b' no viene en inputs.

**Grafo**: como S3.1 pero sin edge from inputs.b

**Resultado esperado**: Error "Entrada no es un número: b"

**Verificación**: SSE contiene error.

---

### Prueba S3.11: Precisión de punto flotante — números grandes

**Objetivo**: Verificar comportamiento con valores f64 en rango grande.

**Grafo**: idéntico, inputs: `{"a": 1e20, "b": 1}`

**Resultado esperado**: output == 1e20 (la precisión de f64 se redondea, así que 1e20 - 1 ≠ (1e20 - 1) exacto, pero debe calcular sin error/overflow)

**Verificación**: output es numérico, no NaN, no infinity.

---

### Prueba S3.12: Uso como tool del LLM

**Objetivo**: Verificar que subtract se puede declarar en `tool_configurations` de un llm_call.

**Grafo** (tests/graphs/qa/subtract_as_llm_tool.json):
```json
{
  "nodes": {
    "llm": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "prompt": "¿Cuánto es 15 menos 7? Usa la herramienta de resta para calcularlo.",
        "tool_configurations": {
          "math_subtract": {
            "name": "math_subtract",
            "description": "Resta dos números: a - b",
            "node_type": "subtract",
            "node_schema": {
              "a": { "type": "number", "required": true, "description": "Minuendo" },
              "b": { "type": "number", "required": true, "description": "Sustraendo" }
            }
          }
        }
      }
    },
    "output": { "type": "output" }
  },
  "edges": [
    { "from": "llm", "to": "output" }
  ]
}
```

**Comando**: `cargo run --bin dag_engine -- run tests/graphs/qa/subtract_as_llm_tool.json`

**Resultado esperado**: LLM llama tool math_subtract con a=15, b=7; recibe output=8; respuesta menciona que 15 - 7 = 8.

**Verificación**: SSE contiene tool-call frame para math_subtract, tool-result con output=8, y respuesta final contiene "8" o "ocho".

---

**Resumen de casos cubiertos:**
- 6 happy path / límites (positivos, negativos, decimales, ceros)
- 4 errores fail-closed (entrada no numérica o faltante)
- 1 precisión numérica
- 1 integración como LLM tool

**Total: 12 casos de prueba**
