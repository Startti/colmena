# QA — Nodo `document_read`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs:314-406`
Fuentes de doc revisadas:
  - `docs/node_configurations.json:2579-2624`
  - `docs/node_as_tools_reference.json` (consultado; NO contiene entrada)
  - `docs/agent_context/node_ports_reference.md` (tabla de nodos)
  - `src/libs/colmena/src/documents/application/runtime.rs:40,91` (defaults ejecutables)

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas.

El código implementa toda la config documentada en `node_configurations.json:2579-2624`:
  - `artifact_id` requerido (línea 352-357)
  - `version` opcional (línea 359-363)
  - `storage_backend` y `storage_root` opcionales, con defaults aplicados en `DocumentRuntime::from_config()` (runtime.rs:88-99)

Nota sobre defaults: `node_configurations.json` declara explícitamente `"default": "localfs"` y `"default": ".colmena/documents"` para los campos storage, coincidiendo con `DEFAULT_STORAGE_ROOT` en runtime.rs:40 y el default "localfs" en runtime.rs:91.

## 2) Código NO documentado

### Hallazgo 1: `document_read` ausente de `docs/node_as_tools_reference.json`
- **Qué dice la doc**: node_configurations.json describe todos los campos de config, pero `node_as_tools_reference.json` (que documenta cómo usar nodos como herramientas LLM) NO tiene una entrada para `document_read`.
- **Qué hace el código**: El nodo IS executable y puede usarse como LLM tool via `tool_configurations` en un `llm_call`, igual que otros nodos.
- **Impacto QA**: Un usuario que quiera exponer `document_read` como herramienta LLM (p.ej., para que el agente lea documentos a demanda) no encontrará ejemplos ni configuración recomendada en la doc canónica de tools. Debe inferir la config desde node_configurations.json.
- **Confirmación**: grep -i 'document_read\|document_create\|document_edit' docs/node_as_tools_reference.json devuelve nada; solo hay refs a `crdt_documents` y `gdocs`.

### Hallazgo 2: Inconsistencia en schema() — defaults no mencionados en `document_read`
- **Qué dice la doc**: `node_configurations.json:2596-2607` especifica `storage_backend` y `storage_root` con `"default": "localfs"` y `"default": ".colmena/documents"`.
- **Qué hace el código**: 
  - `document_create.schema()` (línea 191-192) menciona explícitamente: `"storage_backend": "string (optional, default localfs)"` y `"storage_root": "string (optional, default ./.colmena/documents)"`.
  - `document_read.schema()` (línea 398-399) dice solo `"storage_backend": "string (optional)"` y `"storage_root": "string (optional)"` SIN mencionar defaults.
  - Los defaults ejecutables están en runtime.rs:40 (`DEFAULT_STORAGE_ROOT = ".colmena/documents"`) y runtime.rs:91 (default backend = "localfs").
- **Impacto QA**: Un desarrollador que lee el schema inline del nodo `document_read` no sabe qué defaults se aplicarán. Debe consultar `node_configurations.json` o el código de `DocumentRuntime::from_config()` para averiguarlo.

## 3) Plan de pruebas QA

### Caso 1: Roundtrip create → read HEAD (artifact_id en config)
**Objetivo:** Verificar que document_read obtiene la versión más reciente cuando no se especifica `version`.

**Grafo mínimo:**
```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "read", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}" } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "read.artifact_id" }
  ]
}
```

**Entrada/Ejecución:**
```bash
cargo run --bin dag_engine -- run case1.json
```

**Resultado esperado:**
```json
{
  "output": {
    "ir": { "kind": "excel", ... },
    "version_id": "v1"
  }
}
```

**Pass/Fail:** 
- ✓ PASS si `output.version_id == "v1"` y `output.ir` es un objeto con `kind="excel"`.
- ✗ FAIL si falta `output.ir` o `version_id`, o si el error es devuelto.

### Caso 2: Roundtrip create → edit → read versión específica
**Objetivo:** Verificar que `version` específica es respetada, y que `version` en input toma precedencia sobre config.

**Grafo mínimo:**
```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel", "initial_ir": { "kind": "excel", "artifact_id": "x", "version_id": "v1", "schema_version": "1.0.0", "workbook": { "sheets": [...], "named_styles": {} } } } },
    { "id": "edit", "type": "document_edit", "config": { "artifact_id": "{{artifact_id}}", "base_version": "v1", "ops": [] } },
    { "id": "read_v1", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}", "version": "v1" } },
    { "id": "read_input_override", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}", "version": "v2" } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "edit.artifact_id" },
    { "from": "create.artifact_id", "to": "read_v1.artifact_id" },
    { "from": "create.artifact_id", "to": "read_input_override.artifact_id" }
  ]
}
```

**Entrada/Ejecución:**
```bash
cargo run --bin dag_engine -- run case2.json
```

**Resultado esperado:**
- `read_v1.output.version_id == "v1"`
- `read_input_override.output.version_id == "v2"`

**Pass/Fail:**
- ✓ PASS si ambas lecturas devuelven los version_id correctos.
- ✗ FAIL si se devuelven versiones incorrectas o errores.

### Caso 3: Input vs config — artifact_id y version
**Objetivo:** Verificar que `artifact_id` y `version` en inputs toman precedencia sobre config.

**Grafo:**
```json
{
  "nodes": [
    { "id": "create1", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "create2", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "read_via_input", "type": "document_read", "config": { "artifact_id": "ignored_id" }, "inputs": { "artifact_id": "{{artifact_id_from_create2}}" } }
  ],
  "edges": [
    { "from": "create1.artifact_id", "to": "somewhere_unused" },
    { "from": "create2.artifact_id", "to": "read_via_input.artifact_id" }
  ]
}
```

**Resultado esperado:** El read devuelve el IR del `create2`, no del `create1`, probando que input.artifact_id ganó sobre config.artifact_id.

**Pass/Fail:**
- ✓ PASS si el artifact leído coincide con create2.
- ✗ FAIL si accidentally lee create1.

### Caso 4: Artifact no existe
**Objetivo:** Verificar que leer un artifact_id inexistente devuelve un error estructurado en `output.error`.

**Grafo:**
```json
{
  "nodes": [
    { "id": "read_missing", "type": "document_read", "config": { "artifact_id": "art_nonexistent" } }
  ],
  "edges": []
}
```

**Resultado esperado:**
```json
{
  "output": {
    "error": "artifact not found" // o similar
  }
}
```

**Pass/Fail:**
- ✓ PASS si `output.error` es un string no vacío (el nodo devuelve errores en output, no lanza excepción).
- ✗ FAIL si la ejecución lanza un error o devuelve un éxito falso.

### Caso 5: Version no existe
**Objetivo:** Verificar que pedir una versión inexistente (p.ej., "v99" cuando solo existen "v1", "v2") devuelve un error.

**Grafo:**
```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "read_bad_version", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}", "version": "v99" } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "read_bad_version.artifact_id" }
  ]
}
```

**Resultado esperado:**
```json
{
  "output": {
    "error": "version not found" // o similar
  }
}
```

**Pass/Fail:**
- ✓ PASS si `output.error` está presente.
- ✗ FAIL si el error es lanzado o la lectura devuelve v1 (fallback no deseado).

### Caso 6: Storage backend — localfs vs GCS (si soportado)
**Objetivo:** Verificar que el nodo respeta el backend de almacenamiento.

**Grafo para localfs (default):**
```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel", "storage_backend": "localfs", "storage_root": "/tmp/test_colmena" } },
    { "id": "read", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}", "storage_backend": "localfs", "storage_root": "/tmp/test_colmena" } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "read.artifact_id" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run case6_localfs.json
```

**Resultado esperado:** Create y read comparten la raíz `/tmp/test_colmena`, por lo que la lectura devuelve el IR creado.

**Pass/Fail:**
- ✓ PASS si la lectura coincide con el create.
- ✗ FAIL si se obtiene un error "artifact not found" (indicaría que no usa el mismo storage_root).

**Nota GCS:** Si GCS está habilitado (`feature = "gcs"`), ejecutar un test similar con `"storage_backend": "gcs"` requiere credenciales GCS. Aplazable si no hay credenciales en CI.

### Caso 7: Custom storage_root vs default
**Objetivo:** Verificar que omitir `storage_root` aplica el default `.colmena/documents`.

**Grafo sin storage_root:**
```json
{
  "nodes": [
    { "id": "create_no_root", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "read_no_root", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}" } }
  ],
  "edges": [
    { "from": "create_no_root.artifact_id", "to": "read_no_root.artifact_id" }
  ]
}
```

**Resultado esperado:** Los archivos se crean en `.colmena/documents` (default), y la lectura los recupera sin error.

**Pass/Fail:**
- ✓ PASS si read devuelve el IR creado.
- ✗ FAIL si error "artifact not found" sugiere que el default no se aplicó.

### Caso 8: artifact_id faltante (error MissingField)
**Objetivo:** Verificar que omitir `artifact_id` tanto en input como en config genera un error fail-closed.

**Grafo:**
```json
{
  "nodes": [
    { "id": "read_no_id", "type": "document_read", "config": { } }
  ],
  "edges": []
}
```

**Resultado esperado:**
```json
{
  "output": {
    "error": "missing required field: artifact_id" // o similar
  }
}
```

**Pass/Fail:**
- ✓ PASS si `output.error` contiene "artifact_id" o "missing".
- ✗ FAIL si el nodo ignora la ausencia o lanza un panic.

### Caso 9: Version como null/empty string
**Objetivo:** Verificar que pasar `version: null` o `version: ""` en config/input trata como "omitido" (lee HEAD).

**Grafo:**
```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "read_null_version", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}", "version": null } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "read_null_version.artifact_id" }
  ]
}
```

**Resultado esperado:**
```json
{
  "output": {
    "version_id": "v1"  // HEAD, no error
  }
}
```

**Pass/Fail:**
- ✓ PASS si lee HEAD sin error.
- ✗ FAIL si devuelve error "version not found" o similar.

### Caso 10: Output ports — verifica estructura
**Objetivo:** Verificar que el output tiene exactamente los puertos esperados (`ir` y `version_id`).

**Grafo:**
```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel", "initial_ir": { "kind": "excel", "artifact_id": "x", "version_id": "v1", "schema_version": "1.0.0", "workbook": { "sheets": [], "named_styles": {} } } } },
    { "id": "read", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}" } },
    { "id": "log", "type": "log", "inputs": { "message": "ir={{ir}}, version={{version_id}}" } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "read.artifact_id" },
    { "from": "read.ir", "to": "log.message" },
    { "from": "read.version_id", "to": "log.message" }
  ]
}
```

**Resultado esperado:** El nodo log recibe ambos valores y no lanza error de puerto faltante.

**Pass/Fail:**
- ✓ PASS si el log contiene tanto ir como version_id.
- ✗ FAIL si error "port not found" o si solo uno está disponible.

### Caso 11: Schema publication (default_output)
**Objetivo:** Verificar que el schema() del nodo declara correctamente `default_output: "output"`.

**Verificación de código:**
```rust
// En document_nodes.rs línea 381-383
fn default_output(&self) -> Option<&str> {
    Some("output")
}
```

**Prueba:** Invoca un edge desde el nodo sin especificar `.output` y verifica que el valor llega correctamente.

```json
{
  "nodes": [
    { "id": "create", "type": "document_create", "config": { "kind": "excel" } },
    { "id": "read", "type": "document_read", "config": { "artifact_id": "{{artifact_id}}" } },
    { "id": "output", "type": "output", "inputs": { "result": "{{default_output_of_read}}" } }
  ],
  "edges": [
    { "from": "create.artifact_id", "to": "read.artifact_id" },
    { "from": "read", "to": "output.result" }  // Sin especificar .output — depende del default
  ]
}
```

**Pass/Fail:**
- ✓ PASS si la arista `read` → `output.result` sin `.output` rellena correctamente con el output del read.
- ✗ FAIL si error de puerto o si el valor no llega.
