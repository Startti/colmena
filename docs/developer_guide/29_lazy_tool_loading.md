# 29. Lazy Tool Loading

Carga progresiva del schema de tools para nodos LLM. Cuando un `llm_call` tiene muchas tools (>10), inyectar todos los schemas completos en cada request al provider degrada la atención del modelo. Esta feature expone un catálogo ligero (`name + summary`) y solo revela el schema completo de las tools que el LLM decide usar, llamando al tool sintético `describe_tool`.

## Activación

Boolean a nivel del `llm_call`:

```json
{
  "type": "llm_call",
  "config": {
    "lazy_tool_loading": true,
    "tool_configurations": { ... }
  }
}
```

Ausente o `false` → comportamiento idéntico al de hoy. Backward-compat total.

## Configuración por tool

Dos campos opcionales nuevos en cada `ToolConfiguration`:

```jsonc
{
  "name": "search_orders",
  "summary": "Find historical orders. Use when the user asks about past purchases.",
  "description": "Search the orders table by date range, status, customer ID, or product SKU...",
  "node_type": "sql_query",
  "node_schema": { ... },
  "eager": false
}
```

- `summary`: opcional. Lo que el LLM ve en el catálogo. Si falta, se usa `description` truncada (~120 chars). Máximo 200 chars (warning + truncate al cargar).
- `eager`: opcional, default `false`. Una tool `eager: true` se registra en cada request con su schema completo y NO aparece en el catálogo. Úsalo para tools que se llaman casi siempre (ej. `current_time`, `get_user_id`).

## Cómo funciona en runtime

1. Al cargar el grafo se construye el catálogo: `[(name, summary), ...]` para cada tool no-eager.
2. En cada request al provider, el `tools[]` enviado se rebuild-ea:
   ```
   tools[] = [tools no-catálogo (eager, load_skill, document_*)]
           + [describe_tool si quedan pending]
           + [tools en discovered_set, con schema completo]
   ```
3. Cuando el LLM llama `describe_tool("X")`, el `DagToolExecutor` intercepta la call (mismo patrón que `load_skill`), genera el markdown curado del schema de `X`, y devuelve el contenido.
4. En el siguiente request, `X` deja el catálogo (ya descubierta) y aparece tipada en `tools[]` con su schema completo. El LLM la invoca normalmente.

## Persistencia con memoria

`discovered_set` no se guarda en BD. Es una vista derivada del historial: cada vez que el `llm_call` arranca con un `session_id` que tiene memoria, se scan-ean los mensajes pasados y se reconstruye el set:

- **Regla 1:** una llamada pasada a `describe_tool(name="X")` añade `X` al set.
- **Regla 2:** una llamada pasada directa a `X` (donde `X` está en el catálogo actual) añade `X` al set.

La regla 2 maneja tres casos: truncación que dropea el `describe_tool` original, sesiones que cambiaron de `eager` a `lazy` mid-flight, e historiales sembrados manualmente. Si AMBOS rastros caen del historial, la tool sale del set y el LLM la re-descubre la próxima vez que la necesite.

## Observabilidad

Por cada call a `describe_tool` exitosa el engine emite:

- Eventos estándar `LlmToolCallStart` / `LlmToolCallFinish` (como cualquier tool).
- Evento extra `ToolDescribed { tool_id, tool_name }` que en el data-stream-protocol del CLI/serve aparece como:
  ```json
  { "type": "tool-described", "nodeId": "...", "toolCallId": "...", "toolName": "search_orders" }
  ```

El summary final (`extra_info`) incluye:
```json
{ "tools_discovered": ["search_orders", "send_email"] }
```
solo cuando `lazy_tool_loading: true` y al menos una tool fue descubierta.

## Tools prefabricadas y lazy

El motor expone tools al LLM por dos vías independientes; **solo una participa del lazy**.

### `enabled_tools` — atajo eager para nodos prefabricados

```json
"enabled_tools": ["add", "multiply", "current_time"]
```

- Cada string debe coincidir con un `node_type` registrado en [`registry.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/registry.rs).
- Para cada nombre, el `DagToolExecutor` genera la `ToolDefinition` automáticamente:
  - `description` ← `node.description()`
  - `parameters` ← `node.schema().inputs`
- **No existe slot para `summary`, `eager`, `fixed_config` ni nombre custom.**
- Tools expuestas por esta vía van **siempre eager**: aparecen tipadas en `tools[]` desde el primer turno y nunca pasan por `describe_tool`.

### `tool_configurations` — control completo (única vía lazy-capable)

```jsonc
"tool_configurations": {
  "add": {
    "name": "add",            // nombre que ve el LLM (puede ser distinto de node_type)
    "description": "...",
    "summary": "...",         // catálogo lazy
    "node_type": "add",       // backing real registrado en registry.rs
    "eager": false,           // false → entra al catálogo lazy
    "fixed_config": {}
  }
}
```

- El campo `node_type` apunta al mismo nodo prefabricado: el código ejecutado es exactamente el mismo (`AddNode`, `MultiplyNode`, `CurrentTimeNode`, etc.).
- Es la única vía donde puedes definir `summary` y `eager` — y por tanto la única que el motor lazy considera al construir el catálogo.

### Regla de oro

> El sistema lazy **solo mira `tool_configurations`**. Cualquier tool listada en `enabled_tools` sale eager, sin importar el valor de `lazy_tool_loading`.

Implementación en [`llm.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs) (resumido):

```rust
for cfg in tool_configurations.values() {
    if cfg.eager { continue; }
    catalog.push(CatalogEntry { name: cfg.name.clone(), summary: ... });
}
```

### Patrones recomendados

| Quiero… | Configuración |
|---|---|
| Exponer un nodo prefabricado tal cual, eager, sin custom | `"enabled_tools": ["add"]` |
| Lo mismo, pero renombrado o con `fixed_config` | `tool_configurations` con `eager: true` |
| Hacer un nodo prefabricado lazy | `tool_configurations` con `node_type: "<built-in>"` y `eager: false` |
| Mezclar ambos | Usar las dos secciones; se hacen union/dedup por nombre |

### Ejemplo combinado

```json
{
  "type": "llm_call",
  "config": {
    "lazy_tool_loading": true,
    "enabled_tools": ["current_time"],
    "tool_configurations": {
      "add": {
        "name": "add",
        "description": "Add two numbers. Inputs: a (number), b (number). Returns a + b.",
        "summary": "Sum of two numbers a and b.",
        "node_type": "add",
        "fixed_config": {}
      },
      "multiply": {
        "name": "multiply",
        "description": "Multiply two numbers. Inputs: a (number), b (number). Returns a * b.",
        "summary": "Product of two numbers a and b.",
        "node_type": "multiply",
        "fixed_config": {}
      }
    }
  }
}
```

Resultado:
- `current_time` aparece tipada desde el turno 1 (entró por `enabled_tools` → eager).
- `add` y `multiply` viven solo en el catálogo del `describe_tool`. Cuando el LLM las descubre, suben a `tools[]` tipadas en el turno siguiente.

Grafo completo verificado: [`tests/graphs/agents/tools_lazy_basic.json`](../../tests/graphs/agents/tools_lazy_basic.json).

## Edge cases conocidos

- **LLM emite describe_tool y el tool real en el mismo turno**: algunos modelos pueden emitir tool calls paralelos. Si el LLM intenta llamar `X` en el mismo turno que llamó `describe_tool("X")`, el provider rechaza la segunda call (porque `X` no estaba en `tools[]` ese turno). El turno siguiente sí la verá tipada. La descripción del tool sintético dice explícitamente "Call it directly on your next turn" para reforzar el comportamiento. Es raro en práctica.
- **Truncation agresiva**: si el rolling window dropea TANTO el `describe_tool` como cualquier llamada directa a una tool, la tool sale del `discovered_set` y el LLM tiene que re-descubrirla. Es comportamiento natural — no estás "olvidando" tools que el LLM nunca volverá a usar.

## Trust model

Mismo posture que skills: el engine valida estructura (`summary` length, schema válido) pero no contenido semántico. Un `summary` redactado para inducir prompt injection es responsabilidad de quien configura la tool. El catálogo se fija al cargar el grafo — el LLM no puede añadir tools nuevas en runtime.

## Referencia rápida

- Tool sintético: `describe_tool(name: string)`.
- La descripción del tool contiene el catálogo completo (nombre + summary de cada tool no-eager no-descubierta).
- Si no se configura `lazy_tool_loading`, la feature está completamente deshabilitada (zero overhead).
## Tool context block

When the engine builds the markdown that `describe_tool` returns (lazy)
or the description that ships in `tools[]` (eager / non-lazy), it now
assembles a **layered block** with up to five sections:

1. `# {tool_name}` + the tool's description.
2. `## Access policy` — if `ExecutableNode::tool_description_supplement`
   returned `Some`, derived from the tool's fixed config (e.g.
   `sql_query`'s preset + allowed_schemas + max_rows).
3. `## Best practices` — body of the `SKILL.md` whose frontmatter has
   `node_type: <this_node_type>`. One guide per node_type, validated at
   graph load.
4. `## Parameters` — present only in the lazy `describe_tool` variant;
   the eager / non-lazy path omits it because the schema travels typed.
5. `## Related knowledge` — names + descriptions of every skill listed
   in this tool's `tool_configurations.<name>.skills` array. The model
   loads them with `load_skill(name)` based on intent.

Routing follows the existing lazy/eager bifurcation: lazy + non-eager
goes through `describe_tool` (on demand); eager OR `lazy_tool_loading:
false` goes via the tool description (always in the prompt).

See [24_skills.md](24_skills.md) ("Layered routing") for the full layer
classification and validation rules. See
[docs/superpowers/specs/2026-05-29-layered-tool-context-design.md](../superpowers/specs/2026-05-29-layered-tool-context-design.md)
for the feature spec.

- Spec completo: [docs/superpowers/specs/2026-05-03-lazy-tool-loading-design.md](../superpowers/specs/2026-05-03-lazy-tool-loading-design.md)
