# QA — Nodo `log`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/debug.rs:8-58`
Fuentes de doc revisadas:
- `docs/node_configurations.json:694-715`
- `docs/agent_context/node_ports_reference.md:27, 97, 256`
- `docs/node_as_tools_reference.json` (sin entrada para `log`)
- `src/libs/colmena/text/` (sin archivos específicos para `log`)

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación en `node_configurations.json` describe fielmente el comportamiento del código:
- El nodo busca claves estándar ("input", "result", "output") y usa la primera encontrada (debug.rs:21-24).
- Si ninguna está presente pero hay otras entradas, agrega todas en un objeto (debug.rs:27-34).
- Si no hay entradas, devuelve `null` (debug.rs:36).
- Imprime el resultado a stdout (debug.rs:39).
- Pass-through exacto del valor (debug.rs:41).
- No tiene campos de configuración (config_fields: {} en doc; _config ignorado en código:16).

## 2) Código NO documentado

### 2.1 Métodos de ExecutableNode no mencionados en docs

El código implementa métodos de trait que no se reflejan en `node_configurations.json`:

- **`description()` (debug.rs:43-44)**: Devuelve `"Log data to console for debugging. Useful for inspecting intermediate values in the flow."` — ausente de la sección `description` en node_configurations.json (que usa una descripción más larga y centrada en el comportamiento pass-through).

- **`default_input()` (debug.rs:47-48)**: Devuelve `"input"` — consistente con `default_input: "input"` en doc, pero no es típicamente documentado.

- **`default_output()` (debug.rs:51-52)**: Devuelve `"output"` — consistente con `default_output: "output"` en doc.

- **`schema()` (debug.rs:55-56)**: Devuelve JSON schema `{"type": "log", "inputs": {"input": "any"}, "outputs": {"output": "any"}}` — ausente de node_configurations.json. Este esquema es el usado internamente por el DAG engine para validación de tipos, pero no aparece en docs públicas.

### 2.2 Comportamiento de auto-flattening parcialmente documentado

La documentación en `node_configurations.json:696` menciona: *"Accepts input from common keys ('input', 'result', 'output') or, if none of those are present, aggregates all inputs into a single object for display."*

Sin embargo, el código tiene un detalle no documentado:
- Si exactamente una de las tres claves está presente, usa SOLO ESE valor (debug.rs:21-26), NO lo agrupa.
- Si NINGUNA está presente pero hay otras claves, agrupa TODAS (debug.rs:27-34).
- Esta lógica es clara en código pero la doc podría ser más explícita en el orden de precedencia: "input" > "result" > "output" > {all others}.

### 2.3 Falta de entrada en node_as_tools_reference.json

El nodo `log` no aparece en `node_as_tools_reference.json`. Esto es coherente: `log` es un nodo de debugging, no está destinado a ser expuesto como herramienta del LLM. Sin embargo, la doc no clarifica esta distinción en los guías públicos.

## 3) Plan de pruebas QA

**Stack LLM por defecto**: Google Gemini 2.5 Flash (no aplica; nodo debug puro).
**Base de pruebas**: Ejecutar grafos con `cargo run --bin dag_engine -- run <graph.json>`.

### Caso 1: Input con clave estándar "input"

**Objetivo**: Verificar que el nodo log consume y devuelve fielmente el valor recibido en clave "input".

**Grafo mínimo**:
```json
{
  "nodes": {
    "source": {
      "type": "mock_input",
      "config": { "input": 42 }
    },
    "logger": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "source", "to": "logger" }
  ]
}
```

**Entrada/prompt**: N/A (grafo auto-ejecutable).

**Resultado esperado**:
- stdout: `[LogNode]: 42`
- Output del nodo `logger`: `42` (pass-through exacto).
- Sin errores.

**Verificación**: Capturar stdout, comparar con `42`; verificar que el output es numéricamente igual (no string "42").

---

### Caso 2: Fallback a "result"

**Objetivo**: Verificar que cuando "input" no existe, el nodo busca "result" como fallback.

**Grafo mínimo**:
```json
{
  "nodes": {
    "source": {
      "type": "mock_input",
      "config": { "result": "fallback_value" }
    },
    "logger": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "source", "to": "logger" }
  ]
}
```

**Resultado esperado**:
- stdout: `[LogNode]: "fallback_value"`
- Output: string `"fallback_value"`.

**Verificación**: stdout contiene "fallback_value"; output es exactamente ese valor.

---

### Caso 3: Fallback a "output"

**Objetivo**: Verificar que cuando "input" y "result" no existen, el nodo busca "output".

**Grafo mínimo**:
```json
{
  "nodes": {
    "source": {
      "type": "mock_input",
      "config": { "output": { "status": "ok" } }
    },
    "logger": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "source", "to": "logger" }
  ]
}
```

**Resultado esperado**:
- stdout: `[LogNode]: { "status": "ok" }`
- Output: objeto `{"status": "ok"}`.

**Verificación**: stdout contiene JSON; output es objeto con status=ok.

---

### Caso 4: Auto-flattening (múltiples entradas sin claves estándar)

**Objetivo**: Verificar que cuando no hay "input"/"result"/"output", el nodo agrega todas las entradas en un objeto.

**Grafo mínimo**:
```json
{
  "nodes": {
    "source1": {
      "type": "mock_input",
      "config": { "a": 1 }
    },
    "source2": {
      "type": "mock_input",
      "config": { "b": 2 }
    },
    "logger": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "source1", "to": "logger", "output": "output", "input": "a" },
    { "from": "source2", "to": "logger", "output": "output", "input": "b" }
  ]
}
```

**Resultado esperado**:
- stdout: `[LogNode]: { "a": 1, "b": 2 }`
- Output: objeto con ambas claves.

**Verificación**: stdout contiene JSON con "a" y "b"; output tiene ambos pares clave-valor.

---

### Caso 5: Entrada vacía (no inputs)

**Objetivo**: Verificar que cuando no hay entradas, el nodo devuelve `null`.

**Grafo mínimo**:
```json
{
  "nodes": {
    "logger": {
      "type": "log"
    }
  },
  "edges": []
}
```

**Resultado esperado**:
- stdout: `[LogNode]: null`
- Output: `null`.

**Verificación**: stdout contiene "null"; output es JSON null.

---

### Caso 6: Precedencia: "input" tiene prioridad sobre "result"

**Objetivo**: Verificar que si "input" existe, NO se busca "result" aunque también esté presente.

**Grafo mínimo**:
```json
{
  "nodes": {
    "source": {
      "type": "mock_input",
      "config": { "input": "primary", "result": "ignored" }
    },
    "logger": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "source", "to": "logger" }
  ]
}
```

**Resultado esperado**:
- stdout: `[LogNode]: "primary"` (NO "ignored").
- Output: `"primary"`.

**Verificación**: stdout contiene "primary" y NO contiene "ignored".

---

### Caso 7: Pass-through exacto (sin mutación)

**Objetivo**: Verificar que el nodo no modifica, serializa de más, o transforma el valor — devuelve el valor original intacto.

**Grafo mínimo**:
```json
{
  "nodes": {
    "source": {
      "type": "mock_input",
      "config": { 
        "input": {
          "nested": { "deep": [1, 2, null, true, "string"] },
          "number": 3.14159265359
        }
      }
    },
    "logger": {
      "type": "log"
    },
    "verify": {
      "type": "add",
      "config": { "a": 0, "b": 0 }
    }
  },
  "edges": [
    { "from": "source", "to": "logger" },
    { "from": "logger", "to": "verify" }
  ]
}
```

**Resultado esperado**:
- Output del logger: estructura exacta con números de alta precisión intactos, arrays, null, booleanos.
- Verificación por JSON serialization: `JSON.stringify(input) === JSON.stringify(output)`.

**Verificación**: Parsear stdout y output como JSON, comparar con `===` (deep equality); no deben diferir.

---

### Caso 8: Comportamiento con valores primitivos vs objetos

**Objetivo**: Verificar que el nodo maneja correctamente scalars, arrays, objetos y null sin sorpresas.

**Variantes**:
- **Caso 8a**: `{ "input": null }` → stdout `null`, output `null`.
- **Caso 8b**: `{ "input": [] }` → stdout `[]`, output `[]`.
- **Caso 8c**: `{ "input": "string with \"quotes\"" }` → stdout y output con escape correcto.
- **Caso 8d**: `{ "input": 0 }` → stdout `0` (no falsy-drop), output `0`.
- **Caso 8e**: `{ "input": false }` → stdout `false`, output `false`.

**Verificación**: Cada variante ejecutada; stdout formateado correctamente; output exacto.

---

### Caso 9: Error handling (si aplica)

**Objetivo**: Verificar que el nodo NO falla en casos limite.

**Variantes**:
- **Caso 9a**: Entrada muy grande (objeto con 1000+ claves) → NO panic, stdout se trunca o se imprime.
- **Caso 9b**: Valores con caracteres especiales/UTF-8 → imprime sin corrupción.
- **Caso 9c**: Referencias circulares (si Rust lo permite) → definir comportamiento.

**Verificación**: Ejecución completada sin panic; stdout legible.

---

### Caso 10: Registración y disponibilidad

**Objetivo**: Verificar que `log` está registrado en `registry.rs` y es dispatchable por nombre.

**Comando**:
```bash
grep -n '"log"' src/libs/colmena/src/dag_engine/infrastructure/registry.rs
```

**Resultado esperado**: Línea 87: `nodes.insert("log".to_string(), Arc::new(LogNode));`

**Verificación**: Grafo con `"type": "log"` se ejecuta sin error "unknown node type".
