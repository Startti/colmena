# `child_graph_ref`: el puerto con que el motor pide el grafo de un hijo

**Acción de ADP: implementar el puerto en el worker** para usar `child_graph_ref`
(plan de ADP). Sin eso, un ref con `agent_id` ya resuelto falla con `unavailable`;
uno que todavía trae `${…}` sin templar falla con `not_found` de todas formas,
haya o no resolvedor. Nada más cambia.

## Qué cambia

Un `subgraph` va a poder nombrar a su hijo por referencia
(`child_graph_ref: { "agent_id": "<id>", "context": { … } }`) en vez de traer el grafo
(`child_graph_inline`) o una ruta (`child_graph_path`). El motor no busca grafos por su
cuenta: se los pide al embebedor por un puerto nuevo.

### El puerto (`colmena::dag_engine::application::ports`)

```rust
#[async_trait::async_trait]
pub trait ChildGraphResolverPort: Send + Sync {
    async fn resolve(&self, req: ChildGraphRequest)
        -> Result<ResolvedChildGraph, ChildGraphResolveError>;
}

pub struct ChildGraphRequest {
    pub agent_id: String,                 // del ref, ya templado
    pub context: serde_json::Value,       // el `context` del ref, tal cual
    pub session_id: String,               // sesión de Colmena del run padre
    pub agent_session_id: Option<String>, // la sesión estable del embebedor
    pub parent_path: String,              // linaje del nodo `subgraph`
}

pub struct ResolvedChildGraph { pub graph: serde_json::Value, pub display_name: String }
```

`ChildGraphResolveError` tiene cinco variantes, cada una con su mensaje. `code()`
devuelve el código estable y `Display` arma `CHILD_GRAPH_RESOLVE_FAILED:<code>: <msg>`:

| Variante | `code()` | Cuándo |
|---|---|---|
| `NotFound` | `not_found` | El agente no existe |
| `Forbidden` | `forbidden` | Existe, pero la sesión del job no lo puede usar |
| `NeedsConfig` | `needs_config` | Le falta configuración a su dueño |
| `NotRunnable` | `not_runnable` | Su grafo está incompleto o vacío |
| `Unavailable` | `unavailable` | No hay resolvedor, falló o no contestó |

### Cómo se inyecta

Campo nuevo en `EngineConfig`:

```rust
pub child_graph_resolver: Option<Arc<dyn ChildGraphResolverPort>>,
```

`EngineConfig::from_env()` lo deja en `None`; el worker lo setea después de
`from_env` y antes de `ColmenaEngine::new`. Queda cableado en `SubGraphNode` y en las
ramas `subgraph` de `router` — y, de punta a punta desde el PR 3/5, `router`,
`orchestrator` y el pre-flight de proveedores reconocen `child_graph_ref` como
fuente válida igual que `child_graph_path`/`child_graph_inline`: una rama o un
agente que solo declara un ref ya no se rechaza al cargar el grafo, y el
pre-flight lo salta explícitamente (`subgraph.child_graph_ref: resolved at run
time`) en vez de tratarlo como el `child_graph_path`/`child_graph_inline`
ausentes de antes.

## Desde la entrada 67: el motor resuelve el ref

- Se resuelve **antes** del frame `subgraph-node-start`: un hijo que no se pudo
  resolver no emite ningún frame.
- Sin resolvedor → `unavailable: no child graph resolver configured`. Un `agent_id`
  que todavía contiene `${` (el modelo no mandó el argumento) → `not_found`, sin
  llamar al resolvedor. Si el resolvedor tarda más de 30 s → `unavailable`.
- El error llega a la tool con el prefijo estable: `ToolResult.error` empieza con
  `CHILD_GRAPH_RESOLVE_FAILED:<code>:`; el `output` (lo que muestra
  `tool-output-available`) es `Error executing node <tool>: CHILD_GRAPH_RESOLVE_FAILED:…`.
- El grafo resuelto no aparece en frames, en la salida de la tool ni en el estado del
  hijo. Se guarda en `dag_runs.graph_json` del run hijo, como un inline. Desde la
  entrada 80 un resume vuelve a llamar al resolvedor
  ([nota](2026-09-24-subgraph-resume-fresh-graph.md)).
- `display_name` todavía no se usa (el nombre en la frontera llega en un PR posterior).

## Qué se rompe si se ignora

Nada para los grafos de hoy: solo un `subgraph` (standalone, en una rama de
`router` o en un agente de `orchestrator`) que declare `child_graph_ref` necesita
el resolvedor — con `agent_id` ya resuelto falla con `unavailable`, sin resolver
falla con `not_found` igual. `EngineConfig` suma un campo público: un literal `EngineConfig { … }` tiene que
agregar `child_graph_resolver: None`. El worker de ADP arma su config con
`EngineConfig::from_env()` (`apps/service/ia/platform/worker/src/main.rs`), así que
compila sin cambios.
