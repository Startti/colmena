# QA — Nodo `output`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/output.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 196–217)
- `docs/agent_context/node_ports_reference.md` (línea 28)
- `docs/DEVELOPER_GUIDE.md` (referencias indirectas)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.**

La documentación en `node_configurations.json` describe el comportamiento del nodo con precisión:
- `config_fields: {}` → el código no inspecciona ningún campo de config (línea 14: `_config` unused).
- `input_ports.input` → el código obtiene `input` de `NodeInputs` (línea 18).
- `output_ports.result` → el código retorna `{ "result": <value>, "extra_info": {...} }` (líneas 24–29).
- `default_input: "input"` → implementado en `default_input()` (línea 33).
- `default_output: "result"` → implementado en `default_output()` (línea 37).
- Comportamiento null → si no hay input, output es null (línea 21), consistente con doc.

---

## 2) Código NO documentado

### 2.1 Campo `supports_templates` ausente

**Hallazgo:** El campo `supports_templates` está documentado para otros nodos control_flow (ej. "input" en línea 193 de `node_configurations.json`) pero **no aparece** en la entrada "output" (líneas 196–217).

**Impacto:** QA no puede determinar si el nodo evalúa variables `${}` o plantillas en config. Aunque el nodo "output" no tiene campos de config (vacíos), la pauta de documentación sugiere que el campo debe estar presente por completitud.

**Recomendación:** Agregar `"supports_templates": false` a la entrada "output" en `node_configurations.json` línea 216, antes de `}`.

---

## 3) Plan de pruebas QA

### Caso 1: Happy path — entrada desde nodo previo
**Objetivo:** Verificar que el nodo captura y formatea la entrada correctamente.

**Grafo mínimo:**
```json
{
  "nodes": [
    {
      "id": "start",
      "node_type": "input",
      "config": {
        "data": { "message": "Hello, world!" }
      }
    },
    {
      "id": "end",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "start.output", "to": "end.input" }
  ]
}
```

**Entrada:** `cargo run --bin dag_engine -- run test.json`

**Resultado esperado:**
```json
{
  "result": {
    "message": "Hello, world!"
  },
  "extra_info": {
    "__colmena_is_output_node": true
  }
}
```

**Verificación:** Output contiene `result` con el objeto input y `extra_info.__colmena_is_output_node === true`.

---

### Caso 2: Sin entrada (puerto input vacío)
**Objetivo:** Verificar comportamiento cuando no llega ningún valor al puerto input.

**Grafo mínimo:**
```json
{
  "nodes": [
    {
      "id": "end",
      "node_type": "output"
    }
  ],
  "edges": []
}
```

**Entrada:** `cargo run --bin dag_engine -- run test.json`

**Resultado esperado:**
```json
{
  "result": null,
  "extra_info": {
    "__colmena_is_output_node": true
  }
}
```

**Verificación:** `result` es `null` sin error; `extra_info` siempre presente.

---

### Caso 3: Input null explícito
**Objetivo:** Verificar que `null` se captura como valor válido.

**Grafo mínimo:**
```json
{
  "nodes": [
    {
      "id": "input_node",
      "node_type": "input",
      "config": {
        "data": null
      }
    },
    {
      "id": "end",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_node.output", "to": "end.input" }
  ]
}
```

**Entrada:** `cargo run --bin dag_engine -- run test.json`

**Resultado esperado:**
```json
{
  "result": null,
  "extra_info": {
    "__colmena_is_output_node": true
  }
}
```

**Verificación:** Null pasa sin conversión; estructura siempre presente.

---

### Caso 4: Input complejo (object/array)
**Objetivo:** Verificar que datos estructurados se pasan sin mutación.

**Grafo mínimo:**
```json
{
  "nodes": [
    {
      "id": "input_node",
      "node_type": "input",
      "config": {
        "data": {
          "users": [
            { "id": 1, "name": "Alice" },
            { "id": 2, "name": "Bob" }
          ],
          "metadata": {
            "version": "1.0",
            "timestamp": "2026-08-30T12:00:00Z"
          }
        }
      }
    },
    {
      "id": "end",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_node.output", "to": "end.input" }
  ]
}
```

**Entrada:** `cargo run --bin dag_engine -- run test.json`

**Resultado esperado:**
```json
{
  "result": {
    "users": [
      { "id": 1, "name": "Alice" },
      { "id": 2, "name": "Bob" }
    ],
    "metadata": {
      "version": "1.0",
      "timestamp": "2026-08-30T12:00:00Z"
    }
  },
  "extra_info": {
    "__colmena_is_output_node": true
  }
}
```

**Verificación:** Input complejo preserva estructura y tipos; no hay wrapping adicional del input.

---

### Caso 5: Default ports (sin especificar edge explícitamente)
**Objetivo:** Verificar que los puertos default funcionan sin edges nombrados.

**Grafo mínimo:**
```json
{
  "nodes": [
    {
      "id": "start",
      "node_type": "input",
      "config": {
        "data": { "test": "value" }
      }
    },
    {
      "id": "end",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "start", "to": "end" }
  ]
}
```

**Entrada:** `cargo run --bin dag_engine -- run test.json` (edge sin puerto explícito = usa `default_output` de start → `default_input` de end)

**Resultado esperado:**
```json
{
  "result": {
    "test": "value"
  },
  "extra_info": {
    "__colmena_is_output_node": true
  }
}
```

**Verificación:** Edge shorthand (sin `.puerto`) resuelve a `input_node.output` → `output.input` automáticamente.

---

### Caso 6: Validar siempre estructura `extra_info`
**Objetivo:** Confirmar que `extra_info` con `__colmena_is_output_node` está SIEMPRE presente.

**Grafo mínimo:** (reutilizar Caso 1)

**Entrada:** `cargo run --bin dag_engine -- run test.json`

**Verificación:**
- Ejecutar 5 veces con inputs distintos (null, string, number, object, array).
- Para cada resultado, verificar:
  - `result` existe y tiene el valor input.
  - `extra_info` existe.
  - `extra_info.__colmena_is_output_node === true`.
  - No hay campos adicionales en `extra_info` (solo la clave `__colmena_is_output_node`).

---

## Resumen

| Sección | Hallazgos |
|---------|-----------|
| S1 | 0 discrepancias (config doc ↔ código: fiel) |
| S2 | 1 campo documentado faltante: `supports_templates` |
| S3 | 6 casos de prueba (happy path, null, complejo, defaults, estructura) |
