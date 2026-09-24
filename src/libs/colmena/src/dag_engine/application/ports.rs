use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::node::ExecutableNode;
use serde_json::Value;
use std::sync::Arc;

/// Define el "Puerto" que el `DagRunUseCase` utiliza para
/// obtener una implementación concreta de un nodo.
///
/// La infraestructura (`infrastructure`) será responsable de
/// implementar este trait.
pub trait NodeRegistryPort: Send + Sync {
    /// Busca y retorna una implementación de nodo basada en su
    /// `node_type` (ej. "add", "log").
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>>;

    /// Retorna todos los nodos registrados.
    fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>>;

    /// Return the node as a `ToolkitNode` if it was registered as one; `None`
    /// otherwise (including for standalone ExecutableNode registrations). Default
    /// impl returns `None` so existing registries don't need changes.
    fn get_toolkit_node(
        &self,
        _node_type: &str,
    ) -> Option<std::sync::Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>> {
        None
    }
}

/// Define el "Puerto" que un Nodo SubGraph utiliza para ejecutar
/// su grafo hijo interno. Esto evita la dependencia circular entre
/// la capa de Nodos y el DagRunUseCase.
#[async_trait::async_trait]
#[allow(clippy::too_many_arguments)]
pub trait SubGraphExecutorPort: Send + Sync {
    /// Ejecuta un subgrafo desde cero.
    async fn run_subgraph(
        &self,
        session_id: &str,
        graph_json: Value,
        global_state: Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        parent_session_id: Option<String>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError>;

    /// Reanuda un subgrafo suspendido tras un Human-in-the-Loop con el grafo
    /// que diga `graph` (ver [`ResumeGraph`]). Un `Fresh` cuyo esqueleto no
    /// calza con el guardado, o un `Unavailable`, cierra la fila del hijo como
    /// FAILED y devuelve `DagError::ResumeRefused` sin correr nada.
    async fn resume_subgraph(
        &self,
        session_id: &str,
        answer: String,
        graph: ResumeGraph,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError>;

    /// Finds the SUSPENDED child run whose parent_session_id matches.
    /// (Currently single-leaf-per-parent design; the second arg is reserved
    /// for future disambiguation when multiple children may suspend in parallel.)
    async fn find_child_session_id_for_resume(
        &self,
        parent_session_id: &str,
        parent_node_path: &str,
    ) -> Result<Option<String>, DagError>;
}

/// Which graph a suspended child resumes with.
///
/// `Debug` is hand-written: `Fresh` carries a runnable graph with its secrets
/// already resolved, like [`ResolvedChildGraph`].
#[derive(Clone, PartialEq)]
pub enum ResumeGraph {
    /// Derived again from the child's source (the parent's config or inputs,
    /// the file, the resolver). The executor checks its skeleton against the
    /// stored graph and refuses it if the structure changed.
    Fresh(Value),
    /// The source could not give a graph (the resolver refused, the file is
    /// gone). The executor closes the child's row as FAILED and returns this
    /// text verbatim.
    Unavailable(String),
    /// The graph stored in the child's row, as up to v0.16 — what the
    /// `COLMENA_SUBGRAPH_RESUME_GRAPH=stored` valve passes.
    Stored,
}

impl std::fmt::Debug for ResumeGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fresh(_) => f.write_str("Fresh(<redacted>)"),
            Self::Unavailable(reason) => f.debug_tuple("Unavailable").field(reason).finish(),
            Self::Stored => f.write_str("Stored"),
        }
    }
}

/// Resolves a child graph named by reference (`child_graph_ref`). Implemented by
/// the embedder: the ADP worker asks ADP for the agent's runnable graph. The
/// resolved graph is never emitted, never returned to the model and never stored
/// in a node output — only in the child run's own persisted state, like an inline
/// graph.
#[async_trait::async_trait]
pub trait ChildGraphResolverPort: Send + Sync {
    /// Returns the runnable graph for `req.agent_id`, or why it cannot run.
    async fn resolve(
        &self,
        req: ChildGraphRequest,
    ) -> Result<ResolvedChildGraph, ChildGraphResolveError>;
}

/// What the engine knows when a `subgraph` asks for a child by reference.
#[derive(Debug, Clone)]
pub struct ChildGraphRequest {
    /// The ref's `agent_id`, already templated and trimmed.
    pub agent_id: String,
    /// The ref's `context`, verbatim (opaque to the engine).
    pub context: Value,
    /// The Colmena session of the run that owns the `subgraph` node.
    pub session_id: String,
    /// The embedder's stable session, when the run has one.
    pub agent_session_id: Option<String>,
    /// The `subgraph` node's own lineage path.
    pub parent_path: String,
}

/// A child graph returned by the embedder.
///
/// `Debug` is hand-written to redact `graph` — it carries secrets already
/// resolved (see the field doc) — so a stray `{:?}` / `tracing::debug!` can
/// never leak them (mirrors the redacted `Debug` convention on `OAuthAuthSpec`).
#[derive(Clone)]
pub struct ResolvedChildGraph {
    /// The runnable graph (`{ "nodes": …, "edges": … }`), secrets included.
    pub graph: Value,
    /// The agent's human-facing name.
    pub display_name: String,
}

impl std::fmt::Debug for ResolvedChildGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedChildGraph")
            .field("display_name", &self.display_name)
            .field("graph", &"<redacted>")
            .finish()
    }
}

/// Why a `child_graph_ref` could not be resolved. Reaches the calling model as
/// `CHILD_GRAPH_RESOLVE_FAILED:<code>: <message>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildGraphResolveError {
    /// No agent with that id, or the ref carried no usable `agent_id`.
    NotFound(String),
    /// The agent exists but the run's session may not use it.
    Forbidden(String),
    /// The agent needs configuration its owner has not provided.
    NeedsConfig(String),
    /// The agent's graph is incomplete or empty.
    NotRunnable(String),
    /// No resolver configured, the resolver failed or it timed out.
    Unavailable(String),
}

impl ChildGraphResolveError {
    /// Stable code after the `CHILD_GRAPH_RESOLVE_FAILED:` prefix.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::Forbidden(_) => "forbidden",
            Self::NeedsConfig(_) => "needs_config",
            Self::NotRunnable(_) => "not_runnable",
            Self::Unavailable(_) => "unavailable",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::NotFound(m)
            | Self::Forbidden(m)
            | Self::NeedsConfig(m)
            | Self::NotRunnable(m)
            | Self::Unavailable(m) => m,
        }
    }
}

impl std::fmt::Display for ChildGraphResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CHILD_GRAPH_RESOLVE_FAILED:{}: {}",
            self.code(),
            self.message()
        )
    }
}

impl std::error::Error for ChildGraphResolveError {}

#[cfg(test)]
mod resolved_child_graph_tests {
    use super::ResolvedChildGraph;
    use super::ResumeGraph;
    use serde_json::json;

    #[test]
    fn resume_graph_debug_redacts_a_fresh_graph() {
        let secret = "sk-super-secret-token-do-not-leak";
        let g = ResumeGraph::Fresh(json!({
            "nodes": { "llm": { "type": "llm_call", "config": { "api_key": secret } } },
            "edges": []
        }));
        let debug_str = format!("{g:?}");
        assert!(!debug_str.contains(secret), "{debug_str}");
        assert!(debug_str.contains("Fresh"));
    }

    #[test]
    fn debug_redacts_the_graph_but_keeps_the_display_name() {
        let secret = "sk-super-secret-token-do-not-leak";
        let r = ResolvedChildGraph {
            graph: json!({
                "nodes": { "llm": { "type": "llm_call", "config": { "api_key": secret } } },
                "edges": []
            }),
            display_name: "Packing Expert".to_string(),
        };
        let debug_str = format!("{r:?}");
        assert!(
            !debug_str.contains(secret),
            "Debug output must never contain a value from the graph: {debug_str}"
        );
        assert!(debug_str.contains("<redacted>"));
        assert!(debug_str.contains("Packing Expert"));
    }
}
