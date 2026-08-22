# Frames de frontera para subgrafos usados como tool

**Acción de ADP: OPCIONAL.** El cambio es puramente aditivo: aparecen frames
nuevos donde antes no había ninguno. Ignorarlos no rompe nada.

## Qué pasaba

Cuando un `subgraph` corre, emite un par de frames sintéticos que delimitan su
sub-árbol: `subgraph-node-start` al abrir y `subgraph-node-end` al cerrar.
Necesita un nombre para esos frames, y lo buscaba en dos lugares:

1. `__agent_name` — lo inyecta el `orchestrator` cuando el subgrafo corre como
   agente con nombre.
2. `__node_id` — lo inyecta el loop de ejecución del grafo.

El problema: **un `subgraph` despachado como tool no pasa por el loop del
grafo**, así que nunca tenía `__node_id`. Y como tampoco es un agente de
orchestrator, tampoco tenía `__agent_name`. Los dos caminos daban `None`, así que
**no se emitía ninguna frontera**.

El fallback a `__node_id` estaba escrito explícitamente para cubrir este caso —
el comentario en el código lo decía — pero era código muerto: nada poblaba esa
clave en la ruta tool.

## Qué cambia

`DagToolExecutor` ahora inyecta el nombre visible del tool que el modelo llamó, y
`SubGraphNode` lo usa como tercer fallback. La cadena queda:

1. `__agent_name` (agente de orchestrator)
2. `__node_id` (ruta edge-based)
3. `__colmena_tool_name` (ruta tool) ← **nuevo**

Resultado: los 9 roles del creador de agentes (`Specs_Writer`, `Plans_Writer`,
`Prompt_Engineer`, `Writer`, `Research`, `Reviewer`, `Resources`, `Testing`,
`Implementation`) ahora **sí** emiten frontera, nombrada como el tool.

### Lo que aparece nuevo

```json
{ "type": "subgraph-node-start", "node_id": "Specs_Writer",
  "node_type": "subgraph", "level": 1, "path": "…>Specs_Writer" }

{ "type": "subgraph-node-end", "node_id": "Specs_Writer",
  "node_type": "subgraph", "output": { … }, "level": 1, "path": "…>Specs_Writer" }
```

## Qué tiene que hacer ADP

Nada obligatorio. Si el frontend hoy infiere el agrupamiento de un rol a partir
del `tool-input-available` / `tool-output-available` de nivel 0, ahora tiene un
delimitador explícito y puede simplificar esa lógica.

El nombre del frame es el **nombre visible del tool**, o sea lo mismo que llega
en `toolName` del `tool-input-available` correspondiente. Se pueden correlacionar
por ahí.

## Riesgo

**Bajo.** Aditivo: dos frames más por cada invocación de subgrafo como tool.

El único cuidado que hubo que tener es que la clave nueva
(`__colmena_tool_name`) lleva prefijo `__colmena_` a propósito: los nodos que
reenvían inputs desconocidos hacia afuera — `http_request` en particular —
filtran las claves internas del motor **por ese prefijo**. Es exactamente la
trampa en la que cayó `__colmena_subgraph_depth` en su momento, cuando se coló
como query param y rompió contra una API que validaba parámetros. Con el prefijo,
no puede pasar.

## Cómo verificar

Correr el agente creador y pedirle algo que dispare un rol. En el stream tiene
que aparecer un `subgraph-node-start` con `node_id` igual al nombre del rol,
justo antes de los eventos del grafo hijo.
