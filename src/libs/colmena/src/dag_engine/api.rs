use axum::{
    extract::{Json, State},
    response::IntoResponse,
    routing::post,
    Router,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// Import from crate since this is part of the colmena library
use crate::dag_engine::domain::graph::Graph;

pub async fn run_dag(
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let engine_config = crate::dag_engine::engine::EngineConfig::from_env()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let engine = crate::dag_engine::engine::ColmenaEngine::new(engine_config)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    let result: Result<Value, Box<dyn std::error::Error>> = async {
        // Load and execute the graph
        let file_content = tokio::fs::read_to_string(&file_path).await?;
        let mut graph: Graph = serde_json::from_str(&file_content)?;

        // If an injected payload was provided (e.g. from a previous loop), inject it into start nodes
        if let Some(payload) = inject_payload {
            for (_, node) in graph.nodes.iter_mut() {
                if node.node_type == "trigger_webhook"
                    || node.node_type == "input"
                    || node.node_type == "mock_input"
                {
                    if node.config.is_null() {
                        node.config = serde_json::json!({});
                    }
                    node.config["__payload__"] = payload.clone();
                }
            }
        }

        // Check if any node has streaming enabled
        let is_stream = graph.nodes.values().any(|node| {
            node.config
                .get("stream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });

        if is_stream {
            use crate::dag_engine::domain::events::DagExecutionEvent;
            use futures::StreamExt;

            let internal_stream = engine.execute_stream(
                graph,
                resume_id.clone(),
                resume_answer.clone(),
                include_extra_info,
            );
            tokio::pin!(internal_stream);

            // 1. Send the global START part
            println!(
                "data: {}\n",
                serde_json::json!({
                    "type": "start",
                    "messageId": format!("msg_{}", uuid::Uuid::new_v4())
                })
            );

            let mut text_block_uuids = std::collections::HashMap::new();
            let mut seen_tool_ids = std::collections::HashSet::new();
            let mut total_prompt_tokens = 0;
            let mut total_completion_tokens = 0;
            let mut final_output: Value = Value::Null;

            while let Some(result) = internal_stream.next().await {
                let event = match result {
                    Ok(ev) => ev,
                    Err(e) => {
                        println!(
                            "data: {}\n",
                            serde_json::json!({
                                "type": "error",
                                "errorText": e.to_string()
                            })
                        );
                        continue;
                    }
                };

                // Protocol State Management
                match &event {
                    DagExecutionEvent::LlmToken { node_id, .. }
                        if !text_block_uuids.contains_key(node_id) =>
                    {
                        let part_id = format!("txt_{}", uuid::Uuid::new_v4());
                        println!(
                            "data: {}\n",
                            serde_json::json!({
                                "type": "text-start",
                                "id": part_id
                            })
                        );
                        text_block_uuids.insert(node_id.clone(), part_id);
                    }
                    DagExecutionEvent::NodeFinish { node_id, .. }
                    | DagExecutionEvent::SubgraphNodeFinish { node_id, .. } => {
                        if let Some(part_id) = text_block_uuids.remove(node_id) {
                            println!(
                                "data: {}\n",
                                serde_json::json!({
                                    "type": "text-end",
                                    "id": part_id
                                })
                            );
                        }
                    }
                    DagExecutionEvent::LlmUsage {
                        prompt_tokens,
                        completion_tokens,
                        ..
                    } => {
                        total_prompt_tokens += prompt_tokens;
                        total_completion_tokens += completion_tokens;
                    }
                    DagExecutionEvent::LlmToolCall {
                        tool_id, tool_name, ..
                    } if !seen_tool_ids.contains(tool_id) => {
                        seen_tool_ids.insert(tool_id.clone());
                        println!(
                            "data: {}\n",
                            serde_json::json!({
                                "type": "tool-input-start",
                                "toolCallId": tool_id,
                                "toolName": tool_name
                            })
                        );
                    }
                    DagExecutionEvent::GraphFinish { output } => {
                        final_output = output.clone();
                    }
                    _ => {}
                }

                // Map to official Data Stream Protocol JSON
                let protocol_json = match event {
                    DagExecutionEvent::LlmToken { node_id, token } => {
                        let part_id = text_block_uuids
                            .get(&node_id)
                            .cloned()
                            .unwrap_or_else(|| node_id.clone());
                        Some(serde_json::json!({
                            "type": "text-delta",
                            "id": part_id,
                            "delta": token
                        }))
                    }
                    DagExecutionEvent::ThinkingToken { node_id, token } => Some(serde_json::json!({
                        "type": "thinking-delta",
                        "node_id": node_id,
                        "delta": token
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
                        "input": serde_json::from_str::<serde_json::Value>(&tool_args).unwrap_or(serde_json::Value::String(tool_args))
                    })),
                    DagExecutionEvent::LlmToolCallFinish {
                        tool_id, output, ..
                    } => Some(serde_json::json!({
                        "type": "tool-output-available",
                        "toolCallId": tool_id,
                        "output": serde_json::from_str::<serde_json::Value>(&output).unwrap_or(serde_json::Value::String(output))
                    })),
                    DagExecutionEvent::LlmUsage { .. } => None,
                    DagExecutionEvent::GraphUsageSummary { entries } => Some(serde_json::json!({
                        "type": "usage-summary",
                        "nodes": entries
                    })),
                    DagExecutionEvent::NodeFinish { node_id, .. } => {
                        text_block_uuids.get(&node_id).map(|part_id| {
                            serde_json::json!({
                                "type": "node-end",
                                "id": part_id,
                                "usage": {
                                    "promptTokens": total_prompt_tokens,
                                    "completionTokens": total_completion_tokens
                                }
                            })
                        })
                    }
                    DagExecutionEvent::SubgraphNodeFinish { node_id, output } => {
                        Some(serde_json::json!({
                            "type": "node-end",
                            "node_id": node_id,
                            "node_type": "subgraph",
                            "output": output
                        }))
                    }
                    DagExecutionEvent::GraphFinish { .. } => Some(serde_json::json!({
                        "type": "finish",
                        "finishReason": "stop",
                        "usage": {
                            "promptTokens": total_prompt_tokens,
                            "completionTokens": total_completion_tokens
                        }
                    })),
                    DagExecutionEvent::Error { message } => Some(serde_json::json!({
                        "type": "error",
                        "errorText": message
                    })),
                    _ => None,
                };

                if let Some(json) = protocol_json {
                    println!("data: {}\n", json);
                }
            }

            // 3. Finalization: Ensure all pending text blocks are ended
            for (_, part_id) in text_block_uuids {
                println!(
                    "data: {}\n",
                    serde_json::json!({
                        "type": "text-end",
                        "id": part_id
                    })
                );
            }

            // 4. Send the literal [DONE] marker
            println!("data: [DONE]\n");

            Ok(final_output)
        } else {
            engine
                .run_dag(graph, resume_id, resume_answer, include_extra_info)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
    }
    .await;

    engine.shutdown().await;
    result
}

/// Serve a DAG as an HTTP API
pub async fn serve_dag(
    file_path: String,
    host: String,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let engine_config = crate::dag_engine::engine::EngineConfig::from_env()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let engine = Arc::new(
        crate::dag_engine::engine::ColmenaEngine::new(engine_config)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?,
    );

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
                    engine: engine.clone(),
                };

                app = app.route(path, post(handler_webhook).with_state(state));
                routes_count += 1;
            }
        }
    }

    if routes_count == 0 {
        eprintln!(
            "⚠️ ALERT: No 'trigger_webhook' nodes found. The server is running but has no default routes."
        );
    } else {
        // Also register the resume route when serving
        let state = AppState {
            graph: graph_arc.clone(),
            engine: engine.clone(),
        };
        println!("   └── Registering route: POST /resume (System)");
        app = app.route("/resume", post(handler_resume).with_state(state));
    }

    // Start the TCP server
    let addr_str = format!("{}:{}", host, port);
    println!("✅ Server listening on http://{}", addr_str);

    let shutdown_future = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let serve_result: Result<(), std::io::Error> = async {
        let listener = tokio::net::TcpListener::bind(&addr_str).await?;
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_future)
            .await
    }
    .await;
    engine.shutdown().await;
    serve_result?;
    Ok(())
}

// Shared state for Axum handlers
#[derive(Clone)]
struct AppState {
    graph: Arc<Graph>,
    engine: Arc<crate::dag_engine::engine::ColmenaEngine>,
}

#[derive(serde::Deserialize)]
struct ResumePayload {
    session_id: String,
    answer: String,
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
        use crate::dag_engine::domain::events::DagExecutionEvent;
        use axum::response::sse::{Event, KeepAlive, Sse};
        use futures::StreamExt;

        let engine = state.engine.clone();

        // Wrap the internal stream to manage protocol state (text-start, text-end, [DONE])
        let protocol_stream = async_stream::stream! {
            let is_loop = params.get("loop").map(|v| v == "true").unwrap_or(false);
            let mut turn_count = 1;
            let mut current_graph = graph_instance;

            loop {
                // Node start messages are emitted dynamically below.

                let mut text_block_uuids = std::collections::HashMap::new();
                let mut seen_tool_ids = std::collections::HashSet::new();
                let mut total_prompt_tokens = 0;
                let mut total_completion_tokens = 0;
                let mut node_types: std::collections::HashMap<String, String> = std::collections::HashMap::new();

                // Emitting custom event to show turn change in UI
                if is_loop {
                    yield Ok::<Event, std::io::Error>(Event::default().json_data(serde_json::json!({
                        "type": "text-delta",
                        "id": format!("txt_sys_{}", uuid::Uuid::new_v4()),
                        "delta": format!("\n\n*--- Starting Turn {} ---*\n\n", turn_count)
                    })).expect("json_data"));
                }

                let internal_stream = engine.execute_stream(current_graph.clone(), None, None, false);
                let mut final_output_value: Option<Value> = None;

                tokio::pin!(internal_stream);

                while let Some(result) = internal_stream.next().await {
                    // Here we ignore DAG errors for the protocol stream but we could also yield an Error part
                    let event = match result {
                        Ok(ev) => ev,
                        Err(e) => {
                            yield Ok(Event::default().json_data(serde_json::json!({
                                "type": "error",
                                "errorText": e.to_string()
                            })).expect("json_data"));
                            continue;
                        }
                    };

                    // Protocol State Management
                    match &event {
                        DagExecutionEvent::LlmToken { node_id, .. }
                            if !text_block_uuids.contains_key(node_id) =>
                        {
                            let part_id = format!("txt_{}", uuid::Uuid::new_v4());
                            yield Ok(Event::default().json_data(serde_json::json!({
                                "type": "text-start",
                                "id": part_id
                            })).expect("json_data"));
                            text_block_uuids.insert(node_id.clone(), part_id);
                        },
                        DagExecutionEvent::NodeFinish { node_id, .. } => {
                            if let Some(part_id) = text_block_uuids.remove(node_id) {
                                yield Ok(Event::default().json_data(serde_json::json!({
                                    "type": "text-end",
                                    "id": part_id
                                })).expect("json_data"));
                            }
                        },
                        DagExecutionEvent::LlmUsage { prompt_tokens, completion_tokens, .. } => {
                            total_prompt_tokens += prompt_tokens;
                            total_completion_tokens += completion_tokens;
                        },
                        DagExecutionEvent::LlmToolCall { tool_id, tool_name, .. }
                            if !seen_tool_ids.contains(tool_id) =>
                        {
                            seen_tool_ids.insert(tool_id.clone());
                            yield Ok(Event::default().json_data(serde_json::json!({
                                "type": "tool-input-start",
                                "toolCallId": tool_id,
                                "toolName": tool_name
                            })).expect("json_data"));
                        },
                        DagExecutionEvent::GraphFinish { output } => {
                            final_output_value = Some(output.clone());
                        },
                        _ => {}
                    }

                    // Map to official Data Stream Protocol JSON
                    let protocol_json = match &event {
                        DagExecutionEvent::NodeStart { node_id, config, node_type, inputs } => {
                            node_types.insert(node_id.clone(), node_type.clone());
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
                        },
                        DagExecutionEvent::NodeFinish { node_id, output } => {
                            let ntype = node_types.get(node_id).cloned().unwrap_or_default();
                            Some(serde_json::json!({
                                "type": "node-end",
                                "node_id": node_id,
                                "node_type": ntype,
                                "output": output
                            }))
                        },
                        DagExecutionEvent::LlmMessageFinish { .. } => {
                            // Intermediate per-message finish (tool-call step) — suppressed.
                            // The final NodeFinish carries the real output and usage.
                            None
                        },
                        DagExecutionEvent::LlmToken { node_id, token } => {
                            Some(serde_json::json!({
                                "type": "node-delta",
                                "node_id": node_id,
                                "delta": token
                            }))
                        },
                        DagExecutionEvent::ThinkingToken { node_id, token } => {
                            Some(serde_json::json!({
                                "type": "thinking-delta",
                                "node_id": node_id,
                                "delta": token
                            }))
                        },
                        DagExecutionEvent::LlmToolCall { tool_id, args_chunk, .. } => Some(serde_json::json!({
                            "type": "tool-input-delta",
                            "toolCallId": tool_id,
                            "inputTextDelta": args_chunk
                        })),
                        DagExecutionEvent::LlmToolCallStart { tool_id, tool_name, tool_args, .. } => Some(serde_json::json!({
                            "type": "tool-input-available",
                            "toolCallId": tool_id,
                            "toolName": tool_name,
                            "input": serde_json::from_str::<serde_json::Value>(tool_args).unwrap_or(serde_json::Value::String(tool_args.clone()))
                        })),
                        DagExecutionEvent::LlmToolCallFinish { tool_id, output, .. } => Some(serde_json::json!({
                            "type": "tool-output-available",
                            "toolCallId": tool_id,
                            "output": serde_json::from_str::<serde_json::Value>(output).unwrap_or(serde_json::Value::String(output.clone()))
                        })),
                        DagExecutionEvent::LlmUsage { .. } => None,
                        DagExecutionEvent::GraphUsageSummary { entries } => Some(serde_json::json!({
                            "type": "usage-summary",
                            "nodes": entries
                        })),
                        DagExecutionEvent::GraphFinish { .. } => Some(serde_json::json!({
                            "type": "finish",
                            "finishReason": "stop",
                            "usage": {
                                "promptTokens": total_prompt_tokens,
                                "completionTokens": total_completion_tokens
                            }
                        })),
                        DagExecutionEvent::Error { message } => Some(serde_json::json!({
                            "type": "error",
                            "errorText": message
                        })),
                        _ => None
                    };

                    if let Some(json) = protocol_json {
                        yield Ok(Event::default().json_data(json).expect("json_data"));
                    }
                }

                // 3. Finalization: Ensure all pending text blocks are ended for this turn
                for (_, part_id) in text_block_uuids {
                    yield Ok(Event::default().json_data(serde_json::json!({
                        "type": "text-end",
                        "id": part_id
                    })).expect("json_data"));
                }

                // End of turn logic
                let mut should_stop_loop = !is_loop;

                if let Some(out) = final_output_value.as_ref() {
                    if let Some(obj) = out.as_object() {
                        let find_status = |o: &serde_json::Map<String, serde_json::Value>, key: &str| -> Option<String> {
                            if let Some(v) = o.get(key) {
                                return v.as_str().map(|s| s.to_string());
                            }
                            for (_, val) in o {
                                if let Some(child_obj) = val.as_object() {
                                    if let Some(v) = child_obj.get(key) {
                                        return v.as_str().map(|s| s.to_string());
                                    }
                                }
                            }
                            None
                        };

                        let find_bool = |o: &serde_json::Map<String, serde_json::Value>, key: &str| -> bool {
                            if let Some(v) = o.get(key).and_then(|v| v.as_bool()) {
                                if v { return true; }
                            }
                            for (_, val) in o {
                                if let Some(child_obj) = val.as_object() {
                                    if let Some(v) = child_obj.get(key).and_then(|v| v.as_bool()) {
                                        if v { return true; }
                                    }
                                }
                            }
                            false
                        };

                        if let Some(status) = find_status(obj, "__colmena_status") {
                            if status == "SUSPENDED" {
                                should_stop_loop = true;
                            }
                        }
                        if let Some(loop_status) = find_status(obj, "__colmena_loop_status") {
                            if loop_status == "FINISHED" {
                                should_stop_loop = true;
                            }
                        }
                        if find_bool(obj, "__colmena_is_output_node") {
                            should_stop_loop = true;
                        }
                    }

                    if !should_stop_loop {
                        // Prepare next iteration's input
                        for (_, node) in current_graph.nodes.iter_mut() {
                            if node.node_type == "trigger_webhook" || node.node_type == "input" {
                                if node.config.is_null() {
                                    node.config = serde_json::json!({});
                                }
                                node.config["__payload__"] = out.clone();
                            }
                        }
                        turn_count += 1;
                    }
                } else {
                    // No output = graph crashed or finished empty. Stop looping.
                    should_stop_loop = true;
                }

                if should_stop_loop {
                    break;
                }
            }

            // 4. Send the literal [DONE] marker
            yield Ok(Event::default().data("[DONE]"));
        };

        let mut response = Sse::new(protocol_stream)
            .keep_alive(KeepAlive::default())
            .into_response();

        // Essential header for the AI SDK to recognize the Data Stream
        response.headers_mut().insert(
            "x-vercel-ai-ui-message-stream",
            axum::http::HeaderValue::from_static("v1"),
        );

        response
    } else {
        // Normal JSON execution
        let is_loop = params.get("loop").map(|v| v == "true").unwrap_or(false);
        let mut turn_count = 1;
        let mut current_resume_id: Option<String> = None;

        loop {
            if is_loop {
                println!("\n🔄 -- API Turno {} --", turn_count);
            }
            match state
                .engine
                .run_dag(
                    graph_instance.clone(),
                    current_resume_id.clone(),
                    None,
                    false,
                )
                .await
            {
                Ok(mut out) => {
                    let mut should_stop_loop = !is_loop;

                    if let Some(obj) = out.as_object() {
                        let find_field = |o: &serde_json::Map<String, serde_json::Value>,
                                          key: &str|
                         -> Option<serde_json::Value> {
                            // Search root
                            if let Some(v) = o.get(key) {
                                return Some(v.clone());
                            }
                            // Search 1 level deep (since all_outputs has node_id as first level)
                            for (_, val) in o {
                                if let Some(child_obj) = val.as_object() {
                                    // Output is mostly nested inside 'output' key
                                    if let Some(output_obj) =
                                        child_obj.get("output").and_then(|v| v.as_object())
                                    {
                                        if let Some(v) = output_obj.get(key) {
                                            return Some(v.clone());
                                        }
                                        if let Some(extra) =
                                            output_obj.get("extra_info").and_then(|v| v.as_object())
                                        {
                                            if let Some(v) = extra.get(key) {
                                                return Some(v.clone());
                                            }
                                        }
                                        if let Some(res) =
                                            output_obj.get("result").and_then(|v| v.as_object())
                                        {
                                            if let Some(v) = res.get(key) {
                                                return Some(v.clone());
                                            }
                                        }
                                    }
                                    // Direct check just in case
                                    if let Some(v) = child_obj.get(key) {
                                        return Some(v.clone());
                                    }
                                    if let Some(extra) =
                                        child_obj.get("extra_info").and_then(|v| v.as_object())
                                    {
                                        if let Some(v) = extra.get(key) {
                                            return Some(v.clone());
                                        }
                                    }
                                }
                            }
                            None
                        };

                        if let Some(status_val) = find_field(obj, "__colmena_status") {
                            if status_val.as_str() == Some("SUSPENDED") {
                                should_stop_loop = true;
                                println!("⏸️  Ejecución SUSPENDIDA. Esperando input humano.");
                                if let Some(question) = find_field(obj, "question") {
                                    println!(
                                        "❓ Pregunta: {}",
                                        question.as_str().unwrap_or(&question.to_string())
                                    );
                                }
                            }
                        }
                        if let Some(loop_status_val) = find_field(obj, "__colmena_loop_status") {
                            if loop_status_val.as_str() == Some("FINISHED") {
                                should_stop_loop = true;

                                // If an OutputNode ran, its result is the definitive final output of the DAG
                                let mut output_node_result = None;
                                for (_node_name, node_output) in obj {
                                    if let Some(node_output_obj) = node_output.as_object() {
                                        if let Some(extra) =
                                            find_field(node_output_obj, "extra_info")
                                        {
                                            if let Some(is_output) = extra
                                                .as_object()
                                                .and_then(|e| e.get("__colmena_is_output_node"))
                                            {
                                                if is_output.as_bool() == Some(true) {
                                                    output_node_result =
                                                        find_field(node_output_obj, "result");
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }

                                if let Some(actual_output) = output_node_result {
                                    out = serde_json::json!({
                                        "output": {
                                            "__colmena_loop_status": "FINISHED",
                                            "final_result": actual_output
                                        }
                                    });
                                }
                            }
                        }
                    }

                    if should_stop_loop {
                        println!("✅ Execution successful. Returning final output.");
                        return Json(out).into_response();
                    } else {
                        // Prepare next iteration's input
                        println!("Output Parcial: Volviendo a planificar el siguiente turno en background de forma nativa...");

                        // Inject output as input for the next round
                        for (_, node) in graph_instance.nodes.iter_mut() {
                            if node.node_type == "trigger_webhook" || node.node_type == "input" {
                                if node.config.is_null() {
                                    node.config = serde_json::json!({});
                                }
                                node.config["__payload__"] = out.clone();
                            }
                        }

                        if let Some(session_id_val) =
                            out.as_object().and_then(|o| o.get("__colmena_session_id"))
                        {
                            current_resume_id = session_id_val.as_str().map(|s| s.to_string());
                        } else {
                            current_resume_id = None;
                        }

                        turn_count += 1;
                    }
                }
                Err(e) => {
                    eprintln!("❌ Execution error: {}", e);
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": e.to_string() })),
                    )
                        .into_response();
                }
            }
        }
    }
}

/// Handler that executes when a suspended DAG is resumed with human input
async fn handler_resume(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ResumePayload>,
) -> axum::response::Response {
    println!("🔔 Resume requested for session_id: {}", payload.session_id);

    // Check for "Accept: text/event-stream" or Vercel header
    let is_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false)
        || headers.contains_key("x-vercel-ai-ui-message-stream");

    let graph_instance = (*state.graph).clone();

    if is_sse {
        // ... We could duplicate the SSE stream runner here, but for brevity in Phase 1
        // we'll execute the rest. Let's just do a normal execute.
        // If SSE is truly required for resuming, we can abstract the runner.
        eprintln!("⚠️ SSE not fully supported yet on /resume, falling back to JSON");
    }

    match state
        .engine
        .run_dag(
            graph_instance,
            Some(payload.session_id),
            Some(payload.answer),
            false,
        )
        .await
    {
        Ok(output) => {
            println!("✅ Resume successful.");
            Json(output).into_response()
        }
        Err(e) => {
            eprintln!("❌ Resume error: {}", e);
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}
