# Cambios de `level` y `path` en eventos anidados

**Acción de ADP: OBLIGATORIA.** Si el frontend arma el árbol agrupando por
`path`, hay que revisar ese agrupamiento.

## El problema

Cuando un `subgraph` se alcanza por una **frontera sintética** — un agente de
`orchestrator`, o una tool call — el loop que lo ejecuta estampa **su propio**
`node_id` en el linaje del hijo, no el de la frontera. Resultado: el frame de
frontera y el contenido que supuestamente delimita salían como **hermanos** bajo
el mismo padre, en vez de padre e hijo.

Concretamente, con un orchestrator despachando el agente `Test_Runner`:

```
level 2   path "top>orch>Test_Runner"    ← la etiqueta del sub-agente
level 2   path "top>orch>0e71a37d…"      ← el trabajo real del sub-agente
```

Dos ramas paralelas del mismo padre. Un árbol construido por prefijos de `path`
dibuja el trabajo **al lado** de su etiqueta, nunca adentro.

## Qué cambia

El contenido del hijo ahora anida bajo el nombre de la frontera:

```
level 2   path "top>orch>Test_Runner"                ← la etiqueta
level 3   path "top>orch>Test_Runner>0e71a37d…"      ← el trabajo, adentro
```

Aplica a **dos** rutas:

- **Agente de orchestrator** — la frontera es el nombre del agente
  (`Test_Runner`, `DAG_Builder`).
- **Subgrafo como tool** — la frontera es el nombre del tool que el modelo llamó
  (`Specs_Writer`, `Writer`, `Research`…). Ver también la
  [nota 4](2026-08-21-subgraph-tool-boundary-frames.md), que es la que hace
  aparecer esos frames de frontera.

## Qué NO cambia

La ruta **edge-based** — un nodo `subgraph` conectado por aristas en el grafo —
queda **exactamente igual**. Ahí el nombre de la frontera ya *es* el node id que
el loop antepone, así que agregar el scope habría duplicado el segmento
(`sub>sub>hijo`) y le habría dado un nivel fantasma a todos los runs existentes.
La exclusión es deliberada y está cubierta por test.

## Impacto concreto en números

Para los agentes que corren hoy en el creador:

| Rama | `level` antes | `level` después |
|------|---------------|-----------------|
| Los 7 roles simples (Specs_Writer, Writer, Research, Reviewer, Resources, Plans_Writer, Prompt_Engineer) | 1 | 2 |
| `orchestrator` dentro de Testing / Implementation | 1 | 1 |
| planner / critic / phase_reactor | 2 | 2 |
| `Test_Runner` / `DAG_Builder` (la etiqueta) | 2 | 2 |
| El `llm_call` real del sub-agente | 2 | 3 |

O sea: la profundidad máxima que ve ADP en el creador de agentes pasa de **2 a
3**, y los roles simples bajan un nivel.

## Qué tiene que hacer ADP

1. Revisar cualquier lógica que asuma una profundidad máxima o que compare
   `level` contra un número fijo.
2. Revisar el agrupamiento del árbol: ahora el `path` de un hijo **sí** contiene
   el segmento de su frontera, que es lo que hacía falta para colgarlo bien. Si
   había algún workaround para emparejar etiqueta y contenido, probablemente
   sobre.
3. Verificar que el render degrade bien a profundidades mayores (la anidación ya
   no tiene tope — ver la [nota 3](2026-08-21-unbounded-subgraph-nesting.md)). Si
   la UI tiene una indentación por nivel, conviene un tope visual.

## Riesgo

**Medio.** Ningún evento se pierde y ningún tipo cambia: lo único que se mueve
son `level` y `path`. El peor caso realista es que algo se dibuje a la
profundidad equivocada, no que desaparezca. Pero si hay algún `level === 1`
literal en el frontend, ese sí falla en silencio.

## Cómo verificar

Correr el agente creador con una tarea que dispare un rol simple (por ejemplo
Specs_Writer). En el stream, los frames del `llm_call` hijo deben traer
`level: 2` y un `path` que contenga `>Specs_Writer>`.
