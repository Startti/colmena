# El alias de un servidor MCP sale de `name`

**Acción de ADP: ninguna.** El compilador de ADP del arco MCP vía B (Startti/adp#808, mergeado el
2026-09-24) ya compila cada `mcpServer` como una entrada de `tool_configurations`
indexada por id de nodo y con el nombre visible en `name`. Desde este cambio el motor lo
usa.

## Qué cambia

| | Antes | Después |
|---|---|---|
| Nombre que ve el modelo | `<id-de-nodo>__<tool>` | `<name>__<tool>` |
| `alias` en `mcp.server_ready` / `mcp.server_unavailable` | el id de nodo | el `name` |
| Aviso de servidor no disponible (system message) | nombra el id de nodo | nombra el `name` |
| `toolName` de `tool-input-start` en una llamada MCP | `<id-de-nodo>__<tool>` | `<name>__<tool>` |

Sin `name` (o con `name` en blanco) el alias sigue siendo la clave, como antes.

## El formato exacto del nombre expuesto

`normalize(alias, tool)` en `llm/domain/mcp.rs`:

1. `"{alias}__{tool}"`, con todo carácter fuera de `[A-Za-z0-9_-]` cambiado por `_`.
2. Si pasa de 64 caracteres: los primeros 55, `_`, y 8 hex de `sha256` del nombre
   normalizado.

`tool` es el nombre del servidor tal cual (los guiones se conservan). El tope de 16
caracteres de `mcpToolLabel` en ADP deja ~46 para el nombre del tercero.

## Colisiones

Dos entradas que resuelven al mismo alias no se pisan: la segunda (orden del documento)
cae a su clave —el id de nodo— y, si también está tomada, a `<clave>_2`. ADP ya
desambigua los nombres con `_2`, `_3` en `toolNameForNode`, así que esto no debería
verse; si se ve, el modelo recibe `<id-de-nodo>__<tool>` para esa entrada y el log trae un
WARN `mcp.alias_fallback` con `key`, `wanted` y `alias`.
