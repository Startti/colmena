# Un hijo reanudado corre el grafo que su fuente nombra hoy

**Acción de ADP:** al subir a v0.17.0, ningún cambio de código — ADP no implementa
`SubGraphExecutorPort` (`git grep SubGraphExecutorPort apps/`: cero). Sí hay que
enseñarle al agente principal y a la descripción de `Run My Agent` el prefijo
`SUBGRAPH_RESUME_INCOMPATIBLE:` (cuando el resume pase a usar el grafo fresco) y que
un `CHILD_GRAPH_RESOLVE_FAILED:` puede llegar **después** de una pregunta respondida
(cuando un `child_graph_ref` se vuelva a resolver al reanudar).

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

## Qué se rompe si se ignora

Nada en ADP: no implementa el puerto ni matchea `DagError`.
