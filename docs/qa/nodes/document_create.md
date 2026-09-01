# QA — Nodo `document_create`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (línea 2456–2523)
- `docs/agent_context/node_ports_reference.md` (línea 85, 127)
- `docs/node_as_tools_reference.json` (NO ENCONTRADO)
- `docs/DEVELOPER_GUIDE.md` (sin mención específica)

---

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación en `node_configurations.json` describe con precisión todos los campos soportados y sus validaciones:

- `kind` (required, "excel" | "word") → código valida en `execute()` línea 116–128
- `initial_ir` (optional, $DYNAMIC compatible) → código resuelve línea 131–135
- `label` (optional) → código resuelve línea 137–141
- `retention_limit` (optional, u32) → código resuelve línea 143–147
- `session_id` (optional, default "default") → código resuelve línea 149 mediante `resolve_session_id()`
- `storage_backend` y `storage_root` → código los pasa a `DocumentRuntime::from_config()` línea 47–59

Output descrito en `node_configurations.json` línea 2516 coincide con el código línea 161–169: `{ artifact_id, version_id, label }` en caso de éxito, o objeto error en caso de fallo.

---

## 2) Código NO documentado

### 2.1 Nodo ausente de `node_as_tools_reference.json`

El nodo `document_create` está registrado en `node_configurations.json` como nodo ejecutable y puede ser usado como LLM tool (la arquitectura de `dag_tool_executor` lo soporta genéricamente), pero **no aparece una entrada específica en `docs/node_as_tools_reference.json`**.

- Impacto: un agente que intente consultar cómo configurar `document_create` como tool (p.ej., mediante `node_schema`, `fixed_config`, `expose_sub_tools`) no encontrará documentación específica.
- Riesgo QA: si un grafo LLM intenta usar `document_create` como tool, la documentación es incompleta.

### 2.2 Error messages y su fail-closed behavior

El código define un tipo privado `DocNodeError` (línea 25–31) con dos variantes:

```rust
#[derive(Debug, Error)]
enum DocNodeError {
    #[error("config error: {0}")]
    Config(String),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}
```

Casos fail-closed en `execute()`:

- **Línea 120**: `kind` ausente en inputs ni config → `DocNodeError::MissingField("kind")` (fail-closed)
- **Línea 125–127**: `kind` ∉ {"excel", "word"} → `DocNodeError::Config(format!(...))` con descripción "unknown kind `{other}` — expected `excel` or `word`" (fail-closed)

Estos mensajes de error están encapsulados en `Box<dyn StdError>` pero **no se documentan en `node_configurations.json`** ni en `node_ports_reference.md`. La doc dice "MissingField(\"kind\") if absent" pero no detalla la estructura del error retornado al grafo.

- Impacto: un ejecutor de grafo que falla en `execute()` obtiene un error pero su forma exacta (error type, campo message) no está especificada en la documentación. Esto afecta al manejo de errores en nivel DAG.

### 2.3 Session ID resolution details

El código implementa una cadena de prioridades para resolver `session_id` (función `resolve_session_id()` línea 36–45):

```
priority: input "__colmena_session_id" > input "session_id" > config "session_id" > default "default"
```

La documentación en `node_configurations.json` línea 2490 dice "Resolved from input '__colmena_session_id', then input/config 'session_id', defaulting to 'default'", lo cual es **correcto** pero sucinto. No especifica que la función retorna un `SessionId::new(id)`, ni que los valores vacíos son filtrados (p.ej., una string vacía caería al siguiente nivel).

- Impacto QA menor: la resolución funciona como se documenta, pero el comportamiento con inputs vacíos/null no está explícito.

---

## 3) Plan de pruebas QA

### Caso 3.1: Crear documento Excel vacío (happy path mínimo)

**Objetivo**: verificar que `kind: "excel"` sin `initial_ir` crea un documento con artifact_id, version_id ≠ null y label = null.

**Grafo JSON mínimo**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs"
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_excel.json
```

**Resultado esperado**:
- Exit code: 0
- SSE evento de cierre: `{ "output": { "artifact_id": "art_<...>", "version_id": "v1", "label": null } }`
- artifact_id comienza con `art_` (convención)
- version_id = `v1` (primer documento es v1)
- label es null (no fue suministrado)

**Verificación**: capturar SSE, extraer payload final, validar estructura JSON y valores.

---

### Caso 3.2: Crear documento Word con initial_ir

**Objetivo**: verificar que `kind: "word"` + `initial_ir` (objeto IR válido) crea el documento con el IR inicial.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "word",
        "storage_root": "/tmp/colmena_test_docs",
        "initial_ir": {
          "kind": "word",
          "artifact_id": "placeholder",
          "version_id": "v1",
          "schema_version": "1.0.0",
          "document": { "sections": [] }
        }
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_word.json
```

**Resultado esperado**:
- Exit code: 0
- `artifact_id` y `version_id` en output
- El documento debe estar persistido en `/tmp/colmena_test_docs/` con la IR suministrada

**Verificación**: leer el archivo de documento del sistema de archivos, deserializar IR, confirmar estructura coincide con `initial_ir`.

---

### Caso 3.3: Proporcionar label

**Objetivo**: verificar que `label` (string) se almacena y retorna en output.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs",
        "label": "Sales Report Q3 2026"
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_label.json
```

**Resultado esperado**:
- Output contiene `"label": "Sales Report Q3 2026"`

**Verificación**: extraer label del SSE, comparar con config.

---

### Caso 3.4: session_id desde config

**Objetivo**: verificar que `session_id` en config se respeta cuando no hay `__colmena_session_id` ni `session_id` en inputs.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs",
        "session_id": "user_alice_session_xyz"
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_session.json
```

**Resultado esperado**:
- Documento creado con `session_id = "user_alice_session_xyz"` (implícito en la ruta de almacenamiento)

**Verificación**: inspeccionar el path donde se almacena el documento en el backend; debe incluir la sesión como parte de la clave/ruta.

---

### Caso 3.5: session_id desde input (prioridad sobre config)

**Objetivo**: verificar que input `session_id` sobrescribe `config.session_id`.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs",
        "session_id": "config_session"
      }
    },
    {
      "id": "input_session",
      "node_type": "input",
      "config": {
        "value": "input_session_override"
      }
    }
  ],
  "edges": [
    { "from": "input_session.output", "to": "create.session_id" }
  ]
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_session_override.json
```

**Resultado esperado**:
- Documento creado con `session_id = "input_session_override"` (input ganó sobre config)

**Verificación**: inspeccionar ruta de almacenamiento; debe usar la session del input, no la del config.

---

### Caso 3.6: kind ausente (fail-closed)

**Objetivo**: verificar que omitir `kind` falla con error legible.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "storage_root": "/tmp/colmena_test_docs"
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_no_kind.json
```

**Resultado esperado**:
- Exit code: 1 o similar (error)
- SSE evento de error: `{ "output": { "error": "missing required field: kind" } }` o similar (error wrapped en output, no panic)

**Verificación**: validar que el error está en el output (not a Rust panic), mensaje contiene "kind", exit es fallido.

---

### Caso 3.7: kind = "invalid" (fail-closed)

**Objetivo**: verificar que valores inválidos de `kind` fallan con error específico.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "pdf",
        "storage_root": "/tmp/colmena_test_docs"
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_bad_kind.json
```

**Resultado esperado**:
- Exit code: 1 (error)
- SSE evento: `{ "output": { "error": "unknown kind `pdf` — expected `excel` or `word`" } }` (error wrapped en output)

**Verificación**: mensaje de error menciona las opciones válidas (excel, word), error no panics.

---

### Caso 3.8: retention_limit configurado

**Objetivo**: verificar que `retention_limit` se acepta y almacena.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs",
        "retention_limit": 10
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_retention.json
```

**Resultado esperado**:
- Exit code: 0
- Documento creado (el límite se almacena internamente)

**Verificación**: ejecutar múltiples ediciones del documento, verificar que historiales antiguos se descartan cuando se excede retention_limit (requiere prueba E2E con document_edit + document_read en cadena).

---

### Caso 3.9: storage_backend = "localfs" (default)

**Objetivo**: verificar que el backend localfs por defecto persiste en `storage_root`.

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs_localfs",
        "storage_backend": "localfs"
      }
    }
  ],
  "edges": []
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_localfs.json
```

**Resultado esperado**:
- Documento persiste en `/tmp/colmena_test_docs_localfs/`
- Archivos de IR son leibles (JSON o binario según formato)

**Verificación**: listar archivos en storage_root, validar estructura de directorios.

---

### Caso 3.10: Encadenamiento create → read (round-trip)

**Objetivo**: verificar que un documento creado puede leerse inmediatamente (la prueba unitaria cubre esto, pero E2E lo confirma).

**Grafo JSON**:
```json
{
  "nodes": [
    {
      "id": "create",
      "node_type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs",
        "initial_ir": {
          "kind": "excel",
          "artifact_id": "x",
          "version_id": "v1",
          "schema_version": "1.0.0",
          "workbook": { "sheets": [{"id": "s1", "name": "Sheet1", "order": 0, "columns": [], "cells": {}, "tables": []}], "named_styles": {} }
        }
      }
    },
    {
      "id": "read",
      "node_type": "document_read",
      "config": {
        "storage_root": "/tmp/colmena_test_docs"
      }
    }
  ],
  "edges": [
    { "from": "create.output.artifact_id", "to": "read.artifact_id" }
  ]
}
```

**Ejecución**:
```bash
cargo run --bin dag_engine -- run /tmp/test_create_read_roundtrip.json
```

**Resultado esperado**:
- Nodo read retorna el mismo IR que se almacenó, con versión v1
- SSE evento: `{ "output": { "ir": {...}, "version_id": "v1" } }`

**Verificación**: comparar IR leído con IR inicial, validar estructura.

---

**Resumen de cobertura QA**:
- Happy path: 3.1 (Excel vacío), 3.2 (Word con IR), 3.3 (label)
- Configuración: 3.4 (session_id config), 3.5 (session_id input), 3.8 (retention_limit), 3.9 (storage backend)
- Errores fail-closed: 3.6 (kind ausente), 3.7 (kind inválido)
- Integración: 3.10 (create → read roundtrip)
