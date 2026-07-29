# Respuesta — nodos-tool usados como nodo top-level

**Para:** Equipo ADP (Julian Caicedo)
**De:** Equipo Colmena
**Fecha:** 2026-07-28
**Sobre:** `COLMENA_STANDALONE_TOOL_NODES.md`
**Estado:** roturas 1 y 2 **arregladas** en `develop` (`a665dadf`); rotura 3 confirmada como decisión de diseño.

Gracias por el reporte — estaba bien apuntado y nos ahorró la mitad del trabajo.
Verificamos las tres roturas leyendo el código y las corregimos las dos que eran
nuestras. Abajo, punto por punto, qué encontramos y qué hicimos.

---

## Rotura 1 — `sql_query` descarta su `config` → **ARREGLADA**

Confirmada la causa raíz: `execute` leía todo de `inputs` e ignoraba `config`, y
`build_inputs_for` nunca siembra `config` en los `inputs` de un nodo top-level.

**Una corrección al diagnóstico, para que ajusten su modelo mental:** el efecto
que describen ("conecta y consulta bien, pero jamás escribe — falla en silencio")
**no es exacto**. `initialize()` **solo** se invoca desde el camino de tool
(`dag_tool_executor.rs`), **nunca** desde `run_use_case` (top-level). En top-level
la conexión se abre perezosamente dentro de `execute` vía `get_or_init(&inputs)`,
y `execute` chequea `inputs.get("query")` **antes** que nada. Con la config-only
que ustedes emiten (todo en `config`, sin aristas), el nodo fallaba **ruidoso y
temprano** con `sql_query node requires 'query' input` — no en silencio hacia
read-only. El bug de fondo sí era real; el escenario silencioso solo aparecía con
cableado parcial (query y connection_url por aristas, `permissions` no).

**El fix, y la decisión datos-vs-gobierno que preguntaban:**

Nuevo `SqlNode::build_effective_config(config, inputs)` que mergea `config` (base)
con `inputs` (encima), con una distinción explícita:

- **Campos de datos** (`query`): un input entrante **pisa** la config. Es la gracia
  de conectar nodos por aristas.
- **Campos de gobierno** (`connection_url`, `permissions`, `runtime_limits`,
  `guardrail_llm`, `setup_sql`): si la `config` los autora, son **autoritativos** y
  un input **no** puede pisarlos.

Esto arregla la escritura top-level **y** cierra la nota de seguridad que ustedes
mismos señalaron: un nodo aguas arriba que emita una clave `permissions` ya **no**
escala los permisos del SQL. Lo verificamos E2E (ver abajo).

En el camino de tool `config` es `{}`, así que `effective == inputs` y el
comportamiento como herramienta LLM es **byte-idéntico** — no rompe nada de lo que
ya usan.

> Nota: aplicamos "gobierno = config-only" **solo a `sql_query`**, no a
> `http_request` (ver la sección de diseño más abajo).

---

## Rotura 2 — `tavily_client` exige `__sub_tool` → **ARREGLADA**

Confirmada. `execute` asumía que el executor de tools inyecta `__sub_tool`.

Fix: `resolve_sub_tool` resuelve el sub-tool con precedencia
`inputs[__sub_tool]` → `inputs[sub_tool]` → `config[sub_tool]`, y
`build_effective_inputs` mergea `config` en los inputs que ven `handle_search` /
`handle_fetch`. Un nodo top-level ahora selecciona su sub-tool y pasa
`query` / `url` por `config`.

**Cómo emitirlo desde ADP** (nodo suelto `webSearch`):

```json
{
  "type": "tavily_client",
  "config": {
    "api_key": "${TAVILY_API_KEY}",
    "sub_tool": "search",
    "query": "…",
    "max_results": 3
  }
}
```

`sub_tool` acepta `"search"` o `"fetch"`. Para `fetch`, pasen `url` en vez de
`query`. En camino de tool no cambia nada.

---

## Rotura 3 — `data_run_python` no está en el registro → **es tool-only por diseño**

Confirmado: `data_run_python` **no** es un `ExecutableNode` registrado; existe solo
como herramienta sintética dentro del loop del `llm_call` (`data_run_python.rs` +
dispatch en `dag_tool_executor.rs`). Depende del contexto del executor LLM
(`fixed_config`, bindings tabulares, detección de capacidad gsheets/sql) que **no
existe** fuera de ese loop. Registrarlo como nodo standalone sería reimplementar
ese dispatch.

**Decisión (confirmada por ambos lados): `data_run_python` NO debe existir como
nodo suelto.** Se queda como tool-only. Acción: **borren la entrada `dataRunPython`
de la tabla de mapeo de nodos standalone** — no la remapeen. Para el caso "correr
Python sobre datos" como nodo suelto, la gente ya tiene **`python_script`** (nodo
registrado que corre standalone); `data_run_python` era redundante ahí.

---

## La decisión de diseño: datos-vs-gobierno en `http_request`

La distinción que preguntaban **sí** está tomada a propósito ahora, pero la
aplicamos **solo a `sql_query`**, no a `http_request`, **a propósito**:

En `http_request`, que un `bearer_token` o un `header` llegue por una arista es un
**patrón legítimo y común**: un nodo de login/auth aguas arriba calcula el token y
se lo pasa al `http_request`. Bloquear eso (config-only para gobierno en http)
rompería ese caso de uso real. Por eso lo dejamos como está: en http, el input
sigue pisando la config, incluidos `bearer_token` / `authorization` / `headers`.

En `sql_query` es al revés: los `permissions` son gobierno del autor del grafo, no
un dato que tenga sentido calcular aguas arriba, así que ahí sí congelamos gobierno
en config.

Si en su canvas hay un caso donde un `http_request` top-level **debería** proteger
credenciales de ser pisadas por un input, cuéntennos y lo evaluamos como un opt-in
explícito, no como default.

---

## Verificación (grafos reales corridos por el DAG engine)

No solo leímos el código; corrimos grafos reales:

| Grafo | Qué prueba | Resultado |
|---|---|---|
| `tests/graphs/agents/standalone_sql_write_e2e.json` | `sql_query` top-level con `read_write` en config hace `INSERT` | `rows_affected: 1` (antes fallaba) |
| `tests/graphs/agents/standalone_sql_governance_e2e.json` | un nodo aguas arriba inyecta `permissions: read_write_delete`; config es `read_write` | `DELETE` **BLOCKED** — escalada prevenida |
| `tests/graphs/external/standalone_tavily_search_e2e.json` | `tavily_client` top-level `search` con `sub_tool`+`query` en config | devuelve resultados |
| `tests/graphs/agents/sql_read_write_delete_e2e.json` | regresión: `sql_query` como tool LLM (agente Gemini) | sin cambios, OK |

9 unit tests nuevos (TDD), 2170 unit tests en verde, clippy limpio.

---

## Qué necesitamos de ustedes

1. Borren `dataRunPython` del mapeo de nodos standalone (confirmado: no debe
   existir como nodo suelto — no lo remapeen).
2. Confirmen la forma de emisión de `tavily_client` top-level (`sub_tool` en
   `config`).
3. Si tienen un caso de `http_request` top-level que quiera proteger credenciales
   de un input, avísennos para el opt-in.

Con el fix ya en `develop`, su worker lo toma en el próximo build.
