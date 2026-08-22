# La anidación de subgrafos ya no tiene tope

**Acción de ADP: REVISIÓN.** Hay que decidir conscientemente si se configura un
techo en producción.

## Qué cambia

Se eliminó el límite de 5 niveles de anidación de subgrafos. Antes, un
`subgraph` a profundidad 5 fallaba con:

```
Subgraph-as-tool nesting exceeded MAX_SUBGRAPH_TOOL_DEPTH (5).
```

Ahora **no hay límite por defecto**. Un agente puede componer agentes que
componen agentes tan profundo como el autor del grafo quiera.

La constante pública `SubGraphNode::MAX_SUBGRAPH_TOOL_DEPTH` fue **eliminada**.
Es un cambio de API de Rust: si algún código del worker la referenciaba, deja de
compilar. Una búsqueda en colmena solo la encontró en comentarios, pero conviene
que el equipo confirme del lado de ADP antes de subir la dependencia.

## La válvula opcional

Queda un techo **opcional y apagado por defecto**, por variable de entorno:

```
COLMENA_MAX_SUBGRAPH_DEPTH=<n>
```

Semántica:

- Sin definir, vacía, no parseable, o `0` → **sin límite** (el default).
- `n > 0` → un `subgraph` a profundidad `n` o mayor falla.

El `0` se trata como "sin límite" a propósito, no como "rechazar todo": así un
`=0` accidental en un script de deploy no deja fuera de servicio a todos los
subgrafos del ambiente.

La variable se lee una sola vez por proceso y se cachea, así que cambiarla exige
reiniciar el servicio.

## El riesgo que hay que asumir conscientemente

El límite de 5 existía por una razón: acotar la recursión desbocada. Sin tope,
un grafo mal configurado —un subgraph que se referencia a sí mismo, o un ciclo
de agentes A→B→A— **recursa hasta agotar el proceso**. Las consecuencias son
reales:

- **Costo**: cada nivel son llamadas LLM de verdad. Un ciclo factura hasta que
  algo lo mate.
- **Memoria**: cada nivel es un run con su propio estado. El worker de ADP ya
  tiene margen ajustado de memoria; una recursión profunda es una vía directa a
  OOM.

Colmena ya **no** protege contra esto por defecto. La decisión de si eso es
aceptable, o si conviene poner `COLMENA_MAX_SUBGRAPH_DEPTH` en un valor alto
pero finito (por ejemplo 25 o 50, muy por encima de cualquier composición real y
muy por debajo de lo que tumba el worker), es de operaciones.

**Recomendación**: encender la válvula en un valor generoso. Cuesta una línea en
`deploy_gcp.sh` y convierte un incidente de agotamiento en un error de tool
legible.

Si se configura, ver el
[código de error estructurado](2026-08-21-subgraph-depth-error-code.md) para
detectarlo desde el frontend.

## Un límite que SÍ sigue existiendo: el JSON inline

Anidar con `child_graph_inline` mete el grafo hijo **dentro del documento del
padre**. Cada nivel agrega varias capas de anidación JSON, y el deserializador
tiene su propio tope de recursión: alrededor de **30 niveles inline** el parseo
falla con `recursion limit exceeded` antes de que el grafo llegue a ejecutarse.

Es un límite del **documento**, no de la ejecución, y es anterior a este cambio.
No aplica a las otras formas de anidar — `child_graph_path`, assets publicados,
subgrafo-como-tool — donde cada documento queda plano.

Verificado: 50 niveles vía `child_graph_path` corren sin problema; 50 niveles
inline ni siquiera parsean.

Relevante para ADP porque el canvas persiste los grafos **inline**. Una
composición que supere ~30 niveles en un solo documento va a fallar al parsear,
con un error que no menciona profundidad. No es un escenario realista hoy, pero
si alguna vez aparece, el diagnóstico es ese y la salida es partir el grafo.

## Efecto colateral en el stream

Con anidación ilimitada, `level` puede crecer sin cota. Si la UI indenta por
nivel, conviene un tope visual. Ver la
[nota 2](2026-08-21-nested-level-and-path-changes.md).

## Qué NO cambia

- El contador `__colmena_subgraph_depth` sigue existiendo y sigue incrementando;
  ahora alimenta la válvula opcional y reporta el nivel de anidación.
- Los límites que ya existían y son independientes de esto siguen vigentes:
  `maxSteps` del loop de tools, `max_phases` del orchestrator, y los límites de
  llamadas por nodo del grafo.
