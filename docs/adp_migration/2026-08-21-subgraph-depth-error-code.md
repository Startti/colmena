# Código de error estructurado para el techo de profundidad

**Acción de ADP: OPCIONAL.** Solo importa si se configura
`COLMENA_MAX_SUBGRAPH_DEPTH`.

## Qué pasaba

Cuando se superaba el límite de anidación, el error llegaba al LLM que llamó como
un resultado de tool fallido cualquiera:

```
Error executing node subgraph: Subgraph-as-tool nesting exceeded
MAX_SUBGRAPH_TOOL_DEPTH (5). Check for a subgraph tool that references itself…
```

No había tipo de evento SSE propio, ni código, ni campo que dijera "esto fue el
límite de recursión". Para detectarlo había que hacer *substring matching* sobre
el nombre de una constante de Rust dentro del texto de un tool output. Y el
`success: false` no lo distinguía de un timeout HTTP o un error de SQL.

## Qué cambia

El mensaje ahora arranca con un código estable:

```
SUBGRAPH_DEPTH_EXCEEDED: subgraph nesting reached the configured ceiling of 25
(COLMENA_MAX_SUBGRAPH_DEPTH). Nesting is unlimited unless that variable is set,
so hitting this usually means a subgraph tool references itself or a cycle of
agents calls each other.
```

Sigue viajando dentro del output del tool — no es un tipo de evento nuevo — pero
el prefijo `SUBGRAPH_DEPTH_EXCEEDED:` es estable y se puede matchear sin
depender de la prosa.

## Cuándo se dispara

**Nunca en la configuración por defecto.** La anidación es ilimitada salvo que
alguien defina `COLMENA_MAX_SUBGRAPH_DEPTH`. Ver la
[nota 3](2026-08-21-unbounded-subgraph-nesting.md).

## Qué tiene que hacer ADP

Si operaciones decide encender el techo, vale la pena que el frontend detecte el
prefijo y muestre un mensaje entendible — "este agente tiene un ciclo" es mucho
más útil que un error de tool genérico, porque es un problema de diseño del
grafo, no una falla transitoria.

## Riesgo

**Nulo.** Cambia el texto de un mensaje de error que hoy nadie parsea de forma
estable, y en la configuración por defecto ese mensaje ya no se emite nunca.

## Por qué no es un evento SSE dedicado

Se evaluó emitir un tipo de evento propio. Habría implicado tocar el enum de
eventos, las dos ramas del mapper y la serialización, más trabajo de frontend —
todo para un caso que por defecto no ocurre. Si en algún momento se enciende el
techo y hace falta reaccionar en la UI, ese es el momento de promoverlo a evento.
