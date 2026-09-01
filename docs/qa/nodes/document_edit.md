# document_edit — Auditoría QA (Documentación vs Código)

**Nodo:** `document_edit`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs:202-312`  
**Documentación primaria:** `docs/developer_guide/27_documents_library.md` § 6  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.document_edit` (línea 2525)  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 node_as_tools_reference.json — NO tiene entrada para document_edit

**Problema:** `docs/node_as_tools_reference.json` está vacío en la sección `node_types` (no contiene entrada para `document_edit` ni `document_create` ni `document_read`).

**Realidad:** El nodo se usa frecuentemente como herramienta LLM en `llm_call.tool_configurations` (spec: `docs/developer_guide/27_documents_library.md` §7, línea 363). Los campos `artifact_id`, `base_version`, `ops` se exponen como parámetros LLM-visibles, con `storage_backend` y `storage_root` como fixed config.

**Impacto:** Alto. Operadores que buscan "cómo configurar `document_edit` como tool en `tool_configurations`" no encuentran ejemplo canónico en `node_as_tools_reference.json` y deben inferir desde el developer guide. **La guía manual de herramientas tiene por propósito reemplazar la inferencia.**

**Remediación:** Agregar entrada `document_edit` a `docs/node_as_tools_reference.json` bajo `node_types` con ejemplo de `node_schema` + `fixed_config` mostrando:
- LLM-visible: `artifact_id`, `base_version`, `ops`
- Fixed: `storage_backend`, `storage_root`
- Output shape: success `{ version_id, diff_summary }` y error `{ error, current_version?, conflicts? }`

---

### 1.2 developer_guide/27_documents_library.md § 6 — falta mención de `session_id` fallback

**Problema:** `docs/developer_guide/27_documents_library.md:290-299` documenta los campos de `document_edit` (artifact_id, base_version, ops) pero **no menciona que `session_id` se resuelve automáticamente** (input `__colmena_session_id` > input `session_id` > config `session_id` > `"default"`).

**Realidad en código:** `document_nodes.rs:36-45` implementa `resolve_session_id()` que aplica la cascada de fallback. El nodo usa este `session_id` para scoping de artifacts (línea 149 en create, análogo en edit).

**Verificación:** El comment en `document_nodes.rs:34-35` dice explícitamente: "Resolves the session_id used to scope artifacts. Priority matches the LLM node: input `__colmena_session_id` > input `session_id` > config `session_id`. Defaults to "default" when the graph runs standalone."

**Impacto:** Medio. Operadores que quieran sobrescribir el `session_id` por defecto desconocen que pueden pasarlo en inputs (en lugar de solo en config). En un agente multi-usuario, entender ese fallback es crítico para aislar artifacts por sesión.

**Recomendación:** Actualizar `docs/developer_guide/27_documents_library.md` línea ~295 para añadir una nota:
> **Nota sobre session_id:** El nodo resuelve `session_id` automáticamente del contexto de ejecución (input `__colmena_session_id` > input `session_id` > config `session_id` > default `"default"`). Esto asegura que los artifacts permanezcan aislados por sesión incluso cuando se usan en `llm_call` con contexto automático. Ver `resolve_session_id()` en document_nodes.rs:36.

---

### 1.3 node_configurations.json — descripción de output de conflicto es exacta pero poco clara sobre `conflicts` array

**Problema:** `docs/node_configurations.json:2570` describe el output de VersionConflict así:
```
"On version mismatch: { \"error\": \"VersionConflict\", \"current_version\": n, \"conflicts\": [...] }"
```

**Realidad en código:** `document_nodes.rs:61-72` convierte `DocumentError::VersionConflict` a JSON:
```rust
DocumentError::VersionConflict { current, conflicts, .. } => 
  json!({ "error": "VersionConflict", "current_version": current.0, "conflicts": conflicts })
```

**Verificación:** El campo `conflicts` contiene operaciones de patch que colisionaron. Su estructura depende del tipo de documento (Excel ops, Word ops, HTML ops), pero la docs **no especifica qué forma tiene ese array** — no dice si son strings IDs, objetos con detalles, etc.

**Impacto:** Bajo. El comportamiento es correcto; la docs es técnicamente acertada pero poco detallada. Un operador que recibe un conflicto puede inspeccionarlo en runtime pero no tiene guía previa de qué esperar.

**Recomendación:** Extender la descripción a nivel de ejemplo en `node_configurations.json:2570`, o agregar una nota en el developer guide (§6) que muestre un VersionConflict real con estructura de `conflicts`.

---

### 1.4 node_ports_reference.md — entrada resumida pero correcta

**Problema:** `docs/agent_context/node_ports_reference.md:86,128` tiene entradas para `document_edit`, pero son muy resumidas (una línea por puerto de entrada/salida).

**Realidad en código:** El nodo tiene 3 input ports (`artifact_id`, `base_version`, `ops`) y 1 output port (`output`). Todos bien definidos en el código.

**Verificación:** Línea 86 dice: *"Patch a document → new version | Applies an ordered `ops` array against a `base_version`. Optimistic concurrency: returns a structured `VersionConflict` (as output, not a thrown error) if the artifact advanced past `base_version`. Returns `{ version_id, diff_summary }`."* — técnicamente completo pero denso.

**Impacto:** Muy bajo. La entrada está ahí y es correcta; solo falta expandirse en tipo y descripción según el formato de `node_ports_reference.md`.

**Estado:** OK (información presente, solo formateo comprimido).

---

## 2. Hallazgos: Código

### 2.1 Resolución input-first de campos — correctamente alineada con docs

**Problema:** ¿Cómo se resuelven `artifact_id`, `base_version`, `ops` cuando ambas (inputs y config) están presentes?

**Realidad en código:** `document_nodes.rs:243-261` implementa:
```rust
let artifact_id = inputs.get("artifact_id")
    .and_then(|v| v.as_str())
    .or_else(|| config.get("artifact_id").and_then(|v| v.as_str()))
    .ok_or(DocNodeError::MissingField("artifact_id"))?
```

**Verificación:** Documentación en `docs/developer_guide/27_documents_library.md:276` dice explícitamente: "Fields are input-first, config-fallback." Correcto.

**Estado:** Excelente. Comportamiento y docs alineados.

---

### 2.2 Error handling en deserialization de `ops` — mensaje de error claro

**Problema:** Si `ops` no deserializa a `Vec<PatchOp>`, ¿qué error se retorna?

**Realidad en código:** `document_nodes.rs:262-265`:
```rust
let ops: Vec<PatchOp> = serde_json::from_value(ops_raw).map_err(|e| {
    Box::new(DocNodeError::Config(format!("invalid ops array: {e}")))
})?
```

**Verificación:** `node_configurations.json:2546` documenta que `ops` es `"required": true` y acepta `$DYNAMIC`, pero **no menciona que deserialization error emite un `Config` error**.

**Impacto:** Muy bajo. Error es claro y útil en runtime. La docs podría mencionar que arrays malformadas retornan error, pero es comportamiento estándar.

**Estado:** OK. Implementación robusta.

---

### 2.3 `default_output()` retorna None — rationale diferente al de python_script

**Problema:** `document_nodes.rs:285-287` implementa:
```rust
fn default_output(&self) -> Option<&str> {
    Some("output")
}
```

**Verificación:** A diferencia de `python_script` que retorna `None` para pasar valores brutos, `document_edit` retorna `Some("output")` — alineado con casi todos los nodos que envuelven su salida.

**Verificación documental:** `node_configurations.json:2574` documenta `"default_output": "output"`. Correcto.

**Estado:** OK. Decisión coherente y documentada.

---

### 2.4 Comportamiento de `PatchSource::Agent` es hardcoded, no configurable

**Problema:** `document_nodes.rs:270` asigna `source: PatchSource::Agent` siempre, sin exponer un parámetro LLM-visible.

**Realidad:** Por diseño — los patches en `document_edit` vienen siempre del agente (LLM). Ediciones de usuario vienen vía las synthetic tools, que usan `PatchSource::User`.

**Documentación:** `docs/developer_guide/27_documents_library.md:94-95` documenta que `source` puede ser `"agent"` (por el LLM, valor por defecto) o `"user"` (edición humana directa). Pero **no aclara que en el nodo DAG `document_edit`, la fuente siempre es `agent`**.

**Impacto:** Bajo. Comportamiento es correcto (el DAG node es frontal de LLM, no de usuario); la docs podría ser más explícita, pero está implícita en el context.

**Recomendación:** Opcional — agregar nota en §6 del developer guide: "El nodo DAG `document_edit` siempre etiqueta patches con `PatchSource::Agent`. Para ediciones humanas directas, usa las synthetic tools con `PatchSource::User`."

**Estado:** OK (comportamiento correcto, docs podría ser más detallada).

---

### 2.5 VersionConflict estructura de salida — code matches docs shape

**Problema:** ¿Cuál es la forma exacta de `{ error, current_version, conflicts }` en VersionConflict?

**Realidad en código:** `document_nodes.rs:61-72` (función `document_error_to_value`):
```rust
DocumentError::VersionConflict { current, conflicts, .. } => 
  json!({ "error": "VersionConflict", "current_version": current.0, "conflicts": conflicts })
```

**Verificación:** `current.0` es un string (VersionId), `conflicts` es el array de operaciones que colisionaron. Output se envuelve en `{ "output": { ... } }` (línea 281).

**Estado:** Correcto. Shape matches `node_configurations.json:2570`.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying estable.

### 3.1 Test A: Crear, editar y leer (roundtrip básico)

**Archivo:** `tests/graphs/documents/smoke_create_edit_read.json` (existe, líneas 314-351 de developer guide)

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/smoke_create_edit_read.json --agent-session-id test_a_001
```

**Validación esperada:**
- Nodo `create_step` crea Excel vacío, emite `artifact_id` y `version_id: "v1"`
- Nodo `edit_step` aplica patch: 2x `set_cell` a `sheet_id: "s1"`, ubicaciones A1 y B1
- Salida: `version_id: "v2"`, `diff_summary` con resumen NL de cambios
- Nodo `read_step` lee version v2 (especificada en input desde `edit_step.output.version_id`)
- Verifica celdas A1="Hola" y B1=42

---

### 3.2 Test B: VersionConflict — base_version desactualizada

**Archivo:** (crear) `tests/graphs/documents/document_edit_conflict.json`

```json
{
  "nodes": {
    "create": {
      "type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs_conflict",
        "initial_ir": {
          "kind": "excel", "artifact_id": "x", "version_id": "v1",
          "schema_version": "1.0.0",
          "workbook": {
            "sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}],
            "named_styles": {}
          }
        }
      }
    },
    "edit_v1": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_test_docs_conflict",
        "base_version": "v1",
        "ops": [{"op": "set_cell", "sheet_id": "s1", "address": "A1", "value": "First"}]
      }
    },
    "edit_v1_stale": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_test_docs_conflict",
        "base_version": "v1",
        "ops": [{"op": "set_cell", "sheet_id": "s1", "address": "B1", "value": "Stale"}]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "create", "to": "edit_v1" },
    { "from": "create.output.artifact_id", "to": "edit_v1_stale.artifact_id" },
    { "from": "edit_v1", "to": "edit_v1_stale" },
    { "from": "edit_v1_stale", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/document_edit_conflict.json --agent-session-id test_b_001
```

**Validación esperada:**
- `create` → v1
- `edit_v1` aplica sobre v1 → v2, salida `{ version_id: "v2", diff_summary: "..." }`
- `edit_v1_stale` intenta aplicar sobre v1 (desactualizado, pues HEAD es v2) → VersionConflict
- Salida: `{ error: "VersionConflict", current_version: "v2", conflicts: [ ... ] }`
- **Verificación:** error no es una excepción, sino un output estructurado (línea 281 de document_nodes.rs)

---

### 3.3 Test C: Múltiples ops en un patch (atomicidad)

**Archivo:** (crear) `tests/graphs/documents/document_edit_multi_op.json`

```json
{
  "nodes": {
    "create": {
      "type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs_multi",
        "initial_ir": {
          "kind": "excel", "artifact_id": "x", "version_id": "v1",
          "schema_version": "1.0.0",
          "workbook": {
            "sheets": [{"id": "s1", "name": "Data", "order": 0, "columns": [], "cells": {}, "tables": []}],
            "named_styles": {}
          }
        }
      }
    },
    "edit_multi": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_test_docs_multi",
        "base_version": "v1",
        "ops": [
          {"op": "set_cell", "sheet_id": "s1", "address": "A1", "value": "Name"},
          {"op": "set_cell", "sheet_id": "s1", "address": "B1", "value": "Age"},
          {"op": "set_cell", "sheet_id": "s1", "address": "A2", "value": "Alice"},
          {"op": "set_cell", "sheet_id": "s1", "address": "B2", "value": 30},
          {"op": "set_cell", "sheet_id": "s1", "address": "A3", "value": "Bob"},
          {"op": "set_cell", "sheet_id": "s1", "address": "B3", "value": 25}
        ]
      }
    },
    "read": {
      "type": "document_read",
      "config": { "storage_root": "/tmp/colmena_test_docs_multi" }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "create.output.artifact_id", "to": "edit_multi.artifact_id" },
    { "from": "create.output.version_id", "to": "edit_multi.base_version" },
    { "from": "create.output.artifact_id", "to": "read.artifact_id" },
    { "from": "edit_multi.output.version_id", "to": "read.version" },
    { "from": "read", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/document_edit_multi_op.json --agent-session-id test_c_001
```

**Validación esperada:**
- Patch con 6 ops se aplica atómicamente: v1 → v2
- IR resultante tiene todas 6 celdas pobladas (A1:B3)
- `diff_summary` es una cadena NL que resume todos los cambios
- Atomicidad verificada: si alguna op falla, **ninguna** se aplica (el archivo de IR v2 no existe)

---

### 3.4 Test D: Ops inválidas deserialización fallida

**Archivo:** (crear) `tests/graphs/documents/document_edit_invalid_ops.json`

```json
{
  "nodes": {
    "create": {
      "type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs_invalid_ops",
        "initial_ir": {
          "kind": "excel", "artifact_id": "x", "version_id": "v1",
          "schema_version": "1.0.0",
          "workbook": {
            "sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}],
            "named_styles": {}
          }
        }
      }
    },
    "edit_bad_ops": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_test_docs_invalid_ops",
        "base_version": "v1",
        "ops": [
          {"op": "unknown_op", "sheet_id": "s1"}
        ]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "create.output.artifact_id", "to": "edit_bad_ops.artifact_id" },
    { "from": "create.output.version_id", "to": "edit_bad_ops.base_version" },
    { "from": "edit_bad_ops", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/document_edit_invalid_ops.json --agent-session-id test_d_001
```

**Validación esperada:**
- Deserialization de `ops` falla (campo `op: "unknown_op"` no existe en enum `PatchOp`)
- Node emite salida: `{ "output": { "error": "invalid ops array: ..." } }`
- Nodo no retorna excepción Rust; error se retorna como output JSON (línea 281)
- Flujo continúa (error manejado gracefully)

---

### 3.5 Test E: Missing required fields — artifact_id

**Archivo:** (crear) `tests/graphs/documents/document_edit_missing_artifact_id.json`

```json
{
  "nodes": {
    "create": {
      "type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs_missing_id",
        "initial_ir": {
          "kind": "excel", "artifact_id": "x", "version_id": "v1",
          "schema_version": "1.0.0",
          "workbook": {
            "sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}],
            "named_styles": {}
          }
        }
      }
    },
    "edit_no_id": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_test_docs_missing_id",
        "base_version": "v1",
        "ops": []
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "create", "to": "edit_no_id" },
    { "from": "edit_no_id", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/document_edit_missing_artifact_id.json --agent-session-id test_e_001
```

**Validación esperada:**
- `edit_no_id` no recibe `artifact_id` ni en inputs ni en config (edge from create emite toda la salida, pero no hay conexión para extraer `artifact_id` específicamente)
- Nodo retorna: `{ "output": { "error": "PythonNode error: 'code' field is missing in inputs or config" } }`
- **Nota:** Mensaje de error dice "missing in inputs or config", alineado con línea 247-248 de document_nodes.rs
- Verificación: error es un `MissingField("artifact_id")` retornado como output error string

---

### 3.6 Test F: $DYNAMIC placeholders en ops

**Archivo:** (crear) `tests/graphs/documents/document_edit_dynamic_ops.json`

```json
{
  "nodes": {
    "create": {
      "type": "document_create",
      "config": {
        "kind": "excel",
        "storage_root": "/tmp/colmena_test_docs_dynamic",
        "initial_ir": {
          "kind": "excel", "artifact_id": "x", "version_id": "v1",
          "schema_version": "1.0.0",
          "workbook": {
            "sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}],
            "named_styles": {}
          }
        }
      }
    },
    "input_ops": {
      "type": "mock_input",
      "config": {
        "ops": [
          {"op": "set_cell", "sheet_id": "s1", "address": "A1", "value": "Dynamic Value"}
        ]
      }
    },
    "edit_dynamic": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_test_docs_dynamic",
        "base_version": "v1"
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "create.output.artifact_id", "to": "edit_dynamic.artifact_id" },
    { "from": "create.output.version_id", "to": "edit_dynamic.base_version" },
    { "from": "input_ops.ops", "to": "edit_dynamic.ops" },
    { "from": "edit_dynamic", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/document_edit_dynamic_ops.json --agent-session-id test_f_001
```

**Validación esperada:**
- `ops` se pasa vía input edge (input-first fallback funciona correctamente)
- Patch aplica el valor dinámico: A1 = "Dynamic Value"
- Salida: v2 con `diff_summary` que refleja el cambio

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Docs | Alta | `node_as_tools_reference.json` no tiene entrada para `document_edit` (ni otros nodos de documentos) |
| 1.2 | Docs | Media | developer_guide/27_documents_library.md no menciona fallback automático de `session_id` |
| 1.3 | Docs | Baja | `node_configurations.json` describe VersionConflict shape pero no estructura de `conflicts` array |
| 1.4 | Docs | OK | `node_ports_reference.md` tiene entrada resumida pero correcta |
| 2.1 | Código | OK | Resolución input-first alineada con documentación |
| 2.2 | Código | OK | Deserialization error handling es claro y robusto |
| 2.3 | Código | OK | `default_output="output"` coherente con nodos similares |
| 2.4 | Código | OK | `PatchSource::Agent` hardcoded por diseño; documentación implícita |
| 2.5 | Código | OK | VersionConflict output shape coincide con docs |

---

## Remediaciones Recomendadas

### Prioridad ALTA (bloquea documentación automática)

1. **Agregar entrada `document_edit` a `docs/node_as_tools_reference.json`** bajo `node_types` con:
   - Nombre: `document_edit`
   - Descripción LLM-visible de qué hace
   - `node_schema` mostrando `artifact_id`, `base_version`, `ops` LLM-visibles
   - `fixed_config` con `storage_backend`, `storage_root`
   - Ejemplo real de tool_configurations entry

### Prioridad MEDIA (afecta discovery)

2. **Expandir `docs/developer_guide/27_documents_library.md` § 6** para mencionar `session_id` fallback automático.

### Prioridad BAJA (legibilidad)

3. **Enriquecer `node_configurations.json:2570`** con ejemplo real de VersionConflict output, mostrando la estructura de `conflicts` array.

---

**Auditoría completada:** 5 hallazgos en documentación (1 alta, 1 media, 1 baja, 2 OK) + 5 aspectos de código validados (todos OK) + 6 casos de prueba ejecutables.

