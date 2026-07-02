# `data_run_python` — tool unificado de movimiento de datos tabulares (CSV/XLSX ↔ Google Sheets ↔ SQL)

**Fecha:** 2026-07-01
**Estado:** Diseño aprobado en brainstorm (daniel@startti.co) — pendiente plan de implementación
**Autores:** brainstorm daniel + Claude

---

## 1. Problema y objetivo

Hoy un agente Colmena puede computar con pandas server-side sobre tres fuentes
tabulares — pero cada fuente tiene su propio tool, y **el único destino de
write-back estructurado son hojas de cálculo** (`output_sheets`). No existe
manera de aterrizar el resultado de un cómputo pandas en una **tabla SQL** con
semántica `append` / `update` / `upsert`, ni de cruzar fuentes heterogéneas
(un CSV subido contra una tabla Postgres) en una sola operación.

**Objetivo principal (verbatim del brainstorm):** poder mover datos entre
Excel/CSV, Google Sheets y SQL **de la manera más fácil posible, sin gastar
miles de tokens leyendo columnas** — las filas nunca pasan por el contexto del
LLM.

### Estado actual (lo que ya existe y se reutiliza)

| Pieza existente | Qué hace | Qué le falta |
|---|---|---|
| `attachment_run_python` | CSV/XLSX adjunto → `df` preloaded → sandbox pandas | Sin write-back de ningún tipo; una sola fuente por call |
| `gsheets_run_python` | N bindings de Google Sheets (fetch paralelo) + inline `data` → sandbox → `output_sheets` (replace/update_in_place/update_by_position/overwrite) | No puede leer SQL ni attachments; no puede escribir a SQL |
| `crdt_doc_run_python` | Ídem sobre workbook CRDT local | Fuera de scope de este diseño (sin cambios) |
| `sql_query` (nodo) | SQL string → pipeline validate→critic→execute; presets, allowed_schemas, RLS, setup_sql | El LLM escribe SQL a mano; no hay puente DataFrame→tabla |
| `sql_bulk_insert_from_attachment` | CSV adjunto → COPY a Postgres (append crudo del archivo entero) | Sin transformación intermedia, sin update/upsert |
| `diff_writer.rs` | Diff puro de records + validaciones (key única, column mismatch) | Agnóstico de backend — reutilizable tal cual |
| `sheet_collision.rs` | Collision policy fail/auto_suffix/overwrite | Patrón a espejar para tablas |
| `execute_sandboxed_helper` | Sandbox Python `restricted`, 30 s, pandas/numpy/scipy | Reutilizable tal cual |

---

## 2. Decisiones de diseño (cerradas en brainstorm)

1. **Sink dentro del sandbox, no tool separada.** El write-back a SQL es un
   global Python (`output_tables`) asignado por el código, igual que
   `output_sheets`. Una tool separada obligaría a que las filas salgan del
   sandbox y vuelvan a entrar por el contexto del LLM — exactamente el costo
   que esta familia de tools existe para evitar.
2. **Config SQL self-contained (Enlace 2).** El tool lleva su propio bloque
   `sql` en `fixed_config` (`connection_url` + `permissions`), autorado por el
   operador. No referencia por nombre a otro tool. El `PgPoolRegistry`
   compartido garantiza que si un nodo `sql_query` apunta al mismo
   `connection_url`, comparten pool sin config duplicada de conexión.
3. **Tool unificado (Opción 2), no tres tools con matriz de ruteo.** Un solo
   `data_run_python` con bindings polimórficos por `source` elimina el
   problema de "¿cuál run_python llamo?" y habilita el caso estrella: cruzar
   la planilla entrante contra la tabla destino **en una sola call**.
4. **Gating por capacidad configurada.** Cada `source`/sink se habilita solo
   si el operador configuró la capacidad correspondiente (ver §5). Attachment
   e inline están siempre disponibles.
5. **Deprecación por redundancia, gated en verificación.** El tool se ship
   primero de forma **aditiva**; una vez verificado end-to-end (§14), el
   **mismo PR (o uno inmediato)** borra las dos tools 100 % subsumidas —
   `gsheets_run_python` y `attachment_run_python` — porque son pura
   redundancia. `crdt_doc_run_python` y el par `sql_bulk_*` **se mantienen**:
   no son redundantes, cubren capacidades que `data_run_python` no cubre (ver
   §15). Es un breaking change para ADP → sweep obligatorio del worker antes
   del borrado.
6. **Auto-creación de tabla en `append`/`replace`** cuando la tabla no existe,
   con tipos inferidos del DataFrame, gated por permisos (ver §7.4).

---

## 3. Vista de pájaro

```
                        ┌──────────────────────────────────────────┐
   bindings (fuentes)   │       data_run_python (dispatcher)       │   sinks (destinos)
                        │                                          │
  attachment (CSV/XLSX)─┤► fetch/parse ──┐                         │
  gsheets (sheet/range)─┤► fetch paralelo┤   sandbox Python        ├─► output_tables   → Postgres (append/update/upsert)
  sql (SELECT)         ─┤► query        ─┤   (restricted, 30s)     ├─► output_sheets   → Google Sheets (modos existentes)
  inline data          ─┤► normalize   ──┘   pandas/numpy/scipy    ├─► output_attachments → CSV/XLSX registrado en catálogo
                        │                                          │
                        │        output (JSON) → LLM  (~80 tokens) │
                        └──────────────────────────────────────────┘
```

Cualquier fuente → cualquier destino, en una sola tool call, con las filas
viajando solo server-side. Los cuatro "almacenes" (attachment, gsheets, SQL,
inline) quedan totalmente interconectados.

---

## 4. Superficie del tool

### 4.1 Nombre y registro

- Tool sintético `data_run_python`, definido en
  `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs`.
- Descripción y summary en `text/tools/` (registry YAML), como el resto.
- Ruteado en `dag_tool_executor.rs` como los demás sintéticos.
- Activación: entry en `tool_configurations` con `node_type: "data_run_python"`
  (necesita `fixed_config`, así que **no** es flag-only; ver §5).

### 4.2 Args (lo que ve el LLM)

```jsonc
{
  "bindings": [ /* 1..N bindings polimórficos, fetch paralelo */ ],
  "code": "…python…",
  // destinos opcionales:
  "write_to_spreadsheet": "<spreadsheet_id>"   // requerido solo si el código asigna output_sheets
}
```

`connection_url`, `permissions`, `on_existing_sheet`, `on_existing_table`
van en `fixed_config` — el LLM nunca los ve.

### 4.3 Bindings polimórficos

Cada binding tiene `var` (nombre del global Python) + **exactamente un**
discriminador de fuente. El discriminador es estructural (qué campos están
presentes), igual que hoy `gsheets_run_python` distingue sheet vs inline —
no se agrega un campo `source` explícito para no romper el reflejo con los
tools existentes, pero el schema documenta las cuatro formas:

| Forma | Campos | Ejemplo |
|---|---|---|
| **ATTACHMENT** | `attachment_id` (+ `sheet_name?`, `delimiter?`, `header_row?`) | `{"var": "nuevo", "attachment_id": "doc_ab12", "sheet_name": "Hoja1"}` |
| **GSHEETS** | `spreadsheet_id` + `sheet` (+ `range?`) | `{"var": "ref", "spreadsheet_id": "1abc", "sheet": "Q4"}` |
| **SQL** | `query` (SELECT-only) | `{"var": "clientes", "query": "SELECT id, email FROM crm.clientes"}` |
| **INLINE** | `data` (array de records o 2-D con header) | `{"var": "extraida", "data": [{"sku": "A1", "qty": 3}]}` |

Reglas (heredadas de `gsheets_run_python`, mismas validaciones):

- `var` no vacío, sin duplicados.
- Exactamente una forma por binding; ambigüedad → error estructurado.
- Se acepta el deserializer flexible existente (array canónico u objeto
  `{var: binding}`) — se extrae a un helper compartido.
- Fetch **en paralelo** (`join_all`) para gsheets y SQL; attachments en
  paralelo entre sí vía el cable compartido de attachment plumbing
  (`fetch_attachment_stream`, mismo camino que `attachment_run_python`).
- Cada binding queda en el sandbox como lista de records `{col: val}`;
  el LLM llama `pd.DataFrame(<var>)`. `_loaded_columns` disponible como
  global (y se devuelve en errores para self-correction sin round-trip).

#### Binding SQL — restricciones

- **SELECT-only, validado por AST** (`sqlparser`, mismo dialecto Postgres que
  `sql_query`). Un binding jamás muta: INSERT/UPDATE/DELETE/DDL en `query` →
  error `BindingMustBeSelect`. Multi-statement → rechazado.
- Pasa por el **StaticRuleValidator** con las `permissions` del bloque `sql`
  de `fixed_config` (respeta `allowed_schemas`; `information_schema` y
  `pg_catalog` permitidos para introspección).
- Cap de filas por binding: **100 000** (constante compartida con
  `MAX_BULK_INSERT_ROWS`). Se inyecta `LIMIT cap+1` si no hay LIMIT explícito;
  si desborda → error `BindingTooLarge` con el conteo, sugiriendo agregar
  WHERE/agregación al SELECT.
- `statement_timeout_ms` / `work_mem_mb` de `runtime_limits` aplican
  (`SET LOCAL` por transacción, como `sql_query`).
- El marshalling PgRow→JSON reutiliza el mapper del nodo SQL (misma tabla de
  tipos, mismo caveat NUMERIC→f64).

#### Binding ATTACHMENT — restricciones

Mismas caps que `attachment_run_python`: archivo ≤ 50 MB, ≤ 100 000 filas.
CSV con auto-delimiter, XLSX con `sheet_name` opcional (default primera),
`header_row` 1-indexed. Parser reutilizado de `sql_bulk_tools.rs` /
`attachment_run_python.rs` (se extrae a helper compartido si hay
duplicación).

### 4.4 Sandbox

Idéntico a los existentes — se reutiliza `execute_sandboxed_helper`:

- Modo `restricted` (sin os/sys/subprocess/network/filesystem).
- `pd`, `np`, `stats` + stdlib segura.
- Timeout 30 s wall-clock, `spawn_blocking`.
- Caps de respuesta: `output`/`stdout`/`error` a 10 KB c/u.
- Prelude/postlude propios en `text/prompts/python_sandbox/`
  (`data_run_python_prelude.md` / `_postlude.md`), derivados de los de
  gsheets; el postlude empaqueta `{user_output, output_sheets, output_tables,
  output_attachments}`.

---

## 5. Gating por capacidad (el "elemento de protección")

Cada fuente/sink se habilita **solo si el operador configuró la capacidad**.
El LLM no puede inventar una fuente que el operador no habilitó:

| Capacidad | Gate | Cuando falta |
|---|---|---|
| `attachment` (source) | **Siempre disponible** — usa el catálogo de attachments de la conversación | — |
| `inline` (source) | **Siempre disponible** | — |
| `gsheets` (source + sink `output_sheets`) | El agente tiene el toolkit gsheets habilitado (`enabled_tools` contiene `gsheets` o algún `gsheets_*`) **o** `fixed_config.enable_gsheets: true` en el tool | Error `SourceNotEnabled {source: "gsheets"}` con hint al operador |
| `sql` (source + sink `output_tables`) | `fixed_config.sql` presente (bloque con `connection_url` + `permissions`) | Error `SourceNotEnabled {source: "sql"}` |

Mecánica:

1. **En el build de la ToolDefinition** (init del `llm_call`), la
   `description` del tool se ensambla dinámicamente listando SOLO las fuentes
   activas — el modelo ni se entera de que existe un `source` sql si no está
   configurado. (Mismo patrón que el `description_supplement` del nodo SQL.)
2. **En el dispatcher** (defensa en profundidad), un binding/sink de una
   capacidad no configurada devuelve el error estructurado con
   `enabled_sources: [...]` para que el modelo se auto-corrija.

Validación de config en init: `fixed_config.sql` sin `connection_url` o sin
`permissions.allowed_schemas` → el tool falla el init con mensaje claro al
operador (mismo patrón hard-fail que `sql_query`/`setup_sql`).

---

## 6. Config del operador (`fixed_config`)

```jsonc
"tool_configurations": {
  "data_run_python": {
    "name": "data_run_python",
    "node_type": "data_run_python",
    "description": "Mueve y analiza datos entre archivos, Google Sheets y la base de datos.",
    "fixed_config": {
      // ── Capacidad SQL (habilita source sql + sink output_tables) ──
      "sql": {
        "connection_url": "${DATABASE_URL}",          // env-resolved, como sql_query
        "permissions": {
          "preset": "read_write",                     // presets del nodo SQL
          "allowed_schemas": ["analytics", "crm"],
          // tenant_user_id / tenant_column / auto_rls soportados (ver §7.6)
        },
        "runtime_limits": { "statement_timeout_ms": 30000, "work_mem_mb": 64 },
        "on_missing_table": "create",                 // create (default) | fail
        "on_existing_table": "fail"                   // fail (default) | append — solo aplica a mode replace
      },
      // ── Capacidad gsheets (opcional; auto-on si el toolkit gsheets está) ──
      "enable_gsheets": true,
      "on_existing_sheet": "fail"                     // policy existente de sheets
    }
  }
}
```

Notas:

- El bloque `sql.permissions` es **la misma estructura** que el nodo
  `sql_query` (`SqlPermissions`) — se deserializa con el mismo tipo de
  dominio, cero divergencia.
- El pool se obtiene vía `SqlPortFactory::get_adapter(url)` →
  `PgPoolRegistry` compartido. Si el agente además tiene un tool `sql_query`
  al mismo URL, **comparten pool**.
- `setup_sql` NO se soporta en este tool (v1): el provisioning de tablas
  destino es del nodo `sql_query` del grafo o de migraciones. La auto-creación
  de §7.4 cubre el caso "tabla de resultados nueva".

---

## 7. Sink `output_tables` — write-back a Postgres

### 7.1 Forma (global Python)

```python
output_tables = {
    "analytics.ventas_por_region": {          # "schema.tabla" o "tabla" (primer allowed_schema como default)
        "mode": "upsert",                     # append | update | upsert | replace
        "df": resumen,                        # DataFrame o lista de records
        "key": "region",                      # requerido para update/upsert; str o lista (key compuesta)
        "columns": ["total", "updated_at"],   # opcional (update/upsert): solo tocar estas columnas
    },
    "staging.import_raw": df_crudo,           # shorthand: DataFrame pelado = mode append
}
```

- **Shorthand:** un DataFrame directo equivale a `{"mode": "append", "df": ...}`.
  (Nota deliberada: en `output_sheets` el shorthand es `replace` porque crear
  una hoja nueva es barato e inocuo; en SQL el default conservador es `append`
  — nunca destruye ni reescribe datos existentes.)
- Múltiples tablas por call; se escriben **secuencialmente dentro de UNA
  transacción** — si cualquiera falla, rollback total (misma semántica
  atómica que multi-statement del nodo SQL).

### 7.2 Modos

| Modo | Semántica SQL | Requiere `key` | Tabla inexistente |
|---|---|---|---|
| `append` (default) | `INSERT` batcheado (multi-row VALUES; COPY si > 5 000 filas) | No | Auto-create si `on_missing_table: create` (§7.4) |
| `update` | `UPDATE ... SET cols WHERE key = $k` por fila cambiada | **Sí** | Error `TableNotFound` |
| `upsert` | `INSERT ... ON CONFLICT (key) DO UPDATE SET ...` | **Sí** (debe tener UNIQUE/PK en la tabla) | Auto-create (con UNIQUE en `key`) si policy `create` |
| `replace` | `DELETE FROM tabla; INSERT ...` (misma transacción) | No | Auto-create si policy `create`; si existe → gobernado por `on_existing_table` (default `fail`, error estructurado estilo `SheetExists`) |

Decisiones finas:

- **`update` es diff-driven cuando la fuente lo permite:** si la misma tabla
  fue bindeada como fuente SQL en la misma call, el dispatcher diffea el df
  devuelto contra el snapshot cargado (reutilizando `diff_records()` de
  `diff_writer.rs`) y emite UPDATEs solo de las celdas cambiadas — espejo
  exacto de `update_in_place` de sheets, incluida la guarda "0 cambios → 0
  writes". Si la tabla no fue bindeada, todas las filas del df se aplican.
- **`upsert` requiere constraint:** `ON CONFLICT (key)` exige UNIQUE/PK sobre
  `key` en Postgres. Si falta, el error de Postgres se traduce a
  `UpsertKeyNotUnique` con advice ("agregá UNIQUE o usá mode update").
- **`replace` con `DELETE` sin WHERE:** permitido SOLO en este canal
  operador-gobernado (el validador estático del nodo `sql_query` lo sigue
  bloqueando para queries del LLM). El gate es `on_existing_table` (default
  `fail`) — el LLM no puede reemplazar una tabla existente salvo que el
  operador haya optado in.

### 7.3 Pipeline de escritura

```
output_tables (del postlude)
  │
  ▼ normalizar entradas (shorthand → spec dict; records → columnas ordenadas)
  ▼ VALIDAR TODO antes de escribir NADA:
  │   • tabla parseable + schema ∈ allowed_schemas       → SchemaNotAllowed
  │   • mode ∈ {append,update,upsert,replace}            → InvalidMode
  │   • permisos del preset: append/replace→insert(+delete p/replace),
  │     update→update, upsert→insert+update              → OperationNotPermitted
  │   • df no vacío para update/upsert                   → EmptyDataFrame
  │   • key presente en df y en tabla                    → KeyColumnMissing
  │   • sin keys duplicadas en input                     → DuplicateKeyInInput
  │   • columnas del df ⊆ columnas de la tabla
  │     (salvo auto-create)                              → ColumnMismatch {df_only, table_only}
  │   • filas ≤ 100 000                                  → TooManyRows
  ▼ BEGIN
  │   SET LOCAL statement_timeout / work_mem
  │   SET LOCAL app.current_user_id (si tenant configurado)
  │   por tabla: CREATE TABLE IF NOT EXISTS (si aplica) → escritura por modo
  ▼ COMMIT  (cualquier error → ROLLBACK total)
  │
  ▼ respuesta por tabla: {table, mode, rows_affected, created: bool,
                          changes: {rows, cells, columns}}  // changes solo en update diff-driven
```

### 7.4 Auto-creación de tabla (inferencia de tipos)

Cuando `mode` es `append`/`replace`/`upsert`, la tabla no existe y
`on_missing_table: "create"` (default):

| dtype pandas / valores | Tipo Postgres |
|---|---|
| int64 (todos enteros) | `BIGINT` |
| float64 | `DOUBLE PRECISION` |
| bool | `BOOLEAN` |
| datetime64 / strings ISO-8601 consistentes | `TIMESTAMPTZ` |
| todo lo demás (object, mixto) | `TEXT` |

- Columnas 100 % nulas → `TEXT` (con warning en la respuesta).
- Nombres de columna citados (`"Columna Con Espacios"`), validados no vacíos
  y sin duplicados (→ `InvalidColumnName`).
- `upsert` sobre tabla nueva agrega `UNIQUE (<key>)` en el CREATE.
- El `CREATE TABLE` corre con el trust del operador (canal `output_tables`),
  **pero** exige que el preset permita `create_table` (`full`) — con presets
  menores, tabla inexistente → `TableNotFound` con advice "pedile al operador
  que cree la tabla o suba el preset". Esto mantiene la invariante: el preset
  del operador es el techo de lo que el canal puede hacer.
- Respuesta incluye `created: true` + el DDL inferido en `created_ddl` para
  que el modelo lo reporte al usuario (auditabilidad).
- Si `auto_rls` está activo, la tabla nueva recibe policies RLS (mismo hook
  post-CREATE del nodo SQL).

### 7.5 Rendimiento

- `append`/`replace`: multi-row `INSERT ... VALUES` en chunks de 1 000 filas;
  si el total > 5 000 filas, COPY binario vía el camino ya probado de
  `sql_bulk_tools.rs`.
- `update`: statements individuales dentro de la transacción (workload
  esperado: decenas–cientos de filas cambiadas). Diff-driven minimiza N.
- `upsert`: multi-row `INSERT ... ON CONFLICT` en chunks de 1 000.

### 7.6 Multi-tenant / RLS

Si `fixed_config.sql.permissions` trae `tenant_user_id`, **cada** transacción
de binding-fetch y de write-back hace `SET LOCAL app.current_user_id` — las
filas que el sandbox ve y las que escribe quedan tenant-scoped idéntico a
`sql_query`. `auto_rls` aplica a tablas auto-creadas (§7.4).

---

## 8. Sink `output_sheets` — sin cambios de semántica

Reutiliza tal cual el writer de `gsheets_run_python`
(`write_output_sheets` + collision policy + los 4 modos + placeholders
`{{Column}}` + `update_by_position` con snapshots). Requisitos:

- Arg `write_to_spreadsheet` presente (mismo contrato y warnings actuales).
- Capacidad gsheets habilitada (§5).
- Los snapshots para `update_by_position` se retienen para bindings GSHEETS
  exactamente como hoy (bindings SQL/attachment no habilitan ese modo — no
  hay mapeo posicional a una hoja).

El writer se extrae de `gsheets_run_python.rs` a un módulo compartido para
que ambos tools lo llamen (refactor sin cambio de comportamiento).

## 9. Sink `output_attachments` — exportar CSV/XLSX al catálogo

Completa la matriz "cualquier fuente → cualquier destino": el resultado puede
aterrizar como archivo descargable.

```python
output_attachments = {
    "reporte_mensual.xlsx": df_final,                      # formato por extensión
    "errores.csv": {"df": df_err, "delimiter": ";"},       # spec dict opcional
}
```

- Formatos v1: `csv` (UTF-8, delimiter configurable, default `,`) y `xlsx`
  (una hoja, `rust_xlsxwriter` — misma dependencia que documents/CRDT export).
- El archivo se persiste vía `OutputStorageRepository` y se **registra en el
  catálogo de attachments** de la conversación (mismo camino que
  `gsheets_export_xlsx`), devolviendo `document_id` por entrada.
- El binario nunca entra al contexto: la respuesta trae
  `{name, document_id, rows, bytes}`.
- Cap: 100 000 filas / 50 MB por archivo (constantes compartidas).
- Serialización: la hace el **dispatcher en Rust** desde los records que el
  postlude ya exporta (el sandbox no escribe archivos — sigue sin
  filesystem).
- Siempre disponible (no requiere capacidad extra — usa la infra de
  attachments que ya está en todo deployment).

---

## 10. Errores estructurados

Todos los errores del write-back siguen el patrón `SheetExists` (§39 gsheets):
código + estado actual + advice + `valid_next_moves`. Ejemplo:

```json
{
  "error": "ColumnMismatch",
  "table": "analytics.ventas_por_region",
  "df_only": ["margen"],
  "table_columns": ["region", "total", "updated_at"],
  "advice": "The DataFrame has columns that don't exist in the target table. Drop them, rename them, or use mode 'replace' on a new table name.",
  "valid_next_moves": [
    {"action": "drop_column", "example_code": "df = df.drop(columns=['margen'])"},
    {"action": "new_table", "example_code": "output_tables = {'analytics.ventas_v2': df}"}
  ]
}
```

Catálogo de códigos: `SourceNotEnabled`, `BindingMustBeSelect`,
`BindingTooLarge`, `TableNotFound`, `SchemaNotAllowed`, `InvalidMode`,
`OperationNotPermitted`, `KeyColumnMissing`, `DuplicateKeyInInput`,
`UpsertKeyNotUnique`, `ColumnMismatch`, `TooManyRows`, `TableExists` (replace
con policy fail), `InvalidColumnName`, `EmptyDataFrame`.

Los errores de binding incluyen siempre `loaded_columns` (lo ya cargado) para
self-correction sin re-fetch, como hoy.

---

## 11. Cómo el modelo sabe qué usar (disambiguación)

- La **descripción dinámica** del tool (§5) lista las fuentes activas y abre
  con la regla de ruteo: *"Use this tool whenever data must move or be
  computed between files, Google Sheets, and the SQL database — bind each
  source where the data lives NOW; rows never enter your context."*
- Convivencia con los tools existentes: si el operador habilita
  `data_run_python` **y** `gsheets_run_python`/`attachment_run_python`
  simultáneamente, las descripciones de estos últimos ya son source-scoped y
  no colisionan gravemente — pero la **recomendación documentada** para
  operadores es habilitar `data_run_python` *en lugar de* los específicos
  cuando el flujo cruza almacenes. Se agrega esa guía a
  `41_builtin_tools_index.md` y a la matriz de §23 ("elegir la herramienta
  correcta para un attachment").
- Skill best-practices (opt-in, `load_skill`): `data-run-python-recipes` con
  las recetas canónicas — planilla→upsert a DB, DB→reporte xlsx, cruce
  CSV vs tabla, sync gsheet↔tabla. (Espeja `sql-query-best-practices` /
  `gsheets-cross-sheet-analysis`.)

---

## 12. Casos de uso canónicos (validación del diseño)

### A. "Te paso este Excel, actualizá la base" (el pedido original)

```jsonc
// UNA sola call:
{
  "bindings": [
    {"var": "nuevo",     "attachment_id": "doc_ab12"},                       // el xlsx subido
    {"var": "actual",    "query": "SELECT sku, precio, stock FROM crm.productos"} // la tabla destino
  ],
  "code": "
import pandas as pd
df_new = pd.DataFrame(nuevo)
df_cur = pd.DataFrame(actual)
# normalizar y quedarse solo con lo que cambió
df_new['sku'] = df_new['sku'].str.strip().str.upper()
merged = df_new.merge(df_cur, on='sku', how='left', suffixes=('', '_old'))
changed = merged[(merged['precio'] != merged['precio_old']) | (merged['stock'] != merged['stock_old'])]
output_tables = {'crm.productos': {'mode': 'upsert', 'df': changed[['sku','precio','stock']], 'key': 'sku'}}
output = {'filas_upserted': len(changed), 'muestra': changed.head(3).to_dict('records')}
"
}
```

Tokens que ve el LLM: el response (~100). Filas que ve: 3 (la muestra que el
código eligió mostrar).

### B. "Bajame la tabla a un Excel"

```python
# binding: {"var": "ventas", "query": "SELECT * FROM analytics.ventas_2026"}
output_attachments = {"ventas_2026.xlsx": pd.DataFrame(ventas)}
output = {"filas": len(ventas)}
```

### C. "Sincronizá el Google Sheet del equipo con la DB"

```python
# bindings: gsheet "Pipeline" + query sobre crm.oportunidades
# cruce en pandas, upsert a la tabla Y update de una columna de estado en el sheet
output_tables  = {"crm.oportunidades": {"mode": "upsert", "df": df_merged, "key": "opp_id"}}
output_sheets  = {"Pipeline": {"mode": "update_in_place", "df": df_sheet, "key": "opp_id", "columns": ["synced"]}}
```

### D. "Cargá este CSV crudo a staging" — sigue siendo mejor
`sql_bulk_insert_from_attachment` (COPY directo, sin sandbox). La matriz de
§23 se actualiza: transformación/cruce → `data_run_python`; volcado 1:1 del
archivo → bulk insert.

---

## 13. Arquitectura de módulos (hexagonal)

| Módulo | Contenido | Nuevo/Reuso |
|---|---|---|
| `llm_synthetic_tools/data_run_python.rs` | Args, deserializer, dispatcher, gating, orquestación | **Nuevo** |
| `llm_synthetic_tools/table_writer.rs` | Pipeline `output_tables`: validaciones, inferencia DDL, modos, chunking | **Nuevo** |
| `llm_synthetic_tools/sheet_writer.rs` | `write_output_sheets` + snapshots extraídos de `gsheets_run_python.rs` | **Refactor** (movido, sin cambio de comportamiento) |
| `llm_synthetic_tools/attachment_writer.rs` | Serialización CSV/XLSX + registro en catálogo | **Nuevo** (reutiliza camino de `gsheets_export_xlsx`) |
| `llm_synthetic_tools/tabular_bindings.rs` | Parseo/normalización de bindings + parsers CSV/XLSX compartidos | **Nuevo** (extrae de `attachment_run_python`/`sql_bulk_tools`) |
| `diff_writer.rs` | `diff_records()` + validaciones | Reuso tal cual |
| `sql/` (nodo) | `SqlPermissions`, `StaticRuleValidator` (SELECT-only p/ bindings), `SqlPortFactory`/`PgPoolRegistry`, marshaller PgRow→JSON, hook RLS | Reuso (posible extracción de funciones a módulos compartibles) |
| `text/tools/data_run_python.yaml` + preludes | Textos LLM-facing | **Nuevo** |
| `skills/data-run-python-recipes/` | Skill opt-in con recetas | **Nuevo** |

Dependencias de dominio: cero nuevas. `table_writer` depende de los ports SQL
existentes (`SqlConnectionPort`); no se toca ninguna trait pública →
**sin breaking para ADP** (verificar igual con el sweep de
`apps/service/ia/platform/{worker,api}/src/` antes de push, por regla del
repo).

---

## 14. Testing

1. **Unit (sin red, sin DB):** validaciones de bindings (formas, duplicados,
   ambigüedad), gating (fuente no habilitada), inferencia de tipos DDL,
   normalización de `output_tables` (shorthand, key compuesta), chunking.
2. **Unit con mock de `SqlConnectionPort` + wiremock de SheetsClient:**
   pipeline completo de cada modo; rollback en fallo de la segunda tabla;
   errores estructurados exactos.
3. **Integration `#[ignore]`-gated (`TEST_DATABASE_URL`):** append con
   auto-create + tipos; upsert con/sin UNIQUE; update diff-driven (verificar
   que solo cambian las celdas tocadas); replace con policy fail/opt-in;
   RLS tenant-scoped end-to-end; COPY path > 5 000 filas.
4. **E2E graphs** en `tests/graphs/agents/`:
   - `data_run_python_xlsx_to_sql.json` — caso A completo (attachment real +
     Postgres real), prompt realista tipo usuario ("acá está la lista de
     precios actualizada, impactala en la base").
   - `data_run_python_sql_to_xlsx.json` — caso B.
   - `data_run_python_sheet_sync.json` — caso C (gsheets + SQL en una call).
   - Correr con `--agent-session-id`, guardar SSE en `/tmp/colmena_e2e/`,
     verificar contra servicios vivos antes de dar por terminado (regla del
     repo).
5. **`cargo test --verbose`** antes de push (regla CI).

---

## 15. Plan de deprecación y migración ADP

### 15.1 Criterio — borrar solo lo 100 % redundante

Redundancia = dos tools que hacen **el mismo** trabajo. Bajo ese filtro:

| Tool | ¿Redundante con `data_run_python`? | Acción |
|---|---|---|
| `gsheets_run_python` | Sí — source gsheets + sink `output_sheets`, idéntico | **Borrar** |
| `attachment_run_python` | Sí — source attachment (CSV/XLSX) | **Borrar** |
| `crdt_doc_run_python` | **No** — backend CRDT colaborativo (artifact + WS, sin connection_url/attachment_id). Capacidad distinta, no cubierta | **Mantener** (fold-in = v1.1, §15.4) |
| `sql_bulk_insert_from_attachment` | **No** — COPY streaming de archivo crudo sin el cap de 100k del sandbox; vía de escala | **Mantener** |
| `sql_inspect_attachment` | **No** — inspect helper del flujo bulk | **Mantener** |

Borrar `crdt`/`bulk` no eliminaría redundancia — eliminaría una capacidad y
dejaría un agujero funcional. Se mantienen deliberadamente.

### 15.2 Secuencia de rollout (gated)

1. **Ship aditivo:** `data_run_python` entra sin tocar los tools existentes.
2. **Verificación:** los E2E de §14 pasan contra servicios vivos (Postgres +
   Google reales), incluyendo los casos A/B/C. Recién con eso verde:
3. **Sweep ADP:** revisar `apps/service/ia/platform/{worker,api}/src/` por
   referencias a `gsheets_run_python` / `attachment_run_python` (en
   `enabled_tools`, `tool_configurations`, o graph JSON de agentes). Migrar
   cada uso a `data_run_python` con el `fixed_config` equivalente.
4. **Borrado:** el mismo PR (o uno inmediato encadenado) elimina las dos
   tools redundantes: dispatchers, builders, textos YAML, entradas en
   `enabled_tools`/registry, tests, y referencias en docs. Migrar los graph
   JSON in-repo (`tests/graphs/`) que las usen.
5. **Docs:** actualizar `41_builtin_tools_index.md`, `43_sheets_local_vs_gsheets.md`,
   `23_sql_node.md` (matriz de elección) y `39_gsheets.md` para reflejar que
   el análisis pandas de gsheets/attachment ahora es `data_run_python`.

### 15.3 Tabla de equivalencia de migración

| Antes | Ahora (`data_run_python`) |
|---|---|
| `gsheets_run_python` binding `{var, spreadsheet_id, sheet, range}` | Mismo binding, forma GSHEETS |
| `gsheets_run_python` inline `{var, data}` | Mismo binding, forma INLINE |
| `gsheets_run_python` `write_to_spreadsheet` + `output_sheets` | Idéntico (mismo arg + sink) |
| `attachment_run_python` `{attachment_id, code, delimiter, sheet_name, header_row}` | Binding forma ATTACHMENT + `code` |
| (nuevo) leer/escribir SQL | source `{var, query}` + sink `output_tables` |

El shape de bindings y sinks se preserva 1:1 → la migración ADP es
mecánica (cambiar el `name`/`node_type` del tool y mover la config gsheets/sql
a `fixed_config`), sin reescribir el código pandas del agente.

### 15.4 Camino al end-state (v1.1)

Foldear CRDT como source/sink de `data_run_python` (5ª forma de binding:
artifact CRDT; sink a workbook). Recién ahí `crdt_doc_run_python` se vuelve
redundante y se borra. `sql_bulk_*` sobrevive como vía de escala. End-state:
**una super-tool para el 95 % de los casos + bulk para volumen**.

---

## 16. Fuera de scope (v1) → BACKLOG

- `setup_sql` en el bloque `fixed_config.sql` de este tool.
- Backends no-Postgres (SQLite/MySQL) — mismo backlog que sql_bulk.
- `delete` como modo de `output_tables` (borrar filas por key) — esperar
  demanda real; hoy `sql_query` con DELETE WHERE cubre.
- Formatos extra en `output_attachments` (parquet, json-lines).
- Caps configurables por operador (`runtime_limits.max_rows_write`).
- Streaming de bindings SQL gigantes (cursor server-side) — hoy cap 100k.
- **Fold-in de CRDT como source/sink del unificado (v1.1)** — 5ª forma de
  binding (artifact CRDT) + sink a workbook. Es el prerequisito para borrar
  `crdt_doc_run_python` y llegar al end-state de una sola super-tool (§15.4).
  Hoy `crdt_doc_run_python` se mantiene intacto.

---

## 17. Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Canal de escritura SQL que bypassea el validador de queries | El canal no ejecuta SQL del LLM: el SQL lo genera el dispatcher desde el df. Permisos del preset = techo (§7.3); DELETE-sin-WHERE solo en `replace` opt-in del operador |
| Inferencia de tipos crea una columna TEXT donde iba NUMERIC | `created_ddl` en la respuesta (auditable); tabla mal creada se corrige con migración; futuro: `types` override en el spec dict (backlog) |
| Modelo confunde tool unificado vs específicos | Descripción dinámica + guía de operador + skill de recetas (§11) |
| Upsert masivo corrompe datos productivos | Caps de filas, key única validada en input, transacción atómica, RLS tenant-scoped, y el techo del preset del operador |
| Postlude/prelude divergen de los 3 tools hermanos | Los textos nuevos derivan de los existentes; tests de contrato sobre el shape `{user_output, output_*}` |
| Borrar gsheets/attachment rompe un agente ADP no migrado | Deprecación gated (§15.2): borrado solo tras verificación + sweep del worker; migración 1:1 mecánica (§15.3) |

---

## 18. Referencias

- `docs/developer_guide/39_gsheets.md` — write safety, output_sheets, collision policy
- `docs/developer_guide/43_sheets_local_vs_gsheets.md` — output_sheets compartido CRDT/gsheets
- `docs/developer_guide/23_sql_node.md` — permisos, pipeline, attachment_run_python, sql_bulk, matriz de elección
- `src/.../llm_synthetic_tools/gsheets_run_python.rs` — patrón base del dispatcher
- `src/.../llm_synthetic_tools/sql_bulk_tools.rs` — COPY path, extracción de fixed_config, type inference existente
- `src/.../llm_synthetic_tools/diff_writer.rs` — diff + validaciones compartidas
- Spec sheets write safety: `docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`
