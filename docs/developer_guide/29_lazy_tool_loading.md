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

## Edge cases conocidos

- **LLM emite describe_tool y el tool real en el mismo turno**: algunos modelos pueden emitir tool calls paralelos. Si el LLM intenta llamar `X` en el mismo turno que llamó `describe_tool("X")`, el provider rechaza la segunda call (porque `X` no estaba en `tools[]` ese turno). El turno siguiente sí la verá tipada. La descripción del tool sintético dice explícitamente "Call it directly on your next turn" para reforzar el comportamiento. Es raro en práctica.
- **Truncation agresiva**: si el rolling window dropea TANTO el `describe_tool` como cualquier llamada directa a una tool, la tool sale del `discovered_set` y el LLM tiene que re-descubrirla. Es comportamiento natural — no estás "olvidando" tools que el LLM nunca volverá a usar.

## Trust model

Mismo posture que skills: el engine valida estructura (`summary` length, schema válido) pero no contenido semántico. Un `summary` redactado para inducir prompt injection es responsabilidad de quien configura la tool. El catálogo se fija al cargar el grafo — el LLM no puede añadir tools nuevas en runtime.

## Referencia rápida

- Tool sintético: `describe_tool(name: string)`.
- La descripción del tool contiene el catálogo completo (nombre + summary de cada tool no-eager no-descubierta).
- Si no se configura `lazy_tool_loading`, la feature está completamente deshabilitada (zero overhead).
- Spec completo: [docs/superpowers/specs/2026-05-03-lazy-tool-loading-design.md](../superpowers/specs/2026-05-03-lazy-tool-loading-design.md)
