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
enum CrdtAgentMode {
    /// Connect to ws://.../yjs/<artifact> and apply a set_cell mutation.
    Ws {
        #[arg(long)]
        url: String,
        #[arg(long)]
        sheet: String,
        #[arg(long)]
        addr: String,
        #[arg(long)]
        value: String,
    },
    /// POST to http://.../crdt/agent-op (in-proc mutation via HTTP).
    Inproc {
        #[arg(long)]
        base_url: String,
        #[arg(long)]
        artifact: String,
        #[arg(long)]
        sheet: String,
        #[arg(long)]
        addr: String,
        #[arg(long)]
        value: String,
    },
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run {
        file_path: String,
        #[arg(long, alias = "resume-id")]
        session_id: Option<String>,
        #[arg(long)]
        agent_session_id: Option<String>,
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
    /// Run the documents CRDT server.
    CrdtYws {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Root directory for CRDT document storage.
        #[arg(long, default_value = ".colmena/crdt_documents")]
        dump_dir: String,
    },
    /// One-process E2E: WS server + execute a graph against a SHARED
    /// `CrdtDocumentsRuntime` singleton. The graph's `llm_call` nodes
    /// that have `crdt_documents` config reuse the same runtime as the
    /// server, so any tool-driven mutation is visible LIVE in browser
    /// peers connected via WS (no restart, no disk round-trip).
    ///
    /// After the graph completes, the server stays up so the operator
    /// can keep poking the browser. Ctrl+C exits.
    CrdtYwsGraph {
        /// Path to the graph .json to execute.
        file_path: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8090)]
        port: u16,
        #[arg(long, default_value = ".colmena/crdt_documents")]
        dump_dir: String,
        #[arg(long)]
        agent_session_id: Option<String>,
        #[arg(long, default_value_t = false)]
        include_extra_info: bool,
        /// Pre-create an artifact with this id before running the graph.
        /// Useful for smoke tests: the operator opens the browser on
        /// `?artifact=<id>`, the agent (graph) mutates the same id.
        #[arg(long)]
        seed_artifact_id: Option<String>,
        /// Pause this many seconds after starting the server, BEFORE
        /// running the graph. Gives the operator time to open the
        /// browser so they observe the agent's edits live. Default 0.
        #[arg(long, default_value_t = 0)]
        wait_before_graph: u64,
    },
    /// One-shot agent peer for the CRDT server. Mutates an artifact via WS
    /// (R1 path) or in-proc HTTP POST (sanity-check path).
    CrdtAgent {
        #[command(subcommand)]
        mode: CrdtAgentMode,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    // pyo3 is always linked (the python_script node uses it). The cfg-gated
    // `python` feature is for the PyO3 *bindings* layer (exposing Rust types
    // to Python), not for the engine's own use of Python. Always call
    // `Python::initialize()` (pyo3 0.29; was `prepare_freethreaded_python()`)
    // so python_script works in the standalone CLI without `--features python`.
    pyo3::Python::initialize();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            file_path,
            session_id,
            agent_session_id,
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
                .await
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
                let s = engine.execute_stream(
                    graph,
                    session_id.clone(),
                    answer,
                    include_extra_info,
                    None,
                    agent_session_id.clone(),
                );
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
            }
            .await;

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

        Commands::CrdtYws {
            host,
            port,
            dump_dir,
        } => {
            use colmena::crdt_documents::CrdtDocumentsRuntime;
            use std::{net::SocketAddr, sync::Arc};

            let cfg = serde_json::json!({
                "storage_backend": "localfs",
                "storage_root": dump_dir,
            });
            let runtime = Arc::new(
                CrdtDocumentsRuntime::from_config(&cfg)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
            );
            let app = colmena::crdt_documents::server::router(runtime.clone());
            let addr: SocketAddr = format!("{host}:{port}").parse()?;
            let listener = tokio::net::TcpListener::bind(addr).await?;
            // Resolve the storage path to its absolute form so the operator
            // sees EXACTLY where artifacts will persist. The default
            // ".colmena/crdt_documents" is RELATIVE to cwd, which silently
            // breaks artifact reuse if the server is started from different
            // directories between runs (caused mysterious "sheets: []" on
            // restart during dev). Also report how many artifacts loaded.
            let storage_abs = std::fs::canonicalize(&dump_dir)
                .unwrap_or_else(|_| std::path::PathBuf::from(&dump_dir))
                .display()
                .to_string();
            let n_loaded = runtime.registry.len();
            println!("🧪 crdt-yws listening on http://{addr}");
            println!("   storage → {storage_abs}");
            println!("   loaded  → {n_loaded} artifact(s) from disk");
            if dump_dir.starts_with('.') || !dump_dir.starts_with('/') {
                println!(
                    "   ⚠️  --dump-dir is RELATIVE; start the server from a stable cwd \
                     or pass an absolute path to keep artifacts across restarts."
                );
            }
            axum::serve(listener, app).await?;
        }

        Commands::CrdtYwsGraph {
            file_path,
            host,
            port,
            dump_dir,
            agent_session_id,
            include_extra_info,
            seed_artifact_id,
            wait_before_graph,
        } => {
            use colmena::crdt_documents::{process_runtime, ArtifactId, CrdtDocumentsRuntime};
            use colmena::dag_engine::sse_mapper::SseMapper;
            use futures::StreamExt;
            use std::{net::SocketAddr, sync::Arc};

            // 1. Build the runtime ONCE and install it as the process-wide
            //    singleton. Both the WS server and the llm_call dispatcher
            //    will share it.
            let cfg = serde_json::json!({
                "storage_backend": "localfs",
                "storage_root": dump_dir,
            });
            let runtime = Arc::new(
                CrdtDocumentsRuntime::from_config(&cfg)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
            );
            process_runtime::set_global(runtime.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;

            // 1b. Optionally pre-create an artifact so the graph (which
            //     references it by id) finds it on first tool call.
            if let Some(id_str) = seed_artifact_id.as_ref() {
                let aid: ArtifactId = id_str.parse().map_err(|_| {
                    anyhow::anyhow!(
                        "--seed-artifact-id must be a valid ArtifactId (art_<26-char-ULID>): {id_str}"
                    )
                })?;
                let _entry = runtime.registry.get_or_create(&aid, "seed");
                println!("🌱 Seeded artifact {aid}");
            }

            // 2. Start the WS server in the background.
            let app = colmena::crdt_documents::server::router(runtime.clone());
            let addr: SocketAddr = format!("{host}:{port}").parse()?;
            let listener = tokio::net::TcpListener::bind(addr).await?;
            // Same persistence diagnostics as CrdtYws — see comment there.
            let storage_abs = std::fs::canonicalize(&dump_dir)
                .unwrap_or_else(|_| std::path::PathBuf::from(&dump_dir))
                .display()
                .to_string();
            let n_loaded = runtime.registry.len();
            println!("🧪 crdt-yws-graph listening on http://{addr}");
            println!("   storage → {storage_abs}");
            println!("   loaded  → {n_loaded} artifact(s) from disk");
            if !dump_dir.starts_with('/') {
                println!(
                    "   ⚠️  --dump-dir is RELATIVE; start from a stable cwd \
                     or pass an absolute path to keep artifacts across restarts."
                );
            }
            let server_handle = tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, app).await {
                    eprintln!("server error: {e}");
                }
            });

            // 2b. Optional pause so the operator can open the browser
            //     before the graph (and its tool calls) start firing.
            if wait_before_graph > 0 {
                println!(
                    "⏳ Waiting {wait_before_graph}s before starting graph — open the browser now."
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait_before_graph)).await;
            }

            // 3. Build the engine + run the graph against the shared runtime.
            println!("🚀 Ejecutando grafo: {file_path}");
            let engine_config = colmena::dag_engine::engine::EngineConfig::from_env()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let engine = colmena::dag_engine::engine::ColmenaEngine::new(engine_config)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let graph_result: Result<(), anyhow::Error> = async {
                let file_content = tokio::fs::read_to_string(&file_path).await?;
                let graph: colmena::dag_engine::domain::graph::Graph =
                    serde_json::from_str(&file_content)?;
                graph
                    .validate()
                    .map_err(|e| anyhow::anyhow!("Invalid graph: {}", e))?;

                let mut mapper = SseMapper::new();
                let s = engine.execute_stream(
                    graph,
                    None,
                    None,
                    include_extra_info,
                    None,
                    agent_session_id,
                );
                let stream = Box::pin(s);
                tokio::pin!(stream);

                while let Some(result) = stream.next().await {
                    let event = match result {
                        Ok(ev) => ev,
                        Err(e) => {
                            println!(
                                "data: {}\n",
                                serde_json::json!({
                                    "type": "error",
                                    "errorText": e.to_string(),
                                })
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
            }
            .await;

            engine.shutdown().await;
            if let Err(e) = graph_result {
                eprintln!("graph execution failed: {e}");
            }

            // 4. Keep the server alive until Ctrl+C so the operator can
            //    inspect the browser. The shared runtime is owned by this
            //    closure scope; on Ctrl+C we shutdown gracefully so the
            //    last snapshot writer flushes land on disk.
            println!("✅ Graph done. Server still up on http://{addr} — Ctrl+C to exit.");
            tokio::signal::ctrl_c().await?;
            println!("\n⏸  Shutting down server and draining writers…");
            server_handle.abort();
            runtime.shutdown().await;
            println!("✓ Done.");
        }

        Commands::CrdtAgent { mode } => match mode {
            CrdtAgentMode::Ws {
                url,
                sheet,
                addr,
                value,
            } => {
                use colmena::crdt_documents::tool_executor::apply_set_cell_via_ws;
                let json_val = serde_json::Value::String(value);
                apply_set_cell_via_ws(&url, &sheet, &addr, &json_val).await?;
                println!("✓ ws mutation applied");
            }
            CrdtAgentMode::Inproc {
                base_url,
                artifact,
                sheet,
                addr,
                value,
            } => {
                let resp = reqwest::Client::new()
                    .post(format!("{base_url}/crdt/agent-op"))
                    .json(&serde_json::json!({
                        "artifact": artifact,
                        "sheet": sheet,
                        "addr": addr,
                        "value": value,
                    }))
                    .send()
                    .await?;
                println!("✓ in-proc mutation applied (status {})", resp.status());
            }
        },
    }

    Ok(())
}
