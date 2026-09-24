# El cliente MCP del motor no marca direcciones que no sean públicas

**Acción de ADP:** ninguna. No fijar `COLMENA_MCP_ALLOW_PRIVATE_HOSTS` en producción: apaga la guarda.

## Qué cambia

El motor rechaza conectar un servidor MCP cuya dirección no sea unicast global, con la tabla de
`isGlobalUnicast` (`safe-fetch.ts`). ADP valida al registrar; el motor decide al marcar, sobre la
dirección del socket: cierra el rebinding de DNS y cubre las entradas escritas a mano o importadas.

## Qué ve ADP

- Un servidor en una dirección privada, loopback, link-local o de metadatos cae como uno caído: su
  alias queda sin tools y el turno sigue. Al cablear, el modelo solo recibe «did not respond» y
  `destination is not a public address` va al log (`mcp.wiring_note`); lo lee solo si lo rechaza
  una reconexión en el dispatch. La dirección resuelta queda solo en `mcp.dial_refused`.
- El cliente MCP ignora `HTTP(S)_PROXY`/`ALL_PROXY`. Ningún cambio de SSE ni de API pública.
- Desarrollo local contra `localhost`: `COLMENA_MCP_ALLOW_PRIVATE_HOSTS=1` (o `true`) al arrancar el worker.
