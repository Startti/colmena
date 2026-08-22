# `llm_call` usado como tool ahora es un nivel de anidación real

**Acción de ADP: CONDICIONAL.** Solo aplica si algún grafo persistido expone un
`llm_call` directamente en `tool_configurations`. **Conviene verificarlo antes de
subir la dependencia.**

## Qué pasaba

Había **dos** formas de anidar un agente dentro de otro, y se comportaban de
forma completamente distinta:

| | `subgraph` como tool | `llm_call` como tool |
|---|---|---|
| Nivel SSE | `level ≥ 1`, con node_id propio | `level 0`, con el node_id **del padre** |
| Contador de profundidad | `+1` | **sin incremento** |

La causa: `DagToolExecutor` le pasa al nodo el observer **del agente que llama**.
Un `subgraph` lo usa como destino para reenviar los eventos de su hijo,
marcándolos como eventos de un nivel inferior. Un `llm_call` lo usaba como su
propio observer y emitía sus eventos a secas — así que el loop los estampaba con
el node_id del llm padre.

Consecuencia: un agente anidado por esta vía **hablaba con la voz de su padre**.
Sus tokens, sus tool calls y su uso de tokens salían todos atribuidos al agente
que lo llamó, a nivel 0, entremezclados con los del padre. Y no tenía nodo propio
en el árbol.

## Qué cambia

Un `llm_call` despachado como tool ahora recibe un observer que re-parenta sus
eventos un nivel abajo, bajo el nombre del tool. Queda con la misma forma que ya
produce un `subgraph` como tool.

Además incrementa el contador de profundidad, así que una cadena de agentes
llm-como-tool ahora reporta niveles crecientes en vez de repetir el mismo.

### Antes

```json
{ "type": "text-delta", "id": "txt_…", "delta": "…",
  "level": 0, "path": "agente_padre" }
```

### Después

```json
{ "type": "subgraph-text-delta", "id": "txt_…", "delta": "…",
  "level": 1, "path": "agente_padre>Nombre_Del_Tool" }
```

## Qué tiene que hacer ADP

1. **Verificar si alguien usa este patrón.** Buscar en los grafos persistidos
   entradas de `tool_configurations` con `"node_type": "llm_call"`. Si no hay
   ninguna, este cambio no afecta a nadie y es puro endurecimiento.
2. Si hay alguno: esos eventos se mueven de nivel 0 a nivel 1 y cambian de
   `text-delta` a `subgraph-text-delta`. El frontend ya sabe renderizar
   `subgraph-*`, así que debería absorberlo, pero conviene mirarlo.

## Riesgo

**Medio si el patrón está en uso, bajo si no.** El cambio es correcto en
cualquier caso — hoy un agente anidado se hace pasar por su padre — pero mueve
eventos que alguien podría estar consumiendo a nivel 0.

`subgraph` explícitamente **no** recibe este tratamiento: re-parenta sus propios
eventos y quedaría envuelto dos veces.

## Nota sobre el tope

Este defecto originalmente incluía "anidación ilimitada" como problema. Ya no lo
es: la anidación es ilimitada **por diseño** desde este mismo lote de cambios
(ver la [nota 3](2026-08-21-unbounded-subgraph-nesting.md)). Lo que se arregla
acá es que el contador refleje la realidad, para que el techo opcional y el nivel
reportado sean correctos.
