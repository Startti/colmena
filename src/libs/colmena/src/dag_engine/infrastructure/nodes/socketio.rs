//! Socket.IO request node — emits events to Socket.IO servers from a DAG.
//!
//! ## Standalone use
//! Configure via `config`: `url`, `namespace`, `event`, `payload`, `headers`, `cookies`,
//! `wait_event`, `timeout_ms`, `transport`, `pre_events`. All string values support
//! `${ENV_VAR}` resolution. Input edges override config values (inputs take priority
//! over config). `transport` defaults to `websocket` (single persistent connection —
//! safe behind multi-instance load balancers without session affinity); set it to
//! `"any"` for polling-first + upgrade or `"polling"` for long-polling only.
//!
//! ## Pre-events (multi-event sequence on the same connection)
//! Use `pre_events: [{event, payload?, wait_event?, timeout_ms?}, ...]` to emit a
//! sequence of events BEFORE the main event over the SAME connection. Useful for
//! servers that scope state per-socket (e.g., room subscriptions, sessions).
//! On any pre-event failure the node aborts, returns an error envelope with
//! `failed_pre_event`, and never emits the main event.
//!
//! ## As an LLM tool (via `tool_configurations`)
//! When invoked by `DagToolExecutor`, the LLM provides the dynamic fields (e.g., `payload`)
//! while fixed fields (url, namespace, auth, pre_events) are pre-configured in `node_schema`.
//!
//! ## Response patterns
//! - **Ack mode** (default): Uses Socket.IO acknowledgment callback for the response.
//! - **Wait-event mode** (`wait_event` set): Listens for a separate server event as the response.
//!
//! ## Outputs
//! Returns `{ "success": bool, "event": string, "response": Value }`.
//! With `pre_events`, also includes `pre_responses: [{event, response}, ...]`.
//! On pre-event failure: `{ "success": false, "event", "failed_pre_event", "error", "pre_responses" }`.
//! On failure the envelope may also include `transport_errors` (aggregated
//! transport-level errors captured during the operation) and `advice`.
//! The default output port is `response`.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use futures::FutureExt;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};

/// Routing map: wait_event name → currently-installed sender for the active step.
/// One entry exists only while a step is awaiting that event; the handler removes it
/// on fire and the step removes it on timeout/exception cleanup.
type WaitSlots = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// Max raw transport-error messages captured per execution (drop beyond).
const MAX_TRANSPORT_ERRORS: usize = 10;

/// LLM-facing advice attached to failure envelopes that carry transport errors.
const TRANSPORT_ERROR_ADVICE: &str = "Transport-level errors occurred during this operation: \
    the connection to the server is unstable or the server dropped the session. Retrying the \
    same call is unlikely to help while these errors persist — if the problem continues, \
    inform the user that the realtime backend appears to be unreachable.";

/// One entry of the `pre_events` array.
#[derive(Debug)]
struct PreEventSpec {
    event: String,
    payload: Value,
    wait_event: Option<String>,
    timeout_ms: Option<u64>,
}

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

    /// Render a payload as a compact single-line string for logs/envelopes.
    fn payload_to_compact_string(payload: Payload) -> String {
        match Self::payload_to_value(payload) {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    /// Collapse duplicate transport-error messages preserving first-seen
    /// order: `["E", "E", "F", "E"]` → `["E (x3)", "F"]`.
    fn summarize_transport_errors(raw: &[String]) -> Vec<String> {
        let mut order: Vec<&String> = Vec::new();
        let mut counts: HashMap<&String, usize> = HashMap::new();
        for msg in raw {
            if !counts.contains_key(msg) {
                order.push(msg);
            }
            *counts.entry(msg).or_insert(0) += 1;
        }
        order
            .into_iter()
            .map(|msg| {
                let n = counts[msg];
                if n > 1 {
                    format!("{} (x{})", msg, n)
                } else {
                    msg.clone()
                }
            })
            .collect()
    }

    /// Attach `transport_errors` + `advice` to a failure envelope. No-op when
    /// no transport errors were captured — success envelopes never call this.
    fn attach_transport_context(envelope: &mut Value, raw_errors: &[String]) {
        if raw_errors.is_empty() {
            return;
        }
        envelope["transport_errors"] = json!(Self::summarize_transport_errors(raw_errors));
        envelope["advice"] = json!(TRANSPORT_ERROR_ADVICE);
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

    /// Parse the `pre_events` value into a typed list. Absent / null / empty array
    /// all yield `Ok(vec![])`. Anything else that isn't an array of objects with a
    /// non-empty string `event` returns `Err` (configuration error).
    fn parse_pre_events(val: Option<&Value>) -> Result<Vec<PreEventSpec>, String> {
        let arr = match val {
            None => return Ok(Vec::new()),
            Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(arr)) => arr,
            Some(_) => return Err("socketio_request: 'pre_events' must be an array".to_string()),
        };
        let mut out = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let obj = item
                .as_object()
                .ok_or_else(|| format!("socketio_request: pre_events[{}] must be an object", i))?;
            let event = obj
                .get("event")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    format!(
                        "socketio_request: pre_events[{}] requires non-empty 'event' string",
                        i
                    )
                })?
                .to_string();
            let payload = obj.get("payload").cloned().unwrap_or_else(|| json!({}));
            let wait_event = obj
                .get("wait_event")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let timeout_ms = obj.get("timeout_ms").and_then(|v| v.as_u64());
            out.push(PreEventSpec {
                event,
                payload,
                wait_event,
                timeout_ms,
            });
        }
        Ok(out)
    }

    /// Emit one event over an existing connection and collect its response.
    /// Used for pre-events and the main event. Races ack / wait_event vs
    /// the shared exception channel vs timeout. Returns the parsed response on
    /// success or a plain error message (caller wraps into an envelope).
    async fn emit_step(
        client: &Client,
        event: &str,
        payload: &Value,
        wait_event: Option<&str>,
        timeout_ms: u64,
        exc_rx: &mut mpsc::UnboundedReceiver<Value>,
        wait_slots: &WaitSlots,
    ) -> Result<Value, String> {
        let resolved_payload = Self::resolve_env_vars_in_value(payload)?;
        let timeout_dur = Duration::from_millis(timeout_ms);

        println!(
            "[SocketIoNode] → {} (wait_event: {:?}, timeout: {}ms)",
            event, wait_event, timeout_ms
        );
        println!(
            "[SocketIoNode] 📤 payload: {}",
            serde_json::to_string_pretty(&resolved_payload)
                .unwrap_or_else(|_| format!("{:?}", resolved_payload))
        );

        if let Some(wait_name) = wait_event {
            // ---- Wait-event mode ----
            let (tx, rx) = oneshot::channel::<Value>();
            {
                let mut map = wait_slots.lock().await;
                map.insert(wait_name.to_string(), tx);
            }

            let emit_res = client
                .emit(event.to_string(), resolved_payload)
                .await
                .map_err(|e| format!("failed to emit '{}': {}", event, e));

            let result: Result<Value, String> = match emit_res {
                Err(e) => Err(e),
                Ok(()) => {
                    tokio::select! {
                        r = rx => r.map_err(|_| {
                            format!("wait_event channel for '{}' closed unexpectedly", wait_name)
                        }),
                        Some(exc_val) = exc_rx.recv() => {
                            let msg = exc_val
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("Server exception")
                                .to_string();
                            Err(format!("server exception: {}", msg))
                        }
                        _ = tokio::time::sleep(timeout_dur) => {
                            Err(format!(
                                "Timeout waiting for '{}' after {}ms",
                                wait_name, timeout_ms
                            ))
                        }
                    }
                }
            };

            // Cleanup slot regardless of outcome (idempotent if already removed by handler).
            let mut map = wait_slots.lock().await;
            map.remove(wait_name);
            result
        } else {
            // ---- Ack mode ----
            let (ack_tx, ack_rx) = oneshot::channel::<Value>();
            let ack_tx = Arc::new(Mutex::new(Some(ack_tx)));

            let emit_res = client
                .emit_with_ack(
                    event.to_string(),
                    resolved_payload,
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
                .map_err(|e| format!("failed to emit_with_ack '{}': {}", event, e));

            match emit_res {
                Err(e) => Err(e),
                Ok(()) => {
                    tokio::select! {
                        r = ack_rx => r.map_err(|_| {
                            format!("ack channel for '{}' closed unexpectedly", event)
                        }),
                        Some(exc_val) = exc_rx.recv() => {
                            let msg = exc_val
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("Server exception")
                                .to_string();
                            Err(format!("server exception: {}", msg))
                        }
                        _ = tokio::time::sleep(timeout_dur) => {
                            Err(format!(
                                "Timeout waiting for ack on '{}' after {}ms",
                                event, timeout_ms
                            ))
                        }
                    }
                }
            }
        }
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
        // ---- 1. Resolve top-level config (inputs > config) ----
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

        let main_wait_event = Self::get_str(inputs, config, "wait_event")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let timeout_ms = Self::get_u64(inputs, config, "timeout_ms").unwrap_or(10000);
        // Default is websocket-only: a single persistent connection avoids the
        // polling-handshake stickiness problem behind multi-instance load
        // balancers (e.g. Cloud Run without session affinity). Set
        // `transport: "any"` explicitly to restore polling-first + upgrade.
        let transport = Self::get_str(inputs, config, "transport").unwrap_or("websocket");

        // Main payload (inputs > config). Env vars resolved later in emit_step.
        let main_payload = inputs
            .get("payload")
            .or_else(|| config.get("payload"))
            .cloned()
            .unwrap_or(json!({}));

        // ---- 2. Parse pre_events ----
        let pre_events_val = inputs
            .get("pre_events")
            .or_else(|| config.get("pre_events"));
        let pre_events = Self::parse_pre_events(pre_events_val).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        // ---- 3. Collect unique wait_event names (pre_events ∪ main) ----
        let mut unique_wait_names: Vec<String> = Vec::new();
        for pe in &pre_events {
            if let Some(w) = &pe.wait_event {
                if !unique_wait_names.contains(w) {
                    unique_wait_names.push(w.clone());
                }
            }
        }
        if let Some(w) = &main_wait_event {
            if !unique_wait_names.contains(w) {
                unique_wait_names.push(w.clone());
            }
        }

        // ---- 4. Build client ----
        let transport_type = match transport {
            "websocket" => TransportType::Websocket,
            "polling" => TransportType::Polling,
            _ => TransportType::Any,
        };

        let mut builder = ClientBuilder::new(&url)
            .namespace(&namespace)
            .transport_type(transport_type)
            .reconnect(false);

        if let Some(cookies_raw) = Self::get_str(inputs, config, "cookies") {
            let cookies = Self::resolve_env_vars(cookies_raw).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>
            })?;
            builder = builder.opening_header("Cookie", cookies);
        }

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

        // ---- 4b. Execution-window gate + transport-error capture ----
        // `active` is true only while this execution owns the connection.
        // rust_socketio's background task can outlive disconnect() (it keeps
        // polling and surfacing "EngineIO Error" events); gating every handler
        // on `active` makes those zombie connections silent. `transport_errors`
        // collects error events fired DURING the window so op failures can
        // tell the LLM WHY (unstable connection vs. slow server).
        let active = Arc::new(AtomicBool::new(true));
        let transport_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        // Lifecycle/debug handlers — all gated on `active` (see 4b).
        for evt in ["error", "connect_error"] {
            let active = active.clone();
            let errors = transport_errors.clone();
            builder = builder.on(evt, move |payload, _client| {
                let active = active.clone();
                let errors = errors.clone();
                async move {
                    if !active.load(Ordering::Relaxed) {
                        return; // stale connection from a finished execution
                    }
                    let msg = Self::payload_to_compact_string(payload);
                    println!("[SocketIoNode] ⚠ transport error event: {}", msg);
                    let mut buf = errors.lock().await;
                    if buf.len() < MAX_TRANSPORT_ERRORS {
                        buf.push(msg);
                    }
                }
                .boxed()
            });
        }
        {
            let active = active.clone();
            builder = builder.on("disconnect", move |payload, _client| {
                let active = active.clone();
                async move {
                    if active.load(Ordering::Relaxed) {
                        println!("[SocketIoNode] ⚠ disconnect event: {:?}", payload);
                    }
                }
                .boxed()
            });
        }

        // ---- 5. Exception channel (mpsc, drained per step) ----
        let (exc_tx, mut exc_rx) = mpsc::unbounded_channel::<Value>();
        {
            let active = active.clone();
            builder = builder.on("exception", move |payload, _client| {
                let exc_tx = exc_tx.clone();
                let active = active.clone();
                async move {
                    if !active.load(Ordering::Relaxed) {
                        return;
                    }
                    let val = Self::payload_to_value(payload);
                    println!(
                        "[SocketIoNode] ⚠ exception: {}",
                        serde_json::to_string(&val).unwrap_or_else(|_| format!("{:?}", val))
                    );
                    let _ = exc_tx.send(val);
                }
                .boxed()
            });
        }

        // ---- 6. Wait-event routing: register one handler per unique name ----
        let wait_slots: WaitSlots = Arc::new(Mutex::new(HashMap::new()));
        for w in &unique_wait_names {
            let w_owned = w.clone();
            let slots = wait_slots.clone();
            builder = builder.on(w_owned.clone(), move |payload, _client| {
                let slots = slots.clone();
                let event_name = w_owned.clone();
                async move {
                    println!("[SocketIoNode] ✓ received wait_event '{}'", event_name);
                    let mut map = slots.lock().await;
                    if let Some(sender) = map.remove(&event_name) {
                        let val = Self::payload_to_value(payload);
                        let _ = sender.send(val);
                    }
                }
                .boxed()
            });
        }

        // ---- 7. Catch-all debug handler ----
        {
            let active = active.clone();
            builder = builder.on_any(move |event, payload, _client| {
                let active = active.clone();
                async move {
                    if !active.load(Ordering::Relaxed) {
                        return;
                    }
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
        }

        // ---- 8. Connect ----
        println!(
            "[SocketIoNode] connecting to {} (namespace: {}, transport: {})",
            url, namespace, transport
        );
        let client = match builder.connect().await {
            Ok(c) => c,
            Err(e) => {
                active.store(false, Ordering::Relaxed);
                let captured = transport_errors.lock().await;
                let extra = if captured.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (transport errors during connect: {})",
                        Self::summarize_transport_errors(&captured).join("; ")
                    )
                };
                return Err(format!(
                    "socketio_request: failed to connect to {} (namespace {}): {}{}",
                    url, namespace, e, extra
                )
                .into());
            }
        };

        // Small delay to let the connection fully establish.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // ---- 9. Run pre_events sequentially ----
        let mut pre_responses: Vec<Value> = Vec::new();
        for pe in pre_events {
            // Drain any stale exceptions queued before this step starts.
            while exc_rx.try_recv().is_ok() {}
            let step_timeout = pe.timeout_ms.unwrap_or(timeout_ms);
            match Self::emit_step(
                &client,
                &pe.event,
                &pe.payload,
                pe.wait_event.as_deref(),
                step_timeout,
                &mut exc_rx,
                &wait_slots,
            )
            .await
            {
                Ok(val) => {
                    pre_responses.push(json!({
                        "event": pe.event,
                        "response": val,
                    }));
                }
                Err(msg) => {
                    println!("[SocketIoNode] ✗ pre_event '{}' failed: {}", pe.event, msg);
                    active.store(false, Ordering::Relaxed);
                    if let Err(e) = client.disconnect().await {
                        println!("[SocketIoNode] ⚠ disconnect failed: {}", e);
                    }
                    let mut out = json!({
                        "success": false,
                        "event": event_name,
                        "failed_pre_event": pe.event,
                        "error": msg,
                        "pre_responses": pre_responses,
                    });
                    Self::attach_transport_context(&mut out, &transport_errors.lock().await);
                    return Ok(out);
                }
            }
        }

        // ---- 10. Run main event ----
        while exc_rx.try_recv().is_ok() {}
        let main_result = Self::emit_step(
            &client,
            &event_name,
            &main_payload,
            main_wait_event.as_deref(),
            timeout_ms,
            &mut exc_rx,
            &wait_slots,
        )
        .await;

        // ---- 11. Disconnect (always) ----
        // Flip `active` first so a leaked background task (rust_socketio can
        // keep polling after an incomplete disconnect) goes silent instead of
        // spamming logs. The disconnect result itself is still logged.
        active.store(false, Ordering::Relaxed);
        if let Err(e) = client.disconnect().await {
            println!("[SocketIoNode] ⚠ disconnect failed: {}", e);
        }

        // ---- 12. Build output envelope ----
        let pre_responses_val = if pre_responses.is_empty() {
            None
        } else {
            Some(Value::Array(pre_responses))
        };

        match main_result {
            Ok(val) => {
                println!("[SocketIoNode] ← response received for '{}'", event_name);
                let mut out = json!({
                    "success": true,
                    "event": event_name,
                    "response": val,
                });
                if let Some(pre) = pre_responses_val {
                    out["pre_responses"] = pre;
                }
                Ok(out)
            }
            Err(msg) => {
                println!("[SocketIoNode] ← error: {}", msg);
                let mut out = json!({
                    "success": false,
                    "event": event_name,
                    "error": msg,
                });
                if let Some(pre) = pre_responses_val {
                    out["pre_responses"] = pre;
                }
                Self::attach_transport_context(&mut out, &transport_errors.lock().await);
                Ok(out)
            }
        }
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Connect to a Socket.IO server, emit an event with a JSON payload, \
             and receive the response via acknowledgment callback or a separate server event. \
             Supports namespaces, cookie-based authentication, custom headers, and an \
             optional `pre_events` sequence emitted on the same connection before the main event.",
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
                "transport": "string (websocket|polling|any, default: websocket)",
                "pre_events": "array<{event, payload?, wait_event?, timeout_ms?}> (optional, sequence emitted on the same connection BEFORE the main event)"
            },
            "inputs": {
                "url": "string (optional override)",
                "namespace": "string (optional override)",
                "event": "string (optional override)",
                "payload": "any (optional override)",
                "headers": "map<string, string> (optional override)",
                "cookies": "string (optional override)",
                "wait_event": "string (optional override)",
                "timeout_ms": "integer (optional override)",
                "pre_events": "array (optional override)"
            },
            "outputs": {
                "success": "boolean",
                "event": "string",
                "response": "any",
                "pre_responses": "array<{event, response}> (only present when pre_events were used)",
                "failed_pre_event": "string (only present when a pre_event failed)",
                "error": "string (only present on failure)",
                "transport_errors": "array<string> (only on failure, when transport-level errors occurred during the operation — aggregated, e.g. \"EngineIO Error (x4)\")",
                "advice": "string (only present alongside transport_errors — actionable guidance for the caller/LLM)"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pre_events_none() {
        let result = SocketIoNode::parse_pre_events(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_pre_events_null() {
        let v = Value::Null;
        let result = SocketIoNode::parse_pre_events(Some(&v)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_pre_events_empty_array() {
        let v = json!([]);
        let result = SocketIoNode::parse_pre_events(Some(&v)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_pre_events_valid_minimal() {
        let v = json!([{ "event": "join_room" }]);
        let result = SocketIoNode::parse_pre_events(Some(&v)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event, "join_room");
        assert_eq!(result[0].payload, json!({}));
        assert!(result[0].wait_event.is_none());
        assert!(result[0].timeout_ms.is_none());
    }

    #[test]
    fn parse_pre_events_valid_full() {
        let v = json!([
            {
                "event": "join_room",
                "payload": { "room": "abc" },
                "wait_event": "joined",
                "timeout_ms": 5000
            },
            {
                "event": "subscribe",
                "payload": { "topic": "x" }
            }
        ]);
        let result = SocketIoNode::parse_pre_events(Some(&v)).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].event, "join_room");
        assert_eq!(result[0].payload, json!({ "room": "abc" }));
        assert_eq!(result[0].wait_event.as_deref(), Some("joined"));
        assert_eq!(result[0].timeout_ms, Some(5000));
        assert_eq!(result[1].event, "subscribe");
        assert!(result[1].wait_event.is_none());
    }

    #[test]
    fn parse_pre_events_missing_event() {
        let v = json!([{ "payload": {} }]);
        let result = SocketIoNode::parse_pre_events(Some(&v));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires non-empty 'event'"));
    }

    #[test]
    fn parse_pre_events_empty_event_string() {
        let v = json!([{ "event": "" }]);
        let result = SocketIoNode::parse_pre_events(Some(&v));
        assert!(result.is_err());
    }

    #[test]
    fn parse_pre_events_not_array() {
        let v = json!({ "event": "join_room" });
        let result = SocketIoNode::parse_pre_events(Some(&v));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be an array"));
    }

    #[test]
    fn parse_pre_events_item_not_object() {
        let v = json!(["join_room"]);
        let result = SocketIoNode::parse_pre_events(Some(&v));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be an object"));
    }

    #[test]
    fn resolve_env_vars_in_value_recursive() {
        std::env::set_var("TEST_PRE_EVENT_VAR", "my-room");
        let v = json!({
            "room": "${TEST_PRE_EVENT_VAR}",
            "nested": { "topic": "${TEST_PRE_EVENT_VAR}" },
            "list": ["${TEST_PRE_EVENT_VAR}", 42]
        });
        let resolved = SocketIoNode::resolve_env_vars_in_value(&v).unwrap();
        assert_eq!(resolved["room"], "my-room");
        assert_eq!(resolved["nested"]["topic"], "my-room");
        assert_eq!(resolved["list"][0], "my-room");
        assert_eq!(resolved["list"][1], 42);
        std::env::remove_var("TEST_PRE_EVENT_VAR");
    }

    #[test]
    fn summarize_transport_errors_empty() {
        let out = SocketIoNode::summarize_transport_errors(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn summarize_transport_errors_single() {
        let raw = vec!["EngineIO Error".to_string()];
        let out = SocketIoNode::summarize_transport_errors(&raw);
        assert_eq!(out, vec!["EngineIO Error".to_string()]);
    }

    #[test]
    fn summarize_transport_errors_aggregates_preserving_order() {
        let raw = vec![
            "EngineIO Error".to_string(),
            "EngineIO Error".to_string(),
            "Connection reset".to_string(),
            "EngineIO Error".to_string(),
        ];
        let out = SocketIoNode::summarize_transport_errors(&raw);
        assert_eq!(
            out,
            vec![
                "EngineIO Error (x3)".to_string(),
                "Connection reset".to_string()
            ]
        );
    }

    #[test]
    fn attach_transport_context_noop_when_empty() {
        let mut env = json!({ "success": false, "event": "ping", "error": "Timeout" });
        SocketIoNode::attach_transport_context(&mut env, &[]);
        assert!(env.get("transport_errors").is_none());
        assert!(env.get("advice").is_none());
    }

    #[test]
    fn attach_transport_context_adds_fields() {
        let mut env = json!({ "success": false, "event": "ping", "error": "Timeout" });
        let raw = vec!["EngineIO Error".to_string(), "EngineIO Error".to_string()];
        SocketIoNode::attach_transport_context(&mut env, &raw);
        assert_eq!(env["transport_errors"], json!(["EngineIO Error (x2)"]));
        assert_eq!(env["advice"], json!(TRANSPORT_ERROR_ADVICE));
        // Pre-existing fields untouched
        assert_eq!(env["error"], json!("Timeout"));
    }

    #[test]
    fn payload_to_compact_string_unwraps_single_text() {
        let p = Payload::Text(vec![Value::String("EngineIO Error".to_string())]);
        assert_eq!(SocketIoNode::payload_to_compact_string(p), "EngineIO Error");
    }

    #[test]
    fn payload_to_compact_string_serializes_object() {
        let p = Payload::Text(vec![json!({ "code": 1 })]);
        assert_eq!(SocketIoNode::payload_to_compact_string(p), "{\"code\":1}");
    }
}
