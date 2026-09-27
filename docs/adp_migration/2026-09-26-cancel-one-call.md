# Cancelar una llamada sola

**Acción de ADP:** subir el pin; el worker cambia `execute_stream_cancellable` por
`execute_stream_controlled` y le pasa cada pedido de «cortá esta llamada» a
`RunControl::cancel_call`. Si no lo hace no se rompe nada: sin `RunControl` todo corre como
hoy.

## Qué cambia

- `ColmenaEngine::execute_stream_controlled(graph, resume_session_id, resume_answer,
  include_extra_info, path_prefix, agent_session_id, control)`. `RunControl::new(token)`
  envuelve el token del turno. `control.cancel_call(tool_call_id)` corta una llamada y
  `control.cancel_token().cancel()`, el turno, como hoy.
- La llamada cortada se contesta con «La persona canceló este agente antes de que
  terminara.». Si era un `subgraph` usado como tool (Run My Agent), la fila del hijo en
  `dag_runs` queda `CANCELLED`, y su frontera (`subgraph-node-end`) y su nodo en curso cierran
  con `status: "error"` y un `errorText` que empieza con `CANCELLED_BY_PERSON`. Las demás
  llamadas del turno siguen, y el agente que llamó sigue con su turno.
- Cancelar una llamada que ya terminó, que está en pausa por una pregunta o que no existe no
  hace nada. El Stop del turno no cambia: ninguna llamada se contesta y ningún hijo cierra
  nada con `CANCELLED_BY_PERSON`.
- Rust: `SubGraphExecutorPort::run_subgraph` gana un último parámetro
  `cancel: Option<CancellationToken>`, y `DagError` gana `Cancelled`. ADP no usa ninguno de
  los dos (`git grep SubGraphExecutorPort apps/` y `git grep 'DagError::' apps/` dan cero).

## Qué hace el worker

Un solo suscriptor por job escucha el cancel del turno y el de una llamada, y lee las marcas
durables DESPUÉS de suscribirse, para no perder un pedido que llegó antes. Con cada pedido de
una llamada llama `control.cancel_call(id)`.

Detalle: [12 → Cancelar una llamada sola](../developer_guide/12_dag_engine_guide.md#cancelar-una-llamada-sola-execute_stream_controlled)
y [19 → Cancelar una llamada sola](../developer_guide/19_nested_agents_and_subgraphs.md#cancelar-una-llamada-sola).
