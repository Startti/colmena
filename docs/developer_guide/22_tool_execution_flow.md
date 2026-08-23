# 22. Tool Execution Flow: From `node_schema` to Final Request

## Overview

This document traces the **complete lifecycle** of an LLM tool call — from the moment a `tool_configurations` entry is defined in JSON, through schema generation, LLM invocation, argument parsing, value merging, and final node execution (HTTP or Socket.IO).

**Source files involved:**

| Step | File | Key Function |
|------|------|-------------|
| Schema types | `dag_engine/domain/tool_configuration.rs` | `NodeSchemaField`, `ParsedNodeSchema` |
| Schema parsing | `dag_engine/domain/tool_configuration.rs:320` | `parse_node_schema()` |
| Tool definition generation | `dag_engine/infrastructure/dag_tool_executor.rs:804` | `generate_tool_definition()` |
| Argument merge & execution | `dag_engine/infrastructure/dag_tool_executor.rs:986` (dispatch) → `dag_engine/infrastructure/node_schema_merge.rs:13` (merge) | `execute_inner()` → `merge_args_into_schema()` |
| HTTP node execution | `dag_engine/infrastructure/nodes/http.rs:850` | `HttpNode::execute()` |
| Socket.IO node execution | `dag_engine/infrastructure/nodes/socketio.rs:361` | `SocketIoNode::execute()` |

---

## End-to-End Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        JSON GRAPH DEFINITION                            │
│                                                                         │
│  tool_configurations:                                                   │
│    "search_flights":                                                    │
│       node_type: "http_request"                                         │
│       node_schema:                                                      │
│         base_url:  { fixed: "https://api.amadeus.com" }  ← HIDDEN      │
│         endpoint:  { fixed: "/v2/shopping/flight-offers" }← HIDDEN      │
│         method:    { fixed: "GET" }                       ← HIDDEN      │
│         query_params:                                                   │
│           apikey:  { fixed: "${AMADEUS_KEY}" }            ← HIDDEN      │
│           origin:  { required: true, description: "..." } ← VISIBLE    │
│           dest:    { required: true, description: "..." } ← VISIBLE    │
│           date:    { required: true, pattern: "..." }     ← VISIBLE    │
└────────────────────────────────┬────────────────────────────────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │  STEP 1: PARSE SCHEMA   │
                    │  parse_node_schema()    │
                    └────────────┬────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              ▼                                     ▼
   ┌─────────────────────┐              ┌──────────────────────┐
   │    FIXED VALUES      │              │   LLM PROPERTIES     │
   │  (hidden from LLM)   │              │  (visible to LLM)    │
   │                       │              │                      │
   │  base_url: "https.."  │              │  origin: string, req │
   │  endpoint: "/v2/.."   │              │  dest: string, req   │
   │  method: "GET"        │              │  date: string, req   │
   │  query_params:        │              │                      │
   │    apikey: "sk-..."   │              │  param_to_container: │
   │                       │              │    origin → query_p.. │
   │                       │              │    dest → query_p..   │
   │                       │              │    date → query_p..   │
   └───────────┬───────────┘              └──────────┬───────────┘
               │                                     │
               │           ┌─────────────────────────▼──────┐
               │           │  STEP 2: GENERATE TOOL DEF     │
               │           │  generate_tool_definition()    │
               │           │                                │
               │           │  ToolDefinition {              │
               │           │    name: "search_flights"      │
               │           │    parameters: {               │
               │           │      origin: { type: string }  │
               │           │      dest: { type: string }    │
               │           │      date: { type: string }    │
               │           │    }                           │
               │           │    required: [origin,dest,date] │
               │           │  }                             │
               │           └──────────────┬─────────────────┘
               │                          │
               │           ┌──────────────▼─────────────────┐
               │           │  STEP 3: LLM INVOCATION        │
               │           │                                │
               │           │  LLM sees ONLY:                │
               │           │    - Tool name + description   │
               │           │    - origin, dest, date params │
               │           │                                │
               │           │  LLM responds:                 │
               │           │    { "origin": "JFK",          │
               │           │      "dest": "CDG",            │
               │           │      "date": "2026-05-15" }    │
               │           └──────────────┬─────────────────┘
               │                          │
               │     ┌────────────────────▼───────────────────┐
               │     │  STEP 4: PARSE LLM ARGUMENTS           │
               │     │                                         │
               │     │  args = JSON.parse(tool_call.arguments) │
               │     │  → HashMap { origin, dest, date }       │
               │     └────────────────────┬───────────────────┘
               │                          │
   ┌───────────▼──────────────────────────▼───────────────────┐
   │              STEP 5: MERGE FIXED + LLM VALUES             │
   │              merge_args_into_schema() in                  │
   │              node_schema_merge.rs                          │
   │                                                           │
   │  1. Seed result with ALL fixed values:                    │
   │     result = { base_url, endpoint, method,                │
   │                query_params: { apikey: "sk-..." } }       │
   │                                                           │
   │  2. For each LLM arg, check param_to_container:           │
   │     origin → container "query_params"                     │
   │       → result["query_params"]["origin"] = "JFK"          │
   │     dest → container "query_params"                       │
   │       → result["query_params"]["dest"] = "CDG"            │
   │     date → container "query_params"                       │
   │       → result["query_params"]["date"] = "2026-05-15"     │
   │                                                           │
   │  3. Deep-merge: if LLM arg is an object AND container     │
   │     already has a fixed object for that key, MERGE         │
   │     (don't overwrite). This preserves nested fixed values. │
   │                                                           │
   │  4. Resolve ${VAR_NAME} templates in fixed values.        │
   │                                                           │
   │  MERGED RESULT:                                           │
   │  {                                                        │
   │    "base_url": "https://api.amadeus.com",                 │
   │    "endpoint": "/v2/shopping/flight-offers",              │
   │    "method": "GET",                                       │
   │    "query_params": {                                      │
   │      "apikey": "sk-real-key-123",   ← fixed + env resolved│
   │      "origin": "JFK",               ← from LLM           │
   │      "dest": "CDG",                 ← from LLM           │
   │      "date": "2026-05-15"           ← from LLM           │
   │    }                                                      │
   │  }                                                        │
   └──────────────────────────┬───────────────────────────────┘
                              │
               ┌──────────────▼──────────────────┐
               │  STEP 6: EXECUTE TARGET NODE     │
               │                                  │
               │  node.execute(inputs, config, ..) │
               │                                  │
               │  For http_request:               │
               │    GET https://api.amadeus.com   │
               │      /v2/shopping/flight-offers  │
               │      ?apikey=sk-...&origin=JFK   │
               │      &dest=CDG&date=2026-05-15   │
               │                                  │
               │  For socketio_request:           │
               │    connect(url, namespace)        │
               │    emit(event, payload)           │
               │    wait for ack or wait_event     │
               └──────────────┬──────────────────┘
                              │
               ┌──────────────▼──────────────────┐
               │  STEP 7: RETURN TO LLM           │
               │                                  │
               │  HTTP → { status: 200, body: {}} │
               │  SIO  → { success: true,         │
               │           event: "...",           │
               │           response: {} }          │
               │                                  │
               │  → LLM receives result as tool   │
               │    response, continues reasoning │
               └──────────────────────────────────┘
```

---

## Step-by-Step Breakdown

### Step 1: Parse the `node_schema`

**File:** [tool_configuration.rs:320](../../src/libs/colmena/src/dag_engine/domain/tool_configuration.rs#L320)
**Function:** `parse_node_schema(schema: &NodeSchema) -> Result<ParsedNodeSchema, String>`

The `node_schema` is a HashMap where each key is a node input field (e.g., `base_url`, `query_params`, `payload`). Each field is a `NodeSchemaField` with:

```rust
struct NodeSchemaField {
    field_type: String,              // "string", "object", "integer"
    fixed: Option<Value>,            // If present → hidden from LLM, auto-injected
    required: Option<bool>,
    description: Option<String>,
    pattern: Option<String>,         // Regex constraint shown to LLM
    properties: Option<HashMap<..>>, // Nested children (container field)
}
```

The parser handles **three cases** for each top-level key:

| Case | Condition | What happens |
|------|-----------|-------------|
| **Fixed field** | `fixed` is set | Value stored in `fixed_values` → hidden from LLM |
| **Container** | `properties` is set | Children are iterated: fixed children → `fixed_values[container]`, LLM-visible children → `llm_properties` + `param_to_container` mapping |
| **LLM-visible field** | Neither `fixed` nor `properties` | Added to `llm_properties` at top level |

**Output:**

```rust
ParsedNodeSchema {
    fixed_values: HashMap<String, Value>,          // All fixed values, keyed by field name
    llm_properties: HashMap<String, ParameterProperty>, // Only what the LLM sees
    required_params: Vec<String>,                  // Which LLM params are required
    param_to_container: HashMap<String, String>,   // "origin" → "query_params"
}
```

**Nested containers** (e.g., `payload.edge` with its own `properties`) are also handled: fixed sub-properties are collected into a fixed sub-object, and the child is exposed to the LLM as an object parameter. This enables **deep-merge** in Step 5.

---

### Step 2: Generate the Tool Definition

**File:** [dag_tool_executor.rs:804](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs#L804)
**Function:** `generate_tool_definition()`

Takes the `ParsedNodeSchema` output and builds a `ToolDefinition` that follows the OpenAI function-calling schema:

```json
{
  "name": "search_flights",
  "description": "Search for available flights between two cities",
  "parameters": {
    "type": "object",
    "properties": {
      "origin": { "type": "string", "description": "IATA departure code" },
      "dest": { "type": "string", "description": "IATA arrival code" },
      "date": { "type": "string", "description": "YYYY-MM-DD", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" }
    },
    "required": ["origin", "dest", "date"]
  }
}
```

**Key principle:** The LLM **never sees** fixed fields. It only controls the parameters explicitly exposed in `llm_properties`.

---

### Step 3: LLM Invocation

The `ToolDefinition` is sent to the LLM provider (OpenAI, Gemini, Anthropic) as part of the `tools` array in the API call. The LLM decides when to call the tool and generates arguments:

```json
{
  "id": "call_abc123",
  "type": "function",
  "function": {
    "name": "search_flights",
    "arguments": "{\"origin\": \"JFK\", \"dest\": \"CDG\", \"date\": \"2026-05-15\"}"
  }
}
```

The LLM only provides values for the parameters it can see — it has no knowledge of `base_url`, `apikey`, or any other fixed field.

---

### Step 4: Parse LLM Arguments

**File:** [dag_tool_executor.rs:1730](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs#L1730) (inside `execute_inner()`)

The `arguments` JSON string is deserialized into a `HashMap<String, Value>`:

```rust
let args: HashMap<String, Value> = serde_json::from_str(&tool_call.function.arguments)?;
// → { "origin": "JFK", "dest": "CDG", "date": "2026-05-15" }
```

---

### Step 5: Merge Fixed Values + LLM Arguments

**File:** [node_schema_merge.rs:13-70](../../src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs#L13-L70)
**Function:** `merge_args_into_schema()` — called from `execute_inner()` (`dag_tool_executor.rs:986`) when the tool config has a `node_schema` (PATH 0, highest priority). Extracted into its own module so `for_each` can reuse identical merge semantics for row-driven (non-LLM) calls.

This is the core merge algorithm. It runs in three sub-steps:

#### 5a. Seed with fixed values

```rust
let mut result: HashMap<String, Value> = HashMap::new();
for (k, v) in &parsed.fixed_values {
    result.insert(k.clone(), v.clone());
}
```

After this, `result` contains:
```json
{
  "base_url": "https://api.amadeus.com",
  "endpoint": "/v2/shopping/flight-offers",
  "method": "GET",
  "query_params": { "apikey": "${AMADEUS_KEY}" }
}
```

#### 5b. Place each LLM argument using `param_to_container`

For each LLM argument, the merge checks if it's mapped to a container:

```rust
for (param_name, param_value) in &args {
    if let Some(container) = parsed.param_to_container.get(param_name) {
        // → Insert into the container object
        let entry = result.entry(container).or_insert(json!({}));
        if let Value::Object(map) = entry {
            map.insert(param_name, param_value);
        }
    } else {
        // → Top-level insertion
        result.insert(param_name, param_value);
    }
}
```

After placing `origin`, `dest`, `date` into `query_params`:
```json
{
  "base_url": "https://api.amadeus.com",
  "endpoint": "/v2/shopping/flight-offers",
  "method": "GET",
  "query_params": {
    "apikey": "${AMADEUS_KEY}",
    "origin": "JFK",
    "dest": "CDG",
    "date": "2026-05-15"
  }
}
```

#### 5c. Deep-merge for nested objects

When the LLM provides an **object** for a parameter that already has fixed sub-properties, the merge is **additive** — LLM values are merged into the existing fixed object, not replacing it:

```rust
if let (Some(Value::Object(existing)), Value::Object(incoming)) = 
    (map.get(param_name), param_value) 
{
    let mut merged = existing.clone();
    for (k, v) in incoming {
        merged.insert(k.clone(), v.clone());
    }
    map.insert(param_name, Value::Object(merged));
}
```

**Example — Socket.IO `create_edge` tool:**

```
Fixed (from node_schema):
  payload.edge = { "type": "default", "animated": false, "environmentId": "env-123" }

LLM provides:
  edge = { "source": "node-1", "target": "node-2", "sourceHandle": "out" }

Deep-merge result:
  payload.edge = {
    "type": "default",           ← fixed (preserved)
    "animated": false,           ← fixed (preserved)
    "environmentId": "env-123",  ← fixed (preserved)
    "source": "node-1",          ← from LLM (merged in)
    "target": "node-2",          ← from LLM (merged in)
    "sourceHandle": "out"        ← from LLM (merged in)
  }
```

#### 5d. Resolve environment variables

```rust
let resolved_result = result.iter()
    .map(|(k, v)| (k.clone(), Self::resolve_value_templates(v, &result)))
    .collect();
```

`${AMADEUS_KEY}` → `"sk-real-key-123"` (from `std::env::var`)

---

### Step 6: Execute the Target Node

The merged `inputs` HashMap is passed to the target node's `execute()` method.

#### For `http_request` — [http.rs:850](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs#L850)

```
1. Extract fields from inputs (priority) or config (fallback):
   base_url, endpoint, method, headers, query_params, body, bearer_token

2. Build URL: "{base_url}/{endpoint}"
   → "https://api.amadeus.com/v2/shopping/flight-offers"

3. Add headers (config headers, then input headers override)

4. Add query_params as URL query string
   → ?apikey=sk-...&origin=JFK&dest=CDG&date=2026-05-15

5. Extra primitive inputs (not in reserved_keys) → auto-appended as query params

6. Set body (JSON or string) if present

7. Send HTTP request → receive response

8. Return { "status": 200, "body": { ... } }
```

#### For `socketio_request` — [socketio.rs:361](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs#L361)

```
1. Extract fields from inputs (priority) or config (fallback):
   url, namespace, event, payload, headers, cookies, wait_event, timeout_ms, transport

2. Build Socket.IO client with url + namespace

3. Set headers and cookies as opening headers

4. Register event handlers:
   - If wait_event set → listen for that event name
   - Always listen for "exception" event

5. Connect to server

6. Emit event with payload

7. Wait for response (race condition):
   - Ack callback response (default mode)
   - wait_event response (if configured)
   - Exception event (server error)
   - Timeout

8. Return { "success": true, "event": "...", "response": { ... } }
   or     { "success": false, "event": "...", "error": "..." }
```

---

### Step 7: Return Result to LLM

The node's output is returned to the LLM as the tool call result. The LLM then uses this information to continue its reasoning or make additional tool calls.

For HTTP nodes, the default output port is `body` (the parsed JSON response).
For Socket.IO nodes, the default output port is `response`.

---

## The Three Configuration Approaches (Priority Order)

### 1. `node_schema` (Recommended)

Full declarative control with `fixed`/dynamic fields, containers, and deep nesting.

```json
"node_schema": {
  "url": { "type": "string", "fixed": "${API_URL}" },
  "event": { "type": "string", "fixed": "create_node" },
  "payload": {
    "type": "object",
    "properties": {
      "environmentId": { "type": "string", "fixed": "${ENV_ID}" },
      "node": { "type": "object", "required": true, "description": "Node to create" }
    }
  }
}
```

### 2. `$DYNAMIC` Placeholders (Simpler, flat only)

Mark fields in `fixed_config` with `"$DYNAMIC"` — the executor derives parameters from them.

```json
"fixed_config": {
  "body": {
    "userId": 1,
    "title": "$DYNAMIC",
    "content": "$DYNAMIC"
  }
}
```

### 3. Legacy (`field_mapping` + `mergeable_fields`)

Deprecated but still supported. Explicit mapping of LLM parameters to node input fields.

---

## Comparison: HTTP vs Socket.IO Node Execution

| Aspect | `http_request` | `socketio_request` |
|--------|---------------|-------------------|
| **Protocol** | HTTP/HTTPS | Socket.IO (WebSocket) |
| **Connection** | One-shot request | Persistent connection |
| **Auth** | `headers`, `bearer_token` | `cookies`, `headers` |
| **Request** | Method + URL + body | `emit(event, payload)` |
| **Response** | HTTP status + JSON body | Ack callback or named event |
| **Output format** | `{ status, body }` | `{ success, event, response }` |
| **Default output** | `body` | `response` |
| **Error handling** | HTTP error status codes | Error envelope `{ success: false, error }` |
| **Extra inputs** | Auto-appended as query params | Not applicable |
| **Env var resolution** | `${VAR}` in all strings | `${VAR}` in all strings |
| **Timeout** | reqwest default | Configurable `timeout_ms` |
