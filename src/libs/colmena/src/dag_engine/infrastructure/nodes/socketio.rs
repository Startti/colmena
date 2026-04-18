//! Socket.IO request node — emits events to Socket.IO servers from a DAG.
//!
//! ## Standalone use
//! Configure via `config`: `url`, `namespace`, `event`, `payload`, `headers`, `cookies`,
//! `wait_event`, `timeout_ms`, `transport`. All string values support `${ENV_VAR}` resolution.
//! Input edges override config values (inputs take priority over config).
//!
//! ## As an LLM tool (via `tool_configurations`)
//! When invoked by `DagToolExecutor`, the LLM provides the dynamic fields (e.g., `payload`)
//! while fixed fields (url, namespace, auth) are pre-configured in `node_schema`.
//!
//! ## Response patterns
//! - **Ack mode** (default): Uses Socket.IO acknowledgment callback for the response.
//! - **Wait-event mode** (`wait_event` set): Listens for a separate server event as the response.
//!
//! ## Outputs
//! Returns `{ "success": bool, "event": string, "response": Value }`.
//! The default output port is `response`.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use futures::FutureExt;
use rust_socketio::asynchronous::ClientBuilder;
use rust_socketio::{Payload, TransportType};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Emits Socket.IO events and collects responses. Implements [`ExecutableNode`].
/// Stateless — each execution creates a fresh connection.
pub struct SocketIoNode;

impl SocketIoNode {
    /// Resolve `${ENV_VAR}` placeholders in a string. Identical to HttpNode's resolver.
    fn resolve_env_vars(input: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut last_end = 0;

        while let Some(start) = input[last_end..].find("${") {
            let absolute_start = last_end + start;
            result.push_str(&input[last_end..absolute_start]);

            if let Some(end) = input[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_name = &input[absolute_start + 2..absolute_end];
                let val = std::env::var(var_name)
                    .map_err(|_| format!("Env var {} not found", var_name))?;
                result.push_str(&val);
                last_end = absolute_end + 1;
            } else {
                result.push_str(&input[absolute_start..]);
                last_end = input.len();
                break;
            }
        }
        result.push_str(&input[last_end..]);
        Ok(result)
    }

    /// Resolve env vars in all string values within a JSON Value (recursive).
    fn resolve_env_vars_in_value(val: &Value) -> Result<Value, String> {
        match val {
            Value::String(s) => Ok(Value::String(Self::resolve_env_vars(s)?)),
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), Self::resolve_env_vars_in_value(v)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(arr) => {
                let out: Result<Vec<Value>, String> =
                    arr.iter().map(Self::resolve_env_vars_in_value).collect();
                Ok(Value::Array(out?))
            }
            other => Ok(other.clone()),
        }
    }

    /// Extract a Value from the Payload enum.
    fn payload_to_value(payload: Payload) -> Value {
        match payload {
            Payload::Text(values) => {
                if values.len() == 1 {
                    values.into_iter().next().unwrap_or(Value::Null)
                } else {
                    Value::Array(values)
                }
            }
            Payload::Binary(bytes) => {
                json!({ "__binary": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes) })
            }
            #[allow(deprecated)]
            Payload::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        }
    }

    /// Helper to read a string field from inputs (priority) or config.
    fn get_str<'a>(inputs: &'a NodeInputs, config: &'a Value, key: &str) -> Option<&'a str> {
        inputs
            .get(key)
            .and_then(|v| v.as_str())
            .or_else(|| config.get(key).and_then(|v| v.as_str()))
    }

    /// Helper to read a u64 field from inputs (priority) or config.
    fn get_u64(inputs: &NodeInputs, config: &Value, key: &str) -> Option<u64> {
        inputs
            .get(key)
            .and_then(|v| v.as_u64())
            .or_else(|| config.get(key).and_then(|v| v.as_u64()))
    }
}

#[async_trait::async_trait]
impl ExecutableNode for SocketIoNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // 1. Resolve configuration (inputs > config)
        let url_raw =
            Self::get_str(inputs, config, "url").ok_or("socketio_request: 'url' is required")?;
        let url = Self::resolve_env_vars(url_raw).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        let namespace = Self::get_str(inputs, config, "namespace").unwrap_or("/");
        let namespace = Self::resolve_env_vars(namespace).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        let event_name = Self::get_str(inputs, config, "event")
            .ok_or("socketio_request: 'event' is required")?
            .to_string();

        let wait_event = Self::get_str(inputs, config, "wait_event").map(|s| s.to_string());
        let timeout_ms = Self::get_u64(inputs, config, "timeout_ms").unwrap_or(10000);
        let transport = Self::get_str(inputs, config, "transport").unwrap_or("any");

        // Resolve payload (inputs > config, with env var resolution)
        let payload_val = inputs
            .get("payload")
            .or_else(|| config.get("payload"))
            .cloned()
            .unwrap_or(json!({}));
        let payload_val = Self::resolve_env_vars_in_value(&payload_val).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        println!(
            "[SocketIoNode] → {} {} (namespace: {}, wait_event: {:?})",
            event_name, url, namespace, wait_event
        );
        // Debug: log the full payload being sent
        println!(
            "[SocketIoNode] 📤 payload: {}",
            serde_json::to_string_pretty(&payload_val)
                .unwrap_or_else(|_| format!("{:?}", payload_val))
        );

        // 2. Build client
        let transport_type = match transport {
            "websocket" => TransportType::Websocket,
            "polling" => TransportType::Polling,
            _ => TransportType::Any,
        };

        let mut builder = ClientBuilder::new(&url)
            .namespace(&namespace)
            .transport_type(transport_type)
            .reconnect(false);

        // Apply cookies as opening header
        if let Some(cookies_raw) = Self::get_str(inputs, config, "cookies") {
            let cookies = Self::resolve_env_vars(cookies_raw).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>
            })?;
            builder = builder.opening_header("Cookie", cookies);
        }

        // Apply custom headers
        let headers_val = inputs.get("headers").or_else(|| config.get("headers"));
        if let Some(headers) = headers_val.and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    builder = builder.opening_header(k, v_resolved);
                }
            }
        }

        // 3. Debug handlers — log connection lifecycle events
        builder = builder.on("error", |payload, _client| {
            async move {
                println!("[SocketIoNode] ⚠ server error event: {:?}", payload);
            }
            .boxed()
        });
        builder = builder.on("connect_error", |payload, _client| {
            async move {
                println!("[SocketIoNode] ⚠ connect_error event: {:?}", payload);
            }
            .boxed()
        });
        builder = builder.on("disconnect", |payload, _client| {
            async move {
                println!("[SocketIoNode] ⚠ disconnect event: {:?}", payload);
            }
            .boxed()
        });

        // 4. Exception handler — catch server-side errors and fail fast
        let (exc_tx, exc_rx) = tokio::sync::oneshot::channel::<Value>();
        let exc_tx = Arc::new(Mutex::new(Some(exc_tx)));
        builder = builder.on("exception", move |payload, _client| {
            let exc_tx = exc_tx.clone();
            async move {
                let val = Self::payload_to_value(payload);
                println!(
                    "[SocketIoNode] ⚠ exception: {}",
                    serde_json::to_string(&val).unwrap_or_else(|_| format!("{:?}", val))
                );
                if let Some(sender) = exc_tx.lock().await.take() {
                    let _ = sender.send(val);
                }
            }
            .boxed()
        });
        let exc_rx = Arc::new(Mutex::new(Some(exc_rx)));

        // 5. Set up wait_event listener if needed
        let response_rx = if let Some(ref wait_ev) = wait_event {
            let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
            let tx = Arc::new(Mutex::new(Some(tx)));
            let wait_ev_clone = wait_ev.clone();

            builder = builder.on(wait_ev_clone, move |payload, _client| {
                let tx = tx.clone();
                async move {
                    println!("[SocketIoNode] ✓ received wait_event, forwarding to channel");
                    if let Some(sender) = tx.lock().await.take() {
                        let val = Self::payload_to_value(payload);
                        let _ = sender.send(val);
                    }
                }
                .boxed()
            });
            Some(rx)
        } else {
            None
        };

        // Catch-all handler for debugging: log any unhandled event
        builder = builder.on_any(move |event, payload, _client| {
            async move {
                let preview = match &payload {
                    Payload::Text(vals) => {
                        let s = format!("{:?}", vals);
                        if s.len() > 500 {
                            format!("{}…", &s[..500])
                        } else {
                            s
                        }
                    }
                    _ => format!("{:?}", payload),
                };
                println!("[SocketIoNode] 📡 event '{}': {}", event, preview);
            }
            .boxed()
        });

        // 6. Connect
        let client = builder.connect().await.map_err(|e| {
            format!(
                "socketio_request: failed to connect to {} (namespace {}): {}",
                url, namespace, e
            )
        })?;

        // Small delay to let the connection fully establish
        tokio::time::sleep(Duration::from_millis(200)).await;

        // 7. Emit and wait for response (racing against exception channel)
        let timeout_dur = Duration::from_millis(timeout_ms);
        let event_name_clone = event_name.clone();

        let result: Result<Value, Box<dyn StdError + Send + Sync>> = if let Some(rx) = response_rx {
            // Wait-event mode: emit, then race wait_event vs exception vs timeout
            client
                .emit(event_name.clone(), payload_val)
                .await
                .map_err(|e| format!("socketio_request: failed to emit '{}': {}", event_name, e))?;

            let exc_rx_opt = exc_rx.lock().await.take();
            if let Some(exc_rx_inner) = exc_rx_opt {
                tokio::select! {
                    response = rx => {
                        match response {
                            Ok(val) => Ok(val),
                            Err(_) => Ok(json!({
                                "success": false,
                                "event": event_name_clone,
                                "error": "wait_event channel closed unexpectedly"
                            })),
                        }
                    }
                    exception = exc_rx_inner => {
                        match exception {
                            Ok(val) => {
                                let msg = val.get("message").and_then(|m| m.as_str())
                                    .unwrap_or("Server exception");
                                Ok(json!({
                                    "success": false,
                                    "event": event_name_clone,
                                    "error": msg,
                                    "exception": val
                                }))
                            }
                            Err(_) => Ok(json!({
                                "success": false,
                                "event": event_name_clone,
                                "error": "exception channel closed unexpectedly"
                            })),
                        }
                    }
                    _ = tokio::time::sleep(timeout_dur) => {
                        Ok(json!({
                            "success": false,
                            "event": event_name_clone,
                            "error": format!(
                                "Timeout waiting for '{}' after {}ms",
                                wait_event.as_deref().unwrap_or("?"),
                                timeout_ms
                            )
                        }))
                    }
                }
            } else {
                match tokio::time::timeout(timeout_dur, rx).await {
                    Ok(Ok(val)) => Ok(val),
                    Ok(Err(_)) => Ok(json!({
                        "success": false,
                        "event": event_name_clone,
                        "error": "wait_event channel closed unexpectedly"
                    })),
                    Err(_) => Ok(json!({
                        "success": false,
                        "event": event_name_clone,
                        "error": format!(
                            "Timeout waiting for '{}' after {}ms",
                            wait_event.as_deref().unwrap_or("?"),
                            timeout_ms
                        )
                    })),
                }
            }
        } else {
            // Ack mode: emit_with_ack, race ack vs exception vs timeout
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<Value>();
            let ack_tx = Arc::new(Mutex::new(Some(ack_tx)));

            client
                .emit_with_ack(
                    event_name.clone(),
                    payload_val,
                    timeout_dur,
                    move |payload: Payload, _client| {
                        let ack_tx = ack_tx.clone();
                        async move {
                            if let Some(sender) = ack_tx.lock().await.take() {
                                let val = Self::payload_to_value(payload);
                                let _ = sender.send(val);
                            }
                        }
                        .boxed()
                    },
                )
                .await
                .map_err(|e| {
                    format!(
                        "socketio_request: failed to emit_with_ack '{}': {}",
                        event_name, e
                    )
                })?;

            let exc_rx_opt = exc_rx.lock().await.take();
            if let Some(exc_rx_inner) = exc_rx_opt {
                tokio::select! {
                    ack = ack_rx => {
                        match ack {
                            Ok(val) => Ok(val),
                            Err(_) => Ok(json!({
                                "success": false,
                                "event": event_name_clone,
                                "error": "ack channel closed unexpectedly"
                            })),
                        }
                    }
                    exception = exc_rx_inner => {
                        match exception {
                            Ok(val) => {
                                let msg = val.get("message").and_then(|m| m.as_str())
                                    .unwrap_or("Server exception");
                                Ok(json!({
                                    "success": false,
                                    "event": event_name_clone,
                                    "error": msg,
                                    "exception": val
                                }))
                            }
                            Err(_) => Ok(json!({
                                "success": false,
                                "event": event_name_clone,
                                "error": "exception channel closed unexpectedly"
                            })),
                        }
                    }
                    _ = tokio::time::sleep(timeout_dur) => {
                        Ok(json!({
                            "success": false,
                            "event": event_name_clone,
                            "error": format!("Timeout waiting for ack on '{}' after {}ms", event_name_clone, timeout_ms)
                        }))
                    }
                }
            } else {
                match tokio::time::timeout(timeout_dur, ack_rx).await {
                    Ok(Ok(val)) => Ok(val),
                    Ok(Err(_)) => Ok(json!({
                        "success": false,
                        "event": event_name_clone,
                        "error": "ack channel closed unexpectedly"
                    })),
                    Err(_) => Ok(json!({
                        "success": false,
                        "event": event_name_clone,
                        "error": format!("Timeout waiting for ack on '{}' after {}ms", event_name_clone, timeout_ms)
                    })),
                }
            }
        };

        // 6. Disconnect
        let _ = client.disconnect().await;

        // 7. Build output
        match result {
            Ok(val) => {
                // Check if the value is already a failure envelope we built
                if val.get("success").and_then(|v| v.as_bool()) == Some(false) {
                    println!(
                        "[SocketIoNode] ← error: {}",
                        val.get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                    );
                    Ok(val)
                } else {
                    println!(
                        "[SocketIoNode] ← response received for '{}'",
                        event_name_clone
                    );
                    Ok(json!({
                        "success": true,
                        "event": event_name_clone,
                        "response": val
                    }))
                }
            }
            Err(e) => {
                println!("[SocketIoNode] ← error: {}", e);
                Err(e)
            }
        }
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Connect to a Socket.IO server, emit an event with a JSON payload, \
             and receive the response via acknowledgment callback or a separate server event. \
             Supports namespaces, cookie-based authentication, and custom headers.",
        )
    }

    fn default_output(&self) -> Option<&str> {
        Some("response")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "socketio_request",
            "config": {
                "url": "string (required, supports ${ENV_VAR})",
                "namespace": "string (default: /)",
                "event": "string (required)",
                "payload": "any (optional, default: {})",
                "headers": "map<string, string> (optional, supports ${ENV_VAR})",
                "cookies": "string (optional, shorthand for Cookie header, supports ${ENV_VAR})",
                "wait_event": "string (optional, listen for this server event instead of using ack)",
                "timeout_ms": "integer (default: 10000)",
                "transport": "string (any|websocket|polling, default: any)"
            },
            "inputs": {
                "url": "string (optional override)",
                "namespace": "string (optional override)",
                "event": "string (optional override)",
                "payload": "any (optional override)",
                "headers": "map<string, string> (optional override)",
                "cookies": "string (optional override)",
                "wait_event": "string (optional override)",
                "timeout_ms": "integer (optional override)"
            },
            "outputs": {
                "success": "boolean",
                "event": "string",
                "response": "any"
            }
        })
    }
}
