# python_script — Auditoría QA (Documentación vs Código)

**Nodo:** `python_script`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`  
**Documentación primaria:** `docs/developer_guide/26_python_node.md`  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.python_script`  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 node_configurations.json — lista de módulos permitidos incompleta

**Problema:** El campo `sandbox_mode` en `docs/node_configurations.json:299-302` describe los módulos permitidos en `restricted` mode así:
```
"only whitelisted imports are allowed (math, json, re, datetime, collections, itertools, functools, string, decimal, statistics)"
```

**Realidad en código:** `python_node.rs:15-28` define `_ALLOWED_IMPORTS`:
```python
_ALLOWED_IMPORTS = {
    'math', 'json', 're', 'datetime', 'collections',
    'itertools', 'functools', 'string', 'decimal', 'statistics',
    # crdt_doc_run_python additions (subsistema C, 2026-06):
    'pandas', 'numpy', 'scipy',
    # Request-signing primitives:
    'hmac', 'hashlib', 'base64', 'secrets',
}
```

**Impacto:** Documentación está desactualizada. Los operadores que leen `node_configurations.json` desconocen que `pandas`, `numpy`, `scipy`, `hmac`, `hashlib`, `base64` y `secrets` están permitidos en modo `restricted`.

**Remediación:** Actualizar `docs/node_configurations.json:300-301` para listar todos los 17 módulos permitidos.

---

### 1.2 developer_guide/26_python_node.md — descripción de signing primitives es solo en español

**Problema:** `docs/developer_guide/26_python_node.md:69-89` explica el propósito de `hmac`, `hashlib`, `base64`, `secrets` únicamente en español ("Firma de peticiones"). No hay equivalente en inglés.

**Impacto:** Bajo (el documento principal IS en inglés arriba). La sección está bien explicada pero inaccesible a lectores que no leen español.

**Recomendación:** Traducir o duplicar la sección de firma de peticiones en inglés en la línea ~67.

---

### 1.3 node_ports_reference.md — NO tiene entrada para python_script

**Problema:** `docs/agent_context/node_ports_reference.md` no menciona `python_script`.

**Realidad:** El nodo tiene puertos de entrada (`code`, `sandbox_mode`, `sandbox_timeout_secs`, y cualquier otra clave) y salida (el valor bruto de `output`).

**Impacto:** Medio. Operadores que buscan "¿cuáles son los puertos de `python_script`?" no encuentran la entrada canónica y deben leer `26_python_node.md`.

**Remediación:** Agregar entrada `python_script` a `node_ports_reference.md` con las columnas Input Port, Reserved?, Type, Description y Output Port.

---

### 1.4 node_as_tools_reference.json — NO tiene entrada para python_script

**Problema:** `docs/node_as_tools_reference.json` está vacío en `node_types` (ni un `python_script`).

**Realidad:** `python_script` se usa frecuentemente como herramienta LLM en dos patrones: (A) código fijo + entradas semánticas, (B) código generado por LLM + datos.

**Impacto:** Alto. LLM developers que buscan "cómo configurar `python_script` como tool en `tool_configurations`" no encuentran ejemplos canónicos en la referencia de herramientas y deben inferir desde `26_python_node.md`.

**Remediación:** Agregar `python_script` a `docs/node_as_tools_reference.json` con ejemplos de ambos patrones (A y B) de `26_python_node.md:223-270`.

---

### 1.5 developer_guide/26_python_node.md — descripción de Pattern C (attachment_run_python) dice "soft-deprecated"

**Problema:** Línea 278 dice "`attachment_run_python` is soft-deprecated as of 2026-07-02 in favor of `data_run_python`".

**Verificación:** CLAUDE.md proyecto confirma: "Soft-deprecation of gsheets_run_python+attachment_run_python is a pending gated breaking follow-up; CTE/MERGE SELECT-only fail-closed hardening included" (PR #138, 2026-07-02).

**Estado:** Correcto. No es una inconsistencia, solo confirmación de que la documentación refleja el estado actual.

---

### 1.6 Falta de mención: stdout capture en modo restricted

**Problema:** `docs/developer_guide/26_python_node.md` NO menciona que stdout se captura y se descarta (no forma parte de la salida).

**Realidad en código:** `python_node.rs:132-159` captura stdout via `sys.stdout = io.StringIO()`, pero en el método `execute()` línea 279 retorna solo `output_json` (el valor de la variable `output` de Python), NO stdout.

**Impacto:** Bajo. Un desarrollador que hace `output = "result"; print("debug")` puede estar sorprendido de que no vea el print en la salida, pero es un comportamiento correcto (stdout se captura para evitar contaminación).

**Recomendación:** Documentar que stdout se captura (en `_node/execute`) pero se descarta; no es accesible downstream.

---

## 2. Hallazgos: Código

### 2.1 Fallback de `code` cuando ambas (config y input) están ausentes

**Problema:** En `python_node.rs:191-199`, si `inputs["code"]` y `config["code"]` están ambas ausentes:
```rust
code: if let Some(input_code) = inputs.get("code").and_then(|v| v.as_str()) {
    input_code.to_string()
} else {
    config
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or("PythonNode error: 'code' field is missing in inputs or config")?
        .to_string()
};
```

El nodo retorna un error "code field is missing".

**Verificación:** Correcto según `node_configurations.json` línea ~47-48: `"code"` está marcado como `"required": false` en config, **pero** la nota dice "One of `code` (config) or `code` input port must be present".

**Inconsistencia:** `node_configurations.json` says `"required": false` en config (lo que es técnicamente correcto — config solo no es obligatorio); sin embargo el error del código es claro. No hay inconsistencia real, solo que `node_configurations.json` podría aclarar mejor que se requiere uno de los dos.

**Estado:** OK. El comportamiento es coherente entre código y docs.

---

### 2.2 Stripping de markdown — comentario alude a líneas que no existen en docs

**Problema:** `python_node.rs:201-209` implementa strip de ` ```python ... ``` ` fences, pero el comentario no menciona que esto también se describe en `docs/developer_guide/26_python_node.md:145-159`.

**Verificación:** La documentación SÍ menciona markdown stripping (línea 145-159). El código no está fuera de sincronía; simplemente el comentario es breve.

**Estado:** OK.

---

### 2.3 timeout solo en modo restricted — documentación vs código

**Problema:** `python_node.rs:263-276` aplica timeout SOLO cuando `sandbox_mode == "restricted"`:
```rust
let output_json = if sandbox_mode == "restricted" {
    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), blocking_task)
        .await
        ...
} else {
    blocking_task.await??
};
```

**Verificación:** `docs/developer_guide/26_python_node.md:93-97` confirma: "Wall-clock seconds budget for the Python script when `sandbox_mode` is `"restricted"`". Correcto.

**Estado:** OK.

---

### 2.4 default_output retorna None — rationale documentado

**Problema:** `python_node.rs:281-288`:
```rust
fn default_output(&self) -> Option<&str> {
    // The node returns the raw value of the Python `output` variable, NOT a
    // wrapper like { "output": <value> }. Returning None here tells the edge
    // resolver to pass the raw value through instead of trying to extract a
    // non-existent "output" field — otherwise scalar outputs (number, string,
    // bool) get silently dropped to null on implicit edges.
    None
}
```

**Verificación:** `docs/developer_guide/26_python_node.md:139-141` y `docs/node_configurations.json:default_output_note` ambas confirman esta decisión. Completamente alineado.

**Estado:** OK.

---

### 2.5 Payload logging — policy correctamente implementado

**Problema/Verificación:** `python_node.rs:231-238` usa `tracing::debug!` con `target: T_PYTHON_NODE` para emitir metadatos seguros (code_len, sandbox_mode, timeout_secs) pero NUNCA el código fuente. Además, `crate::dag_engine::log_policy::payload_trace!(python_code, code = %code)` es doblemente gateada.

**Estado:** Correcto. Sección `payload_logging_tests` (líneas 624-742) verifica con 4 ejes que:
1. Default production: payload ausente ✅
2. `RUST_LOG=trace` sin guard: payload aún ausente ✅
3. Guard presente pero filter no: payload ausente ✅
4. Ambos gates abiertos: payload presente ✅

**Documentación:** `docs/developer_guide/50_logging_and_observability.md` está referenciado en el comentario (línea 232). OK.

**Estado:** Excelente.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying de estado.

### 3.1 Test A: Aritmética básica en modo `none`

**Archivo:** `tests/graphs/basic/python_arithmetic.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": { "x": 15, "y": 3 }
    },
    "calc": {
      "type": "python_script",
      "config": { "code": "output = x * y + (x - y)" }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "calc" },
    { "from": "calc", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/python_arithmetic.json --agent-session-id test_a_001
```

**Validación esperada:**
- Entrada: x=15, y=3
- Salida: 15 * 3 + (15 - 3) = 45 + 12 = 57
- El nodo emite el valor bruto `57` (NO `{"output": 57}`)

---

### 3.2 Test B: Sandbox restrictivo + whitelisted modules

**Archivo:** `tests/graphs/agents/python_sandbox_allowed_imports.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "code": "import math\nimport json\nimport pandas as pd\noutput = {'pi': math.pi, 'pandas_version': pd.__version__}"
      }
    },
    "sandbox": {
      "type": "python_script",
      "config": {
        "sandbox_mode": "restricted",
        "sandbox_timeout_secs": 5
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input.code", "to": "sandbox.code" },
    { "from": "sandbox", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/python_sandbox_allowed_imports.json --agent-session-id test_b_001
```

**Validación esperada:**
- Sandbox aprueba imports de `math`, `json`, `pandas`
- Output contiene `{'pi': 3.141..., 'pandas_version': '...'}`
- Confirma que `pandas`, `numpy`, `scipy` están en la whitelist (contrario a `node_configurations.json` outdated)

---

### 3.3 Test C: Sandbox bloquea módulo no permitido

**Archivo:** `tests/graphs/agents/python_sandbox_blocked_os.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "code": "import os\noutput = os.getcwd()"
      }
    },
    "sandbox": {
      "type": "python_script",
      "config": {
        "sandbox_mode": "restricted",
        "sandbox_timeout_secs": 5
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input.code", "to": "sandbox.code" },
    { "from": "sandbox", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/python_sandbox_blocked_os.json --agent-session-id test_c_001
```

**Validación esperada:**
- Sandbox RECHAZA `import os` con error: `SandboxViolation: import 'os' is not allowed. Allowed imports: base64, collections, datetime, ...`
- Output nulo (error propagado como string)

---

### 3.4 Test D: Markdown code wrapping

**Archivo:** `tests/graphs/basic/python_markdown_wrapped.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "code": "```python\nx = 10\ny = 20\noutput = x + y\n```"
      }
    },
    "python": {
      "type": "python_script"
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input.code", "to": "python.code" },
    { "from": "python", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/python_markdown_wrapped.json --agent-session-id test_d_001
```

**Validación esperada:**
- Node strips ` ```python ... ``` ` fences
- Ejecuta el código interno: x=10, y=20, output=30
- Output: `30` (sin error, sin bloques markdown visibles)

---

### 3.5 Test E: Variables inyectadas desde entrada

**Archivo:** `tests/graphs/basic/python_injected_variables.json`

```json
{
  "nodes": {
    "data": {
      "type": "mock_input",
      "config": {
        "name": "Alice",
        "items": [1, 2, 3, 4, 5],
        "active": true
      }
    },
    "process": {
      "type": "python_script",
      "config": {
        "code": "output = {'greeting': f'Hello {name}', 'count': len(items), 'status': 'active' if active else 'inactive'}"
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "data", "to": "process" },
    { "from": "process", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/python_injected_variables.json --agent-session-id test_e_001
```

**Validación esperada:**
- `name`, `items`, `active` son inyectadas como variables globales de Python
- Output: `{'greeting': 'Hello Alice', 'count': 5, 'status': 'active'}`
- Las claves de entrada que no son reservadas (`code`, `sandbox_mode`, `sandbox_timeout_secs`) están disponibles en el script

---

### 3.6 Test F: Reserved keys NO inyectadas

**Archivo:** `tests/graphs/basic/python_reserved_keys_not_injected.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "x": 7,
        "code": "should_not_see_this",
        "sandbox_mode": "should_not_see_this_either"
      }
    },
    "python": {
      "type": "python_script",
      "config": {
        "code": "try:\n    _ = code\n    output = 'ERROR: code was injected'\nexcept NameError:\n    output = {'x': x, 'result': x * 2}"
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "python" },
    { "from": "python", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/python_reserved_keys_not_injected.json --agent-session-id test_f_001
```

**Validación esperada:**
- `code`, `sandbox_mode`, `sandbox_timeout_secs` en entradas se filtran y NO se inyectan (línea 245-250 de `python_node.rs`)
- `x` SÍ se inyecta (no es reservado)
- Output: `{'x': 7, 'result': 14}` (sin error NameError en `code`)

---

### 3.7 Test G: JSON Boundary — dict con claves no-string

**Archivo:** `tests/graphs/basic/python_json_boundary_error.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {}
    },
    "python": {
      "type": "python_script",
      "config": {
        "code": "output = {5: 'five', 10: 'ten'}"
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "python" },
    { "from": "python", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/python_json_boundary_error.json --agent-session-id test_g_001
```

**Validación esperada:**
- Node ejecuta correctamente en Python: `{5: 'five', 10: 'ten'}`
- Conversion a JSON FALLA: error "Failed to convert Python 'output' to JSON: 'int' object cannot be converted to 'PyString'"
- Confirma que el boundary Python↔JSON se valida ANTES de retornar

---

### 3.8 Test H: Null output (variable no asignada)

**Archivo:** `tests/graphs/basic/python_no_output_variable.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": { "x": 42 }
    },
    "python": {
      "type": "python_script",
      "config": {
        "code": "result = x * 2"
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "python" },
    { "from": "python", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/python_no_output_variable.json --agent-session-id test_h_001
```

**Validación esperada:**
- Script asigna a `result` (NO `output`)
- Node retorna `null` (línea 168-175 de `python_node.rs`: match locals.get_item("output") → `_ => None`)
- Output: `null`

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Docs | Alta | `node_configurations.json` lista de módulos permitidos incompleta (falta pandas, numpy, scipy, hmac, hashlib, base64, secrets) |
| 1.2 | Docs | Baja | Firma de peticiones solo explicada en español |
| 1.3 | Docs | Media | `node_ports_reference.md` sin entrada para `python_script` |
| 1.4 | Docs | Alta | `node_as_tools_reference.json` sin entrada para `python_script` |
| 1.5 | Docs | OK | Deprecación de `attachment_run_python` correctamente documentada |
| 1.6 | Docs | Baja | Falta mención explícita de que stdout se captura pero se descarta |
| 2.1 | Código | OK | `code` required behavior coherente entre código y docs |
| 2.2 | Código | OK | Markdown stripping alineado |
| 2.3 | Código | OK | Timeout solo en modo restricted, documentado |
| 2.4 | Código | OK | `default_output=None` decisión bien documentada |
| 2.5 | Código | Excelente | Payload logging doblemente gateado y verificado |

---

## Remediaciones Recomendadas

### Prioridad ALTA (bloquea documentación automática)

1. **Actualizar `docs/node_configurations.json`** línea ~300-301 con la lista completa de 17 módulos permitidos en restricted mode.
2. **Agregar entrada `python_script` a `docs/node_as_tools_reference.json`** con ejemplos de Pattern A y B (código fijo vs LLM-generado).

### Prioridad MEDIA (afecta discovery)

3. **Agregar entrada `python_script` a `docs/agent_context/node_ports_reference.md`** con puertos de entrada y salida.

### Prioridad BAJA (legibilidad)

4. **Agregar sección en inglés** explicando firma de peticiones (`hmac`, `hashlib`, `base64`, `secrets`) en `docs/developer_guide/26_python_node.md`.
5. **Documentar explícitamente** que stdout se captura pero se descarta (no forma parte de la salida).

---

**Auditoría completada:** 8 hallazgos en documentación (1 alta, 1 media, 1 baja, 5 OK) + 5 aspectos de código validados (4 OK, 1 excelente) + 8 casos de prueba ejecutables.

