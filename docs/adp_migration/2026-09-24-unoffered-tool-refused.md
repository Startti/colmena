# El modelo solo corre las tools que el request le ofreció

**Acción de ADP: ninguna de código; subir el motor.**

## Qué cambia

Hasta ahora, el motor corría cualquier nombre que el modelo emitiera como tool call, aunque el
agente no lo expusiera: todo tipo de nodo registrado (`python_script` sin sandbox por defecto y
con el entorno del worker, `http_request`, `sql_query`…) y las tools sintéticas que el executor
despacha por nombre (`gsheets_*`, `gdocs_*`, `data_run_python`, `api_explorer__*`). Ningún adapter
de proveedor compara el nombre devuelto con las tools declaradas, así que una inyección de prompt
que nombrara `python_script` ejecutaba código en el worker. Ahora el loop del agente corre una
llamada solo si su nombre está en la lista que ese mismo request le mandó al proveedor.

## ¿ADP depende de llamar tipos no expuestos?

No, visto desde el motor. ADP pliega cada tool en `tool_configurations` (clave cuid, `name`
semántico) y el modelo recibe esas tools por `name` (`filter_enabled_tools`, regresión
`folded_tool_shadowing_builtin_dedups_to_single_config_wins`); "Run My Agent" y los sub-agentes son
`subgraph` declarados; `recall_history`, `load_attachment`, `load_skill`, los paquetes `gsheets` /
`gdocs` y MCP (`<name>__<tool>`) entran en la misma lista antes de mandarse. El creador usa
`lazy_tool_loading`: sus reglas no cambian (una tool del catálogo aún no cargada devuelve su schema
y `describe_tool` sigue respondiendo). No se verificó desde este repo ningún grafo compilado por ADP.

## Qué ve ADP

- SSE: ningún frame nuevo. La llamada rechazada se ve como cualquier tool: `tool-input-available`
  con el nombre y los argumentos del modelo, y `tool-output-available` con
  `Error executing tool: Tool not found: <nombre>` — el mismo texto que un nombre inexistente.
- Log del worker: WARN `tool.not_offered` con el nombre y el `tool_call_id`, nunca los argumentos.
