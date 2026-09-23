# Una tool-subgrafo reanudada devuelve su salida, no el estado del hijo

**Acción de ADP: ninguna**, salvo re-medir el banco del creador (ver abajo).

## Qué pasaba / qué cambia

Un `subgraph` usado como tool (`tool_configurations.<tool>.node_type: "subgraph"`,
o cualquier subgrafo-como-tool, no solo `child_graph_ref`) que se suspendía y
después se reanudaba devolvía el `result` crudo del hijo — el mapa completo de
todos sus nodos, `__colmena_session_id` incluido — en vez del valor del nodo
marcado `__colmena_is_output_node`. El camino fresco (child que no se suspende)
siempre extrajo ese nodo; el camino de resume no lo hacía. Para un hijo de un
solo nodo el bug era invisible (el nodo output y el estado completo coinciden);
para un hijo con más de un nodo — el caso típico, un `llm_call` seguido de
`output` — el modelo padre recibía, en el resultado de la tool, el output crudo
de cada nodo intermedio del hijo (potencialmente un `http_request` con datos
sensibles en su respuesta, o simplemente ruido que infla el contexto).

Ahora las dos rutas comparten `SubGraphNode::extract_final_output`, así que un
resume devuelve exactamente lo que devolvería una corrida fresca. Además, el
resultado de una tool reanudada ahora pasa por el mismo recorte de tamaño
(`scrub_tool_result_output`) que ya aplicaba una tool fresca — antes lo
saltaba, así que una respuesta de resume que colgaba una cadena grande (o
binaria) llegaba al modelo sin recortar.

## Qué NO cambió

- El contrato de la clave `__colmena_is_output_node` es el mismo — sigue siendo
  el flag que el nodo `output` estándar de Colmena pone en su `extra_info`, y
  sigue siendo lo que `docs/node_configurations.json` (`node_types.subgraph`) ya
  documentaba para el resultado de un `subgraph`. Ese doc no distinguía fresco
  de resume; ahora el código cumple esa distinción también en resume.
- Un hijo de un solo nodo (output = todo el estado) no cambia de comportamiento
  observable.
- SUSPENDED sigue burbujeando verbatim (sin extracción) cuando el hijo se
  suspende de nuevo — eso no lo toca este fix.

## Qué tiene que hacer ADP

Nada del lado del grafo o del canvas — es aditivo/correctivo y no cambia el
wire-format SSE. La única superficie donde esto es observable es el **resultado
de la tool** que el modelo padre recibe tras un resume, que ahora es más chico
y más limpio que antes (nunca al revés).

Si algún flujo de ADP (por ejemplo el banco del creador, o cualquier evaluación
automatizada) inspeccionaba ese resultado crudo esperando ver las claves
internas del hijo (`__colmena_session_id`, el output de un nodo intermedio),
deja de verlas. No se conoce ningún caso así, pero conviene re-medir el banco
del creador después de tomar este release por las dudas — es la superficie que
más invoca subgrafos-como-tool con HITL.

## Qué se rompe si se ignora

Nada. El cambio solo puede reducir lo que un resume expone; ningún consumidor
puede depender de recibir el estado completo del hijo porque el camino fresco
nunca lo ofreció.
