# 29. Lazy Tool Loading

Carga progresiva del schema de tools para nodos LLM. Cuando un `llm_call` tiene muchas tools (>10), inyectar todos los schemas completos en cada request al provider degrada la atención del modelo. Esta feature expone un catálogo ligero (`name + summary`) y solo revela el schema completo de las tools que el LLM decide usar, llamando al tool sintético `describe_tool`.

## Summary requirement

Every Rust-side synthetic tool MUST declare a `summary` between 10 and 200
characters via `build_synthetic_tool_with_summary` (or by setting the
`summary` field on `ToolDefinition` directly). This is enforced in CI by
the `every_registered_tool_has_text_entry` test in
`llm_synthetic_tools/mod.rs`. Builds refuse to ship if any synthetic tool
is missing a summary.

DAG nodes used as tools (via `tool_configurations`) are exempt — their
descriptions are user-supplied per agent and dynamic. For those, the
lazy-loading catalog falls back to a truncated `description`.

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

## Persistencia con memoria — descubrimiento POR TURNO (2026-06-27)

`discovered_set` no se guarda en BD. Es una vista derivada del historial, pero
**acotada al turno de usuario actual** (`current_turn_slice`: los mensajes desde el
último mensaje `user` en adelante). Se reconstruye en cada iteración del ReAct loop:

- **Regla 1:** una llamada a `describe_tool(name="X")` **en este turno** añade `X` al set.
- **Regla 2:** una llamada directa a `X` (donde `X` está en el catálogo) **en este turno** añade `X` al set.

**Por qué por turno** (mirror del inspect-before-code de gsheets, que re-inspecciona
cada turno "por si algo cambió"): cross-turn, la compactación puede tirar la guía que
el modelo vio en el turno 1, pero un set history-wide igual lo marcaría "descubierto" →
el modelo actuaría en el turno 2 sin la guía en contexto. Acotar al turno garantiza que
el schema/guía se recargue fresco la primera vez que se usa la tool en cada turno. La
Regla 2 además cubre sesiones que cambiaron de `eager` a `lazy` mid-flight e historiales
sembrados manualmente — dentro del turno.

## Guard describe-before-use (2026-06-27)

El catálogo correctamente excluye del `tools[]` las tools no-descubiertas-este-turno,
pero el provider (p.ej. Gemini) puede **alucinar** un `functionCall` a una tool fuera de
`tools[]`, y el `DagToolExecutor` la despacharía **a ciegas** (sin que el modelo haya
cargado su schema → args inventados → falla). El guard cierra ese agujero
([`agent_service.rs`](../../src/libs/colmena/src/llm/application/agent_service.rs)):

> Si el modelo llama una tool del catálogo que NO está en el `iteration_tools` de este
> turno, **no se ejecuta** — se devuelve su **schema** (vía `describe_tool`) envuelto en
> un aviso explícito ("⚠️ NOT A RESULT … call again with matching args"). El modelo lee
> el schema y re-llama con args correctos. La call original queda en el historial → la
> Regla 2 la marca descubierta-este-turno → en la iteración siguiente entra a `tools[]` y
> ejecuta normal.

Es **schema-only** (no auto-retry: los args estaban mal por no tener el schema; re-correrlos
no sirve). Activo **solo bajo `lazy_tool_loading: true`** (vía `AgentRunParams.lazy_catalog_names`);
agentes eager no se ven afectados.

Los prompts en lazy son explícitos sobre este flujo (la descripción de `describe_tool`
y un bloque de sistema "Lazy tools (load before use)") para que el modelo entienda los
pasos y NO confunda el redirect del guard con el resultado real.

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

Implementación en la función pura [`build_lazy_catalog`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs) (resumido); `llm_call` la invoca y vuelca sus salidas:

```rust
for (map_key, cfg) in tool_configurations.iter() {
    if !cfg.enters_lazy_catalog() { continue; } // salta eager y entradas mcp
    let name = cfg.effective_name(map_key).to_string(); // name opcional -> clave
    catalog.push(CatalogEntry { name, summary: ... });
}
```

### Las tools MCP nunca son lazy

Ni la entrada ni sus tools entran al catálogo. La entrada `node_type: "mcp"` no aporta línea porque es
un **servidor**, no una tool (y como `name` es opcional ahí, aportaría una línea sin nombre que el
modelo no puede accionar). Y las tools que el servidor expone tampoco: quedan en `tools`, siempre
presentes, con el schema que publicó el servidor.

La razón es estructural. El catálogo es lo que `describe_tool` revela a demanda, y `describe_tool`
resuelve contra `lookup_for_describe`, que guarda `ToolConfiguration`s. Una tool MCP no tiene una: es
un `ToolDefinition` que llegó del servidor. Catalogarla la escondería de `tools[]` hasta un
descubrimiento que no puede ocurrir, y en un grafo cuyas únicas tools son MCP el modelo recibiría un
`describe_tool` sin handler detrás.

**El costo:** un servidor que expone muchas tools manda todos sus schemas cada turno, que es
justamente lo que lazy existe para evitar (el tope es 64 tools por servidor, hasta 32 KB de schema cada
una). Volverlas lazy de verdad exige enseñarle a `describe_tool` a responder desde un
`ToolDefinition`; es un cambio aparte.

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

## Relación con skills

Lazy tool loading y skills son features independientes. Para cómo cargar skills (vía `skills_path` / `skills_paths` en el `llm_call` o vía el tool sintético `load_skill({name, reference?})`), ver [24_skills.md](24_skills.md).

- Spec completo: [docs/superpowers/specs/2026-05-03-lazy-tool-loading-design.md](../superpowers/specs/2026-05-03-lazy-tool-loading-design.md)
