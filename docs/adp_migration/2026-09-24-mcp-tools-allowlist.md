# `mcp.tools`: el motor expone solo las tools listadas

**Acción de ADP: ninguna obligatoria.** El compilador de ADP del arco MCP vía B (Startti/adp#808,
mergeado el 2026-09-24) ya emite `mcp.tools` (la lista del nodo, con el techo de
`allowedTools` aplicado en `aplicarTechoDeTools`). Hasta este cambio el motor lo
ignoraba y exponía el catálogo entero del servidor; ahora lo respeta.

## Qué cambia

| | Antes | Después |
|---|---|---|
| Tools que ve el modelo con `tools: ["a"]` | todas las del servidor | solo `<alias>__a` |
| El modelo llama una tool no listada | se despachaba al servidor | se rechaza como tool desconocida; nunca llega al servidor |
| Un nombre listado que el servidor no publica | nada | `mcp.wiring_note` en el log (el modelo no lo ve) |
| `tools` ausente, `null` o `[]` | todas | todas (sin cambio) |
| `tools` que no es lista de strings | se ignoraba | en la carga validada (`Graph::validate`, lint), el grafo falla; si llega en ejecución por `inputs.tool_configurations` (sin validar), ese servidor se descarta entero |

## Lo que ADP tiene que saber

- **Nombres del servidor, verbatim.** `tools` compara contra el `name` que devuelve
  `tools/list` (`resolve-library-id`), nunca contra el nombre expuesto
  (`<alias>__resolve-library-id`). Un nombre expuesto en la lista no matchea nada.
- **`[]` es "todas", no "ninguna".** El motor no tiene forma de expresar "ninguna tool
  de este servidor"; para eso, no se compila la entrada. `aplicarTechoDeTools` ya tira
  un error cuando la intersección queda vacía, en vez de emitir `[]`: mantenerlo así.
- **El tope de 64 tools por servidor corre después del filtro.** Una tool elegida que
  está más allá de la posición 64 del catálogo sigue exponiéndose.
- **Una lista guardada envejece.** DeepWiki renombró `ask_question` a
  `ask_wiki_question` entre el 2026-09-01 y el 2026-09-24. Un nodo que eligió
  `ask_question` ahora expone 0 tools de ese servidor: el operador lo ve como
  `mcp.wiring_note`, pero el modelo **no recibe aviso** — el aviso de "servidor no
  disponible" es solo para servidores que no contestaron.
- **Seguimiento (este cambio no lo toca):** ADP debería marcar, en el servidor y en el
  nodo, las tools elegidas que el último `tools/list` ya no trae.
