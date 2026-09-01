# QA — Nodo `multiply`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs:71–93`
Fuentes de doc revisadas:
- `docs/node_configurations.json:797–822`
- `docs/node_as_tools_reference.json:2190–2191`
- `docs/agent_context/node_ports_reference.md:37,106`

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación en `node_configurations.json` especifica correctamente que `config_fields` es vacío y que ambos inputs (`a` y `b`) son requeridos y de tipo `number`. El código coincide exactamente: no valida campos de config (línea 78: `_config: &Value`) y requiere dos inputs f64 vía `get_f64()`.

## 2) Código NO documentado

**Hallazgo S2.1:** Error no tipificado en documentación

El código en `math.rs:18–21` define y lanza `MathError::NotANumber` cuando un input no es un número JSON válido:

```rust
fn get_f64(val: Option<&Value>, input_name: &str) -> Result<f64, MathError> {
    val.and_then(Value::as_f64)
        .ok_or_else(|| MathError::NotANumber(input_name.to_string()))
}
```

Cuando `a` o `b` no se puede convertir a f64, la ejecución falla con error `"Entrada no es un número: {0}"` (línea 12). Las fuentes de doc revisadas NO mencionan este error fail-closed ni los casos que lo desencadenan. `node_configurations.json` describe solo los tipos esperados (number) pero no qué sucede si se incumple.

## 3) Plan de pruebas QA

### Caso 3.1: Happy path — multiplicación básica de enteros

**Objetivo:** Verificar que dos enteros se multiplican correctamente.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input_a": { "type": "input", "config": { "data": { "a": 3 } } },
    "input_b": { "type": "input", "config": { "data": { "b": 4 } } },
    "multiply": { "type": "multiply" },
    "output": { "type": "output" }
  },
  "edges": [
    { "from": "input_a.a", "to": "multiply.a" },
    { "from": "input_b.b", "to": "multiply.b" },
    { "from": "multiply.output", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run /tmp/test_multiply_basic.json
```

**Resultado esperado:** Output node recibe `{ "output": 12 }`

**Verificación:** En la salida SSE, la clave `output` del nodo output debe ser `12`.

---

### Caso 3.2: Happy path — multiplicación con decimales

**Objetivo:** Verificar que números float se multiplican correctamente.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input_a": { "type": "input", "config": { "data": { "a": 2.5 } } },
    "input_b": { "type": "input", "config": { "data": { "b": 4 } } },
    "multiply": { "type": "multiply" },
    "output": { "type": "output" }
  },
  "edges": [
    { "from": "input_a.a", "to": "multiply.a" },
    { "from": "input_b.b", "to": "multiply.b" },
    { "from": "multiply.output", "to": "output.input" }
  ]
}
```

**Resultado esperado:** `{ "output": 10.0 }`

**Verificación:** La salida debe ser el producto exacto de 2.5 × 4.

---

### Caso 3.3: Happy path — multiplicación con números negativos

**Objetivo:** Verificar que multiplicación con negativos y reglas de signo se respetan.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input_a": { "type": "input", "config": { "data": { "a": -3 } } },
    "input_b": { "type": "input", "config": { "data": { "b": 4 } } },
    "multiply": { "type": "multiply" },
    "output": { "type": "output" }
  },
  "edges": [
    { "from": "input_a.a", "to": "multiply.a" },
    { "from": "input_b.b", "to": "multiply.b" },
    { "from": "multiply.output", "to": "output.input" }
  ]
}
```

**Resultado esperado:** `{ "output": -12 }`

**Verificación:** Confirmación de que el signo se calcula correctamente.

---

### Caso 3.4: Edge case — multiplicación por cero (a = 0)

**Objetivo:** Verificar que el resultado es 0 cuando a = 0, sin errores especiales.

**Entrada:** a = 0, b = 5

**Resultado esperado:** `{ "output": 0 }`

**Verificación:** Sin errores; output exacto = 0.

---

### Caso 3.5: Edge case — multiplicación por cero (b = 0)

**Objetivo:** Verificar que el resultado es 0 cuando b = 0.

**Entrada:** a = 100, b = 0

**Resultado esperado:** `{ "output": 0 }`

**Verificación:** Sin errores; output exacto = 0.

---

### Caso 3.6: Error fail-closed — input `a` no es un número (string)

**Objetivo:** Verificar que el nodo rechaza con error cuando `a` es una cadena de texto.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input_a": { "type": "input", "config": { "data": { "a": "not_a_number" } } },
    "input_b": { "type": "input", "config": { "data": { "b": 4 } } },
    "multiply": { "type": "multiply" }
  },
  "edges": [
    { "from": "input_a.a", "to": "multiply.a" },
    { "from": "input_b.b", "to": "multiply.b" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run /tmp/test_multiply_error_a.json
```

**Resultado esperado:** El nodo falla con error `MathError::NotANumber("a")` → mensaje de error contiene "Entrada no es un número: a"

**Verificación:** En la salida SSE, el nodo multiply emite un evento de error; el DAG run finaliza con estado error y el mensaje de error es visible.

---

### Caso 3.7: Error fail-closed — input `b` no es un número (null)

**Objetivo:** Verificar que el nodo rechaza cuando `b` es null.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input_a": { "type": "input", "config": { "data": { "a": 3 } } },
    "input_b": { "type": "input", "config": { "data": { "b": null } } },
    "multiply": { "type": "multiply" }
  },
  "edges": [
    { "from": "input_a.a", "to": "multiply.a" },
    { "from": "input_b.b", "to": "multiply.b" }
  ]
}
```

**Resultado esperado:** Error `MathError::NotANumber("b")` → "Entrada no es un número: b"

**Verificación:** El DAG falla con error visible en SSE y en la salida estándar del comando.
