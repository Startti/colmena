# Design — Enriched schema/capability context + `crud` preset for `sql_query`

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
(incluyendo DELETE) sobre las tablas definidas, sin poder tocar el schema"*.

| Preset | SELECT | INSERT/UPDATE | DELETE | CREATE TABLE/FUNC |
|---|:--:|:--:|:--:|:--:|
| `read_only` | ✅ | — | — | — |
| `read_write` | ✅ | ✅ | — | — |
| `full` | ✅ | ✅ | ✅ | ✅ |

Para "CRUD sin DDL" hoy hay que usar el workaround poco intuitivo
`{ "preset": "full", "deny": ["create_table", "create_function"] }`.

**El enforcement de presets ya es correcto** (el validador estático bloquea operaciones
fuera del preset — `read_write` ya NO puede crear tablas). Este diseño **no relaja
enforcement**; agrega un preset nombrado y enriquece la *comunicación* hacia el agente.

---

## 2. Scope

Una feature cohesiva con dos componentes ligados por el bloque de capacidades:

1. **Preset `crud`** (enforcement, aditivo) = SELECT/INSERT/UPDATE/DELETE, sin DDL.
2. **Contexto enriquecido** en `build_description_supplement`:
   - Schema completo por tabla (columnas + tipos + `NOT NULL` + PK/UNIQUE/FK).
   - Bloque de capacidades en lenguaje natural, derivado del **set real de operaciones
     permitidas** (describe correctamente cualquier combo: `read_only`, `read_write`,
     `crud`, `full`, y variantes con `deny`).
   - Cap con degradación elegante para schemas grandes.

Fuera de scope: cambiar enforcement de presets existentes; inyección por SSE; parsear el
`setup_sql` (usamos introspección del estado real de la DB).

---

## 3. Diseño

### 3.A — Preset `crud`

En `domain/sql_permissions.rs`:

- `SqlPermissions::from_preset` (o el match equivalente, ~líneas 56-84) acepta `"crud"`:
  inserta `{ Select, Insert, Update, Delete }`. No incluye `CreateFunction`/`CreateTable`.
- El parser de preset (default `read_only`) reconoce `"crud"`.
- `DELETE`/`UPDATE` siguen sujetos al guard "requiere WHERE" del `StaticRuleValidator`
  (sin cambios).

Tabla final de presets:

| Preset | Operaciones |
|---|---|
| `read_only` | SELECT |
| `read_write` | SELECT, INSERT, UPDATE |
| `crud` | SELECT, INSERT, UPDATE, DELETE |
| `full` | SELECT, INSERT, UPDATE, DELETE, CREATE FUNCTION, CREATE TABLE |

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

- **read_only:** "Permisos: solo lectura. Podés ejecutar SELECT sobre las tablas listadas. No podés insertar, actualizar, borrar ni crear nada."
- **read_write:** "Permisos: lectura y escritura sin borrado. Podés SELECT, INSERT y UPDATE sobre las tablas listadas. NO podés borrar filas (DELETE) ni crear/alterar tablas."
- **crud:** "Permisos: CRUD sobre tablas existentes. Podés SELECT, INSERT, UPDATE y DELETE sobre las tablas listadas (DELETE/UPDATE requieren WHERE). NO podés crear ni alterar tablas/funciones."
- **full:** "Permisos: acceso completo a datos + crear tablas/funciones. Podés SELECT/INSERT/UPDATE/DELETE y crear tablas y funciones nuevas en el schema sandbox '<sandbox>'. NO podés crear schemas (los define el operador) ni agregar columnas a tablas existentes. (CREATE SCHEMA / ALTER / TRUNCATE / DROP siempre bloqueados, incluso con full.)"

Reglas de construcción (determinísticas a partir del op-set):
- Verbos por operación presente: SELECT→"leer", INSERT→"insertar", UPDATE→"actualizar", DELETE→"borrar filas", CreateTable/CreateFunction→"crear tablas/funciones en el sandbox".
- Una frase negativa explícita listando lo que NO puede (las operaciones ausentes que el agente podría asumir disponibles: DELETE y CREATE TABLE son las más importantes).
- Nota fija: "DELETE/UPDATE requieren WHERE; TRUNCATE/DROP/ALTER siempre bloqueados."

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
- Un valor de preset nuevo (`crud`) aceptado por el parser; los existentes no cambian.
- Sin cambio de API pública, sin evento SSE nuevo, sin migración.
- Métodos/tipos nuevos en un trait interno con un solo impl in-repo → el worker de ADP
  compila sin cambios.

---

## 5. Testing

- **Unit (`sql_permissions.rs`):**
  - `crud` preset resuelve a {Select, Insert, Update, Delete}; NO permite CreateTable/CreateFunction.
  - `describe_capabilities_nl` por preset (read_only / read_write / crud / full) + un caso `full + deny [delete]` y `full + deny [create_table, create_function]` (== crud).
- **Unit (render):** `build_description_supplement` con un set de `TableSchema` fixture →
  contiene columnas, PK, FK (`→ ref.tabla.col`), UNIQUE, NOT NULL, y el bloque NL.
  Caso cap: > MAX_SCHEMA_TABLES → degrada a solo-nombres + nota de introspección.
- **Integración (`#[ignore]`, real Postgres):** crear un schema con PK/FK/UNIQUE →
  `load_table_schemas` los devuelve correctos; el supplement los renderiza.
- **E2E (grafo real):** agente `crud` sobre finanzas; prompt que borra un gasto ("borrá el
  gasto de comida de ayer") → DELETE con WHERE permitido; e intento de crear tabla →
  bloqueado, y el agente ya sabía por el bloque NL que no podía. Guardar SSE en
  `/tmp/colmena_e2e/` + reporte limpio.

---

## 6. Documentación a actualizar

- `docs/developer_guide/23_sql_node.md` — preset `crud` en la tabla de presets; nueva
  sección "Contexto de schema y capacidades que ve el agente" (qué se inyecta, ejemplo,
  cap).
- `docs/node_configurations.json` — `crud` como valor válido de `permissions.preset`.
- Nota: el bloque de capacidades reemplaza la línea de permisos cruda en el supplement.
