// src/dag_engine/main.rs
use clap::{Parser, Subcommand};
use colmena::dag_engine::api;

#[derive(Parser, Debug)]
#[command(version, about = "Motor de ejecución de grafos DAG en Rust")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run {
        file_path: String,
        #[arg(long, alias = "resume-id")]
        session_id: Option<String>,
        #[arg(long)]
        answer: Option<String>,
        #[arg(long, default_value_t = false)]
        include_extra_info: bool,
        /// Enable verbose internal debug output (also set via COLMENA_VERBOSE=1)
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
    Serve {
        file_path: String,
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
        /// Enable verbose internal debug output (also set via COLMENA_VERBOSE=1)
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // pyo3 is always linked (the python_script node uses it). The cfg-gated
    // `python` feature is for the PyO3 *bindings* layer (exposing Rust types
    // to Python), not for the engine's own use of Python. Always call
    // `prepare_freethreaded_python()` so python_script works in the standalone
    // CLI without `--features python`.
    pyo3::prepare_freethreaded_python();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            file_path,
            session_id,
            answer,
            include_extra_info,
            verbose,
        } => {
            use colmena::dag_engine::sse_mapper::SseMapper;
            use futures::StreamExt;

            // Enable verbose mode via flag or env var
            let verbose_env = std::env::var("COLMENA_VERBOSE")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false);
            colmena::dag_engine::verbose::set_verbose(verbose || verbose_env);

            println!("🚀 Ejecutando grafo: {}", file_path);

            dotenvy::dotenv().ok();
            let engine_config = colmena::dag_engine::engine::EngineConfig::from_env()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let engine = colmena::dag_engine::engine::ColmenaEngine::new(engine_config)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            // Run the graph in an inner block so we can always shut down the engine afterward
            let result: Result<(), anyhow::Error> = async {
                let file_content = tokio::fs::read_to_string(&file_path).await?;
                let graph: colmena::dag_engine::domain::graph::Graph =
                    serde_json::from_str(&file_content)?;
                graph
                    .validate()
                    .map_err(|e| anyhow::anyhow!("Invalid graph: {}", e))?;

                let mut mapper = SseMapper::new();

                // The new active_queue engine natively handles both linear and cyclic graphs
                let s =
                    engine.execute_stream(graph, session_id.clone(), answer, include_extra_info, None, None);
                let stream = Box::pin(s);
                tokio::pin!(stream);

                while let Some(result) = stream.next().await {
                    let event = match result {
                        Ok(ev) => ev,
                        Err(e) => {
                            println!(
                                "data: {}\n",
                                serde_json::json!({ "type": "error", "errorText": e.to_string() })
                            );
                            continue;
                        }
                    };

                    for part in mapper.map(&event) {
                        println!("data: {}\n", part);
                    }
                }

                println!("data: [DONE]\n");
                Ok(())
            }.await;

            // Always shut down the engine (closes all pools), even if the graph execution failed
            engine.shutdown().await;

            // Propagate the error if any
            result?;
        }

        Commands::Serve {
            file_path,
            host,
            port,
            verbose,
        } => {
            let verbose_env = std::env::var("COLMENA_VERBOSE")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false);
            colmena::dag_engine::verbose::set_verbose(verbose || verbose_env);
            println!("🌐 Modo Serve: Iniciando...");
            api::serve_dag(file_path, host, port).await?;
        }
    }

    Ok(())
}
