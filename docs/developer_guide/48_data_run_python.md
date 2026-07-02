# 48. `data_run_python` — tool unificado de movimiento de datos tabulares

**Estado:** shipped 2026-07-01/02. **Es el tool tabular primario.**
`gsheets_run_python` y `attachment_run_python` quedan **soft-deprecados desde
2026-07-02** (siguen funcionando por compatibilidad, pero toda la guía, skills
y aliases apuntan a `data_run_python`); ver §"Convivencia con los tools
existentes" más abajo.

## Qué es

`data_run_python` es el **tool tabular primario** — un único tool sintético
LLM que mueve y analiza datos
tabulares entre **CSV/XLSX (attachment)**, **Google Sheets**, **SQL** y
**datos inline**, en una sola tool call. El código pandas corre server-side
en el sandbox restringido — **las filas nunca pasan por el contexto del
LLM**; solo lo que el código elige poner en `output` (típicamente un conteo
y una muestra de 2-3 filas) cruza el wire de vuelta al modelo.

Antes de este tool, cada "almacén" tenía su propio camino (`gsheets_run_python`,
`attachment_run_python`, `sql_query`) y el único destino de write-back
estructurado eran las hojas de cálculo. `data_run_python` permite cruzar
fuentes heterogéneas (un Excel subido contra una tabla Postgres, por
ejemplo) **y** aterrizar el resultado en una tabla SQL con semántica
`append`/`update`/`upsert`/`replace`, todo en una sola call.

Implementación:
`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/{data_run_python.rs,table_writer.rs,tabular_bindings.rs,attachment_writer.rs,sheet_writer.rs}`.
Spec de diseño: [`docs/superpowers/specs/2026-07-01-data-run-python-design.md`](../superpowers/specs/2026-07-01-data-run-python-design.md).

```
                        ┌──────────────────────────────────────────┐
   bindings (fuentes)   │       data_run_python (dispatcher)       │   sinks (destinos)
                        │                                          │
  attachment (CSV/XLSX)─┤► fetch/parse ──┐                         │
  gsheets (sheet/range)─┤► fetch paralelo┤   sandbox Python        ├─► output_tables   → Postgres (append/update/upsert/replace)
  sql (SELECT)         ─┤► query        ─┤   (restricted, 30s)     ├─► output_sheets   → Google Sheets (modos existentes)
  inline data          ─┤► normalize   ──┘   pandas/numpy/scipy    ├─► output_attachments → CSV/XLSX registrado en catálogo
                        │                                          │
                        │        output (JSON) → LLM  (~80 tokens) │
                        └──────────────────────────────────────────┘
```

## Fuentes (bindings)

Cada entrada de `bindings` nombra un global Python (`var`) y **exactamente
un** discriminador de fuente estructural (no hay campo `source` explícito):

| Forma | Campos | Ejemplo |
|---|---|---|
| **ATTACHMENT** | `attachment_id` (+ `sheet_name?`, `delimiter?`, `header_row?`) | `{"var": "nuevo", "attachment_id": "doc_ab12"}` |
| **GSHEETS** | `spreadsheet_id` + `sheet` (+ `range?`) | `{"var": "ref", "spreadsheet_id": "1abc", "sheet": "Q4"}` |
| **SQL** | `query` (SELECT-only) | `{"var": "clientes", "query": "SELECT id, email FROM crm.clientes"}` |
| **INLINE** | `data` (array de records o 2-D con header) | `{"var": "extraida", "data": [{"sku": "A1", "qty": 3}]}` |

Reglas (heredadas de `gsheets_run_python`, mismas validaciones):

- `var` no vacío, sin duplicados; exactamente una forma por binding
  (ambigüedad → error estructurado).
- Fetch **en paralelo** entre bindings (gsheets y SQL vía `join_all`;
  attachments por su propio camino compartido).
- Cada binding queda en el sandbox como lista de records `{col: val}`; el
  código llama `pd.DataFrame(<var>)`. `loaded_columns` viaja en los errores
  para self-correction sin round-trip.

### Binding SQL — restricciones

- **SELECT-only, validado por AST** (`sqlparser`, mismo validador que
  `sql_query`). Cualquier mutación en `query` (incluidas mutaciones
  envueltas en un CTE) → error `BindingMustBeSelect` — el validador es
  fail-closed, no un simple substring-check. Multi-statement rechazado.
- Pasa por el mismo `StaticRuleValidator`/`SqlPermissions` que el nodo
  `sql_query`, respetando `allowed_schemas` (`information_schema`/
  `pg_catalog` permitidos para introspección).
- Cap de 100 000 filas por binding (constante compartida con
  `MAX_BULK_INSERT_ROWS`) → `BindingTooLarge` si se desborda.
- `statement_timeout_ms`/`work_mem_mb` de `runtime_limits` aplican vía `SET
  LOCAL`, igual que `sql_query`.

### Binding ATTACHMENT — restricciones

Mismas caps que `attachment_run_python`: archivo ≤ 50 MB, ≤ 100 000 filas.
CSV con auto-delimiter; XLSX con `sheet_name` opcional (default primera
hoja) y `header_row` 1-indexed.

### Activación

`data_run_python` se activa de dos formas:

1. **`tool_configurations`** — una entrada explícita con
   `node_type: "data_run_python"` (necesaria para habilitar la capacidad SQL
   vía `fixed_config.sql`, y para ajustar policies de operador). Ver
   §"Config del operador" más abajo.
2. **`enabled_tools`** — desde 2026-07-02 el alias del toolkit **`gsheets`**
   incluye `data_run_python`, así que `enabled_tools: ["gsheets"]` ya lo
   expone (junto al resto de la superficie gsheets). También lo activan el
   wildcard `"*"` y el nombre exacto `"data_run_python"`.

Cuando entra **por alias** (sin `tool_configurations`), la capacidad gsheets
se auto-detecta (el agente ya tiene el toolkit gsheets habilitado) pero la
capacidad **SQL requiere sí o sí un `fixed_config.sql`** — que solo se puede
declarar vía `tool_configurations`. Es decir: `["gsheets"]` te da
`data_run_python` con lectura/escritura de attachments, inline y Google
Sheets, pero no SQL.

### Gating de fuentes

| Fuente | Gate |
|---|---|
| `attachment` | Siempre disponible |
| `inline` | Siempre disponible |
| `gsheets` (source + sink `output_sheets`) | El agente ya tiene el toolkit gsheets habilitado, **o** `fixed_config.enable_gsheets: true` |
| `sql` (source + sink `output_tables`) | `fixed_config.sql` presente (con `connection_url` + `permissions.allowed_schemas`) |

La `description` del tool se ensambla dinámicamente en el build de la
`ToolDefinition` (init del `llm_call`) listando **solo** las fuentes activas
— el modelo ni se entera de que existe una fuente SQL si el operador no la
configuró. En el dispatcher, además, un binding/sink de una capacidad no
habilitada devuelve `{"error": "SourceNotEnabled", "source": "sql"|"gsheets"}`
como defensa en profundidad.

## Sinks — globals del código pandas (PLURALES)

Los tres sinks son **dicts** que el código pandas asigna como globals al
final del script. Son plurales — un error común es escribir `output_table`
(singular), que se ignora silenciosamente y no escribe nada.

### `output_tables` → write-back a Postgres

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

Modos:

| Modo | Semántica SQL | Requiere `key` | Tabla inexistente |
|---|---|---|---|
| `append` (default, shorthand) | `INSERT` batcheado (multi-row VALUES; COPY si > 5 000 filas) | No | Auto-create si `on_missing_table: create` |
| `update` | `UPDATE ... SET cols WHERE key = $k`; **diff-driven** si la misma tabla también fue bindeada como fuente en la misma call (solo se tocan celdas cambiadas, guarda 0-cambios → 0-writes) | Sí | Error `TableNotFound` |
| `upsert` | `INSERT ... ON CONFLICT (key) DO UPDATE` — requiere UNIQUE/PK sobre `key` en la tabla, si no → `UpsertKeyNotUnique` | Sí | Auto-create (con `UNIQUE` en `key`) si policy `create` |
| `replace` | `DELETE FROM tabla; INSERT ...` en la misma transacción | No | Auto-create si policy `create`; si existe → gobernado por `on_existing_table` (default `fail`) |

Nota deliberada de diseño: el shorthand de `output_sheets` (un DataFrame
pelado) es `replace`, porque crear una hoja nueva es barato; en SQL el
default conservador es **`append`** — nunca destruye ni reescribe datos
existentes salvo que el código pida `replace` explícitamente y el operador
haya optado in con `on_existing_table`.

Múltiples tablas por call se escriben **secuencialmente dentro de UNA
transacción** — si cualquiera falla, rollback total.

**Auto-creación de tabla** (append/replace/upsert, tabla inexistente,
`on_missing_table: "create"`, default): tipos inferidos por dtype pandas
(`int64`→`BIGINT`, `float64`→`DOUBLE PRECISION`, `bool`→`BOOLEAN`,
`datetime64`/ISO-8601 consistente→`TIMESTAMPTZ`, todo lo demás→`TEXT`).
`upsert` agrega `UNIQUE (<key>)` en el CREATE. El `CREATE TABLE` exige que
el preset del operador permita `create_table` (preset `full`) — con presets
menores, tabla inexistente → `TableNotFound` con advice de pedirle al
operador que suba el preset o cree la tabla. La respuesta incluye
`created: true` + `created_ddl` para auditabilidad.

**Políticas operator-gobernadas** (van en `fixed_config.sql`, el LLM nunca
las ve ni las puede pasar desde el código):

- `on_missing_table`: `"create"` (default) | `"fail"`.
- `on_existing_table`: `"fail"` (default) | `"append"` | `"overwrite"` —
  aplica cuando `mode: "replace"` apunta a una tabla que ya existe. `"fail"`
  aborta con `TableExists` antes de tocar nada; `"append"` deja pasar la
  operación sin bloquear; `"overwrite"` asume que el operador acepta el
  reemplazo destructivo. `replace` con `DELETE` sin
  `WHERE` está permitido SOLO en este canal operador-gobernado (el
  validador estático de `sql_query` lo sigue bloqueando para queries que el
  LLM escribe a mano).

Catálogo de errores estructurados (patrón `{error, ...state, advice,
valid_next_moves}`, igual que `SheetExists`): `SourceNotEnabled`,
`BindingMustBeSelect`, `BindingTooLarge`, `TableNotFound`,
`SchemaNotAllowed`, `InvalidMode`, `OperationNotPermitted`,
`KeyColumnMissing`, `DuplicateKeyInInput`, `UpsertKeyNotUnique`,
`ColumnMismatch`, `TooManyRows`, `TableExists`, `InvalidColumnName`,
`EmptyDataFrame`.

### `output_sheets` → Google Sheets

Reutiliza tal cual el writer de `gsheets_run_python` (extraído a
`sheet_writer.rs`, sin cambio de comportamiento): mismos 4 modos
(`replace`/`update_in_place`/`update_by_position`/`overwrite`), collision
policy (`fail`/`auto_suffix`/`overwrite`), placeholders `{{Column}}`. Ver
[§39 Google Sheets](39_gsheets.md) para el detalle de los modos. Requiere el
arg `write_to_spreadsheet` en la tool call y la capacidad `gsheets`
habilitada.

### `output_attachments` → CSV/XLSX al catálogo

```python
output_attachments = {
    "reporte_mensual.xlsx": df_final,                      # formato por extensión
    "errores.csv": {"df": df_err, "delimiter": ";"},       # spec dict opcional
}
```

Formatos v1: `csv` (UTF-8, delimiter configurable) y `xlsx` (una hoja). El
archivo se persiste vía `OutputStorageRepository` y se registra en el
catálogo de attachments de la conversación (mismo camino que
`gsheets_export_xlsx`); la respuesta trae `{name, document_id, rows, bytes}`
— el binario nunca entra al contexto. Cap 100 000 filas / 50 MB. La
serialización la hace el **dispatcher en Rust**, no el código pandas.
Siempre disponible (no requiere capacidad extra).

### Anti-patterns — cosas que rompen el tool

- ❌ `output_table = {...}` (singular). El global correcto es
  `output_tables` (plural). Una asignación singular se ignora
  silenciosamente.
- ❌ Manipular bytes a mano con `io`, `base64`, o similar — esos módulos
  están bloqueados en el sandbox (sin filesystem). Para exportar un
  archivo, asignar un DataFrame a `output_attachments`; el dispatcher
  serializa.
- ❌ Pasar `on_existing_sheet` / `on_existing_table` como si fueran
  argumentos que el código puede fijar. Son políticas de **operador** en
  `fixed_config` — el LLM no las ve ni las puede sobreescribir desde
  Python.

## Config del operador (`fixed_config`)

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
          "allowed_schemas": ["analytics", "crm"]
          // tenant_user_id / tenant_column / auto_rls también soportados
        },
        "runtime_limits": { "statement_timeout_ms": 30000, "work_mem_mb": 64 },
        "on_missing_table": "create",                 // create (default) | fail
        "on_existing_table": "fail"                   // fail (default) | append | overwrite — solo aplica a mode replace
      },
      // ── Capacidad gsheets (opcional; auto-on si el toolkit gsheets está habilitado) ──
      "enable_gsheets": true,
      "on_existing_sheet": "fail"                     // policy existente de sheets
    }
  }
}
```

Notas:

- `sql.permissions` usa **la misma estructura** que el nodo `sql_query`
  (`SqlPermissions`) — mismo tipo de dominio, cero divergencia.
- `sql.permissions.allowed_schemas` es obligatorio y no vacío; si falta, el
  init del tool falla con mensaje claro al operador (mismo patrón hard-fail
  que `sql_query`/`setup_sql`).
- El pool se obtiene vía `SqlPortFactory`/`PgPoolRegistry` compartido. Si el
  agente también tiene un `sql_query` al mismo `connection_url`, comparten
  pool.
- **`setup_sql` NO se soporta en este tool.** El provisioning de tablas
  destino es responsabilidad del nodo `sql_query` del grafo o de
  migraciones; la auto-creación de `output_tables` cubre solo el caso
  "tabla de resultados nueva".

## Seguridad

- Los bindings SQL son **SELECT-only, validados por AST** (`sqlparser`), no
  por heurísticas de substring — incluye el caso fail-closed de una
  mutación envuelta en un CTE (`WITH x AS (UPDATE ...) SELECT ...`), que se
  rechaza igual que una mutación directa.
- El write-back a `output_tables` **nunca ejecuta SQL que el LLM escribió**:
  el SQL de escritura lo genera el dispatcher Rust (`table_writer.rs`) a
  partir del DataFrame y del `mode` declarado — el canal no tiene un
  camino por el que el modelo inyecte SQL arbitrario.
- El **preset del operador es el techo**: los permisos del preset
  (`read_only`/`read_write`/`read_write_delete`/`full`) determinan qué
  operaciones puede hacer el canal (`append`/`replace`→insert(+delete para
  replace), `update`→update, `upsert`→insert+update), independientemente de
  lo que pida el código pandas. `create_table` en auto-creación exige
  preset `full`.
- `replace` con `DELETE` sin `WHERE` es posible únicamente en este canal
  operador-gobernado — el validador estático de `sql_query` lo sigue
  bloqueando para SQL que el LLM escribe a mano en otros tools.
- Multi-tenant: si `permissions.tenant_user_id` está configurado, cada
  transacción de fetch y de write-back hace `SET LOCAL app.current_user_id`
  — mismas garantías RLS que `sql_query`.

## Los grafos E2E

En `tests/graphs/agents/`:

La matriz cubre CSV, Excel, Google Sheets y SQL como **fuente** y como
**sink**. Los 7 marcados ✅ están verificados en vivo (Postgres real / Sheets
API real):

| Grafo | Caso | Requiere | Live |
|---|---|---|---|
| `data_run_python_csv_to_sql.json` | CSV adjunto → tabla SQL nueva (`output_tables` append + auto-create) | `tests/fixtures/products_100.csv` | ✅ |
| `data_run_python_excel_to_sql.json` | Excel adjunto + tabla SQL → cruce en pandas → **upsert** por key | `drp_e2e.productos` + `tests/fixtures/precios_actualizados.xlsx` | ✅ |
| `data_run_python_sql_to_csv.json` | SQL fuente → CSV descargable (`output_attachments`) | `drp_e2e.productos` seed | ✅ |
| `data_run_python_sql_to_sql.json` | SQL fuente → agregación pandas → SQL sink | `drp_e2e.ventas` seed | ✅ |
| `data_run_python_sql_to_xlsx.json` | SQL fuente → export a Excel (`output_attachments`) | `drp_e2e.ventas` seed | ✅ |
| `data_run_python_gsheet_to_sql.json` | Google Sheet real → agregación → tabla SQL | Google Sheet (`<SPREADSHEET_ID>`, tab `Ventas`) + creds OAuth | ✅ |
| `data_run_python_sql_to_gsheet.json` | SQL fuente → pestaña nueva en Google Sheet (`output_sheets` + `write_to_spreadsheet`) | Google Sheet escribible + creds OAuth | ✅ |
| `data_run_python_xlsx_to_sql.json` | Variante del caso A con fixture propio | `drp_e2e.productos` + fixture xlsx | autorado |
| `data_run_python_sheet_sync.json` | gsheet + tabla SQL en una call: upsert a SQL Y `output_sheets` | Google Sheet + `drp_e2e.oportunidades` | autorado |

Seed mínimo (`drp_e2e`): `CREATE SCHEMA IF NOT EXISTS drp_e2e;` + las tablas
que cada grafo usa (ver el `_comment` de cada JSON). Los grafos con
`<SPREADSHEET_ID>` usan un placeholder — sustituilo por un id real al correr
(el repo no guarda ids reales de planillas).

Comando de corrida (patrón común, agent-session-id estable por regla del
repo):

```bash
set -a; source .env; set +a
cargo run --release --bin dag_engine -- run tests/graphs/agents/data_run_python_xlsx_to_sql.json \
  --agent-session-id e2e_drp_$(date +%s) --include-extra-info
```

Los grafos gsheets requieren además las credenciales OAuth de
`COLMENA_GOOGLE_OAUTH_*` (ver [§47 Google OAuth](47_google_oauth.md)) y
`COLMENA_LOCAL` sin setear.

## Skill de recetas

Skill opt-in (`load_skill`) `data-run-python-recipes` con las recetas
canónicas: planilla→upsert a DB (`spreadsheet_to_db`), DB→reporte xlsx
(`db_to_file`), cruce de dos fuentes vivas (`cross_source_join`), y el deep
dive de modos/anti-patterns (`sinks_and_modes`). Fuente:
`src/libs/colmena/skills/data-run-python-recipes/`. Ver también
[§24 Skills](24_skills.md) para cómo se auto-cargan.

## Convivencia con los tools existentes — `gsheets_run_python` y `attachment_run_python` están deprecados

Desde **2026-07-02**, `gsheets_run_python` y `attachment_run_python` están
**soft-deprecados a favor de `data_run_python`**. Siguen **funcionando** y se
mantienen registrados por compatibilidad con grafos persistidos que los
nombran (por eso `gsheets_run_python` sigue en el alias `gsheets` durante el
bridge), pero:

- **No los recomiendes.** `data_run_python` cubre su funcionalidad completa —
  comparten `sheet_writer.rs`/`table_writer.rs` — y además agrega cruce entre
  fuentes heterogéneas, write-back a SQL (`output_tables`) y export a archivos
  descargables (`output_attachments`).
- Sus descripciones ahora llevan un prefijo `DEPRECATED — usá data_run_python`.
- Las 11 skills de gsheets instruyen al modelo a llamar `data_run_python`.
- El borrado real de estos dos tools está **diferido a una Fase 2 gated**
  (telemetría + verificación de grafos persistidos en ADP). Ver el plan
  [`docs/superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md`](../superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md).

`sql_inspect_attachment` y `sql_bulk_insert_from_attachment` **no** están
deprecados (el volcado 1:1 crudo de un CSV entero vía COPY sigue siendo su
dominio). Ver la matriz de elección en
[§23 Nodo SQL — "Elegir la herramienta correcta para un attachment"](23_sql_node.md#elegir-la-herramienta-correcta-para-un-attachment).

Fuera de scope de v1 (backlog): `setup_sql` dentro de este tool, backends
no-Postgres, modo `delete` en `output_tables`, formatos extra en
`output_attachments` (parquet/json-lines), y el fold-in de CRDT como 5ª
forma de binding (prerequisito para eventualmente deprecar
`crdt_doc_run_python`).

## Referencias

- Spec de diseño: [`docs/superpowers/specs/2026-07-01-data-run-python-design.md`](../superpowers/specs/2026-07-01-data-run-python-design.md)
- [§23 Nodo SQL](23_sql_node.md) — permisos, presets, `sql_bulk_insert_from_attachment`
- [§39 Google Sheets](39_gsheets.md) — modos de `output_sheets`, collision policy
- [§43 Sheets local (CRDT) vs Google Sheets](43_sheets_local_vs_gsheets.md)
- [§24 Skills](24_skills.md) — cómo se auto-cargan las skills opt-in
