# Design — Enriched schema/capability context + `read_write_delete` preset for `sql_query`

- **Fecha:** 2026-06-21
- **Estado:** Aprobado (brainstorm) — pendiente plan de implementación
- **Componente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`, `domain/sql_permissions.rs`, `domain/sql_ports.rs`, `infrastructure/sql_pool_adapter.rs`
- **Consumo principal:** ADP (agentes SQL en el canvas), con Colmena como motor

---

## 1. Problema

Un agente que usa `sql_query` como tool hoy recibe **contexto pobre** sobre su entorno:

- El `description_supplement` (construido en `SqlNode::do_initialize_inner` →
  `build_description_supplement`) lista **solo nombres de tablas** (sin columnas ni
  claves) y los permisos como operaciones crudas (`"SELECT, INSERT, UPDATE"`), con un
  *"usá introspección para ver columnas"*. El agente tiene que gastar turnos
  introspeccionando, y se entera de sus límites solo cuando intenta algo y el validador
  lo bloquea.

Además, el **modelo de presets tiene un hueco**: no existe un nivel para *"manipular datos
(incluyendo DELETE) y extender tablas existentes (agregar columnas), sin poder crear tablas
nuevas"*.

| Preset | SELECT | INSERT/UPDATE | DELETE | ADD COLUMN | CREATE TABLE/FUNC |
|---|:--:|:--:|:--:|:--:|:--:|
| `read_only` | ✅ | — | — | — | — |
| `read_write` | ✅ | ✅ | — | — | — |
| `full` | ✅ | ✅ | ✅ | — | ✅ |

Hoy `read_write` no puede borrar; `full` puede borrar y crear tablas, pero **ningún preset
puede agregar columnas** (`ALTER` está siempre bloqueado). No hay un nivel intermedio para
"CRUD completo + extender tablas, sin crear tablas nuevas".

**El enforcement de presets existentes ya es correcto** (`read_write` ya NO puede crear
tablas). Este diseño es **aditivo**: agrega un preset nuevo (`read_write_delete`), habilita
`ALTER TABLE ADD COLUMN` (solo esa variante) en `read_write_delete` y `full`, y enriquece la
*comunicación* hacia el agente. No cambia el significado de `read_only`/`read_write`/`full`
(salvo que `full` gana ADD COLUMN).

---

## 2. Scope

Una feature cohesiva con dos componentes ligados por el bloque de capacidades:

1. **Preset `read_write_delete`** (enforcement, aditivo) = SELECT/INSERT/UPDATE/DELETE +
   `ALTER TABLE ADD COLUMN`. No crea tablas nuevas ni funciones.
2. **`ADD COLUMN` habilitado** (solo esa variante de `ALTER`) en `read_write_delete` y
   `full`. Las demás variantes de `ALTER` (DROP COLUMN, ALTER TYPE, RENAME, DROP
   CONSTRAINT) siguen **siempre bloqueadas**.
3. **Contexto enriquecido** en `build_description_supplement`:
   - Schema completo por tabla (columnas + tipos + `NOT NULL` + PK/UNIQUE/FK).
   - Bloque de capacidades en lenguaje natural, derivado del **set real de operaciones
     permitidas** (describe correctamente cualquier combo: `read_only`, `read_write`,
     `read_write_delete`, `full`, y variantes con `deny`).
   - Cap con degradación elegante para schemas grandes.

Fuera de scope: cambiar el significado de `read_only`/`read_write`; permitir variantes
destructivas de `ALTER`; inyección por SSE; parsear el `setup_sql` (usamos introspección
del estado real de la DB).

---

## 3. Diseño

### 3.A — Preset `read_write_delete` + operación `AddColumn`

En `domain/sql_permissions.rs`:

- Nueva variante de operación `SqlOperation::AddColumn` (representa `ALTER TABLE … ADD
  COLUMN`, y **solo** esa variante de ALTER).
- `SqlPermissions::from_preset` (~líneas 56-84) acepta `"read_write_delete"`:
  `{ Select, Insert, Update, Delete, AddColumn }`. No incluye `CreateFunction`/`CreateTable`.
- `full` pasa a incluir `AddColumn` también: `{ Select, Insert, Update, Delete, AddColumn,
  CreateFunction, CreateTable }`.
- El parser de preset (default `read_only`) reconoce `"read_write_delete"`.
- `from_str_loose` mapea `"add_column"` → `AddColumn` (para `deny`).
- `DELETE`/`UPDATE` siguen sujetos al guard "requiere WHERE" del `StaticRuleValidator`.

**Validador (`StaticRuleValidator` / `sql_ast`):** hoy un `Statement::AlterTable` se
bloquea siempre. Cambio: detectar `ALTER TABLE` y clasificarlo como operación `AddColumn`
**solo si TODAS sus operaciones del AST son `AlterTableOperation::AddColumn`**. Si contiene
cualquier otra variante (`DropColumn`, `AlterColumn`/change type, `RenameColumn`,
`RenameTable`, `DropConstraint`, etc.) → se bloquea como hasta ahora ("ALTER no permitido").
Una vez clasificado como `AddColumn`, se aplica el chequeo de preset normal (permitido en
`read_write_delete`/`full`) + el chequeo de `allowed_schemas` sobre la tabla alterada.

Tabla final de presets:

| Preset | Operaciones |
|---|---|
| `read_only` | SELECT |
| `read_write` | SELECT, INSERT, UPDATE |
| `read_write_delete` | SELECT, INSERT, UPDATE, DELETE, **ADD COLUMN** |
| `full` | SELECT, INSERT, UPDATE, DELETE, **ADD COLUMN**, CREATE FUNCTION, CREATE TABLE |

**Siempre bloqueado para el LLM (todos los presets):** `CREATE SCHEMA`, `DROP`, `TRUNCATE`,
y todo `ALTER` que no sea exclusivamente `ADD COLUMN` (DROP/RENAME/ALTER COLUMN TYPE).

### 3.B — Introspección de columnas + claves

Nuevo método en el port `SqlConnectionPort` (`domain/sql_ports.rs`), implementado en
`PgPoolAdapter` (`infrastructure/sql_pool_adapter.rs`):

```rust
async fn load_table_schemas(&self, schemas: &[String]) -> Result<Vec<TableSchema>, SqlNodeError>;
```

Nuevos tipos de dominio (en `sql_ports.rs`):

```rust
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,   // e.g. "integer", "numeric", "text"
    pub not_null: bool,
    pub is_pk: bool,
    pub is_unique: bool,
}
pub struct ForeignKey {
    pub column: String,          // local column
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_column: String,
}
pub struct TableSchema {
    pub schema: String,
    pub table: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKey>,
}
```

Fuentes:
- Columnas + tipo + nullable: `information_schema.columns` (filtrado por `table_schema = ANY($1)`).
- PK / UNIQUE: `pg_catalog.pg_constraint` (`contype IN ('p','u')`) + `pg_attribute`.
- FK con destino: `pg_catalog.pg_constraint` (`contype = 'f'`) resolviendo `confrelid`/`confkey`.

`TableSchema` **reemplaza** a `TableInfo` como salida de la introspección que alimenta el
supplement (el viejo `load_table_metadata` queda; el nuevo método agrega el detalle).
Aditivo al trait, un solo impl in-repo → **ADP no se rompe**.

### 3.C — Bloque de capacidades (NL, derivado del op-set)

Nueva función en `sql_permissions.rs`, p.ej. `describe_capabilities_nl(&self) -> String`,
que produce el bloque a partir del **set de operaciones permitidas** (no del nombre del
preset), de modo que `full + deny [...]` también queda correcto. Ejemplos del texto que
ve el agente:

- **read_only:** "Permisos: solo lectura. Podés ejecutar SELECT sobre las tablas listadas. No podés insertar, actualizar, borrar ni modificar la estructura."
- **read_write:** "Permisos: lectura y escritura sin borrado. Podés SELECT, INSERT y UPDATE sobre las tablas listadas. NO podés borrar filas (DELETE) ni modificar la estructura (crear tablas, agregar columnas)."
- **read_write_delete:** "Permisos: CRUD completo + agregar columnas. Podés SELECT, INSERT, UPDATE y DELETE sobre las tablas listadas (DELETE/UPDATE requieren WHERE), y agregar columnas nuevas a tablas existentes (ALTER TABLE ADD COLUMN). NO podés crear tablas nuevas, borrar/renombrar columnas, cambiar tipos, ni crear schemas."
- **full:** "Permisos: acceso completo. Podés SELECT/INSERT/UPDATE/DELETE, agregar columnas a tablas existentes, y crear tablas y funciones nuevas en el schema sandbox '<sandbox>'. NO podés crear schemas (los define el operador), ni borrar/renombrar columnas ni cambiar sus tipos. (CREATE SCHEMA / DROP / TRUNCATE / ALTER-no-ADD-COLUMN siempre bloqueados, incluso con full.)"

Reglas de construcción (determinísticas a partir del op-set):
- Verbos por operación presente: SELECT→"leer", INSERT→"insertar", UPDATE→"actualizar", DELETE→"borrar filas", AddColumn→"agregar columnas a tablas existentes", CreateTable/CreateFunction→"crear tablas/funciones en el sandbox".
- Una frase negativa explícita listando lo que NO puede (las ausencias que el agente podría asumir disponibles: DELETE, ADD COLUMN, CREATE TABLE).
- Nota fija: "DELETE/UPDATE requieren WHERE; CREATE SCHEMA / DROP / TRUNCATE / ALTER destructivo (DROP/RENAME/ALTER COLUMN) siempre bloqueados."

### 3.D — Render del schema + cap graceful

En `build_description_supplement`, reemplazar el bloque actual de "Available tables"
(solo nombres) por:

```
<capabilities NL block>

Esquema disponible (schema: finanzas):
  • categorias  [PK: id]
      - id      integer   NOT NULL
      - nombre  text      NOT NULL, UNIQUE
  • gastos  [PK: id]
      - id            integer   NOT NULL
      - categoria_id  integer   → finanzas.categorias.id   (FK)
      - monto         numeric
      - descripcion   text

Max rows: 100
```

**Cap (constantes en `sql.rs`):**
- `MAX_SCHEMA_TABLES = 40` y `MAX_SCHEMA_CHARS ≈ 8000`.
- Construir el detalle completo; si el número de tablas supera `MAX_SCHEMA_TABLES` **o**
  el render supera `MAX_SCHEMA_CHARS`, degradar a **solo nombres de tablas** + la línea
  *"(Schema grande: usá introspección sobre information_schema para ver columnas.)"*.
- El bloque de capacidades NL siempre se incluye (es chico).

### Dónde corre
Sin cambios de flujo: `do_initialize_inner` ya carga metadata después de `setup_sql`/
provisión de schemas y construye el supplement. Se cambia (1) la introspección que
alimenta el supplement: pasa a usar `load_table_schemas` (el `load_table_metadata`
existente queda en el trait para compatibilidad pero el supplement deja de usarlo), y
(2) el builder `build_description_supplement`. Corre en init, cacheado por `OnceCell`.

---

## 4. Impacto ADP

Puramente aditivo:
- Texto enriquecido dentro de la **descripción de la tool** (que ADP ya pasa al LLM tal cual).
- Un valor de preset nuevo (`read_write_delete`) aceptado por el parser; `read_only`/
  `read_write` no cambian. **Único cambio de comportamiento en un preset existente:** `full`
  ahora permite `ALTER TABLE ADD COLUMN` (antes lo bloqueaba) — es una ampliación, no rompe
  grafos (nadie dependía de que `full` fallara al agregar una columna).
- Sin cambio de API pública, sin evento SSE nuevo, sin migración.
- Métodos/tipos nuevos en un trait interno con un solo impl in-repo → el worker de ADP
  compila sin cambios.

---

## 5. Testing

- **Unit (`sql_permissions.rs`):**
  - `read_write_delete` resuelve a {Select, Insert, Update, Delete, AddColumn}; NO permite CreateTable/CreateFunction.
  - `full` ahora incluye AddColumn.
  - `describe_capabilities_nl` por preset (read_only / read_write / read_write_delete / full) + un caso `full + deny [delete]` y `full + deny [create_table, create_function]`.
- **Unit (validador / `sql_ast`):**
  - `ALTER TABLE t ADD COLUMN c int` → clasifica como `AddColumn` (permitido en read_write_delete/full, bloqueado en read_only/read_write).
  - `ALTER TABLE t DROP COLUMN c`, `ALTER TABLE t ALTER COLUMN c TYPE text`, `RENAME` → **bloqueados en todos los presets** (incluido full).
  - `ALTER TABLE t ADD COLUMN a int, DROP COLUMN b` (mezcla) → bloqueado (no todas las ops son AddColumn).
- **Unit (render):** `build_description_supplement` con un set de `TableSchema` fixture →
  contiene columnas, PK, FK (`→ ref.tabla.col`), UNIQUE, NOT NULL, y el bloque NL.
  Caso cap: > MAX_SCHEMA_TABLES → degrada a solo-nombres + nota de introspección.
- **Integración (`#[ignore]`, real Postgres):** crear un schema con PK/FK/UNIQUE →
  `load_table_schemas` los devuelve correctos; el supplement los renderiza.
- **E2E (grafo real):** agente `read_write_delete` sobre finanzas; prompts: (a) borrar un
  gasto ("borrá el gasto de comida de ayer") → DELETE con WHERE permitido; (b) agregar una
  columna ("agregá un campo 'metodo_pago' a gastos") → ADD COLUMN permitido; (c) intento de
  crear tabla nueva → bloqueado, y el agente ya sabía por el bloque NL que no podía. Guardar
  SSE en `/tmp/colmena_e2e/` + reporte limpio.

---

## 6. Documentación a actualizar

- `docs/developer_guide/23_sql_node.md` — preset `read_write_delete` en la tabla de
  presets; documentar que `read_write_delete`/`full` permiten `ALTER TABLE ADD COLUMN`
  (solo esa variante) y que el resto de `ALTER` sigue bloqueado; actualizar la lista de
  "siempre bloqueados"; nueva sección "Contexto de schema y capacidades que ve el agente"
  (qué se inyecta, ejemplo, cap).
- `docs/node_configurations.json` — `read_write_delete` como valor válido de
  `permissions.preset`; `add_column` como valor válido de `permissions.deny`.
- Nota: el bloque de capacidades reemplaza la línea de permisos cruda en el supplement.
