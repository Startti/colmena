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

    #[cfg(feature = "python")]
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
            use colmena::dag_engine::application::run_use_case::DagRunUseCase;
            use colmena::dag_engine::domain::events::DagExecutionEvent;
            use colmena::dag_engine::infrastructure::registry::HashMapNodeRegistry;
            use colmena::llm::infrastructure::ConversationRepositoryFactory;
            use futures::StreamExt;
            use std::sync::Arc;

            // Enable verbose mode via flag or env var
            let verbose_env = std::env::var("COLMENA_VERBOSE").map(|v| v == "1" || v == "true").unwrap_or(false);
            colmena::dag_engine::verbose::set_verbose(verbose || verbose_env);

            println!("🚀 Ejecutando grafo: {}", file_path);

            // Bootstrap the engine
            dotenvy::dotenv().ok();
            let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
            let pool = sqlx::PgPool::connect(&db_url).await?;
            let repository_factory = Arc::new(ConversationRepositoryFactory::new());
            let state_repo = Arc::new(
                colmena::dag_engine::infrastructure::persistence::postgres_dag_state_repository::PostgresDagStateRepository::new(pool.clone())
            );
            state_repo
                .migrate()
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;

            // Migrate secure values table
            let secure_value_repo = Arc::new(
                colmena::dag_engine::infrastructure::persistence::PostgresSecureValueRepository::new(pool.clone())
            );
            secure_value_repo
                .migrate()
                .await
                .map_err(|e| anyhow::anyhow!("Secure values migration failed: {:?}", e))?;

            let secure_value_service = Arc::new(
                colmena::dag_engine::application::secure_value_service::SecureValueService::new(
                    secure_value_repo,
                ),
            );

            let registry = HashMapNodeRegistry::new_with_secure_values(
                repository_factory,
                Some(state_repo.clone()
                    as Arc<dyn colmena::dag_engine::domain::state::DagTaskMemoryRepository>),
                Some(secure_value_service.clone()),
            );
            let run_use_case = DagRunUseCase::with_secure_values_and_service(registry.clone(), Some(state_repo), secure_value_service);
            registry.set_subgraph_executor(Arc::new(run_use_case.clone()));

            let file_content = tokio::fs::read_to_string(&file_path).await?;
            let graph: colmena::dag_engine::domain::graph::Graph =
                serde_json::from_str(&file_content)?;

            // State for tracking open text blocks and token counts
            let mut text_block_ids: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut total_prompt_tokens: u32 = 0;
            let mut total_completion_tokens: u32 = 0;
            // Track node_type per node_id so we can include it in node-end events
            let mut node_types: std::collections::HashMap<String, String> = std::collections::HashMap::new();

            // The new active_queue engine natively handles both linear and cyclic graphs
            let s =
                run_use_case.execute_stream(graph, session_id.clone(), answer, include_extra_info);
            let stream = Box::pin(s);
            tokio::pin!(stream);

            while let Some(result) = stream.next().await {
                let event = match result {
                    Ok(ev) => ev,
                    Err(e) => {
                        let err_msg = e.to_string();
                        println!(
                            "data: {}\n",
                            serde_json::json!({ "type": "error", "errorText": err_msg })
                        );
                        continue;
                    }
                };

                // Pre-process: open text blocks for new LLM token streams
                match &event {
                    DagExecutionEvent::LlmToken { node_id, .. } => {
                        if !text_block_ids.contains_key(node_id) {
                            let part_id = format!("txt_{}", uuid::Uuid::new_v4());
                            println!(
                                "data: {}\n",
                                serde_json::json!({ "type": "text-start", "id": part_id })
                            );
                            text_block_ids.insert(node_id.clone(), part_id);
                        }
                    }
                    DagExecutionEvent::NodeFinish { node_id, .. }
                    | DagExecutionEvent::SubgraphNodeFinish { node_id, .. } => {
                        if let Some(part_id) = text_block_ids.remove(node_id) {
                            println!(
                                "data: {}\n",
                                serde_json::json!({ "type": "text-end", "id": part_id })
                            );
                        }
                    }
                    DagExecutionEvent::LlmUsage {
                        prompt_tokens,
                        completion_tokens,
                        ..
                    } => {
                        total_prompt_tokens += *prompt_tokens;
                        total_completion_tokens += *completion_tokens;
                    }
                    _ => {}
                }

                // Map event → Data Stream Protocol
                let protocol_line: Option<serde_json::Value> = match &event {
                    DagExecutionEvent::NodeStart {
                        node_id,
                        config,
                        node_type,
                        inputs,
                    } => {
                        // Record type so node-end can echo it back
                        node_types.insert(node_id.clone(), node_type.clone());
                        // Strip internal engine keys — keep only user-meaningful fields
                        let clean_inputs = if let Some(obj) = inputs.as_object() {
                            serde_json::Value::Object(
                                obj.iter()
                                    .filter(|(k, _)| !k.starts_with("__") && k.as_str() != "session_id")
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect(),
                            )
                        } else {
                            inputs.clone()
                        };
                        Some(serde_json::json!({
                            "type": "node-start",
                            "node_id": node_id,
                            "node_type": node_type,
                            "config": config,
                            "inputs": clean_inputs
                        }))
                    }
                    DagExecutionEvent::TurnStart { turn } => {
                        println!("🔄 [Engine] Starting Turn {}", turn);
                        None
                    }
                    DagExecutionEvent::NodeFinish { node_id, output } => {
                        let ntype = node_types.get(node_id).cloned().unwrap_or_default();
                        Some(serde_json::json!({
                            "type": "node-end",
                            "node_id": node_id,
                            "node_type": ntype,
                            "output": output
                        }))
                    }
                    DagExecutionEvent::SubgraphNodeFinish { node_id, output } => Some(serde_json::json!({
                        "type": "node-end",
                        "node_id": node_id,
                        "node_type": "subgraph",
                        "output": output
                    })),
                    DagExecutionEvent::LlmToken { node_id, token } => Some(
                        serde_json::json!({ "type": "node-delta", "node_id": node_id, "delta": token }),
                    ),
                    DagExecutionEvent::ThinkingToken { node_id, token } => Some(
                        serde_json::json!({ "type": "thinking-delta", "node_id": node_id, "delta": token }),
                    ),
                    DagExecutionEvent::LlmUsage { .. } => None,
                    DagExecutionEvent::GraphUsageSummary { entries } => Some(serde_json::json!({
                        "type": "usage-summary",
                        "nodes": entries
                    })),
                    DagExecutionEvent::LlmToolCall {
                        tool_id,
                        args_chunk,
                        ..
                    } => Some(serde_json::json!({
                        "type": "tool-input-delta",
                        "toolCallId": tool_id,
                        "inputTextDelta": args_chunk
                    })),
                    DagExecutionEvent::LlmToolCallStart {
                        tool_id,
                        tool_name,
                        tool_args,
                        ..
                    } => Some(serde_json::json!({
                        "type": "tool-input-available",
                        "toolCallId": tool_id,
                        "toolName": tool_name,
                        "input": serde_json::from_str::<serde_json::Value>(tool_args).unwrap_or(serde_json::Value::String(tool_args.clone()))
                    })),
                    DagExecutionEvent::LlmToolCallFinish {
                        tool_id, output, ..
                    } => Some(serde_json::json!({
                        "type": "tool-output-available",
                        "toolCallId": tool_id,
                        "output": serde_json::from_str::<serde_json::Value>(output).unwrap_or(serde_json::Value::String(output.clone()))
                    })),
                    DagExecutionEvent::GraphFinish { output } => {
                        let mut finish_reason = "stop";
                        let status = output
                            .get("__colmena_status")
                            .or_else(|| {
                                output
                                    .get("extra_info")
                                    .and_then(|e| e.get("__colmena_status"))
                            })
                            .and_then(|s| s.as_str());

                        if status == Some("SUSPENDED") {
                            finish_reason = "suspended";
                        }

                        Some(serde_json::json!({
                            "type": "finish",
                            "finishReason": finish_reason,
                            "usage": { "promptTokens": total_prompt_tokens, "completionTokens": total_completion_tokens },
                            "output": output
                        }))
                    }
                    DagExecutionEvent::LlmMessageStart { .. } => {
                        // Suppressed — llm_call node-start/-end already wraps the full lifecycle.
                        // Individual API-call steps are visible via tool-input/output events.
                        None
                    }
                    DagExecutionEvent::LlmMessageFinish { .. } => {
                        // Intermediate per-message finish (tool-call step) — suppressed.
                        // The final NodeFinish carries the real output and usage.
                        None
                    }
                    DagExecutionEvent::Error { message } => Some(serde_json::json!({
                        "type": "error", "errorText": message
                    })),
                };

                if let Some(line) = protocol_line {
                    println!("data: {}\n", line);
                }
            }

            // Close any remaining open text blocks
            for (_, part_id) in text_block_ids {
                println!(
                    "data: {}\n",
                    serde_json::json!({ "type": "text-end", "id": part_id })
                );
            }
            println!("data: [DONE]\n");
        }

        Commands::Serve {
            file_path,
            host,
            port,
            verbose,
        } => {
            let verbose_env = std::env::var("COLMENA_VERBOSE").map(|v| v == "1" || v == "true").unwrap_or(false);
            colmena::dag_engine::verbose::set_verbose(verbose || verbose_env);
            println!("🌐 Modo Serve: Iniciando...");
            api::serve_dag(file_path, host, port).await?;
        }
    }

    Ok(())
}
