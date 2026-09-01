# exponential — Auditoría QA (Documentación vs Código)

**Nodo:** `exponential`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs:123-161`  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.exponential`  
**Referencia de herramientas:** `docs/node_as_tools_reference.json` → `math_nodes`  
**Puertos de nodo:** `docs/agent_context/node_ports_reference.md` → tabla Math Nodes  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 node_as_tools_reference.json — Falta ejemplo específico para exponential

**Problema:** La sección `math_nodes` en `docs/node_as_tools_reference.json:1150-1165` describe que "Arithmetic nodes (add, subtract, multiply, divide, exponential) can be used as tools" y proporciona un ejemplo de `add_tool`, pero NO hay ejemplo concreto de `exponential` como herramienta LLM.

**Realidad en código:** `math.rs:123-161` define `ExponentialNode` con:
- Input: `"input"` (number) — el puerto de entrada por defecto
- Config: `"exponent"` (number, required) — parámetro fijo
- Output: `"output"` (number)

**Impacto:** Un operador que busca "¿cómo exponer exponential al LLM como herramienta?" no encuentra un ejemplo canónico y debe inferir desde `node_configurations.json`.

**Remediación:** Agregar ejemplo `exponential_tool` en `docs/node_as_tools_reference.json:1163` con:
```json
"exponential_tool": {
  "name": "calculate_power",
  "description": "Calcular una potencia: base^exponente.",
  "node_type": "exponential",
  "node_schema": {
    "input": { "type": "number", "required": true, "description": "Valor base a elevar." },
    "exponent": { "type": "number", "required": true, "description": "Potencia a la que elevar la base." }
  }
}
```

---

### 1.2 node_ports_reference.md — Descripción sin especificar el nombre del puerto input

**Problema:** En `docs/agent_context/node_ports_reference.md`, la fila de exponential dice:
```
| `exponential` | `input` | `output` | Power function — single numeric input |
```

El texto "single numeric input" es ambiguo — no aclara que ese input **se llama específicamente "input"** (a diferencia de `add`/`subtract`/`multiply`/`divide` que requieren "a" y "b" explícitamente nombrados).

**Impacto:** Bajo. Un usuario que lee la tabla ve la columna Input Port = "input", pero la descripción no refuerza que debe usar `{ "from": "source", "to": "exp", "to_port": "input" }` o depender del `default_input`.

**Recomendación:** Actualizar la descripción en `node_ports_reference.md` para ser paralela a otros nodos:
```
Power function — `output = input ^ exponent` (input from port, exponent in config)
```

---

### 1.3 developer_guide — Sin mención de exponential ni casos edge

**Problema:** Ninguno de los developer guides (`12_dag_engine_guide.md`, `48_python_dag.md`, `49_typescript_dag.md`) menciona el nodo `exponential` ni cómo usarlo.

**Comparación:** Otros nodos math (`add`, `subtract`, etc.) también están ausentes de los guides principales, por lo que esta no es una inconsistencia aislada. Sin embargo, casos especiales (raíz cuadrada con exponent 0.5, base negativa, etc.) NO están documentados en ningún lado.

**Impacto:** Medio. Un usuario que intenta calcular `sqrt(16)` (= 16^0.5) no encuentra documentación confirmando que `powf()` lo soporta con fractional exponents.

**Recomendación:** Considerar una sección en `docs/developer_guide/12_dag_engine_guide.md` (sección Math Nodes) que cubra:
- Exponentes fraccionarios (raíz cuadrada, cúbica, etc.)
- Comportamiento con valores especiales (base negativa, base 0, etc.)
- Límites numéricos (overflow en float64)

---

## 2. Hallazgos: Código

### 2.1 Sin validación de casos especiales — base negativa + exponent fraccionario

**Problema:** En `math.rs:140`, el código ejecuta `base.powf(exponent)` sin validar si el resultado será NaN (Not a Number):

```rust
let result = base.powf(exponent);
Ok(json!({ "output": result }))
```

**Realidad matemática:** En Rust/IEEE 754:
- `(-2.0_f64).powf(0.5)` → `NaN` (la raíz cuadrada de un número negativo)
- `0.0_f64.powf(-1.0)` → `inf` (1/0)
- Estos valores se serializan a JSON pero pueden sorprender al usuario

**Verificación:** Comportamiento de `powf()` es correcto según IEEE 754; el código NO está roto, solo que silent sobre NaN/inf.

**Impacto:** Bajo. El comportamiento es matemáticamente correcto; un usuario que intenta una operación inválida obtendrá NaN y puede interpretarlo como error.

**Recomendación (OPCIONAL):** Documentar en `node_configurations.json:exponent` que "Valores inválidos (p.ej. base negativa con exponent fraccionario) producirán NaN; valores infinitos producirán inf. Ambos se transmiten como números especiales en JSON."

---

### 2.2 Mensaje de error genérico en get_f64

**Problema:** En `math.rs:18-21`, la función `get_f64()` retorna:
```rust
fn get_f64(val: Option<&Value>, input_name: &str) -> Result<f64, MathError> {
    val.and_then(Value::as_f64)
        .ok_or_else(|| MathError::NotANumber(input_name.to_string()))
}
```

Para `exponential`, si el input es string (p.ej. `"5"` en lugar de `5`), el error será:
```
Entrada no es un número: input
```

**Impacto:** Bajo. El mensaje es claro; solo falta contexto que es el input del nodo exponential (vs. config.exponent).

**Verificación:** Correcto. El parámetro `input_name` se pasa con el nombre exacto ("input" o "config.exponent"), permitiendo al usuario saber dónde está el problema.

**Estado:** OK.

---

### 2.3 Schema() no especifica defaults para config.exponent

**Problema:** En `math.rs:153-160`, el método `schema()` retorna:
```rust
fn schema(&self) -> Value {
    json!({
        "type": "exponential",
        "inputs": {"input": "number"},
        "config": {"exponent": "number"},
        "outputs": {"output": "number"}
    })
}
```

El schema no incluye `"required"` o anotaciones sobre defaults.

**Comparación con documentación:** `docs/node_configurations.json` claramente marca `"required": true` para `exponent` y `"default": null`. El schema() en código no refleja esto.

**Impacto:** Bajo. El esquema de ejecución es simple; falta de anotaciones en `schema()` es una deficiency de metadatos, no un bug funcional.

**Verificación:** `node_configurations.json` es la fuente canónica de esquema y prevalece. El código ejecutable rechaza un grafo sin `exponent` (vía `get_f64(config.get("exponent"), ...)?`).

**Estado:** OK. `node_configurations.json` es authoritative.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying de estado.

### 3.1 Test A: Aritmética básica — cubo de 5

**Archivo:** `tests/graphs/basic/power.json` (ya existe)

**Verificación local:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/power.json --agent-session-id exp_test_a_001
```

**Validación esperada:**
- Input: 5
- Exponent: 3
- Output: 125
- El nodo retorna `125` directamente (no `{"output": 125}`)

**Estado:** ✅ Prueba actual funciona.

---

### 3.2 Test B: Raíz cuadrada con exponent fraccionario

**Archivo:** `tests/graphs/basic/exponential_sqrt.json` (crear)

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "input": 16
      }
    },
    "sqrt_step": {
      "type": "exponential",
      "config": {
        "exponent": 0.5
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "sqrt_step" },
    { "from": "sqrt_step", "to": "log_result" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/exponential_sqrt.json --agent-session-id exp_test_b_001
```

**Validación esperada:**
- Input: 16
- Exponent: 0.5 (raíz cuadrada)
- Output: 4.0 (aprox)
- Confirma que fractional exponents funcionan

---

### 3.3 Test C: Base negativa con exponent entero

**Archivo:** `tests/graphs/basic/exponential_negative_base.json` (crear)

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "input": -2
      }
    },
    "pow_step": {
      "type": "exponential",
      "config": {
        "exponent": 3
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "pow_step" },
    { "from": "pow_step", "to": "log_result" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/exponential_negative_base.json --agent-session-id exp_test_c_001
```

**Validación esperada:**
- Input: -2
- Exponent: 3
- Output: -8
- Confirma que (-2)^3 = -8 (IEEE 754 bien definido)

---

### 3.4 Test D: Input tipo string (error path)

**Archivo:** `tests/graphs/basic/exponential_type_error.json` (crear)

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "input": "five"
      }
    },
    "pow_step": {
      "type": "exponential",
      "config": {
        "exponent": 2
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "pow_step" },
    { "from": "pow_step", "to": "log_result" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/exponential_type_error.json --agent-session-id exp_test_d_001
```

**Validación esperada:**
- Input: "five" (string, not number)
- El nodo retorna error: `"Entrada no es un número: input"`
- Grafo se detiene con status FAILED

---

### 3.5 Test E: Como herramienta LLM (node_schema)

**Archivo:** `tests/graphs/agents/exponential_as_tool.json` (crear)

```json
{
  "nodes": {
    "llm": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_message": "Calcula 2 elevado a la potencia 10."
      },
      "tool_configurations": {
        "power_calculator": {
          "name": "power_calculator",
          "description": "Calcular base^exponent. Útil para operaciones exponenciales.",
          "node_type": "exponential",
          "node_schema": {
            "input": { "type": "number", "required": true, "description": "Valor base." },
            "exponent": { "type": "number", "required": true, "description": "Exponente." }
          }
        }
      }
    }
  }
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/exponential_as_tool.json --agent-session-id exp_test_e_001
```

**Validación esperada:**
- LLM llama al tool `power_calculator` con `input: 2, exponent: 10`
- Output: 1024
- LLM retorna la respuesta en su mensaje final
- Confirma que exponential funciona como herramienta de agente

---

## Resumen de Auditoría

| Categoría | Hallazgos | Severity | Remediación |
|-----------|-----------|----------|-------------|
| Documentación — math_nodes | Falta ejemplo exponential en node_as_tools_reference.json | Bajo | Agregar ejemplo JSON (3-5 líneas) |
| Documentación — node_ports | Descripción sin claridad sobre nombre del puerto | Muy Bajo | Mejorar redacción (1 frase) |
| Documentación — developer_guide | Sin mención de exponential o casos especiales | Bajo | Opcional: sección sobre math nodes + casos edge |
| Código — validación | Sin validar NaN/inf en casos especiales | Muy Bajo | Opcional: documentar comportamiento de edge cases |
| Código — errores | Mensaje de error genérico pero correcto | OK | N/A |
| Código — schema | Falta anotaciones required/default en schema() | Muy Bajo | No afecta; node_configurations.json es authoritative |

