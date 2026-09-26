# Tras un Stop antes de la primera respuesta del modelo, el mensaje siguiente no rehace el pedido

**Acción de ADP: ninguna.** Subir el pin de Colmena. Sin cambios de API, de SSE ni de base.

## Qué cambia

Con Stop (`/chat/cancel`) antes de que el modelo conteste (o un error, o el watchdog), el hilo
en `llm_node_history` quedaba en el `user` del pedido detenido, y el mensaje siguiente llegaba
pegado a ese: medido en dev, «decime solo "recibido"» volvió a anotar las cuatro notas del pedido
detenido. Ahora la corrida siguiente guarda antes una fila `assistant` con este texto:

> (Este pedido quedó sin respuesta: el turno se detuvo o falló antes de terminar. Retomalo solo
> si el mensaje siguiente lo pide o se refiere a él.)

Así un Stop seguido de un ajuste («más corto», «en inglés») sigue funcionando. Un Stop durante
una tool (lo más común en el agente principal) lo cubre la curación de llamadas abandonadas
(v0.20), cuyo texto ahora dice lo mismo («…Retomala solo si el mensaje siguiente lo pide o se
refiere a ella.»); decía «No la retomes; si todavía hace falta, volvé a hacerla.».

## Qué ve ADP

- **Nada en el stream.** La fila no se emite como frame.
- **En `llm_node_history`,** esa fila `assistant` entre el pedido detenido y el siguiente. ADP
  no lee la tabla en runtime; los scripts que la leen (`capture-bench.ts`, los evals) la ven
  como una respuesta más. `recall_history` también la muestra.

Detalle: [19 → Un pedido sin respuesta](../developer_guide/19_nested_agents_and_subgraphs.md#un-pedido-sin-respuesta).
