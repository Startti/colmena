---
name: capability-data-sql
description: Use when the user wants to read from or save into a database (lay terms like "base de datos", "guardar registros", "consultar datos"). Covers the sql_query node, permission presets and the side-effects of writes.
---

# Capacidad: leer y guardar en una base de datos (`sql_query`)

Usá esta capacidad cuando la persona habla de una **base de datos**: quiere
**consultar datos** que ya existen ("mostrame los pedidos de hoy"), **guardar
registros** nuevos ("anotá este gasto"), o **actualizar/borrar** información. El
nodo que hace todo esto es `sql_query`: ejecuta consultas PostgreSQL con control
de permisos.

> Para cómo se arma y se conecta un grafo (anatomía, nodos, edges, triggers,
> puertos), mirá [[building-graphs-core]]. Esta skill solo cubre el nodo
> `sql_query`.

---

## Qué es `sql_query`

Es un nodo que corre una consulta SQL contra una base de datos PostgreSQL. Casi
siempre se usa **como herramienta de un `llm_call`** (la IA decide qué consulta
escribir), pero también puede ir como nodo suelto con una consulta fija.

La gracia es que vos, el autor del grafo, fijás la conexión y los **permisos**;
la IA solo escribe el SQL dentro de los límites que le diste.

### Campos de `config` que importan

Estos son los nombres **exactos** de los campos (no inventar otros):

| Campo              | Requerido | Qué es |
|--------------------|-----------|--------|
| `connection_url`   | Sí        | URL de conexión a PostgreSQL. Soporta `${VAR}` para leer de variables de entorno (ej. `"${DATABASE_URL}"`). |
| `query`            | Sí (en nodo suelto) | La consulta SQL a ejecutar. Como herramienta, la escribe la IA. |
| `permissions`      | No        | Objeto que controla **qué operaciones** se permiten. Si se omite, queda en `read_only`. |
| `setup_sql`        | No        | DDL + datos semilla que vos escribís para que el grafo cree sus tablas la primera vez. La IA nunca lo ve. |
| `runtime_limits`   | No        | Límites por consulta: `max_rows` (default 100), `statement_timeout_ms` (default 30000), `work_mem_mb` (default 64). |
| `guardrail_llm`    | No        | El **crítico** opcional: una segunda IA revisa cada consulta antes de ejecutarla (ver abajo). Default desactivado. |

### El objeto `permissions`

Dentro de `permissions` los campos clave son:

| Campo             | Qué es |
|-------------------|--------|
| `preset`          | Qué operaciones se permiten (ver la tabla de presets abajo). |
| `allowed_schemas` | Lista de schemas (carpetas lógicas) que la IA puede tocar, ej. `["public"]`. **Recomendado siempre.** |
| `deny`            | Lista para quitar operaciones de un preset, ej. `["delete"]`. |

---

## Presets de permisos (los nombres exactos)

El `preset` decide qué puede hacer la IA. Estos son los **nombres exactos** —
copialos tal cual:

| Preset              | Operaciones permitidas |
|---------------------|------------------------|
| `read_only`         | Solo SELECT (leer). |
| `read_write`        | SELECT, INSERT, UPDATE (leer, agregar, modificar). |
| `read_write_delete` | SELECT, INSERT, UPDATE, DELETE + ALTER TABLE ADD COLUMN (también borrar filas). |
| `full`              | Todo lo anterior + CREATE FUNCTION y CREATE TABLE. |

**Regla de oro para elegir preset:**
- Si la persona solo quiere **ver/consultar** datos → `read_only`.
- Si quiere **guardar o actualizar** registros → `read_write`.
- Solo subí a `read_write_delete` o `full` si pidió explícitamente **borrar** o
  **crear tablas**.

**Siempre bloqueado (ningún preset lo habilita):** `DROP`, `TRUNCATE`,
`CREATE SCHEMA` y cualquier `ALTER` que no sea `ADD COLUMN`. Además, `DELETE` y
`UPDATE` **siempre requieren una cláusula `WHERE`** (no se puede borrar/modificar
toda una tabla de un saque). Esto protege los datos por diseño.

### El crítico opcional (`guardrail_llm`)

Si activás `guardrail_llm` con `{ "enabled": true }`, una segunda IA revisa cada
consulta buscando riesgos (borrados masivos, fugas de datos sensibles, inyección
SQL) antes de ejecutarla. Es una capa extra de seguridad para agentes que
escriben en producción. Por defecto está apagado; actívalo si la persona maneja
datos delicados.

---

## Cómo usarlo como herramienta de la IA (`tool_configurations`)

El patrón canónico: dentro del `config` de un `llm_call`, agregás una entrada en
`tool_configurations` con `node_type: "sql_query"`. La conexión y los permisos van
como campos **`fixed`** dentro de `node_schema` (la IA no los ve ni los puede
cambiar); lo único que la IA escribe es `query`.

```json
"tool_configurations": {
  "query_database": {
    "name": "query_database",
    "node_type": "sql_query",
    "description": "Query the production database.",
    "node_schema": {
      "connection_url": {
        "type": "string",
        "fixed": "${DATABASE_URL}"
      },
      "permissions": {
        "type": "object",
        "fixed": {
          "preset": "read_only",
          "allowed_schemas": ["public"]
        }
      },
      "runtime_limits": {
        "type": "object",
        "fixed": {
          "max_rows": 50,
          "statement_timeout_ms": 15000,
          "work_mem_mb": 32
        }
      },
      "guardrail_llm": { "type": "object", "fixed": { "enabled": false } },
      "query": {
        "type": "string",
        "required": true,
        "description": "SQL SELECT query to execute against the PostgreSQL database."
      }
    }
  }
}
```

Notá que `connection_url`, `permissions`, `runtime_limits` y
`guardrail_llm` van **`fixed`** (ocultos a la IA), y solo `query` queda visible y
editable por el modelo. El nodo además introspecciona la base y le agrega
automáticamente a la descripción de la herramienta la lista de tablas
disponibles, así la IA sabe qué consultar sin que vos lo escribas.

---

## Ejemplo runnable (solo lectura) — copialo verbatim

Agente analista de datos que solo **lee**. La IA recibe la pregunta, lista las
tablas y responde consultando la base. Preset `read_only`, así que es imposible
que modifique nada. Flujo: `sql_agent` → `result`.

```json
{
  "nodes": {
    "sql_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a helpful data analyst. Use the query_database tool to answer questions about the database. Always start by listing available tables.",
        "enabled_tools": ["query_database"],
        "stream": false,
        "tool_configurations": {
          "query_database": {
            "name": "query_database",
            "node_type": "sql_query",
            "description": "Query the production database.",
            "node_schema": {
              "connection_url": {
                "type": "string",
                "fixed": "${DATABASE_URL}"
              },
              "permissions": {
                "type": "object",
                "fixed": {
                  "preset": "read_only",
                  "allowed_schemas": ["public"]
                }
              },
              "runtime_limits": {
                "type": "object",
                "fixed": {
                  "max_rows": 50,
                  "statement_timeout_ms": 15000,
                  "work_mem_mb": 32
                }
              },
                      "guardrail_llm": { "type": "object", "fixed": { "enabled": false } },
              "query": {
                "type": "string",
                "required": true,
                "description": "SQL SELECT query to execute against the PostgreSQL database."
              }
            }
          }
        },
        "prompt": "¿Cuáles son las tablas que están en esta base de datos y cuál es su estructura?"
      }
    },
    "result": {
      "type": "output",
      "config": { "label": "SQL Agent Result" }
    }
  },
  "edges": [
    { "from": "sql_agent", "to": "result" }
  ]
}
```

Qué hace cada parte:
- `sql_agent`: un `llm_call` con la herramienta `query_database` (un `sql_query`
  fijado a `read_only`).
- `prompt`: la pregunta de la persona. La IA decide qué SELECT escribir.
- `result`: devuelve la respuesta final del agente.

---

## ⚠️ ADVERTENCIA — Las escrituras modifican datos de verdad

Esto es lo más importante de toda la skill, leelo siempre antes de armar un grafo
con SQL:

- `SELECT` (leer) **no cambia nada** — es seguro probarlo cuantas veces quieras.
- `INSERT`, `UPDATE` y `DELETE` **modifican datos reales y permanentes** en la
  base de datos. Una vez ejecutados, **no hay deshacer**.

Por eso, como agente constructor de grafos, **antes de hacer una corrida de
prueba (test-run) de un grafo que puede escribir**:

1. **Avisá y pedí confirmación explícita** a la persona. Decile claramente: "este
   grafo puede INSERTAR / ACTUALIZAR / BORRAR registros reales en tu base; ¿lo
   pruebo igual?". No corras pruebas con escrituras sin que la persona diga que sí.
2. **Preferí lo read-only para probar.** Si lo único que querés es verificar que el
   grafo arma bien, usá un preset `read_only` o consultas SELECT primero.
3. **Usá un schema/base desechable para las pruebas.** Si necesitás probar
   escrituras de verdad, hacelo contra un schema de prueba o una base de juguete,
   nunca contra datos de producción de la persona.

Regla práctica: si el `preset` es `read_write`, `read_write_delete` o `full`,
asumí que el grafo **puede dañar datos** y tratá cada test-run con el cuidado
correspondiente. Cuando dudes, quedate en `read_only`.

---

Para cómo se enganchan los nodos, los edges, los puertos y los triggers que
disparan este agente, volvé a [[building-graphs-core]].
