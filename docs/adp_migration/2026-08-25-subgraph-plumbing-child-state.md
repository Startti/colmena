# El `fixed_config` de un subgraph ya no cruza al estado del hijo

**Acción de ADP: ninguna del lado del grafo.** No hay que cambiar cómo declaran sus
`tool_configurations` ni cómo sintetizan sus nodos de entrada. Sí hay que **tomar el
release nuevo** y **purgar las 3 filas** que ya identificaron.

Cierra el reporte `COLMENA_HANDOFF_SUBGRAPH_CONFIG_LEAK.md` (medido en dev el
2026-08-25) y su respuesta de §5/§6.

## Qué cambió

`SubGraphNode` excluye `child_graph_inline` y `child_graph_path` del estado global que
arma para el grafo hijo. El nodo sigue resolviendo su grafo desde esas claves —la
lectura ocurre antes del mapeo IN— pero ya no se las pasa al hijo.

Efecto observable: el `input` del hijo con `data: {}` ya no las ve, así que no llegan al
`prompt` del `llm_call` ni se persisten en `llm_node_history`.

## Qué NO cambió

Lo que ustedes preguntaron explícitamente:

- **Los argumentos del modelo siguen pasando.** `task`, `docKind`, `name`, `scope`,
  `confirmation` — y cualquier otro que el modelo decida mandar. Hay un test que fija
  exactamente ese set, tomado de sus 215 filas.
- **`files` sigue pasando.** Es el que hace que el fix vaya en `subgraph.rs` y no en
  `input.rs`.
- **La precedencia de resolución del grafo hijo es idéntica**: `config.inline` →
  `config.path` → `inputs.inline` → `inputs.path`. Los 5 tests que la fijaban siguen
  verdes.
- **`input.rs` no se tocó.** Su §6 lo dejó claro: endurecerlo habría tocado todos sus
  grafos, no solo los anidados.

## El segundo bug que cierra, sin que lo pidieran

Su §5 (anidamiento a profundidad 2, en 7 rutas) nos aplicaba de lleno. Con el plumbing
en el estado, un `subgraph` anidado **sin `config` propia** caía al fallback
`inputs.get("child_graph_inline")` y resolvía **el grafo del padre** — recursión
silenciosa.

Ojo con lo que ustedes mismos advirtieron: esto **no** explica el deadlock de julio
(`COLMENA_HANDOFF_RUN_AGENT_NESTED_DEADLOCK.md`), que tiene otra causa. Pero si ven
corridas anidadas con trabajo repetido sin explicación, ahora hay una causa menos.

## Lo que sigue de su lado

1. **Purgar las 3 filas de la sesión que identificaron.** El fix corta el flujo nuevo;
   no toca lo ya escrito. Mientras esas filas existan, el tool `research` en
   `persistent` sigue reenviando ese mensaje en cada llamada.
2. **Tomar el release.** Ver abajo.
3. **El bug de escritura silenciosa.** Su §9 midió que el documento no existe: el
   sub-agente **anunció** haber creado el spec y nunca llamó a la tool. Lo tienen como
   bug propio, y así es — pero conviene no dejarlo en la cola larga. Acá la fuga quedó
   contenida por ese bug, o sea por suerte, no por una defensa.

## Entrega

**Release desde `develop`, no un patch sobre el tag.** Pidieron un patch cortado sobre
`colmena_dag_engine-v0.12.0` (`edd928c1`); elegimos no hacerlo: los 12 tags de
`colmena_dag_engine` son ancestros de `develop` y nunca se cortó una rama fuera de la
línea. Abrir la primera divergencia por este fix cuesta más de lo que ahorra.

Lo que se llevan de más respecto del tag, y por qué es de bajo riesgo:

| PR | Qué es | Riesgo |
|---|---|---|
| #201 | Fail-closed cuando falla el upload de un adjunto sin `DATABASE_URL` | Cambia un descarte silencioso por un error explícito |
| #202 | `COLMENA_EXTRA_CA_CERT` para proxies TLS locales | **Opt-in e inerte** sin la env var; sin ella el cliente HTTP es idéntico al de antes |

Si aun así necesitan el patch aislado sobre el tag, díganlo y lo cortamos — es más
trabajo de nuestro lado, no un problema.

## Reproducción

`tests/graphs/advanced/subgraph_plumbing_isolation.json` reproduce la forma exacta
**sin llamar a ningún proveedor**: el eslabón que hace visible la fuga es el `input` con
`data: {}` del hijo, no el `llm_call`. Entrega el `child_graph_inline` por arista, así el
nodo `subgraph` lo recibe en `inputs` con `config` vacía — el mismo camino que usa
`DagToolExecutor` al mezclar el `fixed_config`.

```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/subgraph_plumbing_isolation.json
```
