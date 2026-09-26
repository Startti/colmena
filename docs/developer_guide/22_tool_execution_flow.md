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

#### Step 3b: Only a name the request offered reaches the executor

**File:** `llm/application/agent_service.rs` (ReAct loop, before `tool_executor.execute`)

`DagToolExecutor` resolves any registered node type by name, and no provider
adapter checks a returned name against the declared tools. So the loop runs a
call only if its name is in `iteration_tools` — the exact list serialized into
that request (`tool_configurations`, `enabled_tools`, the engine's synthetic
tools, MCP). Anything else — e.g. a `python_script` the operator never
exposed — gets `Error executing tool: Tool not found: <name>` (the same text as a
name that does not exist; never the arguments) and a WARN `tool.not_offered`.
Lazy mode keeps its own rules: a cataloged tool not loaded this turn gets its
schema (the describe-before-use guard), and `describe_tool` still answers once
the list stops carrying it.

---

### Step 4: Parse LLM Arguments

**File:** [dag_tool_executor.rs:1730](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs#L1730) (inside `execute_inner()`)

The `arguments` JSON string is deserialized into a `HashMap<String, Value>`:

```rust
let args: HashMap<String, Value> = serde_json::from_str(&tool_call.function.arguments)?;
// → { "origin": "JFK", "dest": "CDG", "date": "2026-05-15" }
```

#### Step 4b: Reserved-prefix arguments are dropped before merge

**Function:** `DagToolExecutor::strip_engine_keys()` — [dag_tool_executor.rs:275](../../src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs#L275), called from `execute_inner()` right after parsing (above), and from the toolkit sub-tool dispatch path before the sub-tool discriminator is injected.

Any argument key starting with `__colmena` or `__node` is removed from the
parsed arguments **before** they reach any of the three merge strategies in
Step 5 (`node_schema`, `$DYNAMIC`, legacy `field_mapping`) or the
no-`fixed_config` passthrough. These prefixes are reserved for engine-authored
context — session id, resume answer, subgraph depth, node id path, tool name —
injected later in this same function (after Step 5, see `execute_inner`
around dag_tool_executor.rs:2130 onward) with `insert()`, which is
authoritative and overwrites anything already in `inputs`.

That later injection is unconditional for some keys (`__colmena_node_id_path`,
`__colmena_subgraph_depth`, `__colmena_tool_name`) but conditional for others:
`__colmena_resume_answer` is only inserted when the call is an actual resume
(`execute_with_resume_answer`), and `__colmena_session_id` /
`__colmena_agent_session_id` only when the executor was built with a session
id. On an ordinary (non-resume) call, or an executor with no session id
configured, nothing downstream would have overwritten a model-forged copy of
those keys — and `__node_id` is never written by `DagToolExecutor` in any tool
dispatch path at all (only the graph execution loop sets it, in graph mode).
Stripping first closes both cases uniformly instead of relying on each engine
key happening to be reinjected later.

A `for_each` row goes through the same strip (`ForEachNode`, in
`nodes/for_each.rs`) before its own merge into the target's schema — see
[49_for_each.md](49_for_each.md#las-claves-de-fila-se-filtran-antes-del-merge).
Graph mode applies it too: `build_inputs_for` drops the reserved keys its edges
deliver before the loop injects its own (the rule lives in
`dag_engine::domain::node::strip_engine_keys`; CHANGELOG 2026-09 §89).

#### Step 4c: A field only the author sets is dropped unless the tool offers it

**Function:** `drop_unoffered_author_owned()` in
[node_schema_merge.rs](../../src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs),
called right after `strip_engine_keys()`, and by `for_each` for each row.

Author-set fields are config-only: an argument naming one of the target node's
`author_owned_inputs()` (for example `headers` of `http_request`,
`tool_configurations` of `llm_call`, `code` of `python_script`, `target` of
`for_each`) or a child-graph source (`CHILD_GRAPH_SOURCE_KEYS`) is removed
before Step 5 unless the tool offers it as a parameter, with a warning that
names the key, never its value. "Offered" = the parameters of the definition
the model was sent (for a raw node name, its schema's `inputs`); for a
`for_each` row, the target's LLM-visible fields. A declared field still passes:
declaring it is the author's explicit wiring (`probar_grafo` in
`tests/graphs/agents/graph_builder/graph_builder.json` offers a child-graph
source on purpose). CHANGELOG 2026-09 §105.

---

### Step 5: Merge Fixed Values + LLM Arguments

**File:** [node_schema_merge.rs](../../src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs)
**Function:** `merge_args_into_schema()` — called from `execute_inner()` when the tool config has a `node_schema` (PATH 0, highest priority). Extracted into its own module so `for_each` can reuse identical merge semantics for row-driven (non-LLM) calls.

This is the core merge algorithm. It runs in four sub-steps — templating happens
**first**, against a restricted source map, before anything is seeded or merged:

#### 5a. Template fixed values against a restricted source, then seed

`${key}` references inside the operator's own fixed values are resolved
**before** any LLM argument is placed, against an explicit, restricted source
map — not against the environment (see the callout below 5d for what this
step does and does not do):

```rust
// template_sources = the operator's own fixed values (so one fixed field can
// reference another) ∪ each declared TOP-LEVEL LLM param actually supplied
// this call. A param nested inside a container never qualifies.
let mut result: HashMap<String, Value> = parsed.fixed_values.iter()
    .map(|(k, v)| (k.clone(), DagToolExecutor::resolve_value_templates(v, &template_sources)))
    .collect();
```

`fixed: "SELECT * FROM t WHERE client_id = '${client_id}'"` with a declared
top-level param `client_id` resolves to the caller's value once that param is
placed in step 5b. `fixed: "${AMADEUS_KEY}"` — no declared param named
`AMADEUS_KEY` — is left exactly as written; nothing here looks it up against
`std::env`. After this step, `result` contains:
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

#### 5d. The caller's own values are never templated

The LLM arguments placed in 5b–5c are used exactly as supplied — there is no
second templating pass over the merged result. An LLM-supplied `q:
"${SOMETHING}"` always stays literal in the merged input map; it is never
looked up against anything.

> **What `${key}` templating here actually resolves against — and what it
> doesn't.** This step (5a) never talks to the process environment. It
> resolves `${key}` against a map built from the operator's own fixed values
> and the caller's declared top-level arguments — nothing else. A fixed
> `base_url: "${API_BASE}"` used to resolve against **any** key present after
> the merge, including an **undeclared** argument the model supplied without
> the operator ever declaring it as a parameter (a fixed `base_url:
> "${API_BASE}"` plus a model-sent `API_BASE` argument would redirect the
> call to wherever the model named). That is now closed: only the operator's
> own fixed values, or a declared top-level parameter's value, can satisfy a
> `${key}` reference inside a fixed value. Real environment-variable
> expansion (`${DATABASE_URL}` → the value of that process env var) happens
> **later**, inside the target node itself (Step 6 below) — this step never
> calls `std::env::var`. A `${ENV_VAR}`-shaped placeholder that no declared
> param resolves here is simply left exactly as written, for the node to
> resolve against the environment when it runs.

#### 5e. Env-expansion provenance: which values may later resolve `${VAR}`

**File:** [env_provenance.rs](../../src/libs/colmena/src/dag_engine/infrastructure/env_provenance.rs)

Right after the merge (5a–5d) produces `inputs`, using the SAME
operator-authored values that fed 5a as `authored_fixed` (the schema's
`fixed_values` for `node_schema`, or the raw `fixed_config` map for
`$DYNAMIC`/legacy — empty for neither), the executor computes
`trusted_pointers(authored_fixed, inputs)`: one RFC 6901 JSON pointer per
STRING leaf in `inputs` that contains `${` and is byte-identical to the
authored value at that same pointer. A container is never itself trusted —
only its string leaves. An LLM-introduced key with no counterpart in
`authored_fixed` never produces a pointer (the walk only descends where
`authored_fixed` has a value at the same key).

This is why 5a templates fixed values only once, before this step: a fixed
`base_url: "${API_BASE}"` still equals its authored form here, so it is
trusted; a fixed `path: "/anything/${bearer_token}"` was already templated
in 5a, so it may already differ from the authored string by the time this
step runs — correctly NOT trusted.

The pointer list is written last among the engine keys (§ Step 4b), under
`__colmena_env_trusted_paths` (`env_provenance::ENV_TRUSTED_PATHS_KEY`) —
after `strip_engine_keys` already removed any caller-supplied copy, so a
forged value cannot survive. `EnvPolicy::from_inputs` is how a node reads
this key: no key (graph mode) or a malformed one → `Restricted(∅)`, i.e.
**fail closed** — an `inputs` value never expands; `config` still does.

After `inject_secrets` (below) replaces any `<value_N>` placeholder with its
decrypted value, `prune_after_secrets` drops any trusted pointer whose value
just changed — a decrypted secret containing literal `${...}` text must
never be re-interpreted as an env placeholder.

`http_request` reads `__colmena_env_trusted_paths` on both body paths, and
`for_each` sends it for each row (see
`docs/developer_guide/13_security_strategy.md`).

Order of operations in `execute_inner`: `strip_engine_keys(args)` → merge
(5a–5d) → `trusted_pointers(authored_fixed, merged)` (5e) → insert engine
keys (resume_answer, session ids, node_id_path, subgraph_depth, tool_name,
unchanged order) → insert `__colmena_env_trusted_paths` LAST →
`inject_secrets` → `prune_after_secrets` → `node.execute(inputs)`.

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
