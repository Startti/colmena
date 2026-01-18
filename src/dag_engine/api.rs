use axum::{
    extract::{Json, State},
    routing::post,
    response::IntoResponse,
    Router,
};
use std::collections::HashMap;
use serde_json::Value;
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
pub async fn serve_dag(
    file_path: String,
    host: String,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
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

                let state = AppState {
                    graph: graph_arc.clone(),
                    use_case: run_use_case.clone(),
                };

                app = app.route(
                    path,
                    post(handler_webhook)
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
    let addr_str = format!("{}:{}", host, port);
    println!("✅ Server listening on http://{}", addr_str);

    let listener = tokio::net::TcpListener::bind(&addr_str).await?;
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
async fn handler_webhook(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> axum::response::Response {
    println!("🔔 Webhook received.");
    
    // Debug: Print headers to see what Postman sends
    for (key, value) in &headers {
        println!("   Header: {:?}: {:?}", key, value);
    }

    // Check for "Accept: text/event-stream" or Vercel header OR query param
    let is_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false)
        || headers.contains_key("x-vercel-ai-ui-message-stream")
        || params.get("stream").map(|v| v == "true").unwrap_or(false);

    // Clone the graph for this specific execution
    let mut graph_instance = (*state.graph).clone();

    // Find the trigger node (we iterate to find 'trigger_webhook' type)
    // Note: In the current simpler implementation we might assume there is one trigger or we inject to all.
    // The previous code injected to "trigger_node_id" passed as closure.
    // However, axum handler here is generic.
    // To solve the closure context issue, we'll iterate and inject to all trigger_webhooks in the graph.
    for (_, node) in graph_instance.nodes.iter_mut() {
        if node.node_type == "trigger_webhook" {
            if node.config.is_null() {
                node.config = serde_json::json!({});
            }
            node.config["__payload__"] = payload.clone();
        }
    }

    if is_sse {
        use axum::response::sse::{Event, KeepAlive, Sse};
        use futures::StreamExt;

        let use_case = (*state.use_case).clone();
        let stream = use_case.execute_stream(graph_instance).map(|result| {
            match result {
                Ok(event) => {
                    use crate::dag_engine::domain::events::DagExecutionEvent;
                    
                    // Map DagExecutionEvent to Vercel Data Stream Protocol JSON
                    let protocol_json = match event {
                        DagExecutionEvent::LlmToken { token, .. } => serde_json::json!({
                            "type": "text-delta",
                            "textDelta": token
                        }),
                        DagExecutionEvent::LlmToolCall { tool_id, args_chunk, .. } => serde_json::json!({
                            "type": "tool-input-delta",
                            "toolCallId": tool_id,
                            "argsTextDelta": args_chunk
                        }),
                        DagExecutionEvent::LlmToolCallStart { tool_id, tool_name, tool_args, .. } => serde_json::json!({
                            "type": "tool-input-available",
                            "toolCallId": tool_id,
                            "toolName": tool_name,
                            "input": serde_json::from_str::<serde_json::Value>(&tool_args).unwrap_or(serde_json::Value::String(tool_args))
                        }),
                        DagExecutionEvent::LlmToolCallFinish { tool_id, output, success, .. } => {
                             // Treat output as result. If it's a JSON string, parse it.
                             let result_val = serde_json::from_str::<serde_json::Value>(&output)
                                .unwrap_or(serde_json::Value::String(output));
                                
                             // If tool failed, we might want to signal error, but protocol says "result". 
                             // We'll send the output as is.
                             serde_json::json!({
                                "type": "tool-output-available",
                                "toolCallId": tool_id,
                                "output": result_val,
                                "isError": !success 
                            })
                        },
                        DagExecutionEvent::LlmUsage { prompt_tokens, completion_tokens, .. } => serde_json::json!({
                            "type": "finish",
                            "usage": {
                                "promptTokens": prompt_tokens,
                                "completionTokens": completion_tokens
                            }
                        }),
                        // Internal events - Keep valid JSON but use custom types for debugging/logging
                        DagExecutionEvent::NodeStart { node_id, node_type, .. } => serde_json::json!({
                            "type": "custom-node-start",
                            "nodeId": node_id,
                            "nodeType": node_type
                        }),
                        DagExecutionEvent::NodeFinish { node_id, output } => serde_json::json!({
                            "type": "custom-node-finish",
                            "nodeId": node_id,
                            "output": output
                        }),
                        DagExecutionEvent::GraphFinish { output } => serde_json::json!({
                            "type": "custom-graph-finish",
                            "output": output
                        }),
                        DagExecutionEvent::Error { message } => serde_json::json!({
                            "type": "error",
                            "error": message
                        }),
                    };
                    
                    Event::default().json_data(protocol_json)
                }
                Err(e) => {
                     // Error event
                     let err_json = serde_json::json!({
                        "type": "error",
                        "error": e.to_string()
                     });
                     Event::default().json_data(err_json)
                }
            }
        });
        
        // Append [DONE] event? The Vercel protocol usually implies connection close or explicit DONE.
        // We'll trust the stream end. But AI SDK often expects a [DONE] if strictly following text stream, 
        // for Data stream it might just end. Let's add a wrapper stream to append [DONE] if needed, 
        // but for now let's serve the data events.
        
        Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        // Normal JSON execution
        match state.use_case.execute(graph_instance).await {
            Ok(output) => {
                println!("✅ Execution successful.");
                Json(output).into_response()
            }
            Err(e) => {
                eprintln!("❌ Execution error: {}", e);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}
