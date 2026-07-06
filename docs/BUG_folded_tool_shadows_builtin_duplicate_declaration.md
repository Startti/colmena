# BUG — Folded tool cuyo `name` colisiona con un built-in duplica la declaración (Gemini `Duplicate function declaration`)

**Fecha:** 2026-07-05
**Reportado por:** investigación ADP ↔ colmena (verificado E2E contra colmena-api dev)
**Severidad:** media — rompe cualquier `llm_call` (provider google) con declaración eager cuyo folded tool se llame como un built-in y tenga key ≠ name. Ortogonal a la feature de visibilidad anidada.
**Repo/crate:** `Startti/colmena` → `src/libs/colmena` (`colmena_dag_engine`)
**Estado:** ✅ **RESUELTO** (2026-07-05) — fix (a): `dedup_tools_by_name` en `nodes/llm.rs` dedup la lista final `tools` por `name` (keeping-first = config-wins, porque el executor lista folded configs antes que built-ins). Repro E2E ya no lanza `Duplicate function declaration`.

---

## Síntoma

Un `llm_call` con provider `google` falla al ejecutarse:

```
Error de ejecución en el nodo: Request failed: Gemini API error:
[{ 'error': { 'code': 400, 'message': 'Duplicate function declaration found: multiply', 'status': 'INVALID_ARGUMENT' } }]
```

El nodo nunca corre; el error se propaga como `error`/`subgraph-error`.

## Trigger exacto (verificado con matriz E2E)

Se necesitan **las tres** condiciones a la vez:

1. El folded tool (`tool_configurations`) tiene **`name` = un tool/nodo built-in** — p.ej. `multiply`, `add`, `current_time` (registrados en `infrastructure/registry.rs`, ~L92: `nodes.insert("multiply", Arc::new(MultiplyNode))`).
2. La **key** del map `tool_configurations` **≠** ese `name` (típico cuando el frontend usa cuids como key).
3. Declaración **eager** (`lazy_tool_loading` no está en `true`).

| Caso | Resultado |
|---|---|
| `name="multiply"` (builtin), key `"mult"` ≠ name, eager | ❌ Duplicate |
| `name="add"` (builtin), key `"k"` ≠ name, eager | ❌ Duplicate |
| `name="add"` (builtin), key `"add"` == name, eager | ✅ ok |
| `name="calc"`/`"xyzzy"` (no builtin), key ≠ name, eager | ✅ ok |
| cualquiera con `lazy_tool_loading: true` | ✅ ok |

## Repro mínimo

`POST /api/v1/executions` con:

```json
{
  "dag_json": {
    "nodes": {
      "top": {
        "type": "llm_call",
        "config": {
          "provider": "google", "model": "gemini-2.5-flash", "api_key": "<GEMINI_KEY>",
          "system_message": "Call 'add' once (code sets output=1), then reply DONE.",
          "enabled_tools": ["add"],
          "tool_configurations": {
            "k": {
              "name": "add", "node_type": "python_script", "description": "t",
              "node_schema": { "sandbox_mode": { "fixed": "none" },
                "code": { "type": "string", "required": true, "description": "set output" } }
            }
          }
        }
      },
      "trigger": { "type": "input", "config": { "inputType": ["Text"] } }
    },
    "edges": [{ "from": "trigger", "to": "top" }]
  },
  "inputs": { "user_message": "go" }, "agent_session_id": null
}
```
→ `Duplicate function declaration found: add`. Cambiar la key `"k"` → `"add"` (o agregar `"lazy_tool_loading": true`) lo arregla.

## Root cause

1. El catálogo de tools (`all_tools`) que se declara al LLM incluye los **built-ins por su `name`** (`multiply`, `add`, …) — `infrastructure/registry.rs`.
2. Un folded `tool_configuration` produce un `ToolDefinition` cuyo `name = tool_config.name` (o la key si name vacío) — `infrastructure/dag_tool_executor.rs` (~L810-813, `effective_name`).
3. `configured_aliases` se siembra con las **keys** del map, no con los names — `infrastructure/nodes/llm.rs` (~L2000-2001: `tool_configurations.keys()`).
4. En `filter_enabled_tools` (`infrastructure/nodes/llm.rs` ~L51):
   - `raw_includes = configured_aliases ∪ enabled_tools` = `{"k"} ∪ {"add"}` = `{"k","add"}`.
   - El filtro incluye por `t.name ∈ final_includes`. Sobreviven **el built-in `add`** (name="add") **y** el folded `add` (name="add", entró por su key/enabled_tools) → **dos ToolDefinition con name `add`**.
5. Al construir el request Gemini, dos funciones con el mismo name → `Duplicate function declaration`.

Cuando **key == name**, el folded tool entra por la misma llave y sobrescribe al built-in en el catálogo → una sola entrada. Con **lazy loading**, los tools no se declaran upfront (van por `describe_tool`), así que la colisión no llega al request. Un name que **no** es built-in nunca coincide con dos entradas.

## Fix propuesto

Preferir uno (o combinar (a)+(c)):

- **(a) Dedup final por `name`** — antes de mandar `tools` al proveedor (tras el filtro y los pushes sintéticos, `nodes/llm.rs` ~L2533-2575), deduplicar por `ToolDefinition.name`, con **precedencia del folded tool sobre el built-in** (config-wins). Robusto y de una línea conceptual; elimina toda posibilidad de duplicados regardless de key/builtin.
- **(b) Override en el catálogo** — al construir `all_tools`, si un folded `tool_configuration.name` coincide con un built-in, excluir el built-in (el config del usuario manda).
- **(c) Guardia + warning** — si aún así quedaran dos con el mismo name, colapsar y emitir un `eprintln!`/warn con el name colisionado, para no romper el request nunca.

## Tests de regresión

Unit sobre el ensamblado de tools (`nodes/llm.rs`):
- folded tool `name="add"` con key `"k"` + `enabled_tools:["add"]` eager → la lista final de `tools` contiene **exactamente uno** llamado `add` (el folded), no dos.
- `name="add"` key `"add"` → sigue funcionando (uno).
- `name` no-builtin key≠name → sin cambios (uno).
- (integración) el repro mínimo de arriba contra un provider stub → no lanza duplicate.

## Notas
- No afecta al agente-creador de ADP (sus tools no colisionan con built-ins y usa `lazy_tool_loading`), pero es un footgun para cualquier `llm_call` eager cuyo tool se llame como un built-in — y el frontend genera keys tipo cuid ≠ name, así que la condición (2) es el caso común.
- Descubierto durante el E2E de la feature de visibilidad anidada (PR #146/#147), como bug pre-existente e independiente.
