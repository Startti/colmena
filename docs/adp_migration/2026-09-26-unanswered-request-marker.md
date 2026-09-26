# Tras un Stop, el mensaje siguiente no rehace el pedido detenido

**Acción de ADP: ninguna.** Subir el pin de Colmena. Sin cambios de API, de SSE ni de base.

## Qué cambia

Con Stop (`/chat/cancel`) antes de que el modelo conteste, el hilo del agente en
`llm_node_history` quedaba en el `user` del pedido detenido. El mensaje siguiente (por ejemplo,
uno en cola) llegaba al modelo pegado a ese, y el modelo hacía los dos: medido en dev, «decime
solo "recibido"» volvió a anotar las cuatro notas del pedido detenido. Pasa lo mismo tras un
turno que falla o que corta el watchdog antes de la primera respuesta.

Ahora la corrida siguiente guarda primero una fila `assistant` con este texto y después el
prompt nuevo:

> (Este pedido quedó sin respuesta: el turno se detuvo o falló antes de terminar. No lo retomes
> salvo que te lo vuelvan a pedir.)

## Qué ve ADP

- **Nada en el stream.** La fila no se emite como frame.
- **En `llm_node_history`,** esa fila `assistant` entre el pedido detenido y el siguiente. ADP
  no lee la tabla en runtime; los scripts que la leen (`capture-bench.ts`, los evals) la ven
  como una respuesta más. `recall_history` también la muestra.

Detalle: [19 → Un pedido sin respuesta](../developer_guide/19_nested_agents_and_subgraphs.md#un-pedido-sin-respuesta).
