# 52. Observabilidad del módulo MCP — eventos `colmena::mcp`

> PR1 de una cadena de 3. Documenta los eventos `tracing` que hoy existen bajo
> el target `colmena::mcp` — dos nuevos en la ruta de fetch/connect
> ([`wire.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs))
> y dos retrofiteados en [`llm.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)
> para llevar el mismo campo `event`. Sigue el contrato general de
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
- **Nunca** el contenido de los argumentos de una llamada.
- **Nunca** la URL completa de un servidor (puede llevar query string) —
  solo `host[:port]`, vía el helper `host_of` en `wire.rs`.

## Tabla de eventos

| Evento | Nivel | Campos | Cuándo |
|---|---|---|---|
| `mcp.server_unavailable` | WARN | `alias`, `host`, `reason` (`prepare` \| `tools_list` \| `no_result`), `ms` (ausente para `no_result`) | Un servidor MCP declarado no aportó tools este turno. `prepare`/`tools_list` vienen del fan-out real (falló el bind o `tools/list`) y siempre llevan `ms`. `no_result` es el caso degenerado en el que `assemble()` no encuentra ninguna entrada para un alias declarado — no hubo intento medible, así que no se inventa un `ms`. Uno por servidor caído. |
| `mcp.server_ready` | DEBUG | `alias`, `host`, `tools` (conteo expuesto tras dedupe), `ms` | Un servidor MCP respondió y sus tools quedaron expuestas al modelo. Uno por servidor sano. |
| `mcp.wiring_note` | WARN | (mensaje libre en `notes`, sin campos estructurados) | El catch-all legible de `wire()`/`assemble()`. `notes` tiene **dos orígenes**: un drop a nivel de tool (colisión de nombre, schema sobredimensionado) y **también** un fallo a nivel de servidor, que además ya se reportó de forma estructurada en `mcp.server_unavailable`. Uno por nota. |
| `mcp.tools_exposed` | INFO | `exposed`, `servers`, `unavailable` | Resumen de fin de turno: cuántas tools quedaron expuestas, cuántos servidores respondieron, cuántos alias quedaron fuera. Solo se emite si `exposed > 0`. |

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

- Los dos eventos de despacho en
  [`dag_tool_executor.rs`, función `execute_inner`](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)
  (`"an MCP tool call failed"` / `"dispatched an MCP tool call"`) todavía no
  llevan `event =` ni latencia — se retrofitean en una PR de seguimiento de
  esta misma cadena. (Citado por nombre de función, no por número de línea:
  el número se desplaza con cada cambio en el archivo.)
- Los eventos de acierto/fallo de caché del pool de conexiones MCP
  (`McpConnectionRegistry`) llegan en otra PR de seguimiento.

## Ver también

- [Logging y observabilidad — contrato general](./50_logging_and_observability.md)
- [Subagente/LLM como tools](./19_nested_agents_and_subgraphs.md)
