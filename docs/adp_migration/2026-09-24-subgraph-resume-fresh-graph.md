# Un hijo reanudado corre el grafo que su fuente nombra hoy

**Acción de ADP:** al subir a v0.18.0, ningún cambio de código — ADP no implementa
`SubGraphExecutorPort` (`git grep SubGraphExecutorPort apps/`: cero). Sí hay que
enseñarle al agente principal y a la descripción de `Run My Agent` el prefijo
`SUBGRAPH_RESUME_INCOMPATIBLE:`, que desde la entrada 78 es real. Un
`CHILD_GRAPH_RESOLVE_FAILED:` puede llegar **después** de una pregunta respondida:
desde la entrada 81 esto alcanza también a `Run My Agent` (un `child_graph_ref`
vuelve a pedirle el grafo al resolvedor en cada resume, no solo al arrancar) —
un agente despublicado, borrado o sin acceso entre el suspend y el resume falla
así. Un inline de ADP no puede fallar con este prefijo (mantiene su estructura
por construcción).

## Superficie de Rust (desde la entrada 74)

- `SubGraphExecutorPort::resume_subgraph` gana un tercer parámetro, `graph: ResumeGraph`:

  ```rust
  pub enum ResumeGraph { Fresh(serde_json::Value), Unavailable(String), Stored }
  ```

  Quien implemente el puerto fuera del crate deja de compilar hasta agregarlo. `Debug`
  redacta el grafo de `Fresh` (trae secretos resueltos).
- `DagError::ResumeRefused(String)`: un resume rechazado antes de correr nada. Su
  `Display` es el motivo tal cual, con el prefijo adelante. Un `match` exhaustivo sobre
  `DagError` necesita el brazo nuevo.
- `colmena::dag_engine::domain::graph_skeleton::SUBGRAPH_RESUME_INCOMPATIBLE`
  (`"SUBGRAPH_RESUME_INCOMPATIBLE:"`), prefijo estable como `SUBGRAPH_DEPTH_EXCEEDED:`.

En la entrada 74 el motor todavía pasa `Stored` en todo resume: el comportamiento es el
de v0.16.

## Superficie de Rust (desde la entrada 75)

- `DagStateRepository` (`domain/state.rs`) gana `fail_if_suspended` y
  `fail_suspended_descendants`, ambos con impl por defecto — **non-breaking** para
  quien implemente el trait fuera del crate, mismo patrón que
  `cancel_running_descendants`.
- Un rechazo cierra `FAILED` no solo la fila del hijo, también sus propios
  descendientes que sigan `SUSPENDED` (`close_refused`), para que
  `find_resume_entry` no los cuente como una cadena propia. Sin efecto en ADP: no
  implementa el puerto, y todo caller sigue pasando `Stored`.

## Desde la entrada 78: el comportamiento

- Al reanudar, un `child_graph_inline` toma su grafo del que llega en ESTE job (en
  ADP, la copia de cable del `suspendedDag` del turno del resume, con su token,
  claves y skills); un `child_graph_path` relee el archivo. Un `child_graph_ref`
  seguía reanudando la versión guardada hasta la entrada 81, que lo vuelve a pedir
  al resolvedor también (ver más abajo).
- Si el esqueleto (ids con `type` + aristas) cambió, nada corre y la fila del
  hijo queda `FAILED`. Por tool: `ToolResult.error` empieza con
  `SUBGRAPH_RESUME_INCOMPATIBLE:`; por arista, orquestador o router: el run
  falla con `Error de ejecución en el nodo: SUBGRAPH_RESUME_INCOMPATIBLE: …`
  (por router, con `router branch '<rama>': ` delante — vuelve a elegir su rama en cada resume).
- Ningún frame SSE nuevo. Un run suspendido por v0.16 se reanuda fresco sin
  migración; un worker v0.16 que tome un resume corre la copia guardada (volver
  atrás es seguro). Válvula: `COLMENA_SUBGRAPH_RESUME_GRAPH=stored`, que **ya no
  existe desde v0.19** y no se debe fijar en v0.18 una vez que v0.19 escribió filas
  (ver [El grafo en reposo](2026-09-25-graph-at-rest.md)).

## Desde la entrada 81: un `child_graph_ref` vuelve a pedir su grafo

- Al reanudar, el motor llama otra vez a `ChildGraphResolverPort::resolve` con el mismo
  `ChildGraphRequest` que armó al arrancar (`agent_id`, `context`, sesión, sesión
  estable, ruta), después de encontrar al hijo suspendido: sin hijo, no hay resolve.
- En ADP: un `POST /internal/agents/:id/runnable-graph` por resume (compila el agente,
  acuña un token de 2 h) y el worker vuelve a preprocesar las skills. `context.messageId`
  llega con el del turno del resume.
- Un agente despublicado, borrado o sin acceso entre el suspend y el resume falla con
  `CHILD_GRAPH_RESOLVE_FAILED:<code>:` **después** de la respuesta; un agente editado sin
  cambiar su forma se reanuda con la versión nueva; con otra forma,
  `SUBGRAPH_RESUME_INCOMPATIBLE:`. La fila del hijo queda `FAILED` en los dos casos.
- La válvula `COLMENA_SUBGRAPH_RESUME_GRAPH=stored` también apaga esto (solo en v0.18;
  en v0.19 no existe).

## Qué se rompe si se ignora

Nada al compilar. En producción: el agente principal y la descripción de
`Run My Agent` tratan `SUBGRAPH_RESUME_INCOMPATIBLE:` y `CHILD_GRAPH_RESOLVE_FAILED:`
como una falla genérica en vez de explicarle al usuario que el agente cambió, o dejó
de estar disponible, entre su pregunta y su respuesta.
