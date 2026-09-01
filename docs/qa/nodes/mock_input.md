# QA — Nodo `mock_input`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/debug.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json`
- `docs/agent_context/node_ports_reference.md`
- `docs/DEVELOPER_GUIDE.md` (sin mención explícita)

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas.

La documentación afirma que:
- El nodo emite su config sin transformación (línea: "emits its entire config object as output")
- Cualquier campo en config se incluye en la salida (línea: "Any key-value pair defined in config becomes part of the emitted output")
- No hay un `default_output` (null)
- No hace wrapping en `{"output": ...}` — emite raw config

El código (debug.rs:66-74) confirma exactamente esto:
```
async fn execute(..., config: &Value, ...) -> Result<Value, ...> {
    Ok(config.clone())
}
```

## 2) Código NO documentado

### 2.1 Comentario especial no documentado
**Ubicación:** debug.rs:61
```
/// ¡NO CAMBIAR! Este nodo es especial.
/// Su trabajo es emitir su config como el objeto de datos raíz.
```

**Hallazgo:** El comentario advertencia "¡NO CAMBIAR!" sugiere un nodo crítico cuyo comportamiento es frágil o depende de una invariante específica. No aparece en ninguna doc pública. La frase "el objeto de datos raíz" implica que el nodo tiene un rol especial en cómo se propagan datos, pero no está explicada en `node_configurations.json` ni en guías.

**Impacto QA:** Desconocimiento de por qué el nodo es "especial" puede llevar a cambios inadvertidos en el núcleo si alguien refactoriza.

### 2.2 Ignorancia de inputs no explícita
**Ubicación:** debug.rs:68 (parámetro `_inputs` con guion bajo)

**Hallazgo:** El código ignora completamente cualquier entrada upstream (las inputs nunca se leen). La documentación en `node_configurations.json` dice "ignores all upstream inputs" en la descripción textual, pero `input_ports: {}` (vacío) no es suficientemente explícito. Un usuario podría conectar nodos upstream sin darse cuenta de que serán ignorados.

**Impacto QA:** Potencial para grafos mal configurados donde edges hacia `mock_input` se creen útiles pero son inútiles.

### 2.3 Validación de config ausente
**Ubicación:** debug.rs:66-74

**Hallazgo:** El nodo no valida la config. Acepta `{}`, `null`, o cualquier JSON. La documentación no menciona límites de tamaño, tipos rechazados, o errores que puedan ocurrir.

**Impacto QA:** Config vacía `{}` es válida y emite `{}` como salida. Config con valores nulos o muy grandes (1 GB de JSON) no son rechazados. Falta especificación de comportamiento límite.

### 2.4 Schema inconsistencia (menor)
**Ubicación:** debug.rs:82
```json
{"type": "mock_input", "inputs": {}, "outputs": {"output": "any (from config)"}}
```

**Hallazgo:** El schema menciona un output field `"output": "any (from config)"`, pero `default_output()` retorna `None` (debug.rs:77-78). La salida real es el raw config sin wrapping. Si config tiene campo `output`, se propaga; si no, no hay field artificial. El esquema es engañoso: puede dar la impresión de que siempre hay un campo llamado `"output"`.

**Impacto QA:** Usuarios pueden confundir el schema() documentado con la salida real, especialmente si conectan edges esperando un field `output` que no existe en su config.

## 3) Plan de pruebas QA

### Caso 1: Config simple con un campo
**Objetivo:** Verificar que el nodo emite su config sin transformación.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": { "input": 5 }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna (mock_input no lee inputs)

**Resultado esperado:** El log imprime `{ "input": 5 }`

**Cómo verificar:** `cargo run --bin dag_engine -- run <graph.json>` debe mostrar en stdout `[LogNode]: { "input": 5 }`

---

### Caso 2: Config con múltiples campos
**Objetivo:** Verificar que todos los campos se emiten y se puede acceder via dotted paths.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": { "x": 10, "y": 5, "operation": "test" }
    },
    "add_node": {
      "type": "add",
      "config": {}
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "add_node", "source_field": "x", "target_field": "a" },
    { "from": "start", "to": "add_node", "source_field": "y", "target_field": "b" },
    { "from": "add_node", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** `add_node` recibe `a: 10, b: 5` via dotted paths; suma es 15; log imprime `{ "output": 15 }`

**Cómo verificar:** SSE logs muestran edges resueltos correctamente y output final es 15.

---

### Caso 3: Config vacía
**Objetivo:** Verificar comportamiento con config nula/vacía.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {}
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** Log imprime `{}`

**Cómo verificar:** `cargo run --bin dag_engine -- run <graph.json>` stdout muestra `[LogNode]: {}`

---

### Caso 4: Config con valores complejos
**Objetivo:** Verificar que config nested (objetos, arrays) se emite intacta.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "nested": { "a": 1, "b": 2 },
        "array": [1, 2, 3],
        "null_val": null
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** Log imprime la config exacta:
```json
{
  "nested": { "a": 1, "b": 2 },
  "array": [1, 2, 3],
  "null_val": null
}
```

**Cómo verificar:** JSON output en log es idéntico (byte-for-byte) a config.

---

### Caso 5: Upstream edges ignored
**Objetivo:** Verificar que inputs upstream son ignorados y la salida es siempre la config.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "source": {
      "type": "add",
      "config": {},
      "inputs": { "a": 100, "b": 200 }
    },
    "mock": {
      "type": "mock_input",
      "config": { "fixed_value": 42 }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "source", "to": "mock" },
    { "from": "mock", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** A pesar de que `add` emite `{ "output": 300 }`, `mock_input` ignora eso y emite `{ "fixed_value": 42 }`

**Cómo verificar:** Log imprime `{ "fixed_value": 42 }`, no `{ "output": 300 }`

---

### Caso 6: Config con campo "output"
**Objetivo:** Verificar comportamiento cuando config explícitamente define un campo "output" (posible fuente de confusión con schema).

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": { "output": "my_custom_value", "other": "data" }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** Log imprime raw config: `{ "output": "my_custom_value", "other": "data" }`

**Cómo verificar:** El campo `output` se incluye en la salida como un campo más, no como wrapping.

---

### Caso 7: Large config object
**Objetivo:** Verificar que nodos pueden manejar config grande sin error.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "data": "[<array de 1000 objetos con 10 campos cada uno>]"
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "start", "to": "log_result" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** El nodo ejecuta sin error y emite la config completa.

**Cómo verificar:** Ejecución completada sin panic; SSE logs muestran node execution success; output size ~size de config.

---

### Caso 8: Default input behavior (none)
**Objetivo:** Verificar que no existe `default_input`.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": { "value": 123 }
    },
    "add": {
      "type": "add",
      "config": {},
      "inputs": { "a": 10, "b": 20 }
    },
    "output": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "add" },
    { "from": "add", "to": "output" }
  ]
}
```

**Entrada:** ninguna

**Resultado esperado:** Grafo resuelve sin confusiones sobre default input; `add` recibe 10+20=30 (no se interpola la salida de mock_input automáticamente).

**Cómo verificar:** Edge resolution logs show explicit field mapping; no auto-merge de mock_input output.
