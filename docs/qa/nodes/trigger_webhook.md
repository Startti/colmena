# QA — Nodo `trigger_webhook`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/trigger.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 219–272)
- `docs/agent_context/node_ports_reference.md` (línea 121)
- `docs/developer_guide/12_dag_engine_guide.md` (secciones sobre serve/trigger_webhook)
- `docs/developer_guide/05_testing.md` (test_payload para testing local)

## 1) Config documentada NO soportada por el código

**Validación de `method` no implementada**

La doc en `node_configurations.json` (línea ~232) declara:
```json
"method": {
  "valid_values": ["GET", "POST", "PUT", "DELETE", "PATCH"]
}
```

El código en `trigger.rs` (línea 50–62 schema()) simplemente expone:
```rust
"method": "string", // e.g., "POST"
```

No hay validación fail-closed: el código no valida que `method` sea uno de los valores permitidos. Cualquier string es aceptado (p.ej. "INVALID", "push", etc.) sin error. Este mismatch puede causar confusión en operadores que confían en la doc de valores válidos.

**Impacto QA:** tests deben verificar que el código acepta ANY string para `method`, incluyendo valores fuera del rango documentado, sin fallar.

## 2) Código NO documentado

**Prioridad explícita de payload (líneas 28–35)**

El código implementa orden de cascada:
1. `config.__payload__` (inyectado por serve)
2. `config.test_payload` (definido por operador)
3. `serde_json::to_value(inputs)` (fallback)

`node_configurations.json` (línea ~263) menciona que `__payload__` "Takes priority over test_payload", pero NO documenta el fallback a `inputs`. Esta ruta (fallback a inputs) ocurre cuando ninguno de los dos anteriores está presente, pero la doc es silenciosa al respecto.

**Impacto QA:** se necesita verificar que el fallback a `inputs` funciona correctamente cuando ni `__payload__` ni `test_payload` están presentes.

**Posible error de serialización no documentado (línea 34)**

El código ejecuta `serde_json::to_value(inputs)?`, que puede fallar si `inputs` contiene datos no serializables. Esta condición de error (¿qué estructura de inputs causa rechazo?) no está documentada. La doc en `node_configurations.json` (líneas ~267–269) dice el output es "any", pero no menciona restricciones de serialización.

**Impacto QA:** tests deben verificar qué sucede si `inputs` contiene datos binarios o referencias no serializables (si es posible pasar tales entradas).

**Auto-flatten es responsabilidad downstream, no del nodo (línea 39)**

El código devuelve `Ok(payload)` directamente (línea 39), sin encapsular en `{ output: ... }`. La doc en `node_ports_reference.md` (línea 121) describe esto como "relies on downstream auto-flatten, like `input`", sugiriendo que el DAG engine es quien "desenvuelve" automáticamente. El código solo cumple su parte (no envuelve); la doc no deja claro que esto es un contrato con el DAG engine, no una propiedad automática del nodo.

**Impacto QA:** acepta la implementación; solo verificar que los tests reales del DAG engine downstream aplican el auto-flatten correctamente.

## 3) Plan de pruebas QA

### Caso 1: Test mode con test_payload
**Objetivo:** Verificar que `run` mode usa `test_payload` cuando `__payload__` no está presente.

**Grafo mínimo:**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test",
        "test_payload": { "message": "hello", "value": 42 }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [{ "from": "trigger", "to": "log" }]
}
```

**Comando:** `cargo run --bin dag_engine -- run test_graph.json`

**Resultado esperado:** El nodo emite output con `message: "hello"`, `value: 42`. El log muestra el payload crudo (no envuelto).

**Verificación:** SSE output contiene `"output": { "message": "hello", "value": 42 }` (o campos individuales si auto-flatten está activo).

---

### Caso 2: Method POST vs GET (sin validación)
**Objetivo:** Verificar que cualquier string `method` es aceptado sin error (no hay whitelist).

**Grafo:**
```json
{
  "nodes": {
    "trigger_get": {
      "type": "trigger_webhook",
      "config": {
        "path": "/data",
        "method": "INVALID_METHOD",
        "test_payload": { "status": "ok" }
      }
    }
  }
}
```

**Comando:** `cargo run --bin dag_engine -- run invalid_method.json`

**Resultado esperado:** Ejecuta sin error, emite `{ "status": "ok" }`. No hay validación fail-closed en el nodo.

**Verificación:** No hay error de configuración ni de ejecución.

---

### Caso 3: Path personalizado vs default
**Objetivo:** Verificar que `path` se configura sin validación (acepta cualquier string).

**Grafo:**
```json
{
  "nodes": {
    "t1": {
      "type": "trigger_webhook",
      "config": {
        "path": "/custom/webhook/path/with/slashes",
        "test_payload": { "data": "test" }
      }
    }
  }
}
```

**Comando:** `cargo run --bin dag_engine -- run path_test.json`

**Resultado esperado:** Ejecuta sin error, emite payload.

**Verificación:** No hay rechazo de rutas; el servidor HTTP posterior debe rutear basado en este valor (fuera del alcance del nodo).

---

### Caso 4: Payload vacío (null) y objeto vacío
**Objetivo:** Verificar comportamiento con payloads mínimos.

**Grafo 1 (null):**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/empty", "test_payload": null }
    }
  }
}
```

**Comando:** `cargo run --bin dag_engine -- run empty_null.json`

**Resultado esperado:** Emite `null`.

---

**Grafo 2 (objeto vacío):**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/empty", "test_payload": {} }
    }
  }
}
```

**Resultado esperado:** Emite `{}`.

**Verificación:** En ambos casos, el output refleja exactamente el `test_payload` sin transformaciones.

---

### Caso 5: Payload anidado complejo
**Objetivo:** Verificar que payloads arbitrarios (arrays, objetos anidados, tipos mixtos) se pasan sin cambios.

**Grafo:**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/complex",
        "test_payload": {
          "user": {
            "id": 123,
            "emails": ["a@x.com", "b@x.com"]
          },
          "meta": {
            "timestamp": "2026-08-30T10:00:00Z",
            "tags": ["urgent", "review"]
          }
        }
      }
    }
  }
}
```

**Comando:** `cargo run --bin dag_engine -- run complex_payload.json`

**Resultado esperado:** Emite la estructura completa, sin aplanar (el nodo devuelve el objeto bruto).

**Verificación:** SSE contiene el payload anidado exacto.

---

### Caso 6: Fallback a inputs (sin test_payload ni __payload__)
**Objetivo:** Verificar que si no hay `test_payload` ni `__payload__`, el nodo intenta `serde_json::to_value(inputs)`.

**Grafo (sin test_payload):**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/input_edge" }
    },
    "downstream": { "type": "log" }
  },
  "edges": [{ "from": "trigger", "to": "downstream" }]
}
```

**Comando:** `cargo run --bin dag_engine -- run input_fallback.json`

**Resultado esperado:** El nodo ejecuta, intenta serializar `inputs` (que estará vacío o contará con lo que el DAG engine proporcione). Emite el resultado (posiblemente `{}` si inputs está vacío).

**Verificación:** Ejecución exitosa; output refleja el valor de `inputs`.

---

### Caso 7: Serve mode con __payload__ inyectado (simulación)
**Objetivo:** Verificar que `__payload__` tiene prioridad sobre `test_payload`.

**Nota:** Este caso requiere API de test unitario o prueba E2E con servidor HTTP. A nivel de `run` command, se simula modificando el config.

**Grafo (mock):**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/serve",
        "test_payload": { "source": "test" },
        "__payload__": { "source": "http", "request_id": "abc123" }
      }
    }
  }
}
```

**Comando:** `cargo run --bin dag_engine -- run payload_priority.json`

**Resultado esperado:** Emite `{ "source": "http", "request_id": "abc123" }` (no el `test_payload`).

**Verificación:** SSE output refleja `__payload__`, no `test_payload`.

---

### Caso 8: Método no documentado: GET vs POST en serve mode
**Objetivo:** Verificar que el servidor HTTP respeta el field `method` (aunque el nodo no lo valida).

**Grafo:**
```json
{
  "nodes": {
    "trigger_get": {
      "type": "trigger_webhook",
      "config": {
        "path": "/webhook",
        "method": "GET",
        "test_payload": { "mode": "get" }
      }
    },
    "trigger_post": {
      "type": "trigger_webhook",
      "config": {
        "path": "/webhook",
        "method": "POST",
        "test_payload": { "mode": "post" }
      }
    }
  }
}
```

**Comando (serve mode):** `cargo run --bin dag_engine -- serve methods.json` (luego enviar GET y POST a http://localhost:3000/webhook)

**Resultado esperado:** El nodo emite el test_payload correspondiente. El servidor HTTP filtra por método (comportamiento downstream).

**Verificación:** Requiere prueba de integración de servidor HTTP; confirmar que ambas rutas funcionan sin error en el nodo.

---

### Caso 9: No hay config de path ni method
**Objetivo:** Verificar comportamiento cuando ambos campos opcionales están ausentes (defaults null).

**Grafo:**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "test_payload": { "event": "default_path" } }
    }
  }
}
```

**Comando:** `cargo run --bin dag_engine -- run no_path_method.json`

**Resultado esperado:** Ejecuta sin error, emite el `test_payload`. El servidor HTTP usa una ruta por defecto (fuera del alcance del nodo).

**Verificación:** No hay rechazo de config; ejecución exitosa.
