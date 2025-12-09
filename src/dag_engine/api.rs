use axum::{
    extract::{Json, State},
    routing::post,
    Router,
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;

// Import from crate since this is part of the colmena library
use crate::dag_engine::application::run_use_case::DagRunUseCase;
use crate::dag_engine::domain::graph::Graph;
use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
use crate::llm::infrastructure::ConversationRepositoryFactory;

/// Execute a DAG from a file path
pub async fn run_dag(file_path: String) -> Result<Value, Box<dyn std::error::Error>> {
    // Load .env file
    dotenvy::dotenv().ok();

    // Initialize Repository Factory
    let repository_factory = Arc::new(ConversationRepositoryFactory::new());
    let registry = HashMapNodeRegistry::new(repository_factory);
    let run_use_case = DagRunUseCase::new(registry);

    // Load and execute the graph
    let file_content = tokio::fs::read_to_string(&file_path).await?;
    let graph: Graph = serde_json::from_str(&file_content)?;

    run_use_case
        .execute(graph)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

/// Serve a DAG as an HTTP API
pub async fn serve_dag(file_path: String, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenvy::dotenv().ok();

    // Initialize Repository Factory
    let repository_factory = Arc::new(ConversationRepositoryFactory::new());
    let registry = HashMapNodeRegistry::new(repository_factory);
    let run_use_case = Arc::new(DagRunUseCase::new(registry));

    // Load the graph
    let file_content = tokio::fs::read_to_string(&file_path).await?;
    let graph: Graph = serde_json::from_str(&file_content)?;
    let graph_arc = Arc::new(graph);

    // Build the Router dynamically
    let mut app = Router::new();
    let mut routes_count = 0;

    // Find Trigger nodes and register routes
    for (node_id, node_config) in &graph_arc.nodes {
        if node_config.node_type == "trigger_webhook" {
            if let Some(path) = node_config.config.get("path").and_then(|v| v.as_str()) {
                println!(
                    "   └── Registering route: POST {} (Node: {})",
                    path, node_id
                );

                // Estado específico para inyectar en el handler
                let state = AppState {
                    graph: graph_arc.clone(),
                    use_case: run_use_case.clone(),
                };

                let node_id_clone = node_id.clone();
                app = app.route(
                    path,
                    post(move |State(state), Json(payload)| {
                        handler_webhook(state, payload, node_id_clone)
                    })
                    .with_state(state),
                );
                routes_count += 1;
            }
        }
    }

    if routes_count == 0 {
        eprintln!(
            "⚠️ ALERT: No 'trigger_webhook' nodes found. The server is running but has no routes."
        );
    }

    // Start the TCP server
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("✅ Server listening on http://0.0.0.0:{}", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// Shared state for Axum handlers
#[derive(Clone)]
struct AppState {
    graph: Arc<Graph>,
    use_case: Arc<DagRunUseCase>,
}

/// Handler that executes when an HTTP request arrives
async fn handler_webhook(state: AppState, payload: Value, trigger_node_id: String) -> Json<Value> {
    println!("🔔 Webhook received for node: {}", trigger_node_id);

    // Clone the graph for this specific execution
    let mut graph_instance = (*state.graph).clone();

    // Inject the payload into the trigger node config
    if let Some(node) = graph_instance.nodes.get_mut(&trigger_node_id) {
        if node.config.is_null() {
            node.config = serde_json::json!({});
        }
        node.config["__payload__"] = payload;
    }

    // Execute the graph
    match state.use_case.execute(graph_instance).await {
        Ok(output) => {
            println!("✅ Execution successful.");
            Json(output)
        }
        Err(e) => {
            eprintln!("❌ Execution error: {}", e);
            Json(serde_json::json!({ "error": e.to_string() }))
        }
    }
}
