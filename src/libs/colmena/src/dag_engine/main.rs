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
        #[arg(long)]
        resume_id: Option<String>,
        #[arg(long)]
        answer: Option<String>,
        #[arg(long, default_value_t = false)]
        r#loop: bool,
    },
    Serve {
        file_path: String,
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    println!("DEBUG: DATABASE_URL={:?}", std::env::var("DATABASE_URL"));
    println!(
        "DEBUG: AMADEUS_CLIENT_ID={:?}",
        std::env::var("AMADEUS_CLIENT_ID")
    );
    println!(
        "DEBUG: AMADEUS_CLIENT_SECRET={:?}",
        std::env::var("AMADEUS_CLIENT_SECRET")
    );
    println!(
        "DEBUG: OPENAI_API_KEY={:?}",
        std::env::var("OPENAI_API_KEY")
    );
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file_path, resume_id, answer, r#loop } => {
            println!("🚀 Modo Run: Cargando grafo desde {}", file_path);
            if let Some(id) = &resume_id {
                println!("Reanudando ejecución con ID: {}", id);
            } else if r#loop {
                println!("Ejecutando grafo en modo Bucle (Iterativo)...");
            } else {
                println!("Ejecutando grafo...");
            }

            let mut current_resume_id = resume_id.clone();
            let mut current_answer = answer.clone();
            let mut inject_payload = None;
            let mut turn_count = 1;

            loop {
                if r#loop {
                    println!("\n🔄 -- Turno {} --", turn_count);
                }

                match api::run_dag(file_path.clone(), current_resume_id.clone(), current_answer.clone(), inject_payload.clone()).await {
                    Ok(mut out) => {
                        // Check if we need to stop the loop
                        let mut should_stop_loop = !r#loop; // If not in loop mode, always break after 1 turn

                        if let Some(obj) = out.as_object() {
                            let find_field = |o: &serde_json::Map<String, serde_json::Value>, key: &str| -> Option<serde_json::Value> {
                                // Search root
                                if let Some(v) = o.get(key) {
                                    return Some(v.clone());
                                }
                                // Search 1 level deep (since all_outputs has node_id as first level)
                                for (_, val) in o {
                                    if let Some(child_obj) = val.as_object() {
                                        // Output is mostly nested inside 'output' key
                                        if let Some(output_obj) = child_obj.get("output").and_then(|v| v.as_object()) {
                                            if let Some(v) = output_obj.get(key) {
                                                return Some(v.clone());
                                            }
                                        }
                                        // Direct check just in case
                                        if let Some(v) = child_obj.get(key) {
                                            return Some(v.clone());
                                        }
                                    }
                                }
                                None
                            };

                            // Stop naturally if SUSPENDED
                            if let Some(status_val) = find_field(obj, "__colmena_status") {
                                if status_val.as_str() == Some("SUSPENDED") {
                                    should_stop_loop = true;
                                    println!("⏸️  Ejecución SUSPENDIDA. Esperando input humano.");
                                    if let Some(question) = find_field(obj, "question") {
                                        println!("❓ Pregunta: {}", question.as_str().unwrap_or(&question.to_string()));
                                    }
                                }
                            }
                            
                            // Check explicit loop status flag from planner/orchestrator
                            if let Some(loop_status_val) = find_field(obj, "__colmena_loop_status") {
                                if loop_status_val.as_str() == Some("FINISHED") {
                                    should_stop_loop = true;
                                    if let Some(final_result) = find_field(obj, "all_tasks") {
                                       out = serde_json::json!({
                                            "output": {
                                                "__colmena_loop_status": "FINISHED",
                                                "final_result": final_result
                                            }
                                       });
                                    }
                                }
                            }
                        }

                        if should_stop_loop {
                            println!("Output Final:\n{}", serde_json::to_string_pretty(&out)?);
                            break;
                        } else {
                            println!("Output Parcial: Volviendo a planificar el siguiente turno en background de forma nativa...");
                            // Extract run_id from the output to reuse in the next turn.
                            // When run_id is available, the Orchestrator reads state from Postgres
                            // directly — no need to reinject the full payload via the InputNode.
                            if let Some(run_id_val) = out.as_object().and_then(|o| o.get("__colmena_run_id")) {
                                current_resume_id = run_id_val.as_str().map(|s| s.to_string());
                                inject_payload = None; // Postgres state is the source of truth
                            } else {
                                // Fallback for stateless DAGs (no DB): reinject output as payload
                                inject_payload = Some(out.clone());
                                current_resume_id = None;
                            }
                            current_answer = None;
                            turn_count += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("❌ Error: {}", e);
                        break;
                    }
                }
            }
        }
        Commands::Serve {
            file_path,
            host,
            port,
        } => {
            println!("🌐 Modo Serve: Iniciando...");
            api::serve_dag(file_path, host, port).await?;
        }
    }

    Ok(())
}
