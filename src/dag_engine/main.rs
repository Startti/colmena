// src/dag_engine/main.rs
pub mod application;
pub mod domain;
pub mod infrastructure;

use crate::application::run_use_case::DagRunUseCase;
use crate::domain::graph::Graph;
use crate::infrastructure::registry::HashMapNodeRegistry;
use clap::{Parser, Subcommand};
use std::sync::Arc;

// --- Imports de Axum ---
use axum::{
    extract::{State, Json},
    routing::post,
    Router,
};
use serde_json::Value;
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[command(version, about = "Motor de ejecución de grafos DAG en Rust")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run { file_path: String },
    Serve {
        file_path: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
}

// Estado compartido para los handlers de Axum
#[derive(Clone)]
struct AppState {
    graph: Arc<Graph>,                // El grafo base
    use_case: Arc<DagRunUseCase>,     // El ejecutor
}

use colmena::llm::infrastructure::ConversationRepositoryFactory;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Initialize Repository Factory
    let repository_factory = Arc::new(ConversationRepositoryFactory::new());

    let registry = Arc::new(HashMapNodeRegistry::new(repository_factory));
    // Envolvemos en Arc para poder compartirlo entre hilos del servidor
    let run_use_case = Arc::new(DagRunUseCase::new(registry));

    match cli.command {
        Commands::Run { file_path } => {
            println!("🚀 Modo Run: Cargando grafo desde {}", file_path);
            let file_content = tokio::fs::read_to_string(&file_path).await?;
            let graph: Graph = serde_json::from_str(&file_content)?;

            println!("Ejecutando grafo...");
            match run_use_case.execute(graph).await {
                Ok(out) => println!("Output Final:\n{}", serde_json::to_string_pretty(&out)?),
                Err(e) => eprintln!("❌ Error: {}", e),
            }
        }
        Commands::Serve { file_path, port } => {
            println!("🌐 Modo Serve: Iniciando...");
            
            // 1. Cargar el grafo en memoria
            let file_content = tokio::fs::read_to_string(&file_path).await?;
            let graph: Graph = serde_json::from_str(&file_content)?;
            // Lo envolvemos en Arc para compartirlo (read-only)
            let graph_arc = Arc::new(graph);

            // 2. Construir el Router de Axum dinámicamente
            let mut app = Router::new();
            let mut routes_count = 0;

            // 3. Buscar nodos Trigger y registrar rutas
            for (node_id, node_config) in &graph_arc.nodes {
                if node_config.node_type == "trigger_webhook" {
                    // Obtener el path de la config (ej: "/hello")
                    if let Some(path) = node_config.config.get("path").and_then(|v| v.as_str()) {
                        println!("   └── Registrando ruta: POST {} (Nodo: {})", path, node_id);
                        
                        // Estado específico para inyectar en el handler
                        let state = AppState {
                            graph: graph_arc.clone(),
                            use_case: run_use_case.clone(),
                        };

                        // Añadir la ruta. Usamos una "closure" move para capturar el node_id
                        let node_id_clone = node_id.clone();
                        app = app.route(
                            path, 
                            post(move |State(state), Json(payload)| {
                                handler_webhook(state, payload, node_id_clone)
                            })
                            .with_state(state)
                        );
                        routes_count += 1;
                    }
                }
            }

            if routes_count == 0 {
                eprintln!("⚠️ ALERTA: No se encontraron nodos 'trigger_webhook'. El servidor está corriendo pero no tiene rutas.");
            }

            // 4. Iniciar el servidor TCP
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            println!("✅ Servidor escuchando en http://0.0.0.0:{}", port);
            
            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}

/// Handler que se ejecuta cuando llega una petición HTTP
async fn handler_webhook(
    state: AppState,
    payload: Value,
    trigger_node_id: String
) -> Json<Value> {
    println!("🔔 Webhook recibido para nodo: {}", trigger_node_id);

    // 1. Clonar el grafo para esta ejecución específica
    // (Necesario porque vamos a modificar el config del nodo trigger con el payload)
    let mut graph_instance = (*state.graph).clone(); 

    // 2. Inyectar el payload en la config del nodo trigger
    if let Some(node) = graph_instance.nodes.get_mut(&trigger_node_id) {
        if node.config.is_null() {
            node.config = serde_json::json!({});
        }
        // Clave mágica que lee el TriggerWebhookNode
        node.config["__payload__"] = payload;
    }

    // 3. Ejecutar el grafo
    match state.use_case.execute(graph_instance).await {
        Ok(output) => {
            println!("✅ Ejecución exitosa.");
            Json(output)
        },
        Err(e) => {
            eprintln!("❌ Error en ejecución: {}", e);
            Json(serde_json::json!({ "error": e.to_string() }))
        }
    }
}