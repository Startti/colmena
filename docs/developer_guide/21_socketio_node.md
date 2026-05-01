# 21. Socket.IO Request Node (`socketio_request`)

## Overview

The `socketio_request` node connects to a Socket.IO server, emits an event with a JSON payload, and receives the response. It is the real-time counterpart to `http_request` — use it when you need to interact with WebSocket-based APIs that use the Socket.IO protocol.

**When to use `socketio_request` vs `http_request`:**

| Criteria | `http_request` | `socketio_request` |
|---|---|---|
| Protocol | HTTP/HTTPS (REST) | Socket.IO (WebSocket + fallback) |
| Use case | REST APIs, one-shot requests | Real-time APIs, event-driven servers |
| Authentication | Headers, Bearer token, query params | Cookies, custom opening headers |
| Response | HTTP status + body | Ack callback or server event |

**Source:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`
**Registered as:** `"socketio_request"` in the node registry

---

## Configuration Reference

All config fields support `${VAR_NAME}` environment variable resolution in string values.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `url` | string | **Yes** | — | Socket.IO server URL (e.g., `https://api.example.com`) |
| `namespace` | string | No | `"/"` | Socket.IO namespace (e.g., `/canvas`, `/chat`) |
| `event` | string | **Yes** | — | Event name to emit (e.g., `load_canvas_state`, `create_node`) |
| `payload` | any | No | `{}` | JSON payload sent with the event. Env vars resolved recursively |
| `headers` | object | No | `{}` | Custom headers sent during the Socket.IO handshake |
| `cookies` | string | No | `null` | Cookie string sent as `Cookie` header (shorthand for auth) |
| `wait_event` | string | No | `null` | If set, listen for this server event instead of using ack callback |
| `timeout_ms` | integer | No | `10000` | Timeout in ms for the response |
| `transport` | string | No | `"any"` | Transport type: `"any"`, `"websocket"`, or `"polling"` |
| `pre_events` | array | No | `[]` | Sequence of events emitted on the SAME connection BEFORE the main event. See [Pre-events](#pre-events-multi-event-sequence-on-the-same-connection) |

---

## Input Ports

All input ports mirror the config fields. **Inputs take priority over config** — if both are provided, the input value wins.

| Port | Type | Description |
|---|---|---|
| `url` | string | Dynamic server URL |
| `namespace` | string | Dynamic namespace |
| `event` | string | Dynamic event name |
| `payload` | any | Dynamic payload (replaces config payload entirely) |
| `headers` | object | Dynamic headers |
| `cookies` | string | Dynamic cookie string |
| `wait_event` | string | Dynamic wait_event |
| `timeout_ms` | integer | Dynamic timeout |

**Default input:** None — there is no single primary input. You must specify fields explicitly in edges.

---

## Output Ports

| Port | Type | Description |
|---|---|---|
| `success` | boolean | `true` if the server responded, `false` on timeout/exception/error (including pre-event failure) |
| `event` | string | The event name that was emitted (echoed back for identification) |
| `response` | any | The server response data (**default output port**) |
| `pre_responses` | array | Only present when `pre_events` were configured and at least one completed. Items: `{ event, response }` in execution order |
| `failed_pre_event` | string | Only present when a pre-event failed. The main event was NOT emitted in this case |

**Default output:** `response` — downstream nodes connected with implicit edges receive this value.

### Success Output

```json
{
  "success": true,
  "event": "load_canvas_state",
  "response": { "nodes": [...], "edges": [...] }
}
```

### Error Envelope

On failure (timeout, server exception, channel error), the node returns an error envelope **without throwing**. This allows downstream nodes to handle errors gracefully:

```json
{
  "success": false,
  "event": "create_node",
  "error": "Timeout waiting for ack on 'create_node' after 10000ms"
}
```

For server-side exceptions (caught via the Socket.IO `exception` event):

```json
{
  "success": false,
  "event": "create_node",
  "error": "Node type 'invalid' not found",
  "exception": { "message": "Node type 'invalid' not found", "code": "VALIDATION_ERROR" }
}
```

---

## Response Patterns

### Ack Mode (Default)

When `wait_event` is **not** set, the node uses Socket.IO's built-in acknowledgment mechanism. The server's callback response is captured and returned as `response`.

```
Client                    Server
  │                         │
  │── emit("event", data) ──►
  │                         │
  │◄── ack(response) ───────│
  │                         │
```

Use ack mode when the server responds directly to the emitted event via the callback function.

### Wait-Event Mode

When `wait_event` **is** set, the node emits the event and then waits for the server to broadcast a separate named event. This is common in servers that decouple request handling from response delivery.

```
Client                    Server
  │                         │
  │── emit("load_state") ──►
  │                         │  (server processes)
  │                         │
  │◄── "state_loaded" ──────│
  │                         │
```

Use wait-event mode when the server responds by emitting a different event name (e.g., emit `load_canvas_state`, wait for `canvas_state_loaded`).

---

## Pre-events: multi-event sequence on the same connection

Some Socket.IO servers scope state per-socket — for example, room subscriptions in NestJS gateways: a client must `join_room` (or equivalent) over a given socket before mutations on that socket are routed to the right room. Because `socketio_request` is stateless (each execution opens a fresh connection and disconnects), chaining two `socketio_request` nodes in the DAG won't work for this case — each socket would have to re-join.

The `pre_events` array solves this by emitting an ordered sequence of events on the **same** connection **before** the main event:

```json
"pre_events": [
  {
    "event": "join_environment_room",         // required
    "payload": { "environmentId": "abc123" }, // optional, default {}
    "wait_event": "joined_environment_room",  // optional; if absent, uses ack
    "timeout_ms": 5000                          // optional; if absent, inherits node timeout_ms
  }
]
```

**Execution order:**
1. Connect once.
2. Emit each entry of `pre_events` in array order, waiting for its ack or `wait_event` before moving on.
3. Emit the main `event`.
4. Disconnect.

```
Client                       Server
  │                            │
  │── emit("join_room") ──────►│
  │                            │
  │◄── ack(joined) ────────────│   ← pre_events[0]
  │                            │
  │── emit("create_canvas") ──►│
  │                            │
  │◄── ack(canvas_id) ─────────│   ← main event
  │                            │
  │── disconnect ─────────────►│
```

**Per-step `payload` env-var resolution:** strings inside each pre-event payload are resolved recursively, identical to the main payload.

### Successful output (with pre_events)

```json
{
  "success": true,
  "event": "create_canvas",
  "response": { "canvasId": "..." },
  "pre_responses": [
    { "event": "join_environment_room", "response": { "success": true, "environmentId": "abc123" } }
  ]
}
```

### Failure: pre-event aborts the main emit

If any pre-event times out, returns a server `exception`, or fails to emit, the node **stops immediately**:
- The main event is **not** emitted.
- The connection is closed.
- The output is a failure envelope including which pre-event failed and the responses already collected:

```json
{
  "success": false,
  "event": "create_canvas",
  "failed_pre_event": "join_environment_room",
  "error": "Timeout waiting for ack on 'join_environment_room' after 5000ms",
  "pre_responses": []
}
```

If a pre-event succeeds and a later step fails, `pre_responses` will contain everything that completed before the failure.

### Backward compatibility

If `pre_events` is absent or empty, the node behaves exactly as before — no `pre_responses` field is added to the output. Existing graphs are unaffected.

### Use as an LLM tool

Lock `pre_events` away from the LLM by declaring it as a `fixed` field in `node_schema`. The LLM never sees the auth/setup logic and only controls the dynamic fields you expose. Example for an ADP canvas mutation tool:

```json
"create_canvas_node": {
  "name": "create_canvas_node",
  "node_type": "socketio_request",
  "description": "Create a new node on the canvas...",
  "node_schema": {
    "url":        { "type": "string", "fixed": "${ADP_API_URL}" },
    "namespace":  { "type": "string", "fixed": "/canvas" },
    "event":      { "type": "string", "fixed": "create_node" },
    "cookies":    { "type": "string", "fixed": "__Secure-better-auth.session_token=${ADP_SESSION_TOKEN}" },
    "timeout_ms": { "type": "integer", "fixed": 15000 },
    "pre_events": {
      "type": "array",
      "fixed": [
        {
          "event": "join_environment_room",
          "payload": { "environmentId": "${ADP_ENVIRONMENT_ID}" }
        }
      ]
    },
    "payload": {
      "type": "object",
      "properties": {
        "environmentId": { "type": "string", "fixed": "${ADP_ENVIRONMENT_ID}" },
        "node":          { "type": "object", "required": true, "description": "..." }
      }
    }
  }
}
```

---

## Environment Variable Resolution

All string values in config and payload support `${VAR_NAME}` syntax:

```json
{
  "url": "${API_URL}",
  "cookies": "__Secure-auth.token=${SESSION_TOKEN}",
  "payload": {
    "environmentId": "${ENVIRONMENT_ID}",
    "nested": {
      "key": "${NESTED_VAR}"
    }
  }
}
```

Resolution is **recursive** — nested string values inside objects and arrays within the payload are also resolved. Non-string values (numbers, booleans, null) are passed through unchanged.

---

## Example 1: Standalone Node — Ack Mode

A simple graph that connects to a Socket.IO server, emits a `ping` event, and logs the ack response.

```json
{
  "comment": "Standalone socketio_request: emit ping, log ack response",
  "metadata": {
    "category": "external",
    "requires_env": ["SOCKETIO_SERVER_URL"]
  },
  "nodes": {
    "trigger": {
      "type": "input",
      "config": {
        "message": "Pinging Socket.IO server"
      }
    },
    "ping_server": {
      "type": "socketio_request",
      "config": {
        "url": "${SOCKETIO_SERVER_URL}",
        "namespace": "/",
        "event": "ping",
        "payload": {
          "timestamp": "2024-01-01T00:00:00Z",
          "client": "colmena-dag"
        },
        "timeout_ms": 5000,
        "transport": "websocket"
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "trigger", "to": "ping_server" },
    { "from": "ping_server", "to": "log_result" }
  ]
}
```

**What happens:**
1. `trigger` emits its config as output
2. `ping_server` connects to the Socket.IO server, emits `ping` with the payload, and waits for an ack response (up to 5s)
3. `log_result` receives `ping_server.response` (the default output) and prints it

---

## Example 2: As LLM Tool — Wait-Event Mode

A graph where an LLM agent uses `socketio_request` as a tool via `tool_configurations` with `node_schema`. The LLM controls the payload while connection details are fixed.

```json
{
  "comment": "LLM agent with Socket.IO tools for canvas interaction",
  "metadata": {
    "category": "external",
    "requires_env": ["GEMINI_API_KEY", "API_URL", "SESSION_TOKEN", "ENVIRONMENT_ID"]
  },
  "nodes": {
    "trigger": {
      "type": "input",
      "config": {
        "prompt": "Load the current canvas state and tell me how many nodes exist"
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "gemini",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "You are a canvas automation agent. Use the available tools to interact with the canvas.",
        "temperature": 0.0,
        "stream": false,
        "enabled_tools": ["load_canvas", "create_node"],
        "tool_configurations": {
          "load_canvas": {
            "name": "load_canvas",
            "node_type": "socketio_request",
            "description": "Load the current canvas state. Returns all nodes, edges, and groups.",
            "node_schema": {
              "url": { "type": "string", "fixed": "${API_URL}" },
              "namespace": { "type": "string", "fixed": "/canvas" },
              "event": { "type": "string", "fixed": "load_canvas_state" },
              "wait_event": { "type": "string", "fixed": "canvas_state_loaded" },
              "cookies": { "type": "string", "fixed": "__Secure-better-auth.session_token=${SESSION_TOKEN}" },
              "timeout_ms": { "type": "integer", "fixed": 15000 },
              "payload": {
                "type": "object",
                "properties": {
                  "environmentId": { "type": "string", "fixed": "${ENVIRONMENT_ID}" }
                }
              }
            }
          },
          "create_node": {
            "name": "create_node",
            "node_type": "socketio_request",
            "description": "Create a new node on the canvas. Provide a node object with type, category, position, and data.",
            "node_schema": {
              "url": { "type": "string", "fixed": "${API_URL}" },
              "namespace": { "type": "string", "fixed": "/canvas" },
              "event": { "type": "string", "fixed": "create_node" },
              "cookies": { "type": "string", "fixed": "__Secure-better-auth.session_token=${SESSION_TOKEN}" },
              "timeout_ms": { "type": "integer", "fixed": 15000 },
              "payload": {
                "type": "object",
                "properties": {
                  "environmentId": { "type": "string", "fixed": "${ENVIRONMENT_ID}" },
                  "node": {
                    "type": "object",
                    "required": true,
                    "description": "Node object: { type, category, position: {x, y}, data: { label, config? } }"
                  }
                }
              }
            }
          }
        }
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent.result", "to": "log_result" }
  ]
}
```

**Key patterns in this example:**

- **`node_schema` with `fixed` fields:** `url`, `namespace`, `event`, `cookies`, and `timeout_ms` are all `fixed` — hidden from the LLM and auto-filled at execution time. The LLM never sees or controls these values.
- **`wait_event` for response:** `load_canvas` uses `wait_event: "canvas_state_loaded"` because the server responds by broadcasting a separate event rather than acknowledging the emit.
- **Ack mode for mutations:** `create_node` does **not** set `wait_event` — the server responds via the ack callback directly.
- **Container with mixed fixed/dynamic:** The `payload` container has a fixed `environmentId` and a dynamic `node` object that the LLM provides.

---

## Troubleshooting

### "Timeout waiting for ack on 'event' after Nms"

**Cause:** The server did not respond within the configured timeout.

**Solutions:**
- Increase `timeout_ms` (e.g., 15000 or 30000 for slow operations)
- Check if the server expects a different event name
- Check if the server responds via a separate event — use `wait_event` instead of ack mode
- Verify the server is running and accessible from the DAG engine

### "Timeout waiting for 'event_name' after Nms"

**Cause:** Wait-event mode — the server did not emit the expected event.

**Solutions:**
- Verify the `wait_event` value matches the exact event name the server emits
- Check server logs for errors processing the original event
- The server may require authentication — verify `cookies` or `headers` are correct

### "socketio_request: 'url' is required"

**Cause:** Neither config nor input provided a `url` value.

**Solution:** Set `url` in config or provide it via an input edge.

### "Env var X not found"

**Cause:** A `${VAR_NAME}` reference in config points to an undefined environment variable.

**Solution:** Export the variable before running the DAG: `export VAR_NAME=value`

### Server exception received

**Cause:** The server emitted an `exception` event, indicating a server-side error.

**What to check:**
- The `exception` field in the error envelope contains the server's error details
- Common causes: invalid payload format, missing required fields, authentication failure
- Check the server's expected payload format

### A pre-event failed — the main event never fired

**Cause:** A `pre_events` entry timed out, returned a server exception, or the channel closed unexpectedly. The node aborts on first pre-event failure and does NOT emit the main event.

**What to check:**
- The `failed_pre_event` field in the envelope tells you which pre-event aborted the chain.
- The `error` field contains the underlying message (timeout / server exception / emit failure).
- The `pre_responses` array shows which pre-events did complete before the failure — useful when the failing pre-event depends on an earlier one.

**Common fixes:**
- Wrong `wait_event` name in the pre-event → use ack mode (omit `wait_event`) if the server replies via the ack callback.
- `timeout_ms` too low for the operation → bump it on the offending pre-event.
- Auth / access denied (e.g., `validateEnvironmentAccess` rejected the join) → verify the cookies/headers and the `environmentId` payload.

### Connection fails silently

**Cause:** The Socket.IO handshake failed (wrong URL, CORS, or transport mismatch).

**Solutions:**
- Try `"transport": "polling"` — some servers don't support WebSocket upgrades
- Check if the URL includes the correct path (some servers use `/socket.io/` as the default path)
- Verify network connectivity and firewall rules
