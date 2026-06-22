# Design — `setup_sql`: bloque de bootstrapping de entorno para el nodo `sql_query`

- **Fecha:** 2026-06-21
- **Estado:** Aprobado (brainstorm) — pendiente plan de implementación
- **Autor:** daniel-garcia
- **Componente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- **Consumo principal:** ADP (canvas / meta-agente constructor), con Colmena como motor

---

## 1. Problema

Un autor quiere crear un grafo "plantilla" (ej. un agente de **finanzas** que registra
tickets de gastos), **definir la estructura de datos que el agente necesita** (schema +
tablas + datos iniciales), y publicarlo de modo que cualquier consumidor pueda usarlo
**directamente** — el entorno se auto-arma en el primer uso y el agente queda listo para
responder preguntas y crear registros, sin que el consumidor configure nada.

Hoy Colmena no tiene forma de que el **autor** adjunte una definición de entorno
(DDL + seed) al grafo. El nodo `sql_query` solo provisiona los schemas listados en
`permissions.allowed_schemas` (`CREATE SCHEMA IF NOT EXISTS`), pero **no** crea tablas
ni inserta datos seed definidos por el autor.

### Lo que ya existe (y se reusa)

- `sql_query` → `do_initialize_inner` ya corre DDL de **nivel-operador** en init:
  `create_schemas_if_missing` ejecuta `CREATE SCHEMA IF NOT EXISTS "<schema>"` para cada
  schema en `allowed_schemas` que no exista. Este es exactamente el trust-level y el
  punto de extensión que necesitamos.
- Soporte **multi-statement** atómico (Política C, 2026-06-09): un script con varios
  statements separados por `;` se ejecuta statement-a-statement en una transacción.
- **Secure values** (`${ENV_VAR}` y handles `<sv_*>`) resueltos sobre `connection_url`.
- Introspección de metadata en init que construye el `description_supplement` (lista de
  tablas) que ve el LLM.

---

## 2. Separación conceptual del feature

| Pieza | Decisión |
|---|---|
| **QUÉ** se ejecuta | **SQL literal** (DDL + seed), escrito por un humano o por un meta-agente constructor. Determinista y auditable. |
| **DÓNDE** se aplica | Configurable por grafo. El mecanismo es **agnóstico al aislamiento**: el aislamiento (otra DB / otro schema / multi-tenant RLS / compartido) sale de cómo el autor configura `connection_url`, `allowed_schemas` y `auto_rls`. |
| **CUÁNDO** corre | **Idempotente, cada vez** que el nodo inicializa. Sin estado de tracking; la idempotencia la garantiza el propio SQL (`IF NOT EXISTS` / `ON CONFLICT`). |

### Por qué idempotente (y no un guard "run-once")

En ADP cada mensaje del usuario es un **DAG run nuevo** (mismo `agent_session_id`,
`session_id` efímero). Un esquema "correr una sola vez" requeriría persistir un flag con
una clave que **espeje el modelo de aislamiento** (`hash(connection_url + schema +
versión)`) — código + tabla de tracking + fragilidad, y **no auto-reparable** (si se
dropea una tabla, el flag miente y el agente se rompe).

El enfoque idempotente esquiva todo eso: **Postgres es el guard**. `CREATE TABLE IF NOT
EXISTS` contra una tabla existente es un chequeo de catálogo (microsegundos). Funciona
idéntico en los 4 modelos de aislamiento, sin estado, y **auto-repara** estructura
borrada. El costo por turno (unos pocos statements baratos en una transacción, ~10–50 ms
contra un pool caliente) es ruido al lado de una llamada al LLM.

El único contrato que impone al autor: **escribir el seed idempotente**
(`INSERT ... ON CONFLICT DO NOTHING`, que requiere un `UNIQUE`). El meta-agente
constructor de ADP lo garantiza al generar el SQL.

---

## 3. Diseño

### 3.1 Forma en el grafo

Nuevo campo opcional `setup_sql` (string SQL multi-statement) en la config del nodo
`sql_query`. Cuando el nodo se usa como tool, va `fixed` dentro de `node_schema` →
**el LLM nunca lo ve ni lo controla**; solo sigue viendo `query`.

```json
"tool_configurations": {
  "gastos_db": {
    "name": "gastos_db",
    "node_type": "sql_query",
    "description": "Gestiona los gastos del usuario.",
    "node_schema": {
      "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
      "permissions": {
        "type": "object",
        "fixed": { "preset": "read_write", "allowed_schemas": ["finanzas"] }
      },
      "setup_sql": {
        "type": "string",
        "fixed": "CREATE SCHEMA IF NOT EXISTS finanzas;\nCREATE TABLE IF NOT EXISTS finanzas.categorias (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL);\nCREATE TABLE IF NOT EXISTS finanzas.gastos (id SERIAL PRIMARY KEY, categoria_id INT REFERENCES finanzas.categorias(id), monto NUMERIC(12,2), fecha DATE DEFAULT CURRENT_DATE, descripcion TEXT);\nINSERT INTO finanzas.categorias (nombre) VALUES ('Comida'),('Transporte'),('Hospedaje') ON CONFLICT (nombre) DO NOTHING;"
      },
      "query": {
        "type": "string",
        "required": true,
        "description": "SQL para gestionar gastos (SELECT/INSERT/UPDATE)."
      }
    }
  }
}
```

En modo **standalone** (nodo directo, no tool), `setup_sql` va en `config` igual que
`connection_url`.

### 3.2 Orden de ejecución en `do_initialize_inner`

El campo se ejecuta en el path de init que ya existe, en este orden:

1. Resolver `connection_url` (env var `${...}` o handle secure `<sv_*>`).
2. Adquirir el adapter/pool vía `SqlPortFactory` (actual).
3. Provisionar `allowed_schemas` con `create_schemas_if_missing` (actual).
4. **▶ Ejecutar `setup_sql`** — NUEVO (ver 3.3).
5. Asegurar sandbox schema + tablas de registry (actual).
6. **Cargar metadata / introspección** (actual). Como corre **después** del setup, el
   `description_supplement` que ve el LLM ya lista las tablas recién creadas — sin trabajo
   extra.

### 3.3 Semántica de ejecución de `setup_sql`

| Aspecto | Decisión | Justificación |
|---|---|---|
| **Nivel de confianza** | **Operador** — NO pasa por el validador estático del LLM (`StaticRuleValidator`) | Es SQL de build-time, mismo trust-level que `create_schemas_if_missing`. Permite DDL que el LLM tiene bloqueado (CREATE TABLE/SCHEMA, etc.). |
| **Atomicidad** | Una sola transacción; rollback completo si cualquier statement falla | Consistente con el executor multi-statement (Política C). Entorno consistente o ninguno. |
| **Si falla** | **Hard-fail** del init del nodo → propaga error claro (`Failed to run setup_sql: <causa>`); el turno del agente aborta | No dejar al agente correr contra un entorno a medio armar. Falla fuerte al primer uso, no corrupción silenciosa. Mismo patrón que el hard-fail de `create_schemas_if_missing`. |
| **Idempotencia** | Contrato del autor (documentado); el motor NO valida ni lintea en v1 | YAGNI. El meta-agente constructor de ADP genera SQL idempotente. |
| **Multi-statement** | Reusa el split/ejecución statement-a-statement existente | Un solo `setup_sql` string con varios `;`. |
| **Vacío / ausente** | No-op (campo opcional) | Backward-compatible: grafos existentes no cambian. |

### 3.4 Relación con `permissions`

`setup_sql` (operador) **crea la estructura**; el path de `query` en runtime (LLM) sigue
**acotado por `permissions`**. El autor típicamente crea tablas dentro de los
`allowed_schemas` que el agente luego usa. No hay relajación del modelo de permisos del
LLM: lo que el agente puede hacer en runtime no cambia.

---

## 4. Límite Colmena / ADP

- **Colmena (este feature):** el campo `setup_sql` + su ejecución idempotente / atómica /
  nivel-operador en `do_initialize_inner`, después de la provisión de schemas y antes de
  la introspección.
- **ADP (sin cambios de motor):** el meta-agente constructor genera el `setup_sql`
  idempotente y lo persiste en el JSON del grafo; el canvas lo renderiza/edita. Puramente
  **aditivo** (campo opcional nuevo) → **ADP no se rompe**; el worker que consume colmena
  develop sigue compilando sin cambios.

---

## 5. Aislamiento — cómo cada modelo cae del mismo mecanismo

El mismo `setup_sql` sirve a los 4 modelos; lo único que cambia es cómo el autor
configura conexión/schema/RLS:

| Modelo | Config del autor | Comportamiento idempotente |
|---|---|---|
| **DB distinta por usuario** | `connection_url` resuelto per-usuario (ADP/infra provee la DB) | El primer mensaje de ese usuario crea las tablas en *su* DB; los siguientes no-op. |
| **Mismo DB, schema por usuario** | `allowed_schemas`/`setup_sql` con schema derivado per-usuario | Crea/seedea contra *su* schema la primera vez. |
| **Multi-tenant RLS** | `auto_rls: true` + `tenant_user_id` + schema compartido | Tablas creadas una vez (primer mensaje de cualquiera); aislamiento por fila en cada query. |
| **Compartido** | Schema único, sin RLS | Tablas creadas una vez; todos escriben las mismas. |

---

## 6. Fuera de scope (Fase 2, documentado)

- **Guard `run_once: true`** + tabla de tracking (`hash(connection_url + schema +
  versión)`), para setups pesados con seed **no** idempotente (miles de filas de
  referencia, extensiones costosas). Solo se justifica si profiling muestra que el setup
  per-turno pesa.
- **Lint de idempotencia**: warning en init si `setup_sql` contiene `INSERT` sin
  `ON CONFLICT` o `CREATE` sin `IF NOT EXISTS`.
- **Versionado del `setup_sql`** (migraciones evolutivas: el autor cambia el schema en v2
  del grafo y los entornos existentes deben migrar). v1 asume `setup_sql` puramente
  aditivo/idempotente; ALTERs evolutivos quedan a cargo del autor vía
  `ADD COLUMN IF NOT EXISTS`.

---

## 7. Riesgos y checks de implementación

1. **`OnceCell` por instancia de nodo.** El init se cachea por instancia de `SqlNode`.
   Para el caso *schema-por-usuario* / *DB-por-usuario*, esto **exige** que ADP instancie
   el grafo/nodo **fresco por run** (que es como corre hoy: cada turno = run nuevo). Si
   ADP reusara instancias de nodo entre usuarios, el setup (y la metadata introspectada)
   quedaría pinneado al primer `connection_url`/schema → bug de aislamiento. **Verificar**
   en implementación que el ciclo de vida del nodo es per-run y documentarlo.
2. **Múltiples nodos `sql_query` en un grafo.** Si hay más de uno apuntando a la misma DB,
   cada uno corre su propio `setup_sql` en su init. Idempotencia lo hace seguro, pero
   documentar que el patrón esperado es **1 DB = 1 tool con setup**.
3. **Latencia del primer uso.** El setup corre sincrónicamente en el init del nodo, que
   es lazy (primer tool call). El primer query del agente paga la latencia del setup; los
   siguientes no. Aceptable.
4. **`setup_sql` grande en el JSON.** Se persiste en la config del grafo. Sin límite duro
   en v1; revisar si hace falta un cap.

---

## 8. Testing

- **Unit:** parsing/presencia del campo `setup_sql`; no-op cuando ausente/vacío;
  hard-fail cuando un statement falla (con rollback verificado).
- **Integración (`#[ignore]`-gated, requiere `DATABASE_URL`):**
  - Primer init crea schema + tablas + seed; segundo init es no-op (sin duplicados de
    seed, conteo de filas estable).
  - `setup_sql` con DDL bloqueado por el validador del LLM (ej. `CREATE TABLE`) corre OK
    (confirma bypass del validador a nivel-operador).
  - El `description_supplement` post-init lista las tablas creadas por `setup_sql`.
  - Rollback: un `setup_sql` con un statement inválido al final no deja estructura parcial.
- **E2E (grafo real):** agente de finanzas con `setup_sql`; prompt "registrá un gasto de
  comida de $20" → crea ticket; verificar contra Postgres real. Guardar SSE en
  `/tmp/colmena_e2e/` y reporte amigable.

---

## 9. Documentación a actualizar

- `docs/developer_guide/23_sql_node.md` — nueva sección "Bootstrapping de entorno con
  `setup_sql`" (forma, semántica, contrato de idempotencia, ejemplo finanzas, los 4
  modelos de aislamiento).
- `docs/node_configurations.json` — agregar `setup_sql` al schema de `sql_query`.
- CLAUDE.md "Current Status" — entry del feature al shippear.
