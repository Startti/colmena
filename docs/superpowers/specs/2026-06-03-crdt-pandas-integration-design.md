# CRDT Documents — Pandas/Python Integration (C)

**Status:** approved 2026-06-03
**Subsystem:** C of the post-V2 MVP roadmap (after [B](2026-06-03-crdt-recent-changes-design.md))
**Predecessor:** [V2 — WS peer mode](2026-06-01-documents-crdt-v1-design.md) + [B — Recent changes awareness](2026-06-03-crdt-recent-changes-design.md)

## 1. Problema

Workbooks de Excel reales tienen miles a decenas de miles de filas. Cuando un usuario le pide al agente "agrupame las ventas por región" o "compará estos dos workbooks", el agente hoy tiene que:

1. **Leer todas las celdas en su contexto** vía `crdt_doc_read` — lo cual gasta cientos de miles de tokens y satura el contexto del LLM antes de poder hacer análisis serio.
2. **Hacer cálculos a mano** en su respuesta — propenso a errores numéricos, lento, y limitado a operaciones simples.

La pattern correcta (la que usan OpenAI Code Interpreter, LangChain pandas agent, etc.) es separar **el contexto que el LLM ve** del **dato que el código procesa**:

- LLM ve solo las primeras N filas para entender schema.
- LLM genera código pandas/python.
- Runtime ejecuta el código contra el dataset completo server-side.
- Output (ya agregado/transformado) vuelve al LLM, chico.

Ahorro típico: 10x-1000x en tokens dependiendo del tamaño del workbook.

## 2. Resultado deseado

Un agente con `crdt_documents` configurado puede:

- Leer un sample del workbook con tools ya existentes (`crdt_doc_read` con range chico).
- Llamar un tool nuevo `crdt_doc_run_python(sheet_ids, code, write_to_sheet?)` que:
  - Carga las sheets pedidas como pandas DataFrames.
  - Ejecuta código Python en sandbox restringido (sin red, sin filesystem, sin eval).
  - Devuelve `output` (cualquier JSON-serializable) al LLM directamente.
  - Opcionalmente escribe `output_sheet` (un DataFrame) como **una sheet nueva** en el workbook.
- Combinar múltiples sheets en un solo análisis (joins, comparisons).
- Usar pandas + numpy + scipy.stats sin importar manualmente librerías de red o I/O.

## 3. Decisiones tomadas (brainstorming 2026-06-03)

| # | Decisión | Resolución |
|---|---|---|
| 1 | Output destination | **Ambos modos en un solo tool**: `output` → LLM, `output_sheet` → new sheet en workbook. Args `write_to_sheet` controla el segundo. |
| 2 | Multi-sheet access | (β) `sheet_ids: Vec<String>`, expone `dfs: dict[sheet_id, DataFrame]`. El agente hace `df = dfs["sh_xxx"]` cuando solo necesita una. |
| 3 | Sandbox base | Reusa el `python_script restricted` mode existente (AST validation + import whitelist + banned builtins). |
| 4 | Sandbox extensions | Agrega `pandas`, `numpy`, `scipy.stats` al import whitelist. No agrega sklearn/matplotlib/requests. |
| 5 | Sheet name collision | Auto-suffix `"(2)"`, `"(3)"` pattern Excel-standard. |
| 6 | DataFrame → sheet conventions | Headers as row 1 (`header=True`). Index NOT included (`index=False`). |
| 7 | Execution location | Worker process (donde corre el llm_call), mismo módulo + sandbox que `python_script`. |
| 8 | Limites v1 | Hardcoded values (ver §6). Documentados como tech debt v1.1. |
| 9 | Preview/schema discovery | Reusa `crdt_doc_read` existente con range chico. No agrega `crdt_doc_describe`. |

## 4. Arquitectura

### 4.1 Flow end-to-end

```
LLM agente
   │
   ├── (típico turn) crdt_doc_list_sheets()                ← lista
   ├──               crdt_doc_read(sh_x, "A1:Z10")         ← sample
   │
   └── crdt_doc_run_python(sheet_ids, code, write_to_sheet?)
              │
              ▼
        ┌──────────────────────────────────────────────────┐
        │  Tool dispatcher (crdt_doc_tools.rs)             │
        │                                                  │
        │  1. Validate args (sheet_ids non-empty,          │
        │     write_to_sheet name valid Excel).            │
        │                                                  │
        │  2. For each sheet_id: build DataFrame from      │
        │     projection (sheet → flat cells dict →        │
        │     row-major table → DataFrame).                │
        │     Enforces: combined size < 100 MB.            │
        │                                                  │
        │  3. Call existing python_script::execute with:   │
        │     - code: the LLM's code                       │
        │     - sandbox_mode: "restricted"                 │
        │     - sandbox_timeout_secs: 30                   │
        │     - inputs: {"dfs": {...DataFrames}}           │
        │                                                  │
        │  4. Extract from python globals:                 │
        │     - `output` (any JSON-serializable)           │
        │     - `output_sheet` (must be pd.DataFrame       │
        │       if write_to_sheet is set)                  │
        │                                                  │
        │  5. If write_to_sheet:                           │
        │     - Resolve name collision via auto-suffix.    │
        │     - apply_add_sheet(doc, name) → sheet_id.     │
        │     - DataFrame → list of cells (row-major).     │
        │     - apply_set_range(doc, sh_new, A1, cells).   │
        │     - Record event(s) via backend.               │
        │                                                  │
        │  6. Apply truncation caps + build response.      │
        └──────────────────────────────────────────────────┘
              │
              ▼
        Response al LLM
        { output, wrote_sheet?, stdout, error? }
```

### 4.2 Componentes nuevos

| Componente | Path | Responsabilidad |
|---|---|---|
| Tool dispatcher | `dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` | Args parse, orchestration, response shaping |
| DataFrame builder | `crdt_documents/df_builder.rs` (NEW) | Convierte `Y.Doc projection` → pandas DataFrame; enforces size cap |
| DataFrame writer | `crdt_documents/df_writer.rs` (NEW) | DataFrame → list of `(row, col, value)` para `apply_set_range`; type coercion |
| Sandbox extension | `dag_engine/infrastructure/nodes/python_script/sandbox.rs` (modify) | Agregar pandas/numpy/scipy.stats al whitelist |

### 4.3 Tool signature

```rust
pub const TOOL_RUN_PYTHON: &str = "crdt_doc_run_python";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunPythonArgs {
    /// Sheets to load as DataFrames. Each available as dfs[<sheet_id>] in the
    /// Python environment. At least one required.
    pub sheet_ids: Vec<String>,
    /// Python code to execute. Must define `output` (any JSON-serializable
    /// value) and/or `output_sheet` (pandas DataFrame).
    pub code: String,
    /// If set, `output_sheet` is written as a new sheet with this name.
    /// Name collisions are resolved by appending " (2)", " (3)", etc.
    #[serde(default)]
    pub write_to_sheet: Option<String>,
}
```

### 4.4 Response shape

```json
{
  "output": <any JSON value or null>,
  "wrote_sheet": {
    "sheet_id": "sh_01H...",
    "name": "Sales by Region (2)",
    "n_rows": 4,
    "n_cols": 3,
    "preview": [
      {"Region": "North", "Sales": 450, "Avg_Qty": 12.3},
      {"Region": "South", "Sales": 320, "Avg_Qty": 9.1},
      ...
    ]
  } or null,
  "stdout": "<print() output, capped 10KB>",
  "error": "<Python traceback, capped 10KB, null if success>",
  "_truncated": <bool, only present if any cap was hit>
}
```

### 4.5 DataFrame construction (Y.Doc projection → pd.DataFrame)

Convención del projection actual (subsistema v1):
```json
{ "sheets": [{ "id": "sh_x", "name": "Inventory",
               "cells": {"A1": "Product", "B1": "Qty", "A2": "Apple", "B2": 10, ...} }] }
```

Mapeo a DataFrame:
1. Parsear cada `addr` a `(row, col)` (1-indexed).
2. Construir matriz dense `Vec<Vec<Value>>` con `None` para celdas ausentes.
3. **Row 1 = column names** (asumimos headers en la primera row — convención xlsx).
4. Rows 2+ son datos.
5. Tipos: pandas infiere dtype de cada column desde los valores.

Si row 1 está vacía o tiene celdas mixtas raras, las columnas se nombran `col_A`, `col_B`, etc. (fallback). Logueamos un warning pero seguimos.

Edge cases:
- Sheet vacía → DataFrame vacío (0 rows, 0 cols).
- Sheet con solo headers → DataFrame de 0 rows con las column names correctas.
- Cell con value `null` en `crdt_doc_set_cell` → `NaN` (float) o `None` (object) según dtype.

### 4.6 DataFrame writer (pd.DataFrame → Y.Doc sheet)

Convención inversa:
1. Crear nueva sheet vía `apply_add_sheet(doc, resolved_name)` → `sheet_id`.
2. Escribir column names en row 1: `set_cell(sh_new, "A1", col_name_1), ...`.
3. Escribir cada row: `set_cell(sh_new, "<col_letter><row+2>", value)`.
4. Type coercion al ir a JSON-of-cells (esquema CRDT):
   - `numpy.int64` / `numpy.float64` → JSON number.
   - `str` → JSON string.
   - `bool` / `numpy.bool_` → JSON boolean.
   - `numpy.datetime64` / `pd.Timestamp` → ISO 8601 string.
   - `NaN`/`None`/`pd.NaT` → JSON null (deletes the cell).
   - Otros → `str(value)` (fallback con warning).

Toda la escritura va en UN `transact_mut` para que sea atómica desde la perspectiva CRDT.

### 4.7 Resolución de sheet name collision

```rust
fn resolve_unique_sheet_name(doc: &Doc, requested: &str) -> String {
    let existing: HashSet<String> = list_sheet_names(doc).into_iter().collect();
    if !existing.contains(requested) { return requested.to_string(); }
    for i in 2..1000 {
        let candidate = format!("{requested} ({i})");
        if !existing.contains(&candidate) && candidate.len() <= 31 {
            return candidate;
        }
    }
    // 1000 conflicts: fall back to ULID suffix
    format!("{requested} {}", crate::crdt_documents::ArtifactId::new())
}
```

### 4.8 Integration con sandbox existente

El sandbox `restricted` de `python_script` ya valida vía AST:
- Imports whitelisted: solo módulos en una lista permitida.
- Banned builtins: `open, exec, eval, compile, __import__` rechazados.

Cambio: extender la whitelist con `pandas`, `numpy`, `scipy.stats`. Cambio mínimo en `sandbox.rs`:

```rust
// ANTES
const ALLOWED_IMPORTS: &[&str] = &[
    "math", "json", "re", "datetime", "collections",
    "itertools", "functools", "string", "decimal", "statistics",
];

// DESPUÉS
const ALLOWED_IMPORTS: &[&str] = &[
    "math", "json", "re", "datetime", "collections",
    "itertools", "functools", "string", "decimal", "statistics",
    // crdt_doc_run_python additions:
    "pandas", "numpy", "scipy.stats",
];
```

`scipy` full (no solo `.stats`) NO va — es 50MB+ y la mayoría de stats que el LLM va a usar viven en `scipy.stats`.

### 4.9 Maybe usar python_script directamente vs duplicarlo

Decisión: **NO usar python_script::execute directamente**. En su lugar, **invocar la misma función helper** que python_script usa internamente (probablemente `run_python_script(code, sandbox_mode, timeout, inputs)`).

Por qué: el tool dispatcher necesita inyectar `dfs` como input ANTES de ejecutar, y leer `output_sheet` DESPUÉS — el python_script node tiene otra semantic (espera SOLO `output`). Reusar la función helper sin la lógica de node permite control fino del environment + extracción.

Si la función helper no existe como public API (probablemente sea privada al módulo), refactorizamos para exponerla: `python_script::core::run(...)` o similar.

## 5. Modo Local vs WsPeer

- **Modo Local** (worker = server colocalizado): el worker tiene el Y.Doc en RAM via DocRegistry. Construcción de DataFrame es directa: leer del doc, projectar, build DF. Escritura: `apply_set_range` mute el doc, subscription notifica → snapshot writer eventualmente persiste.
- **Modo WsPeer** (worker stateless, server separado): el worker tiene una réplica Y.Doc local (vía WS sync). Lectura: misma — la réplica está al día. Escritura: muta réplica local → background WS task propaga al server → server fan-out a browsers.

Cero diferencia funcional entre modos para el tool. La única diferencia es transparente: en WsPeer mode, las escrituras vía `set_range` salen como CRDT updates al server (1 update por celda escrita, pero Yjs batchea).

## 6. Límites operativos (v1 hardcoded, v1.1 deuda técnica)

| Límite | Valor v1 | Razón | Path v1.1 |
|---|---|---|---|
| Combined DataFrame load size | 100 MB | Protege RAM del worker; >100MB raramente cabe en context de cualquier LLM | Hacer configurable vía node config (`max_load_mb`) o env var |
| Code execution timeout | 30s | Reusa default de `python_script` | Configurable vía args (`timeout_secs`) con cap server-side |
| `output` to LLM size | 10 KB serializado JSON | Token budget para que la respuesta sea utilizable por LLM | Configurable + soportar streaming chunks para output progresivo |
| `stdout` size | 10 KB | Idem | Configurable |
| `error` (traceback) size | 10 KB | Idem | Configurable + opcional log completo a archivo persistente para debug |
| `output_sheet` rows | 100 K | Razonable para Excel típico (>100K es raro pero existe en datasets analíticos) | Configurable + chunked writes para evitar transact_mut gigante |
| `write_to_sheet` name length | 31 chars | Excel limit (xlsx export sino falla) | Stays at 31 — Excel spec hard limit |

Cuando un cap se excede:
- `output` / `stdout` / `error` → trunca + agrega `"_truncated": true` (o un campo más específico tipo `"output_truncated": true`) en el response. El LLM ve la truncation explícitamente y puede pedir el resto en otro call.
- `output_sheet` rows > 100K → trunca a 100K + `wrote_sheet.truncated_at: 100000`. El LLM lo ve en el response, puede informar al usuario.
- Combined DataFrame load > 100MB → tool returns `{"error": "load_size_exceeded", "limit_mb": 100, "actual_mb": <N>}` sin ejecutar código. Agente decide qué hacer (pedir menos sheets, pedir un range específico).

### Tech debt explícito para v1.1

**Item BACKLOG.md a crear post-implementación**: "CRDT Documents v1.1 — Configurable limits for run_python tool". Solución propuesta:
1. Estructurar limits como `RunPythonLimits` struct con defaults match v1.
2. Cargar desde `node config` (`crdt_documents.run_python_limits.*`) o env vars (`COLMENA_CRDT_PY_MAX_LOAD_MB` etc).
3. Mantener ceiling absoluto hard-coded para prevenir abuse (ej. nunca permitir >1GB load aunque config diga).
4. Telemetry: emitir métricas cuando se hitea un cap (counter por tipo de cap).
5. Para `output_sheet` > 100K rows: chunked transact_mut (escribir en lotes de 10K, commit intermedio para evitar block del CRDT subscription).

Trigger para hacer v1.1: cuando observemos en producción que usuarios chocan caps regularmente, o cuando un cliente concreto pida específicamente datasets grandes.

## 7. Plan de testing

### Unit tests

- **DataFrame construction** (`df_builder.rs`):
  - Sheet vacía → DataFrame vacío.
  - Solo headers → 0 rows con column names correctos.
  - Mixed types per column → pandas dtype inference correcto.
  - Cells sparse (gaps) → NaN/None en los huecos.
  - Headers ausentes → fallback `col_A`, `col_B`.
  - Combined size > 100MB → error explícito.

- **DataFrame writer** (`df_writer.rs`):
  - Tipos básicos (int, float, str, bool) → JSON correcto.
  - NaN/None → JSON null (delete cell).
  - Timestamps → ISO 8601.
  - Index NOT included (verificar).
  - Headers en row 1 (verificar).

- **Sheet name collision** (`run_python.rs`):
  - Nombre único → se usa tal cual.
  - Nombre existe → `"(2)"`.
  - "(2)" existe también → `"(3)"`.
  - 1000+ collisions → ULID fallback.
  - Name > 31 chars → truncate o error (decidir; v1 trunca).

- **Truncation logic**:
  - `output` > 10KB → marcado con `_truncated`.
  - `output_sheet` > 100K rows → marcado con `truncated_at`.

### Integration tests

- **Full agent flow E2E** (`tests/crdt_doc_run_python_test.rs`):
  - Levanta CRDT server con sheets de prueba.
  - Conecta peer agente.
  - Ejecuta tool con código que computa agregaciones.
  - Verifica `output` returned al LLM.
  - Ejecuta tool con `write_to_sheet`.
  - Verifica nueva sheet existe en projection.
  - Verifica name collision auto-suffix funciona.

- **Sandbox enforcement** (`tests/crdt_run_python_sandbox_test.rs`):
  - Código intentando `open("/etc/passwd")` → error.
  - Código intentando `__import__("os")` → error.
  - Código importando `requests` → error.
  - Código importando `pandas` → OK.

### Browser smoke (manual)

- Subir un Excel con 500 rows.
- Pedir al agente "agrupame las ventas por región y guardame el resultado en una sheet nueva".
- Verificar en browser: aparece la sheet "Sales by Region" con los datos agregados.

## 8. Estimación

| Pieza | LoC aprox | Días |
|---|---|---|
| `df_builder.rs` (Y.Doc → DataFrame) | ~180 | 0.5 |
| `df_writer.rs` (DataFrame → Y.Doc) | ~150 | 0.5 |
| Sandbox extension (whitelist update + tests) | ~40 | 0.25 |
| Tool dispatcher `crdt_doc_run_python` | ~200 | 0.5 |
| Truncation/size cap logic | ~80 | 0.25 |
| Expose `python_script::run` as public helper (refactor) | ~60 | 0.25 |
| Unit tests | ~300 | 1 |
| Integration test (full agent flow) | ~150 | 0.5 |
| Sandbox enforcement integration test | ~80 | 0.25 |
| Docs (dev guide §5.6, node_configurations) | ~80 | 0.25 |
| BACKLOG entry (configurable limits) | ~30 | 0.1 |
| CHANGELOG entry | ~30 | 0.1 |
| **Total** | **~1380 LoC** | **~4.5 días dev** |

## 9. Fuera de scope (deferred)

- **Write-back a sheet existente (no solo new sheet)**: la convención v1 es always-new. Sobrescribir o append a una sheet existente queda para v1.1 cuando UX feedback lo justifique. Riesgo: destructive — necesita confirmación explícita.
- **Chart generation (matplotlib, plotly)**: no agregamos rendering. Si el usuario quiere ver una visualización, el agente puede escribir los datos del chart como sheet y el browser/Univer lo grafica.
- **Visualizaciones en tool result**: idem.
- **scipy/sklearn completos**: solo `scipy.stats`. Sklearn (ML) es out-of-scope; si se necesita, agregar como flag opcional con disclaimer.
- **Streaming output progresivo**: hoy todo el output es batch (waits for code finish). Streaming para análisis lento es v1.1.
- **Per-cell attribution para writes via run_python**: el evento se registra como "wrote N cells via python", coarse. Para per-cell, fuera de scope (incluso get_recent_changes coarse en peer:browser es BACKLOG).
- **Multi-artifact access en un solo tool call**: cargar sheets de DOS artifacts diferentes (cross-workbook joins) es exactamente subsistema F. Este tool opera sobre el artifact del context. Para joins inter-workbook, F resuelve.
- **Configurable limits**: hardcoded en v1, BACKLOG explícito para v1.1.
- **Network access desde código**: NUNCA — sandbox lo bloquea, y v1.1+ tampoco va a habilitar esto (compliance/security).

## 10. Cómo retomar

Spec listo + aprobado. Próximo paso: invocar el skill `writing-plans` para generar el plan de implementación con tasks numeradas, dependencies, y acceptance criteria por task.
