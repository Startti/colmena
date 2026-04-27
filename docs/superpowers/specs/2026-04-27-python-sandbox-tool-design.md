# Python Script Sandboxed Tool — Design Spec

**Date:** 2026-04-27
**Status:** Approved, pending implementation

---

## Objetivo

Permitir que un LLM genere y ejecute código Python dinámicamente como tool call, con sandboxing para uso en contexto SaaS/multi-tenant. El LLM escribe el código; el nodo lo valida y ejecuta de forma segura.

Caso de uso principal: un agente recibe datos de un nodo HTTP (e.g. lista de productos) y necesita procesarlos computacionalmente (contar, filtrar, agregar). El LLM genera el código Python exacto en lugar de intentar hacer la lógica en lenguaje natural.

---

## Enfoque elegido

**Opción A — Flag `sandbox_mode` en el nodo `python_script` existente.**

- Backward compatible: `sandbox_mode` default `"none"`, comportamiento actual sin cambios.
- Sandbox vive en el nodo (cerca de la ejecución), no en la capa de tool.
- El campo `code` se expone al LLM vía `node_schema` sin `fixed`.
- No se crea un nuevo tipo de nodo.

---

## Cambios al nodo `python_script`

### Nuevos campos de config

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `sandbox_mode` | `"none" \| "restricted"` | `"none"` | `"restricted"` activa AST validator + timeout |
| `sandbox_timeout_secs` | number | `10` | Segundos máximos de ejecución (solo en restricted) |

### AST Validator (modo `restricted`)

Se ejecuta antes de `py.run_bound()` usando el módulo `ast` de Python stdlib via PyO3. Sin dependencias externas.

**Imports permitidos (whitelist):**
`math`, `json`, `re`, `datetime`, `collections`, `itertools`, `functools`, `string`, `decimal`, `statistics`

**Imports bloqueados:** todo lo que no esté en la whitelist. Ejemplos: `os`, `sys`, `subprocess`, `socket`, `http`, `urllib`, `requests`, `pathlib`, `shutil`, `pickle`, `importlib`.

**Builtins bloqueados:** `open`, `exec`, `eval`, `compile`, `__import__`

### Timeout

`tokio::time::timeout(Duration::from_secs(sandbox_timeout_secs))` wrapeando el `spawn_blocking` existente. Sin cambios al modelo de threading.

### Errores retornados al LLM

El nodo retorna errores informativos como tool result para que el LLM pueda hacer retry:

```json
{ "error": "SandboxViolation: import 'os' is not allowed. Allowed imports: math, json, re, datetime, collections, itertools, functools, string, decimal, statistics" }
{ "error": "SandboxViolation: 'open' is not allowed in sandbox mode" }
{ "error": "SandboxTimeout: execution exceeded 10 seconds" }
{ "error": "Python execution error: NameError: name 'x' is not defined" }
```

---

## Patrón de tool configuration

### Regla: `node_schema+fixed` para `sandbox_mode`

`sandbox_mode` es un parámetro comportamental del nodo → va en `node_schema` con `fixed`, no en `fixed_config`. Ver `CLAUDE.md` sección "Tool Config Standard".

### Ejemplo canónico

```json
{
  "run_python": {
    "name": "run_python",
    "description": "Ejecuta código Python para procesar datos. Variable disponible: 'rows' (lista de objetos). Asigna el resultado a 'output'.",
    "node_type": "python_script",
    "node_schema": {
      "sandbox_mode":         { "fixed": "restricted" },
      "sandbox_timeout_secs": { "fixed": 10 },
      "code": {
        "type": "string",
        "required": true,
        "description": "Código Python a ejecutar. Solo imports permitidos: math, json, re, datetime, collections, itertools. Asigna el resultado a la variable 'output'. Ejemplo: output = len([r for r in rows if r['active']])"
      }
    },
    "context": {
      "rows": "${http_node.body.items}"
    }
  }
}
```

### Inyección de variables via `context`

El campo `context` mapea outputs de nodos upstream a variables Python disponibles en el script. El LLM puede referenciarlas directamente en el código que genera.

```json
"context": {
  "rows":    "${fetch_products.body.items}",
  "user_id": "${context.user_id}"
}
```

---

## Flujo de ejecución

```
HTTP node → { body: { items: [...] } }
                ↓ context injection: rows = [...]
LLM genera:
  { "code": "output = len([r for r in rows if r['active']])" }
                ↓
AST validator (~1ms, sync):
  ✅ sin imports prohibidos
  ✅ sin open/exec/eval
                ↓ (fallo → SandboxViolation error al LLM)
spawn_blocking + timeout(10s):
  Python ejecuta código con rows inyectado
                ↓ (timeout → SandboxTimeout error al LLM)
tool result: { "output": 42 }
                ↓
LLM responde al usuario
```

---

## Test graph de ejemplo

Path: `tests/graphs/agents/python_sandbox_tool_test.json`

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": { "prompt": "¿Cuántos productos están activos?" }
    },
    "fetch_products": {
      "type": "http_request",
      "config": {
        "base_url": "https://dummyjson.com",
        "endpoint": "/products",
        "method": "GET"
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "${OPENAI_API_KEY}",
        "tool_configurations": {
          "run_python": {
            "name": "run_python",
            "description": "Ejecuta código Python para procesar datos. Variable disponible: 'rows' (lista de productos). Asigna el resultado a 'output'.",
            "node_type": "python_script",
            "node_schema": {
              "sandbox_mode":         { "fixed": "restricted" },
              "sandbox_timeout_secs": { "fixed": 10 },
              "code": {
                "type": "string",
                "required": true,
                "description": "Código Python. Imports permitidos: math, json, re, datetime, collections, itertools. Asigna resultado a 'output'. Ejemplo: output = len([r for r in rows if r['stock'] > 0])"
              }
            },
            "context": {
              "rows": "${fetch_products.body.products}"
            }
          }
        },
        "enabled_tools": "*"
      }
    },
    "output": {
      "type": "output"
    }
  },
  "edges": [
    { "from": "start",          "to": "agent" },
    { "from": "fetch_products", "to": "agent" },
    { "from": "agent",          "to": "output" }
  ]
}
```

---

## Archivos a modificar

| Archivo | Cambio |
|---------|--------|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs` | Agregar `sandbox_mode`, AST validator, timeout |
| `docs/node_configurations.json` | Agregar `sandbox_mode` y `sandbox_timeout_secs` a `python_script` |
| `docs/node_as_tools_reference.json` | Agregar ejemplo de LLM-generated code en sección `python_script` |
| `tests/graphs/agents/python_sandbox_tool_test.json` | Nuevo test graph |

---

## No está en scope

- Subprocess isolation (aislamiento de memoria/CPU) — fase 2 si se requiere
- Retry automático en error de sandbox — el LLM maneja el retry naturalmente
- Whitelist configurable por nodo — la whitelist es fija para simplificar
