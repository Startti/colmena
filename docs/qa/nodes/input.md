# QA — Nodo `input`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (entrada `input` en `node_types`)
- `docs/agent_context/node_ports_reference.md` (tabla línea 98)
- `docs/superpowers/specs/2026-05-03-user-skills-via-signed-urls-design.md` (mención de input node)

---

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. Los campos `data` y `__payload__` están ambos implementados.

---

## 2) Código NO documentado

### A) Filtrado en passthrough mode

**Hallazgo:** La documentación (`node_configurations.json`, input_ports) dice que el nodo emite "all injected inputs" en mode passthrough (config vacío), pero el código filtra específicamente:
- Claves que comienzan con `__` (línea 78: `!k.starts_with("__")`)
- La clave exacta `session_id` (línea 78: `k.as_str() != "session_id"`)

**Impacto QA:** Las pruebas de passthrough deben verificar que estas claves se filtran, no se pasan.

### B) Override rule para valores "vacíos"

**Hallazgo:** En línea 89-92, el código implementa una regla que ignora (no sobrescribe) los valores de inputs en estos casos:
- `Value::Null`
- `Value::Object` vacío (`{}`)
- `Value::String` vacío (`""`)

La documentación (`node_configurations.json`, config_fields.data) menciona que "Null, empty string, and empty object values from inputs are ignored" pero **esta regla es internamente consistente** con el código (✓). Sin embargo, la descripción en `input_ports` es demasiado genérica: dice "can override" sin precisar la excepción para valores vacíos.

**Impacto QA:** Probar que `null`, `{}`, y `""` como input NO sobrescriben el config value.

### C) Resolución de templates

**Hallazgo:** La documentación dice que el nodo soporta `{{key}}` Y `{{key.nested}}` ("Supports '{{key}}' and '{{key.nested}}' template syntax" en `node_configurations.json`), pero el código en línea 21-28 **solo soporta keys simples** (no anidadas). La función `resolve_templates` busca en `state.get(key)` donde `key` es la cadena extraída entre `{{` y `}}`.

Ejemplo: `{{foo.bar}}` extraería la clave literal `"foo.bar"` del estado global (no `state["foo"]["bar"]`). Si el estado no tiene esa clave exacta, devuelve vacío.

**Impacto QA:** Probar que `{{foo.bar}}` busca la clave `foo.bar` (string con punto), no acceso anidado. Si se quiere un acceso anidado real, el usuario debe proporcionar `foo.bar` como clave en el estado (poco probable).

### D) Conversión de valores non-string en templates

**Hallazgo:** En línea 24-27, cuando se resuelve un template, si el valor en `state` no es string, se convierte:
```rust
Value::String(s) => s.clone(),
other => other.to_string(),
```

La documentación NO especifica este comportamiento (dice "Only string values are substituted" como comentario en línea 9, lo cual es técnicamente exacto: solo strings se reemplazan dentro de otro string, pero valores non-string en state se convierten a string).

**Impacto QA:** Probar que `{{numeric_key}}` donde state tiene un número se convierte a string en la salida.

---

## 3) Plan de pruebas QA

### Caso 1: Output básico con `data` field
**Objetivo:** Verificar que el nodo emite el contenido del field `data` sin transformación.

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "inp",
      "node_type": "input",
      "config": {
        "data": {
          "question": "¿Cuál es la capital de Francia?",
          "language": "es"
        }
      }
    },
    {
      "id": "out",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [{"from": "inp:output", "to": "out:input"}]
}
```

**Entrada:** Ninguna (nodo input no recibe inputs de edges).

**Resultado esperado:**
```json
{
  "question": "¿Cuál es la capital de Francia?",
  "language": "es"
}
```

**Verificación:** El output capture en `out` debe ser un objeto con ambas claves y valores.

---

### Caso 2: Passthrough (config vacío)
**Objetivo:** Verificar que config vacío pasa todos los inputs injected (menos claves internas).

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "inject",
      "node_type": "current_time",
      "config": {}
    },
    {
      "id": "inp",
      "node_type": "input",
      "config": {}
    },
    {
      "id": "out",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [
    {"from": "inject:timestamp", "to": "inp:timestamp"},
    {"from": "inject:timestamp", "to": "inp:custom_field"},
    {"from": "inp:output", "to": "out:input"}
  ]
}
```

**Entrada:** `current_time` emite timestamp (ISO string).

**Resultado esperado:**
```json
{
  "timestamp": "<ISO timestamp>",
  "custom_field": "<ISO timestamp>"
}
```

**Verificación:** El output debe contener ambas claves inyectadas.

---

### Caso 3: Template resolution `{{key}}`
**Objetivo:** Verificar que `{{key}}` en strings se reemplaza por valores del state.

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "inp",
      "node_type": "input",
      "config": {
        "data": {
          "greeting": "Hello, {{name}}!",
          "farewell": "Goodbye, {{name}}!"
        }
      }
    },
    {
      "id": "out",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [{"from": "inp:output", "to": "out:input"}]
}
```

**Nota:** Este caso requiere inyectar un state global con `"name": "Alice"`. Sin un mecanismo en el grafo JSON para inyectar state, este caso es difícil de probar. Alternativa: usar un nodo `python_script` anterior que escriba en state (pero está fuera del control de `input`).

**Verificación:** Si el state contiene `"name": "Alice"`, los strings deben ser `"Hello, Alice!"` y `"Goodbye, Alice!"`.

---

### Caso 4: Edge input overrides config field
**Objetivo:** Verificar que un valor inyectado por edge sobrescribe el campo correspondiente en `data`.

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "src",
      "node_type": "current_time",
      "config": {}
    },
    {
      "id": "inp",
      "node_type": "input",
      "config": {
        "data": {
          "timestamp": "2026-01-01T00:00:00Z",
          "language": "es"
        }
      }
    },
    {
      "id": "out",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [
    {"from": "src:timestamp", "to": "inp:timestamp"},
    {"from": "inp:output", "to": "out:input"}
  ]
}
```

**Entrada:** `current_time` emite un timestamp actual.

**Resultado esperado:**
```json
{
  "timestamp": "<timestamp actual, no el hardcoded>",
  "language": "es"
}
```

**Verificación:** El `timestamp` del output debe ser el emitido por `current_time`, no el valor hardcoded en `data`.

---

### Caso 5: Override con `null` se ignora
**Objetivo:** Verificar que `null` como input NO sobrescribe el config value.

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "inp",
      "node_type": "input",
      "config": {
        "data": {
          "name": "Alice",
          "age": 30
        }
      }
    },
    {
      "id": "out",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [
    {"from": "inp:output", "to": "out:input"}
  ]
}
```

**Entrada:** Inyectar manualmente un null a `inp.name` (difícil sin helper; alternativa: usar un `output` intermedio que emita null).

**Resultado esperado:** `name` sigue siendo `"Alice"` (null no sobrescribe).

**Verificación:** El output debe tener `"name": "Alice"`.

---

### Caso 6: Override con empty string se ignora
**Objetivo:** Verificar que `""` como input NO sobrescribe el config value.

**Grafo JSON mínimo:** Igual que Caso 5, pero con `""` en lugar de `null`.

**Entrada:** Inyectar un string vacío a `inp.language`.

**Resultado esperado:** `language` sigue siendo el valor original en `data`.

**Verificación:** El output debe tener el valor original de `language`.

---

### Caso 7: Override con empty object `{}` se ignora
**Objetivo:** Verificar que `{}` como input NO sobrescribe el config value.

**Grafo JSON mínimo:** Igual que Caso 5, pero inyectando `{}` a un campo.

**Resultado esperado:** El campo mantiene su valor de config.

**Verificación:** El output debe tener el valor original.

---

### Caso 8: Filtrado de claves internas `__` en passthrough
**Objetivo:** Verificar que claves que comienzan con `__` se filtran en mode passthrough.

**Grafo JSON mínimo:** Difícil sin poder inyectar directamente claves `__*`. Alternativa: usar un `python_script` que emita keys con `__`.

**Entrada:** Un edge que envía un objeto con claves `__internal`, `normal_key`.

**Resultado esperado:** El output solo contiene `normal_key`.

**Verificación:** El output no tiene claves `__`.

---

### Caso 9: Filtrado de `session_id` en passthrough
**Objetivo:** Verificar que la clave exacta `session_id` se filtra en mode passthrough (incluso si viene de inputs).

**Grafo JSON mínimo:** Difícil sin poder inyectar. 

**Entrada:** Un edge que envía `session_id`.

**Resultado esperado:** El output no tiene `session_id`.

**Verificación:** La clave `session_id` está ausente.

---

### Caso 10: `__payload__` sobrescribe todo
**Objetivo:** Verificar que `__payload__` en config retorna su valor directamente sin procesar.

**Grafo JSON mínimo:**
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "inp",
      "node_type": "input",
      "config": {
        "__payload__": {"raw_result": "bypass_everything"},
        "data": {"ignored": "this_field"}
      }
    },
    {
      "id": "out",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [{"from": "inp:output", "to": "out:input"}]
}
```

**Entrada:** Ninguna.

**Resultado esperado:** `{"raw_result": "bypass_everything"}` (el `data` se ignora totalmente).

**Verificación:** El output es exactamente el valor de `__payload__`, sin el campo `ignored`.

---

### Caso 11: Template resolution con value non-string
**Objetivo:** Verificar que values non-string en state se convierten a string durante template resolution.

**Grafo JSON mínimo:** Requiere state global con valor numérico. Difícil sin helper.

**Entrada:** State con `"count": 42` (número).

**Resultado esperado:** Template `"We have {{count}} items"` resuelve a `"We have 42 items"` (número convertido a string).

**Verificación:** El string resultante contiene el número como substring.

---

### Caso 12: No hay default_input (nodo no recibe input por defecto)
**Objetivo:** Verificar que el nodo `input` no tiene un default input port (a diferencia de `llm_call` que tiene `prompt`).

**Verificación:** En `node_ports_reference.md` línea 98, `default_input` es `—` (vacío). El nodo debe funcionar sin inputs de edge (solo config).

---

### Caso 13: Default output es `output`
**Objetivo:** Verificar que el output default port es `output` (no `result` ni otro nombre).

**Verificación:** El código línea 105-107 retorna `Some("output")`. Las edges pueden usar `inp:output` sin especificar el nombre.

---
