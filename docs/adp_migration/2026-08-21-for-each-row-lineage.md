# Las filas de `for_each` ahora tienen linaje propio

**Acción de ADP: OPCIONAL, pero habilita algo que antes era imposible.**

## Qué pasaba

Todas las filas de un `for_each` corren el **mismo** grafo target, así que todas
emitían los **mismos** `node_id`. Y recibían el observer crudo, sin nada que las
distinguiera. Resultado: en el stream las filas eran indistinguibles — mismo
`path` para todas.

Dos consecuencias:

1. **Imposible atribuir**: no había forma de saber si un token venía de la fila 0
   o de la fila 1. Con `concurrency > 1` los tokens de todas las filas llegaban
   entremezclados bajo un linaje único.
2. **Colisión de bloques de texto**: el `SseMapper` keyea los bloques de texto
   abiertos por `path`. Dos filas concurrentes con el mismo `path` se pisaban el
   bloque: el `text-end` de una cerraba el de la otra, y la otra quedaba abierta
   para siempre.

Este segundo punto lo descubrimos **durante la verificación E2E** de esta misma
tanda de cambios, no antes: el fix de bloques-por-`path` no servía de nada
mientras el `path` fuera idéntico entre filas.

## Qué cambia

Cada fila corre bajo su propia identidad, `<node_id_del_for_each>#<índice>`. El
índice **coincide con el campo `index`** del `batch-item-finished` de esa fila,
así que se pueden correlacionar.

Además, un `for_each` despachado **como tool** ahora se anida bajo el nombre del
tool, igual que ya hacía `subgraph` y que ahora hace `llm_call` (ver la
[nota 5](2026-08-21-llm-call-as-tool-nesting.md)).

### Antes

```
path: "coordinador>eco"     ← fila 0
path: "coordinador>eco"     ← fila 1, idéntico
```

### Después

```
path: "coordinador>abanico>for_each#0>eco"
path: "coordinador>abanico>for_each#1>eco"
```

Verificado en vivo con dos filas concurrentes.

## Qué tiene que hacer ADP

Nada obligatorio. Pero si la UI quiere mostrar el progreso **por fila** de un
fan-out — que hasta ahora no se podía — ya tiene con qué: agrupar por el
segmento `#<índice>` del `path` y cruzarlo con `batch-item-finished.index`.

Ojo con lo que se mueve: los eventos de un `for_each` usado como tool (incluidos
`batch-progress` y `batch-item-finished`) ya no salen en nivel 0; salen anidados
bajo el nombre del tool.

## Riesgo

**Bajo.** No aparecen ni desaparecen tipos de evento. Cambian `level` y `path`,
y en la dirección correcta: de "todo aplastado en un linaje" a "cada fila
identificable".

## Cómo verificar

```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_sse_remediation_e2e.json \
  --agent-session-id verificacion_001 > /tmp/salida.sse
python3 scripts/verify_nested_sse_e2e.py /tmp/salida.sse
```

El grafo trae un `for_each` con `concurrency: 2`; el verificador afirma que las
dos filas abren bloques de texto con `path` distinto.
