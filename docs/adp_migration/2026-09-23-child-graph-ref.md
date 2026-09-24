# `child_graph_ref`: el puerto con que el motor pide el grafo de un hijo

**Acción de ADP: ninguna todavía.** El puerto existe pero el motor no lo consulta
hasta el PR siguiente; el worker lo implementa en el plan de ADP.

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
ramas `subgraph` de `router`.

## Qué se rompe si se ignora

Nada. `EngineConfig` suma un campo público: un literal `EngineConfig { … }` tiene que
agregar `child_graph_resolver: None`. El worker de ADP arma su config con
`EngineConfig::from_env()` (`apps/service/ia/platform/worker/src/main.rs`), así que
compila sin cambios.
