# 52. Observabilidad del módulo MCP — eventos `colmena::mcp`

> PR2 de una cadena de 3. PR1 documentó los eventos `tracing` de la ruta de
> fetch/connect ([`wire.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs)).
> Esta PR cierra la ruta de despacho: los dos eventos de
> [`execute_inner`](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)
> ahora llevan `event =` y latencia (`ms`), y se agrega un tercer evento nuevo
> en [`call_and_contain`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/dispatch.rs)
> para la refusión de secure values. Sigue el contrato general de
> [`50_logging_and_observability.md`](./50_logging_and_observability.md): la
> librería emite, la aplicación decide el filtro.

## Convención

Todo evento de este módulo sigue el mismo molde:

```rust
tracing::<nivel>!(
    target: "colmena::mcp",
    event = "mcp.<acción>",
    <campo> = <valor>,
    ...,
    "mensaje humano breve"
);
```

`event` es siempre un string `"mcp.<acción>"`, estable — un dashboard o una
alerta puede indexar por él sin depender del texto del mensaje, que es libre
y puede cambiar.

### Qué nunca se loguea (privacidad)

- **Nunca** el valor de un header (credenciales, tokens) — solo sus nombres,
  y ni eso en estos eventos.
- **Nunca** el cuerpo de un resultado de tool.
- **Nunca** el VALOR de los argumentos de una llamada. La única excepción es
  el campo `path` de `mcp.dispatch_refused_secret`: nombra la UBICACIÓN del
  argumento ofensivo (nombres de campo / índices, tipo `arguments.token` o
  `items[2].secret`), nunca el valor que llevaba.
- **Nunca** la URL completa de un servidor (puede llevar query string) —
  solo `host[:port]`, vía el helper `host_of` en `wire.rs`.

## Tabla de eventos

| Evento | Nivel | Campos | Cuándo |
|---|---|---|---|
| `mcp.server_unavailable` | WARN | `alias`, `host`, `reason` (`prepare` \| `tools_list` \| `no_result`), `ms` (ausente para `no_result`) | Un servidor MCP declarado no aportó tools este turno. `prepare`/`tools_list` vienen del fan-out real (falló el bind o `tools/list`) y siempre llevan `ms`. `no_result` es el caso degenerado en el que `assemble()` no encuentra ninguna entrada para un alias declarado — no hubo intento medible, así que no se inventa un `ms`. Uno por servidor caído. |
| `mcp.server_ready` | DEBUG | `alias`, `host`, `tools` (conteo expuesto tras dedupe), `ms` | Un servidor MCP respondió y sus tools quedaron expuestas al modelo. Uno por servidor sano. |
| `mcp.wiring_note` | WARN | (mensaje libre en `notes`, sin campos estructurados) | El catch-all legible de `wire()`/`assemble()`. `notes` tiene **dos orígenes**: un drop a nivel de tool (colisión de nombre, schema sobredimensionado) y **también** un fallo a nivel de servidor, que además ya se reportó de forma estructurada en `mcp.server_unavailable`. Uno por nota. |
| `mcp.tools_exposed` | INFO | `exposed`, `servers`, `unavailable` | Resumen de fin de turno: cuántas tools quedaron expuestas, cuántos servidores respondieron, cuántos alias quedaron fuera. Solo se emite si `exposed > 0`. |
| `mcp.dispatch_failed` | WARN | `tool`, `tool_call_id`, `ms` | Una llamada a tool MCP falló — el servidor no fue alcanzable, respondió con error, o el fallo ocurrió antes de la red (ver `mcp.dispatch_refused_secret`). Emitido en `execute_inner`, uno por llamada fallida. |
| `mcp.dispatch_ok` | DEBUG | `tool`, `tool_call_id`, `bytes`, `ms` | Una llamada a tool MCP se despachó y el servidor contestó sin error. Emitido en `execute_inner`, uno por llamada exitosa. |
| `mcp.dispatch_refused_secret` | WARN | `alias`, `tool`, `path` | El motor bloqueó la llamada ANTES de tocar la red: los argumentos cargaban un handle de secure value resuelto por el engine. Emitido en `call_and_contain`. `path` nombra la ubicación del argumento ofensivo (nombres de campo / índices) — nunca el valor. Una llamada rechazada así también cuenta como `mcp.dispatch_failed` (el `failed: true` de `McpDispatched` llega hasta `execute_inner`), así que emite **dos** líneas. Para unirlas, usá el `tool_call_id` que lleva `mcp.dispatch_failed`: este evento **no** lo lleva todavía (enhebrarlo hasta `call_and_contain` obliga a tocar sus 12 call sites de test), así que con varias llamadas concurrentes a la misma tool la unión es por `tool` + tiempo. |

`reason` es una etiqueta estable (`FetchFailure::label()` en `wire.rs`), no el
texto de error — el string humano vive en la nota separada de `wiring.notes`,
que sale por `mcp.wiring_note`.

> **No cuentes fallos de servidor por `mcp.wiring_note`.** Un servidor caído
> emite **dos** líneas: la estructurada (`mcp.server_unavailable`, con `alias`,
> `host`, `reason` y `ms`) y la humana (`mcp.wiring_note`, con el texto). Para
> alertar o contar, usá `mcp.server_unavailable`; `mcp.wiring_note` es para leer,
> no para agregar. Separar los dos orígenes de `notes` en el tipo queda como
> trabajo aparte.

### Por qué `no_result` no lleva `ms`

`wire()` separa el fan-out (I/O concurrente, mide `ms` con
`std::time::Instant`) del fold (`assemble()`, puro, itera `specs.keys()` y
busca cada resultado en un `by_alias: HashMap`). `assemble()` es alcanzable de
forma independiente por sus propios tests, sin pasar por `wire()` — y ahí SÍ
puede recibir un vector de resultados parcial, con un alias declarado en
`specs` para el que jamás llegó una entrada. Cuando eso pasa no hubo ningún
intento cuya duración medir, así que el evento omite el campo `ms` en vez de
inventar un `0` que un dashboard leería como "resolvió instantáneamente".

## Pendiente (fuera de alcance de esta PR)

- Los eventos de acierto/fallo de caché del pool de conexiones MCP
  (`mcp.pool_hit` / `mcp.pool_miss` en `McpConnectionRegistry`) llegan en la
  PR3 de esta misma cadena.

## Ver también

- [Logging y observabilidad — contrato general](./50_logging_and_observability.md)
- [Subagente/LLM como tools](./19_nested_agents_and_subgraphs.md)
