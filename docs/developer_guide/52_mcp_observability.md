# 52. Observabilidad del módulo MCP — eventos `colmena::mcp`

> Referencia de los eventos `tracing` que emite el módulo MCP, en las tres rutas
> donde puede degradarse: el fetch/connect por servidor
> ([`wire.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/wire.rs)),
> el despacho de una llamada
> ([`execute_inner`](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs)
> y [`call_and_contain`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mcp/dispatch.rs)),
> y el pool de conexiones y catálogos
> ([`McpConnectionRegistry`](../../src/libs/colmena/src/dag_engine/infrastructure/mcp_registry/registry.rs)).
> Sigue el contrato general de
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
| `mcp.dispatch_refused_secret` | WARN | `alias`, `tool`, `path` | El motor bloqueó la llamada ANTES de tocar la red: los argumentos cargaban un handle de secure value resuelto por el engine. Emitido en `call_and_contain`. `path` nombra la ubicación del argumento ofensivo (nombres de campo / índices) — nunca el valor. Una llamada rechazada así también cuenta como `mcp.dispatch_failed` (el `failed: true` de `McpDispatched` llega hasta `execute_inner`), así que emite **dos** líneas. Para unirlas, usá el `tool_call_id` que lleva `mcp.dispatch_failed`: este evento **no** lo lleva todavía (enhebrarlo hasta `call_and_contain` obliga a tocar todos sus call sites de test), así que con varias llamadas concurrentes a la misma tool la unión es por `tool` + tiempo. |
| `mcp.catalog_hit` | DEBUG | `key`, `tools`, `raced` | El catálogo de `tools/list` se sirvió de caché, dentro de su `cache_ttl`. `raced: true` es el re-chequeo bajo el fetch lock: otra tarea llenó la entrada mientras esperábamos. |
| `mcp.catalog_miss` | DEBUG | `key`, `tools` | La caché estaba vacía o vencida: se fue al servidor y el resultado quedó cacheado. Se emite DESPUÉS del fetch, así que el conteo es el real y un fetch fallido no aparece como miss (ese camino sale por `mcp.server_unavailable`). |
| `mcp.connection_reused` | DEBUG | `key`, `raced` | Se reusó una conexión ya en el pool. `raced: true` es el re-chequeo bajo el creation lock. |
| `mcp.connection_opened` | DEBUG | `key`, `pooled` (tamaño del pool tras insertar) | Se abrió un handshake nuevo y quedó en el pool. |
| `mcp.connection_cooldown` | DEBUG | `key`, `since_ms`, `window_ms` | **El intento se SALTEÓ**: el servidor falló hace poco y otro intento costaría hasta su `timeout` antes de que corra el modelo, así que el llamador degrada de inmediato. Sin esta línea, un servidor en cooldown se cae de todos los turnos sin nada en el log que lo explique. |
| `mcp.pool_evicted` | DEBUG | `key`, `reason` | El LRU soltó una conexión pooleada y todo su footprint (cliente + catálogo cacheado). `reason` es una etiqueta estable: `lru_capacity` (se superó `COLMENA_MAX_POOLED_MCP_SERVERS`) o `lru_capacity_displaced` (la clave fue desplazada al reinsertarse). La depuración de locks huérfanos (`sweep_orphan_locks`) NO emite este evento: solo suelta locks, nunca una conexión. |

El registry tiene **dos cachés distintas**, y conviene no confundirlas al leer los
logs: la **caché del catálogo** (`catalog_*`) guarda el resultado de `tools/list`
por `cache_ttl` y expira por tiempo; el **pool de conexiones** (`connection_*`,
`pool_evicted`) guarda el cliente vivo y se desaloja por LRU. Un `catalog_miss` no
implica abrir conexión —puede reusar una pooleada—, y un `connection_opened` no
implica refrescar el catálogo.

`key` es el digest SHA-256 del `McpServerKey`: se deriva de la URL, el transporte,
los NOMBRES de header y un fingerprint salado de los valores resueltos. Es seguro
de imprimir y **no** lleva la URL ni credencial alguna; tampoco lleva el alias, así
que para correlacionar con `alias`/`host` usá los eventos de `wire.rs`.

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

## Atribuir el costo en tokens de un servidor MCP

Una pregunta que aparece apenas MCP entra en producción: *¿cuánto me está costando
el servidor X?* Se responde con lo que estos eventos ya emiten, sin instrumentación
adicional.

**Lo primero, para sacarlo del medio: el costo SÍ se cobra.** El resultado de una
tool MCP entra al historial como mensaje `Tool` y por lo tanto aparece en los
`promptTokens` del turno siguiente. Nunca fue costo perdido; lo que faltaba era
saber *de quién* era.

**La atribución es exacta, no una estimación.** El campo `bytes` de
`mcp.dispatch_ok` es `dispatched.output.len()`, y ese `output` viaja **verbatim** al
mensaje `Tool` (`agent_service.rs`, la construcción de `LlmMessage::tool` en el
camino normal de éxito). Los bytes logueados **son** los bytes que entran al
contexto — no hay resumen ni digest en el medio.

Y como el campo `tool` lleva el nombre expuesto, que es `<alias>__<tool>`, el alias
del servidor viene de prefijo. Agrupando por ese prefijo se obtiene el aporte de
cada servidor:

```
# bytes de contexto aportados por servidor, en una ventana de logs
event="mcp.dispatch_ok" | parse tool as "<alias>__*" | stats sum(bytes) by alias
```

**El límite, explícito.** Lo anterior da **bytes**, no tokens. La conversión depende
del tokenizador del proveedor y del contenido (~4 bytes por token para prosa en
inglés es la regla de bolsillo habitual, pero JSON denso rinde distinto). Colmena no
tokeniza el resultado por su cuenta: reporta `promptTokens` del turno completo, sin
desglose por origen. Si algún día hace falta un número exacto en tokens, habría que
tokenizar el resultado contenido en el momento del dispatch — un costo de CPU por
llamada que hoy no se paga porque la aproximación en bytes alcanza para decidir.

## Postura de seguridad: cualquier URL, sin allowlist

Un servidor MCP es un tercero que **escribe las descripciones de tools que tu modelo
lee** y puede cambiarlas entre turnos. La postura actual es deliberada y conviene
tenerla escrita, porque define qué cubre esta instrumentación y qué no.

**Lo que está controlado:**

- La **respuesta** del servidor llega envuelta en un delimitador de contenido no
  confiable con un nonce por llamada, y anunciada como datos y nunca instrucciones.
  El cuerpo tiene tope (32 KB) y el schema también (32 KB, máximo 64 tools).
- Un argumento que lleve un handle de secure value resuelto por el engine se
  **rechaza antes de la red** (`mcp.dispatch_refused_secret`). Un secreto que Colmena
  resolvió nunca se reenvía.
- Las credenciales de conexión viajan por `headers` con referencias a secure values,
  resueltas al conectar, nunca en la clave del pool ni en el log.

**Lo que NO está controlado, y hay que decirlo:**

- **No hay allowlist de hosts.** Cualquier URL HTTPS declarada en el grafo es
  alcanzable, incluidos endpoints internos. Es superficie SSRF: la decide quien
  escribe el grafo.
- **Nada inspecciona los argumentos salientes** más allá del rechazo de secure
  values. Si el modelo decide mandarle a un tercero algo que tenía en contexto, sale.
  Es el problema del *confused deputy*, y sigue abierto.

Habilitar un servidor MCP equivale a confiarle a ese tercero lo que el modelo tenga
en contexto. El destino, eso sí, **no lo elige el modelo**: la URL la escribe el
operador y se valida al cargar el grafo.

## Pendiente

La cadena de observabilidad MCP está cerrada. Quedan estos follow-ups:

- **Seis eventos del crate no llevan el target que dicen llevar.** Usan
  `target = "..."` (con IGUAL, que crea un campo llamado `target`) en vez de
  `target: "..."` (la directiva del macro), así que su target real es el path del
  módulo y filtrar por el nombre que declaran no los encuentra. Son 3 en
  `dag_engine/engine.rs` y 3 en `infrastructure/pool_registry/registry.rs`. El de
  MCP ya se arregló acá.
- **`mcp.dispatch_failed` no distingue un timeout de un error del servidor**: el
  `McpError` se absorbe en el texto que ve el modelo y nunca llega al log. Necesita
  un campo `kind` en `McpDispatched`.
- **`mcp.dispatch_refused_secret` no lleva `tool_call_id`** mientras su hermano sí;
  las tool calls de un turno corren concurrentes, así que unir por `tool` + tiempo
  puede fallar.

> Nota sobre nombres: versiones previas de este documento anunciaban
> `mcp.pool_hit` / `mcp.pool_miss`. Salieron con nombres más precisos porque el
> registry no tiene una caché sino dos — de ahí `catalog_*` y `connection_*`.

## Ver también

- [Logging y observabilidad — contrato general](./50_logging_and_observability.md)
- [Subagente/LLM como tools](./19_nested_agents_and_subgraphs.md)
