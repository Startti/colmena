# Notas de migración para ADP

Un documento por cada cambio de colmena que sea **visible cruzando la frontera
SSE** o que altere la superficie pública de Rust. Cada nota dice qué cambió, qué
ve ADP antes y después, qué tiene que hacer ADP, y qué se rompe si no hace nada.

Los cambios que quedan dentro del motor no llevan nota aquí.

## 2026-09-24 — nombre del agente en la frontera de un `child_graph_ref` y `provider_key_id` en el consumo

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [`node_label` en la frontera + `provider_key_id` por entrada de consumo](2026-09-24-child-label-and-key-id.md) | Leer `node_label` del `subgraph-node-start` de un hijo por referencia; preferir `provider_key_id` de cada fila de `usage-summary` al facturar | Nada; sin la acción, ADP sigue mostrando el `node_id` técnico como etiqueta y mantiene su propio mapeo de clave por nodo |

## 2026-09-23 — `thread_id` fijo: memoria por agente sin exponer el hilo al modelo

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [`thread_id` fijo en `node_schema` (`memory_mode: "dynamic"`)](2026-09-23-fixed-thread-id.md) | Compilar la tool "Run My Agent" con `thread_id: { "fixed": "${agentId}" }` para un hilo de memoria por agente | Nada; sin fijar `thread_id`, `dynamic` sigue como hoy — el modelo nombra el hilo él mismo |

## 2026-09-23 — `child_graph_ref`: un `subgraph` carga su hijo por referencia

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [El puerto `ChildGraphResolverPort` y la fuente `child_graph_ref`](2026-09-23-child-graph-ref.md) | Implementar el puerto en el worker para usar refs | Nada; sin resolvedor, un ref falla con `CHILD_GRAPH_RESOLVE_FAILED:unavailable` |

## 2026-09-23 — una tool-subgrafo reanudada devuelve su salida, no el estado del hijo

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [Resume de subgrafo-como-tool extrae el nodo de salida](2026-09-23-subgraph-resume-output.md) | Ninguna — recomendado re-medir el banco del creador | Nada; el resultado de una tool reanudada solo puede volverse más chico y más limpio |

## 2026-09-15 — `node-end`/`subgraph-node-end` ganan `status`/`errorText`; fronteras de tool cierran al fallar

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [`status`/`errorText` en el cierre de una tool `llm_call`/`for_each`](2026-09-15-node-end-error-status.md) | Recomendada — cambio de una línea en `closeNode(...)` en `event-tree-builder.ts` y `colmena-events.reducer.ts` | Nada; sin el cambio, un sub-agente que falló sigue pintándose como `'done'`, igual que hoy |
| 2 | [La frontera de un `subgraph`-as-tool cierra cuando su child falla](2026-09-15-subgraph-tool-boundary-closes-on-failure.md) | Ninguna nueva — ver la nota 3, que cierra también el nodo interno | Nada; el motor ya cierra el `node_id` de la frontera antes de que la mitigación tuviera que hacerlo |
| 3 | [Un nodo interno que falla dentro de un run anidado ahora cierra](2026-09-15-nested-node-failure-closes.md) | Recomendada — migrar `closeNode` a matchear por `path`, no solo `node_id` | Nada nuevo; cruce entre filas concurrentes de `for_each` sigue siendo un riesgo preexistente si `closeNode` solo mira `node_id` |

## 2026-09-02 — `validate_graph` valida de verdad

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [`validate_graph` ahora valida de verdad](2026-09-02-validate-graph-now-validates.md) | **Ninguna** — verificado que `platform/` no llama a esa función | Nada. Los casos que ahora rechaza ya fallaban al ejecutar |

## 2026-09-03 — el motor valida el grafo en toda entrada

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [`Graph::validate()` corre en toda entrada](2026-09-03-graph-validated-on-every-entry.md) | Ninguna — el error viaja como cualquier `DagError`, sin cambio de formato SSE | Un grafo estructuralmente inválido falla al inicio en vez de más tarde o en silencio. Válvula: `COLMENA_GRAPH_VALIDATION=off` |

## 2026-08-23 — `usage` separa input fresco de tokens de cache

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [Split de tokens de cache en `usage`](2026-08-23-usage-cache-token-split.md) | **Obligatoria si se factura sobre `usage`** — sacar la resta `promptTokens − cacheReadTokens` | Costos de input negativos en Anthropic; `totalTokens` cambia de magnitud |
| 2 | [Identidad del nodo anidado en su frame de frontera](2026-08-23-nested-node-identity.md) | Ninguna — aditivo | Nada; el costo por nodo anidado sigue sin poder calcularse |

Ningún campo cambió de nombre. `promptTokens` pasa a ser input **fresco** en los
tres providers, `totalTokens` incluye el cache, y las dos columnas de cache están
siempre presentes.

## 2026-08-21 — remediación de ejecución anidada y SSE

Salen juntos en una sola rama. Léanse en este orden. El único que exige trabajo
de frontend es el segundo.

| # | Nota | Acción de ADP | Qué se rompe si se ignora |
|---|------|---------------|---------------------------|
| 1 | [`thinking-delta` con nivel correcto](2026-08-21-thinking-delta-level-fix.md) | Ninguna — solo cambian `level` y `path` | Nada; el thinking del planner se indenta un nivel más adentro |
| 2 | [Cambios de `level` y `path`](2026-08-21-nested-level-and-path-changes.md) | **Obligatoria** — revisar el agrupamiento del árbol | El trabajo de un sub-agente se dibuja a la profundidad equivocada o al lado de su etiqueta |
| 3 | [La anidación de subgrafos ya no tiene tope](2026-08-21-unbounded-subgraph-nesting.md) | Revisión — decidir si se pone un techo | Un ciclo de agentes recursa hasta agotar el worker |
| 4 | [Frames de frontera para subgrafos como tool](2026-08-21-subgraph-tool-boundary-frames.md) | Opcional — dibujar los delimitadores nuevos | Nada; es puramente aditivo |
| 4b | [Frontera también para `llm_call` / `for_each` como tool](2026-08-21-inner-work-tool-boundaries.md) | Recomendada — permite un solo camino de código | Aditivo; revisar si algo cuenta nodos |
| 5 | [`llm_call` como tool ahora es un nivel real](2026-08-21-llm-call-as-tool-nesting.md) | Condicional — solo si algún agente lo usa | Esos eventos se mueven de nivel 0 a nivel 1 |
| 6 | [Código de error del techo de profundidad](2026-08-21-subgraph-depth-error-code.md) | Opcional | Nada, salvo que se configure un techo |
| 7 | [Linaje por fila en `for_each`](2026-08-21-for-each-row-lineage.md) | Opcional — habilita progreso por fila | Nada; las filas pasan de indistinguibles a identificables |

## Cómo verificar todo esto de una sola pasada

Un grafo cubre los siete cambios en un solo run:

```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_sse_remediation_e2e.json \
  --agent-session-id verificacion_001 > /tmp/salida.sse
python3 scripts/verify_nested_sse_e2e.py /tmp/salida.sse
```

Para el techo opcional de profundidad, el mismo grafo con
`COLMENA_MAX_SUBGRAPH_DEPTH=3` y el verificador en modo `--ceiling`.
