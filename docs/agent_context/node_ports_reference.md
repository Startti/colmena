# Node Ports Reference — Default Input/Output System

## Overview

In Colmena's DAG engine, each node has optional **default ports** for input and output. This system simplifies edge definition by automatically mapping fields when you don't specify them explicitly.

**Key principle:** Instead of manually specifying every field in an edge, declare sensible defaults once per node type, then use shorthand edges.

---

## Node Types & Descriptions

### **Control Flow Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **suspend** | Human-in-the-loop gate | Pauses execution, waits for user `--answer`, resumes with that answer |
| **loop_controller** | Manages loop state | Controls loop continuation based on `loop_status` input |
| **router** | Declarative branching | Routes input to one of N named output ports. Mode `llm_direct` lets the LLM pick a branch by name; mode `extract_and_route` extracts JSON then matches `when` rules. Each branch may optionally run a subgraph. Always emits `__decision`. Fails fast on no-match (no default branch). |
| **input** | Static configuration | Emits `config` as output; useful for providing constants or test data |

### **I/O & Logging Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **log** | Debug output | Prints input to stdout and passes it through (pass-through logger) |
| **output** | Final output capture | Designed as graph terminal; captures the final result |
| **trigger_webhook** | Event ingestion | Emits `test_payload` or real webhook data as output |

### **Computation Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **add** | Addition | `output = a + b` (requires explicit `.a`, `.b` fields) |
| **subtract** | Subtraction | `output = a - b` (requires explicit `.a`, `.b` fields) |
| **multiply** | Multiplication | `output = a * b` (requires explicit `.a`, `.b` fields) |
| **divide** | Division | `output = a / b` (requires explicit `.a`, `.b` fields) |
| **exponential** | Power function | `output = base ^ exponent` (single numeric input) |

### **LLM & AI Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **llm_call** | Language model inference | Calls OpenAI/Google/Anthropic; streams tokens; supports tool calling |
| **planner** | Multi-step planning | LLM generates structured plan from inputs |
| **critic** | Quality review | LLM reviews outputs; returns pass/fail assessment |
| **information_extraction** | Schema-based extraction | LLM extracts structured data per schema |
| **output_parser** | LLM output → structured JSON | Single `input` port (default), inline-required schema, fails fast on empty input. Designed to be chained right after an `llm_call` or agent. Wrapper over the same extraction engine as `information_extraction`. |
| **reactor** | Summarization & review | LLM summarizes and reviews outputs |
| **orchestrator** | Multi-agent coordination | Manages teams of sub-agents; full lifecycle control |
| **subgraph** | Nested Execution | Encapsulates a child DAG into an isolated execution; supports Human-In-The-Loop suspension bubbling |

### **Integration Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **http_request** | HTTP calls | GET/POST/PUT/DELETE to external APIs; supports auth, body, headers |
| **socketio_request** | Socket.IO events | Connects to Socket.IO server, emits events, receives ack or wait-event responses |
| **sql_query** | PostgreSQL queries | Executes SQL with permission presets, static validation, optional LLM critic, RLS |
| **python_script** | Arbitrary code | Executes Python code; injects inputs as variables; requires feature `python` |
| **task_memory_writer** | Persistence | Writes task state to PostgreSQL; for agent memory |

### **Utility Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **mock_input** | Test data | Emits config as-is without transformation |

### **Media Generation Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **image_generation** | Generate images | Calls OpenAI (gpt-image-1, dall-e-3) or Google Vertex Imagen 4; persists output via `OutputStorageRepository`; returns `{ images: [{ document_id, mime_type, size_bytes, description }], provider, model }`. Plan B (2026-05-25) removed the legacy `attachment_id`/`url` fields. Only registered when an engine storage adapter is present (default for `EngineConfig::from_env`). |
| **tts** | Synthesize speech | OpenAI (tts-1, gpt-4o-mini-tts), ElevenLabs (eleven_multilingual_v2, etc.), or Google Gemini TTS. Persists audio bytes via `OutputStorageRepository`; returns `{ audio: { document_id, mime_type, size_bytes, duration_ms, description }, provider, model }`. Plan B (2026-05-25) removed the legacy `attachment_id`/`url` fields. |
| **image_edit** | Edit an existing image | OpenAI gpt-image-1 (only provider today) via `/v1/images/edits`. Fetches `source_url` (data:, http(s), or `$attachment:<document_id>`) + optional `mask_url`, posts multipart, persists output via `OutputStorageRepository`. Returns same shape as `image_generation`. |

---

## All Nodes: Defaults Table

| Node Type | `default_input` | `default_output` | Notes |
|---|---|---|---|
| `llm_call` | `prompt` | `result` | LLM node — always maps to/from prompt/result |
| `output` | `input` | `result` | Output node — captures final result |
| `log` | `input` | `output` | Debug logger — pass-through |
| `input` | — | `output` | Static input — reads from config |
| `suspend` | `question` | `answer_received` | Suspend/resume — question→answer flow |
| `secure_suspend` | `secrets` | `handles` | Secret collection — pauses, persists encrypted, returns handles only |
| `loop_controller` | `loop_status` | `output` | Loop control — manages loop state |
| `router` | `input` | — | **Multi-port output** — one port per branch (`<branch_name>`, non-null only on selected) + `__decision`. No default output port; downstream nodes read specific branch ports via `from: "router.<branch_name>"`. |
| `output_parser` | `input` | — | **Raw extracted JSON** — emits the parsed object directly (not wrapped in `{ output: ... }`). Downstream reads schema fields via dotted paths (e.g., `parser.intent`). Hard-errors on empty input. |
| **add** | — | `output` | **Requires explicit `a`, `b` fields** |
| **subtract** | — | `output` | **Requires explicit `a`, `b` fields** |
| **multiply** | — | `output` | **Requires explicit `a`, `b` fields** |
| **divide** | — | `output` | **Requires explicit `a`, `b` fields** |
| `exponential` | `input` | `output` | Power function — single numeric input |
| **http_request** | — | `body` | **Requires explicit `url`, `method`, etc.** See multipart mode note below. |
| **socketio_request** | — | `response` | **Requires explicit `url`, `event`, etc.** |
| `sql_query` | `query` | `output` | SQL query execution — permission control, validation, RLS |
| `python_script` | — | — | **Dynamic inputs & outputs** — all inputs flattened as Python variables; output is the raw value of the `output` variable (not wrapped in `{ output: ... }`), so edges pass it through unchanged |
| `planner` | — | `result` | **Dynamic inputs** — any input is treated as text for planning |
| `critic` | — | `result` | **Dynamic inputs** — `texts.*` inputs reviewed by LLM |
| `information_extraction` | — | `result` | **Dynamic inputs** — `texts.*` inputs extracted per schema |
| `reactor` | — | `result` | **Dynamic inputs** — `texts.*` summarized and reviewed |
| `orchestrator` | — | `final_response` | **Dynamic inputs** — full multi-agent lifecycle; suspends at `planner`, `phase_reactor`, `critic`, `critic_max_retries`, `final_reactor`; supports bridge tasks; `allow_suspend` per-component |
| `subgraph` | — | `result` | **Dynamic inputs** — Executes a child execution with isolated session_id. Inputs are injected into child globals. |
| `task_memory_writer` | — | `result` | **Requires explicit fields** for task management |
| `trigger_webhook` | — | `output` | Webhook trigger — emits payload |
| `mock_input` | — | — | **Raw output** — emits config as-is, no specific field |
| `image_generation` | — | `images` | **Requires `provider`, `model`, `prompt` in config**. Output (Plan B, 2026-05-25): `{ images: [{document_id, mime_type, size_bytes, description}], provider, model }`. Legacy `attachment_id`/`url` removed; consumers needing a renderable URL must resolve by `document_id` via a backend endpoint. |
| `tts` | — | `audio` | **Requires `provider`, `model`, `api_key`, `text`, `voice` in config**. Output (Plan B): `{ audio: {document_id, mime_type, size_bytes, duration_ms, description}, provider, model }` — same storage semantics as image_generation. |
| `image_edit` | — | `images` | **Requires `provider`, `api_key`, `source_url`, `prompt` in config**. Output: same shape as `image_generation` (Plan B `document_id`-only). `source_url` accepts `data:` URIs, http(s), or `$attachment:<document_id>`. |

**Multipart mode** (activado por header `Content-Type: multipart/*`):
- El campo `body` se interpreta como un map de parts (string URL/`$attachment:`/texto, array, o objeto explícito con `url`/`attachment`/`value` + overrides opcionales `filename`/`content_type`).
- Config extra: `max_file_size_bytes` (def. 100MB), `max_parts` (def. 10), `url_download_timeout_secs` (def. 30), `allow_http_urls` (def. false).
- Output port `body` y `status` igual que en JSON mode.

---

## The `suspend` Node (In-Depth)

The `suspend` node enables **human-in-the-loop** workflows by pausing DAG execution and waiting for external user input. It's a control flow node, not a computation node — its purpose is to halt the engine until a user provides a response.

### Implementation Details

- **Type**: `"suspend"`
- **Location**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`
- **Default Input**: `question` — can receive from incoming edge
- **Default Output**: `answer_received` — passes resumed answer downstream
- **Requires**: PostgreSQL for state persistence
- **No Dependencies**: Pure Rust implementation, no external libraries

### Node Configuration

#### Static Question (Config)

```json
{
  "type": "suspend",
  "config": {
    "question": "Do you approve this action?" 
  }
}
```

The `question` in config is the default question — used if no edge provides a `question` input.

#### Dynamic Question (From Edge)

You can override the config question by passing one from a previous node:

```json
{
  "nodes": {
    "generate_question": { "type": "python_script", "config": { "code": "output = {'question': f'Approve {request_id}?'}" } },
    "approval": { "type": "suspend", "config": { "question": "Default: Approve?" } }
  },
  "edges": [
    { "from": "generate_question.question", "to": "approval.question" }
  ]
}
```

**Priority:** Edge input > config default. If an edge provides `question`, it overrides the config.

#### Inputs Reference

| Input | Source | Purpose |
|---|---|---|
| `question` | Config OR edge | The question to display to the user |
| `__colmena_resume_answer` | Auto-injected by engine during resume | The user's answer (read-only, managed internally) |

### How It Works

**1. Suspend Phase:** When executed, the node returns:
```json
{
  "__colmena_status": "SUSPENDED",
  "question": "Do you approve this action?"
}
```

The DAG engine automatically saves the execution state (active queue, all node outputs, execution history) to PostgreSQL under the `session_id`.

**2. Output on Suspend:** The `finish` event contains `session_id` for resumption:
```json
{
  "type": "finish",
  "finishReason": "suspended",
  "output": {
    "__colmena_status": "SUSPENDED",
    "question": "Do you approve this action?",
    "session_id": "6d8928e5-e38c-49c3-a40b-16a1202055f3"
  }
}
```

**3. Resume Phase:** Pass the `session_id` and user's answer to continue. The `--answer` payload uses the canonical **ID-keyed Q/A format** — `Q[<id>]:` and `A[<id>]:` line-anchored prefixes, where `<id>` is `config.id` (now **required** on `suspend`):
```bash
cargo run --bin dag_engine -- run graph.json \
  --session-id 6d8928e5-e38c-49c3-a40b-16a1202055f3 \
  --answer "Q[confirm_transfer]: Approve transfer?
A[confirm_transfer]: Approved"
```

The format is **order-independent** when multiple suspends are answered in one payload — the parser binds by id, not position. The text after `Q[<id>]:` is echoed for human readability and is not validated. ID character set: `[A-Za-z0-9_-]{1,64}`. Multi-line answers between `A[<id>]:` and the next prefix (or EOF) are preserved verbatim. Schema reference: `docs/node_configurations.json` → `suspend.resume_answer_format`.

For `question_type: "choice"`, the configured `options` array is a UX suggestion list — the parser does **not** whitelist against it. Any free-text answer is accepted.

**4. Internal: Answer Injection:** The DAG engine automatically injects the user's answer into the node's inputs as `__colmena_resume_answer`. This happens before the node executes on resume. The shared Q/A parser then extracts the answer body keyed by `config.id`.

**5. Resume Execution:** The `suspend` node executes with `__colmena_resume_answer` present and produces:
```json
{
  "status": "resumed",
  "answer_received": "Approved"
}
```

**Important:** The `answer_received` field contains the parsed answer body — the text between `A[<id>]:` and the next prefix (or EOF), with surrounding whitespace and trailing newlines trimmed. Multi-line bodies (e.g. PEM blocks) are preserved internally.

This output is passed downstream via the `answer_received` default output port.

### Example Graph

```json
{
  "nodes": {
    "request": { 
      "type": "input", 
      "config": { "message": "Process order #123" } 
    },
    "approval": { 
      "type": "suspend", 
      "config": { "question": "Approve processing?" } 
    },
    "process": { 
      "type": "log" 
    }
  },
  "edges": [
    { "from": "request", "to": "approval" },
    { "from": "approval", "to": "process" }
  ]
}
```

### Complete Input/Output Reference

#### Inputs (What the node receives)

| Field | Source | Type | Required? | Behavior |
|---|---|---|---|---|
| `question` | Config OR edge | String | No | Question displayed to user. Defaults to "What is your input?" if missing |
| `__colmena_resume_answer` | Engine (on resume only) | Any | No | Auto-injected by DagRunUseCase when resuming; contains the user's `--answer` value |
| Other fields | Edge | Any | No | Passed through but ignored by the node |

#### Outputs (What the node returns)

**On First Execution (Suspend):**
```json
{
  "__colmena_status": "SUSPENDED",
  "question": "Do you approve?"
}
```

**On Resume Execution:**
```json
{
  "status": "resumed",
  "answer_received": <user_answer>
}
```

Where `<user_answer>` is the exact value from `--answer` (string or parsed JSON).

#### Default Ports

| Port | Direction | Field |
|---|---|---|
| Input | Incoming edge | `question` |
| Output | Outgoing edge | `answer_received` |

These allow implicit edge definitions: `{ "from": "upstream", "to": "suspend" }` and `{ "from": "suspend", "to": "downstream" }`.

### Key Implementation Details

| Aspect | Value |
|---|---|
| Node Type | `"suspend"` |
| Location | `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs` |
| `default_input` | `question` — can receive from incoming edge |
| `default_output` | `answer_received` — passes resumed answer to downstream nodes |
| State Persistence | PostgreSQL (requires `DATABASE_URL` env var) |
| Session ID | UUID v4, unique per suspension |
| Resume Aliases | `--session-id` and `--resume-id` are equivalent |
| Time Limit | No hardcoded timeout; state persists indefinitely (cleanup runs every 7 days for expired sessions) |
| Thread Safety | Safe to use in async context (uses tokio) |

### Common Patterns

**Pattern 1: Simple Approval Gate**
```json
{
  "nodes": {
    "process_node": { "type": "log", "config": { "message": "Processing..." } },
    "approval": { "type": "suspend", "config": { "id": "approval", "question": "Approve?" } },
    "final_output": { "type": "log" }
  },
  "edges": [
    { "from": "process_node", "to": "approval" },
    { "from": "approval", "to": "final_output" }
  ]
}
```
After user approves with `--answer "Q[approval]: Approve?\nA[approval]: yes"`, execution continues to `final_output`.

**Pattern 2: Conditional Resume (Route on Answer)**
```json
{
  "edges": [
    { "from": "approval.answer_received", "to": "router.decision" }
  ]
}
```
The downstream `router` node receives the exact answer and can decide what to do next. Example: if answer is "approve", go to `process_step`; if "reject", go to `log_rejection`.

**Pattern 3: Multiple Suspensions (Multi-Stage Approval)**
```json
{
  "edges": [
    { "from": "step1", "to": "manager_approval" },
    { "from": "manager_approval", "to": "director_approval" },
    { "from": "director_approval", "to": "step2" }
  ]
}
```
Chain multiple `suspend` nodes for cascading approvals. Each node has its own `config.id`, so resume payloads use that id in the Q/A prefix. Multiple ids can be answered in a single payload (order-independent).

**Pattern 4: Dynamic Question from Upstream**
```json
{
  "nodes": {
    "order_generator": { "type": "input", "config": { "order_id": "ORD-123" } },
    "approval": { "type": "suspend", "config": { "question": "Default?" } }
  },
  "edges": [
    { "from": "order_generator.order_id", "to": "approval.question" }
  ]
}
```
The `question` input overrides the config default. (Note: this example passes `order_id` as the question for simplicity; in practice, use a generator node to construct the question string.)

---

## Troubleshooting the `suspend` Node

### Issue: Suspension doesn't happen; execution continues

**Cause:** `__colmena_status: "SUSPENDED"` is returned but not detected by the engine.

**Check:**
- Verify `finishReason: "suspended"` in the output event
- Confirm the `suspend` node actually executed (check logs for `node-start` event)
- Ensure PostgreSQL is running and `DATABASE_URL` is set

### Issue: Resume fails with "Session not found"

**Cause:** The `session_id` is incorrect or expired.

**Solutions:**
- Copy the exact `session_id` from the suspend output
- Verify you're using `--session-id` (or `--resume-id`) correctly
- Check if cleanup jobs have deleted old sessions (default: 7 days)

### Issue: Resume executes but `answer_received` is null

**Cause:** `--answer` was not provided to the resume command.

**Solution:**
```bash
# ❌ Wrong: no --answer
cargo run --bin dag_engine -- run graph.json --session-id abc123

# ✅ Correct: ID-keyed Q/A payload (id must match config.id)
cargo run --bin dag_engine -- run graph.json --session-id abc123 \
  --answer "Q[approval]: Approve?
A[approval]: yes"
```

### Issue: Resume rejects payload with parser error

**Cause:** The `--answer` payload doesn't match the canonical Q/A format, references an unknown id, or omits an expected id.

**Checklist:**
- The id in `Q[<id>]:` / `A[<id>]:` matches the `config.id` of the suspend node (or `secret.name` for `secure_suspend`).
- Each expected id appears exactly once as `A[<id>]:` (duplicates → error).
- No empty answers (`A[<id>]:` followed only by whitespace before the next prefix or EOF).
- For multi-id payloads, all expected ids are present; order is irrelevant.

The answer body — the text between `A[<id>]:` and the next prefix — is taken verbatim. To pass a JSON object, place it in the body:
```bash
--answer "Q[approval]: Decision?
A[approval]: {\"approved\": true}"
# answer_received = "{\"approved\": true}" (string body, not auto-parsed)
```

### Issue: No question is displayed to user

**Cause:** The `suspend` node has no question in config and no edge input.

**Check:**
- Set `config.question` to a non-empty string
- Or pass a `question` input from upstream node: `{ "from": "source", "to": "suspend.question" }`

### Issue: Multiple suspensions, but only first one works

**Cause:** Session ID from first suspension used for all subsequent ones.

**Solution:** Each `suspend` node generates a **new** `session_id` on resume. Use the latest `session_id` from the latest `finish` event, not the first one.

---

---

## The `secure_suspend` Node (In-Depth)

El nodo `secure_suspend` pausa el DAG para recolectar uno o más secretos del usuario. El valor real **nunca** aparece en outputs, logs, ni en el contexto del LLM — solo se exponen handles opacos del tipo `sv_<name>`.

- **Type**: `"secure_suspend"`
- **Location**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`
- **Requires**: `DATABASE_URL` — los secretos se persisten cifrados (AES-256-GCM) en PostgreSQL vía `SecureValueService`.
- **Spec completa**: `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`

### Inputs (engine-injected)

- `__node_id` — usado como base ID si `config.id` no está presente.
- `__colmena_session_id` — scope de los handles persistidos.
- `__colmena_resume_answer` — string en formato canónico ID-keyed Q/A (ver §"Reanudación"). Inyectado automáticamente por el engine.

### Outputs (suspend-path)

- `__colmena_status: "SUSPENDED"` — señaliza al engine que pause la ejecución.
- `questions: [{ id, question, type: "secret", options: null }, ...]` — una entrada por ítem en `config.secrets`. **El `id` de cada question es el `name` del secret** — el operador lo usa directamente en `Q[<name>]:` / `A[<name>]:` al reanudar.

### Outputs (resume-path)

- `status: "resumed"`
- `handles: { <name>: "<sv_<name>>", ... }` — mapa con un handle por secreto, indexado por el `name` del config.

**Default input:** `secrets`. **Default output:** `handles`.

### Configuration

| Campo | Tipo | Requerido | Descripción |
|---|---|---|---|
| `secrets` | array | **sí** | Lista de `{ question, name }`. Mínimo 1, máximo 8. `name` debe ser único y matchear `^[a-z][a-z0-9_]{2,63}$`. El `name` cumple doble rol: identifica el handle (`<sv_<name>>`) y es el id usado en la respuesta de resume. |
| `id` | string | no | Base ID del bloque. Default: `__node_id`. |

### Reanudación

El payload `--answer` usa el formato canónico **ID-keyed Q/A** compartido con el nodo `suspend`, keyed por el `name` de cada secret:

```bash
cargo run --bin dag_engine -- run graph.json \
  --agent-session-id agent_x \
  --answer "Q[username]: API key
A[username]: sk-abc123
Q[password]: API secret
A[password]: shh-def456"
```

- Orden-independiente: el operador puede contestar `password` antes que `username` y el parser sigue bindeando por id.
- Multi-line answers (PEM blocks, JSON pegado) se preservan entre `A[<name>]:` y el siguiente prefix o EOF.
- El echo después de `Q[<name>]:` es human-readable y NO se valida — el LLM puede reformular o traducir libremente.
- Cada `name` esperado debe aparecer exactamente una vez como `A[<name>]:`. Duplicados, ids desconocidos, ids faltantes o respuestas vacías → error.
- `secure_suspend` solo soporta open questions (no choice).

Schema canónico: `docs/node_configurations.json` → `secure_suspend.resume_answer_format`.

### Uso como LLM tool

Registrar bajo `tool_configurations` con `node_type: "secure_suspend"`. Los argumentos del LLM (campo `secrets`) llegan como inputs del nodo. El LLM ve solo los handles, nunca el valor real.

```json
"tool_configurations": {
  "collect_credentials": {
    "node_type": "secure_suspend",
    "description": "Asks the user for one or more API credentials and returns opaque handles.",
    "node_schema": {
      "secrets": {
        "type": "array",
        "required": true,
        "description": "List of { question, name } objects describing each credential to collect."
      }
    }
  }
}
```

---

---

## The `orchestrator` Node (In-Depth)

The `orchestrator` node implements a **full multi-agent coordination system** with automatic planning, phased execution, per-task critique, and suspend/resume at five key points with a system of structured clarifying questions. Unlike simpler nodes, the orchestrator runs its entire lifecycle inside a single `execute()` call — it never needs a self-loop edge.

- **Type**: `"orchestrator"`
- **Location**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`
- **Requires**: `DATABASE_URL` — all task state and phase summaries are persisted to PostgreSQL

### Orchestrator Lifecycle

```
INPUT PROMPT
    │
    ▼
0. PLANNER ──► may SUSPEND ⏸ for clarification (`allow_suspend`)
    │          └── Q&A → phase 0 summary; planner re-runs with injected context
    ▼
1. PLANNER ──► Seeds DB with tasks grouped by phase
    │
    ▼ (loop over phases)
2. EXECUTE tasks in current phase
   ├─ [parallel] all tasks with parallel=true run together
   └─ [sequential] one task at a time
    │
    ▼
3. CRITIC (optional, per task) ──► validates result ──► may SUSPEND ⏸ (`allow_suspend`)
    │                              └── if fails N times → SUSPEND critic_max_retries ⏸ (choice: accept/skip/retry/cancel)
    ▼
4. PHASE REACTOR (optional) ──► summarizes phase ──► may SUSPEND ⏸ (`allow_suspend`)
    │     └── may propose bridge tasks (`bridge=true`):
    │          ├─ these run in the CURRENT phase (not the next one)
    │          └─ their results become a bridge summary before phase N+1 starts
    ▼ (repeat for all phases)
5. FINAL REACTOR ──► synthesizes all summaries into final answer ──► may SUSPEND ⏸ (`allow_suspend`)
    │
    ▼
OUTPUT: final_response
```

### Configuration Schema

```json
{
  "type": "orchestrator",
  "config": {
    "verbose": true,
    "max_phases": 5,
    "planner": {
      "provider": "google",
      "model": "gemini-2.5-flash",
      "api_key": "${GEMINI_API_KEY}",
      "system_message": "You are a planner. Decompose the user's request into tasks."
    },
    "agents": {
      "agent_id_1": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "You are a specialist. Do your task concisely."
      },
      "agent_id_2": { "..." : "..." }
    },
    "critic": {
      "provider": "google",
      "model": "gemini-2.5-flash",
      "api_key": "${GEMINI_API_KEY}",
      "system_message": "Review the agent result for quality."
    },
    "phase_reactor": {
      "provider": "google",
      "model": "gemini-2.5-flash",
      "api_key": "${GEMINI_API_KEY}",
      "system_message": "Summarize this phase and identify any gaps."
    },
    "final_reactor": {
      "provider": "google",
      "model": "gemini-2.5-flash",
      "api_key": "${GEMINI_API_KEY}",
      "system_message": "Combine all phase summaries into a final answer."
    }
  }
}
```

| Config Key | Type | Default | Description |
|---|---|---|---|
| `verbose` | bool | `false` | Print detailed input/output for each internal step |
| `max_phases` | int | `10` | Safety limit: stops execution and finalizes if phase count exceeds this value. Prevents infinite loops from reactor replanning. |
| `planner` | object | required | LLM config used for auto-planning. Agent names from `agents` are injected automatically. |
| `agents` | object | required | Map of `agent_id → LLM config`. The planner assigns tasks to these agents by their key name. |
| `critic` | object | optional | If present, every agent result passes through the critic before being marked complete. |
| `critic.max_retries` | int | `3` | Max consecutive critic failures before suspending to let the user decide. |
| `phase_reactor` | object | optional | If present, runs after every phase completes. Summarizes results and can add recovery tasks or suspend. |
| `final_reactor` | object | optional | If present, runs once all phases are done. Synthesizes all phase summaries into the final user-facing response. |
| `allow_suspend` | bool | `true` | **Per-component** flag (placed inside `planner`, `critic`, `phase_reactor`, or `final_reactor`). If `false`, this component will never suspend — questions are printed to logs and execution proceeds. |

### Human-in-the-Loop (HITL)

The orchestrator supports HITL by pausing execution and outputting a structured array of questions. The suspension behavior is controlled per-component by `allow_suspend`.

**SuspendQuestion schema:**
```json
{
  "__colmena_status": "SUSPENDED",
  "questions": [
    { "id": "action", "question": "What should we do?", "type": "choice", "options": ["accept", "skip"] },
    { "id": "clarification", "question": "...", "type": "open" }
  ]
}
```

### The Five Suspension Points

The orchestrator can suspend at five distinct points:

#### 0. `planner` — Before planning

Triggered when the planner detects the user request is ambiguous and it cannot create a meaningful plan without more information.

On resume: The Q&A is accumulated and saved as a special **phase 0 summary** visible to all agents. The planner re-runs with `USER CLARIFICATION BEFORE PLANNING` injected into its `system_message`.

#### 1. `phase_reactor` — After a phase completes

Triggered when the phase reactor detects ambiguity or missing information in the phase results.

On resume: The Q&A is injected into the `system_message` of the reactor, which **runs again** for that phase with the fully enriched context.

#### 2. `critic` — During agent task validation

Triggered when the critic reviews an agent result and determines user input is needed before approving it.

On resume: The agent **runs again** with `USER CLARIFICATION` injected into its enriched prompt.

#### 3. `critic_max_retries` — Fallback after repeated failures

Triggered when a task fails the critic loop `max_retries` times. Issues choice-based questions. The user can select:
- `accept`: Use the agent's current result as-is.
- `skip`: Skip the task (`[SKIPPED by user]`).
- `retry`: Try again with additional instructions.
- `cancel`: Cancel effectively stopping the task.

#### 4. `final_reactor` — Before the final synthesis

Triggered when the final reactor needs clarification before writing the user-facing answer.

On resume: the Q&A is injected as a `system_message` context into the final reactor, which runs again with the clarification.

### Bridge Tasks

The `phase_reactor` can propose **bridge tasks** marked with `"bridge": true`. Bridge tasks are special because they execute in the **same current phase** (not the next phase) before phase N+1 is allowed to begin.

**Bridge Flow:**
```
Phase N completes
    ↓
Phase Reactor → proposes add_tasks with bridge=true
    ↓  (internal flag __orch_reactor_done_N set)
Bridge tasks execute (same agents, same phase N)
    ↓
Bridge summary saved: "[BRIDGE RESULTS — phase N]\n[Bridge — agent]: ..."
    ↓
Flag cleared → Phase N closes → Phase N+1 starts with complete context
```

### Suspend/Resume Workflow

**Step 1 — Run the orchestrator (may suspend):**
```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/my_plan.json
```

Output when suspended:
```json
{
  "finishReason": "suspended",
  "result": {
    "__colmena_status": "SUSPENDED",
    "question": "Which estimate should we use?",
    "session_id": "abc-123"
  }
}
```

**Step 2 — Resume with the answer:**
```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/my_plan.json \
  --session-id abc-123 \
  --answer "Use the clothing expert estimates"
```

On resume:
- The orchestrator restores all completed tasks from DB — **nothing re-runs**
- The answer is injected as `__colmena_resume_answer` by the run_use_case
- The appropriate guard fires (phase_reactor / critic / final_reactor) to handle the answer
- Execution continues from exactly where it stopped

### Agent Prompt Structure

Each agent receives an enriched prompt built by `build_enriched_prompt()`:

```
=== USER CLARIFICATION ===          ← only present on resume with Q&A
Question: Which estimate to use?
Answer: Use the clothing expert's.

=== CONTEXTO DE ESTA TAREA ===      ← task context from planner
The user wants a budget for ski clothing.

=== LO QUE HA OCURRIDO HASTA AHORA ===   ← phase summaries (phase 2+ only)
Fase 1: Clothing expert recommended X, gear expert recommended Y.

=== LO QUE TIENES QUE HACER AHORA TÚ ===
Estimate the total budget for clothing items.
```

### Deduplication & Safety

- **Task deduplication**: Before inserting a recovery task proposed by the reactor, the orchestrator checks if a task with the same `task_name + assigned_to` already exists (completed or pending) in the session. Duplicates are silently discarded.
- **Agent validation**: Recovery tasks proposing agents not in `config.agents` are discarded with a warning.
- **max_phases safety net**: If phase number exceeds `max_phases` (default 10), the orchestrator forces finalization immediately. Set lower (e.g. `"max_phases": 5`) in production to catch runaway replanning loops.
- **Completed task history injection**: The phase reactor's system_message is automatically augmented with the list of already-completed tasks so the LLM avoids re-proposing them.

### Output Structure

```json
{
  "final_response": "The complete user-facing answer synthesized by final_reactor.",
  "all_tasks": [
    {
      "id": "uuid",
      "task_name": "Determine clothing items...",
      "assigned_to": "clothing_expert",
      "completed": true,
      "phase": 1,
      "parallel": true,
      "is_bridge": false,
      "result": { "content": "Ski jacket: $300..." }
    }
  ],
  "extra_info": {
    "__colmena_loop_status": "FINISHED",
    "phase_summaries": [
      { "phase": 0, "summary": "[PLANNER Q&A] Q [scope]: ... A: ..." },
      { "phase": 1, "summary": "Phase 1 covered clothing and gear..." },
      { "phase": 1, "summary": "[BRIDGE RESULTS — phase 1]\n[Bridge — gear_expert]: Helmet costs $100" }
    ]
  }
}
```

> [!NOTE]
> The `is_bridge` field indicates if the task was proposed as a bridge task by the phase_reactor. Phase 0 summaries contain accumulated Q&A from the planner. Summaries prefixed with `[BRIDGE RESULTS]` correspond to completed bridge tasks (multiple bridge tasks per phase are supported).

### State Persistence (`global_shared_state`)

The orchestrator uses `global_shared_state` (persisted in DB as part of `DagRunState`) to store suspend metadata. These keys are internal and managed automatically:

| Key | Purpose |
|---|---|
| `__orchestrator_suspend` | Written on suspend; contains `suspended_at`, `phase`, `task_id`, `questions` (array) |
| `__orchestrator_qa_context` | Q&A from critic/final_reactor; injected into the agent prompt on resume |
| `__orchestrator_phase_reactor_qa` | Q&A from phase_reactor; injected into its `system_message` on resume |
| `__orchestrator_planner_qa` | Accumulated Q&A from planner; injected into planner on resume |
| `__orch_pending_<task_id>` | Temporary stash of agent result before critic suspend; cleaned up on resume |
| `__orch_retries_<task_id>` | Tracks consecutive critic failures for `critic_max_retries` logic |
| `__orch_reactor_done_<phase>` | Flag tracking that phase reactor already ran; uncompleted tasks are bridge tasks |

### Minimal Example Graph

```json
{
  "nodes": {
    "trigger": {
      "type": "input",
      "config": { "prompt": "Plan a ski trip to Aspen." }
    },
    "orchestrator_node": {
      "type": "orchestrator",
      "config": {
        "max_phases": 4,
        "planner": {
          "provider": "google", "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}"
        },
        "agents": {
          "clothing_expert": {
            "provider": "google", "model": "gemini-2.5-flash",
            "api_key": "${GEMINI_API_KEY}",
            "system_message": "You are a clothing expert for cold weather trips."
          },
          "budget_expert": {
            "provider": "google", "model": "gemini-2.5-flash",
            "api_key": "${GEMINI_API_KEY}",
            "system_message": "You are a travel budget estimator."
          }
        },
        "phase_reactor": {
          "provider": "google", "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}",
          "system_message": "Summarize this phase and identify any missing coverage."
        },
        "final_reactor": {
          "provider": "google", "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}",
          "system_message": "Combine all phases into a final 3-4 line trip summary."
        }
      }
    },
    "final_output": { "type": "output", "trigger_on": "FINISHED" }
  },
  "edges": [
    { "from": "trigger", "to": "orchestrator_node" },
    { "from": "orchestrator_node", "to": "final_output" }
  ]
}
```

### Troubleshooting the `orchestrator` Node

**"Orchestrator runs forever across many phases"**
- Cause: `phase_reactor` keeps proposing the same recovery task.
- Solution: Set `"max_phases": 4` in config. Also check that `insurance_expert` (or whichever agent) is completing tasks — if `completed=true` in DB but the reactor re-proposes it, deduplication should catch it.

**"Resume runs everything from scratch"**
- Cause: The `session_id` passed to `--session-id` doesn't match the suspended session.
- Solution: Copy the exact `session_id` from the `SUSPENDED` output (look for `"session_id"` in the terminal output).

**"Phase 2 agents don't see the user's clarification answer"**
- Cause: The Q&A context is only visible in `=== USER CLARIFICATION ===` when `__orchestrator_qa_context` is set in `global_shared_state`.
- Solution: The orchestrator sets this automatically on resume from `phase_reactor` suspend. If agents don't see it, verify you are using `--session-id` + `--answer` (not just re-running fresh).

**"Phase reactor was never called"**
- Cause: `phase_reactor` key is missing in orchestrator config, or all tasks in the phase were already completed (so `handle_phase_completion` was never triggered).
- Solution: Add `phase_reactor` to the orchestrator config. Check DB to confirm tasks have `completed=true`.

**"final_response is null"**
- Cause: `final_reactor` is missing from config, or it returned an empty `result`.
- Solution: Add `final_reactor` config. Verify the final reactor system_message asks it to produce a response.

---

## Advanced: Internal Behavior

### State Persistence

When a `suspend` node returns `__colmena_status: "SUSPENDED"`:

1. The DagRunUseCase captures the active queue (list of nodes waiting to execute)
2. All node outputs so far are captured
3. Execution history is recorded
4. All this state is persisted to PostgreSQL using the `session_id` as key
5. The stream ends with `finishReason: "suspended"`

On resume with `--session-id <id>`:

1. DagRunUseCase loads the saved state from PostgreSQL
2. The execution queue is restored to exactly where it was
3. The `suspend` node re-executes with `__colmena_resume_answer` injected
4. Execution continues from the queue

### Why PostgreSQL is Required

The state persistence is **mandatory** because:
- Suspend/resume spans multiple process invocations (different CLI calls)
- Memory is not shared between runs
- PostgreSQL provides durable storage and cleanup mechanisms

Without `DATABASE_URL` set, suspend nodes will fail at runtime.

---

## Edge Resolution Rules

### **Rule 1: Explicit Fields Always Win**
```json
{ "from": "A.field1", "to": "B.field2" }
```
→ Takes `A.field1` directly to `B.field2`. No defaults used.

---

### **Rule 2: Implicit Edges Use Defaults**
```json
{ "from": "A", "to": "B" }
```
Behavior depends on node defaults:

#### **Case 2a: Both have defaults**
```json
{ "from": "llm1", "to": "llm2" }
```
→ Resolves to `llm1.result → llm2.prompt`  
✅ Works perfectly.

#### **Case 2b: Source has default, target doesn't**
```json
{ "from": "llm1", "to": "add1" }
```
→ **Auto-flatten:** If `llm1.result` is an object, merge all its keys into `add1`'s inputs.  
⚠️ **Warning:** `add1` needs `a` and `b` specifically. May fail at runtime if keys don't match.

#### **Case 2c: Source doesn't have default, target does**
```json
{ "from": "mock_input", "to": "exponential" }
```
→ **Smart extraction:** If source emits raw object `{ input: 5 }` and target expects `default_input="input"`, extract that field.  
Result: `exponential` receives `input: 5` (not `input: { input: 5 }`).

#### **Case 2d: Neither has default**
```json
{ "from": "http_node", "to": "python_node" }
```
→ **Auto-flatten:** All keys from http output merged into Python inputs.

---

### **Rule 3: Partial Explicit (Mixed)**
```json
{ "from": "llm1.result", "to": "B" }
{ "from": "A", "to": "llm2.system_message" }
```
→ Uses specified field on explicit side, default on implicit side.

---

## Common Patterns

### **Pattern 1: LLM Chain (simplest)**
```json
{
  "nodes": {
    "researcher": { "type": "llm_call", "config": { ... } },
    "writer": { "type": "llm_call", "config": { ... } }
  },
  "edges": [
    { "from": "researcher", "to": "writer" }
  ]
}
```
✅ **Works:** `researcher.result → writer.prompt` (both have defaults)

---

### **Pattern 2: Math Operations (ALWAYS explicit)**
```json
{
  "nodes": {
    "input_a": { "type": "input", "config": { "data": 10 } },
    "input_b": { "type": "input", "config": { "data": 5 } },
    "sum": { "type": "add" }
  },
  "edges": [
    { "from": "input_a", "to": "sum.a" },
    { "from": "input_b", "to": "sum.b" }
  ]
}
```
⚠️ **Why explicit?** `AddNode` has no `default_input`. You **must** specify `.a` and `.b`.

---

### **Pattern 3: Dynamic Inputs (Python, Planner, Critic)**
```json
{
  "edges": [
    { "from": "llm_result", "to": "python_node" }
  ]
}
```
✅ **Works:** LLM emits `{ result: "...", usage: {...} }`. Python receives all keys as variables: `result`, `usage`.

```python
# Python script automatically gets:
# result = "..."
# usage = {...}
output = f"Processed: {result}"
```

---

### **Pattern 4: Explicit Override When in Doubt**
```json
{ "from": "llm1.result", "to": "llm2.prompt" }
```
✅ **Always safe:** Completely explicit, no ambiguity.  
Use when:
- You're not sure about defaults
- You want specific field extraction
- You're connecting nodes with no clear primary input/output

---

## Decision Tree

When defining an edge `{ from: "A", to: "B" }`:

```
1. Do BOTH A and B have meaningful defaults?
   ├─ YES → Use implicit: { from: "A", to: "B" } ✅
   │
   └─ NO → Check which one doesn't:
       ├─ A has default, B doesn't → Use B explicit: { from: "A", to: "B.field" }
       │   (or be prepared for auto-flatten)
       │
       ├─ B has default, A doesn't → Use A explicit: { from: "A.field", to: "B" }
       │
       └─ Neither has defaults → Use BOTH explicit: { from: "A.field", to: "B.field" }

2. When in doubt → Always use explicit fields (safest)
```

---

## Test Cases & Examples

All test graphs are in `tests/graphs/edge_resolution/`:

| File | Case | Expected Behavior |
|---|---|---|
| `test_case_1_1_implicit_with_defaults.json` | Implicit + both defaults | Works perfectly |
| `test_case_1_4_fully_explicit.json` | Fully explicit | Always works |
| `test_case_2_2_explicit_required_add.json` | Math node requires explicit | Works with explicit `.a`, `.b` |
| `test_case_4_1_smart_extraction.json` | Raw output + smart extraction | Extracts matching field |
| `test_case_4_2_no_field_match.json` | Raw output, no field match | Falls back to full object |
| `test_case_5_1_auto_flatten_fallback.json` | Dynamic inputs + flatten | All keys become variables |

---

## Troubleshooting

### **Error: "Entrada no es un número: a"**
**Cause:** `AddNode` received wrong input type (object instead of number).  
**Solution:** Use explicit fields: `{ from: "input.value", to: "add_node.a" }`

### **Output is null**
**Cause:** Source field didn't exist or node output is structurally different.  
**Solution:** Check the node schema. Use explicit: `{ from: "A.actual_field", to: "B" }`

### **Node receives extra keys it doesn't expect**
**Cause:** Auto-flattening merged all source fields.  
**Solution:** Use explicit target field: `{ from: "A", to: "B.my_field" }`

---

## Implementation Notes

### For LLM Developers Building Graphs

- **Prefer implicit edges** when both nodes have defaults (cleaner JSON)
- **Use explicit edges** for math nodes, HTTP nodes, and multi-input nodes
- **Check the defaults table** above before writing edge definitions
- **Test locally** with `cargo run --bin dag_engine -- run your_graph.json`

### For Node Implementers

When creating a new node, declare `default_input` and `default_output`:

```rust
impl ExecutableNode for MyNode {
    fn default_input(&self) -> Option<&str> {
        Some("main_input")  // or None if multiple required inputs
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")  // or None if no single primary output
    }
    
    // ... rest of implementation
}
```

Guidelines:
- `default_input = None` for nodes with multiple required inputs (e.g., AddNode)
- `default_input = Some("field")` for nodes with ONE primary input
- `default_output = Some("field")` for ALL nodes (return the primary output field)
- Document in your node's schema and description

---

## Summary

| Want | Do This | Works? |
|---|---|---|
| Clean JSON | `{ from: "A", to: "B" }` | ✅ if both have defaults |
| Explicit/safe | `{ from: "A.x", to: "B.y" }` | ✅ always |
| Math operations | `{ from: "A", to: "add.a" }` | ✅ required |
| Dynamic inputs | `{ from: "A", to: "python" }` | ✅ all keys flattened |
| Extract 1 field | `{ from: "A.result", to: "B" }` | ✅ and B gets just that field |

---

## `tavily_client` (Toolkit)

Nodo de herramientas expuesto a un `llm_call`. Dos sub-herramientas: `search` y `fetch`. También ejecutable como nodo DAG regular.

**Ruteo de sub-tool:** todos los inputs incluyen la clave reservada `__sub_tool` (`"search"` o `"fetch"`) que el nodo lee para elegir el handler. En uso vía `tool_configurations` la inyecta `DagToolExecutor`; en uso directo debe proveerla el edge upstream (p. ej. un `input` node con `data.__sub_tool`).

**Inputs — `search`:** `__sub_tool: "search"`, `query` (string, requerido), `max_results` (1-10), `include_content` (bool), `search_depth` (`basic`|`advanced`), `include_domains` (array), `exclude_domains` (array), `time_range` (`day`|`week`|`month`|`year`).
**Outputs — `search`:** `{ query, results: [{ title, url, snippet, score, content? }], answer?, credits_used }`.

**Inputs — `fetch`:** `__sub_tool: "fetch"`, `url` (string, requerido), `extract_format` (`markdown`|`text`).
**Outputs — `fetch`:** `{ url, title?, content, content_length, credits_used }`.

Errores recuperables por el LLM: `rate_limit`, `timeout`, `upstream_error`. `AdapterInit` e `InvalidConfig` causan fallo de ejecución de DAG.

## `api_explorer` (Toolkit)

Nodo de herramientas expuesto a un `llm_call`. Cinco sub-herramientas que permiten al LLM descubrir endpoints de una especificación OpenAPI 3.x / Swagger 2.0 y construir un `http_request` válido.

**Node type:** `api_explorer`

**Inputs comunes (inyectados por el ejecutor de toolkits, no los rellena el LLM):**

| Key | Type | Required | Source | Description |
|---|---|---|---|---|
| `__sub_tool` | string | yes | toolkit executor | Uno de: `load_spec`, `list_endpoints`, `search_endpoint`, `get_endpoint_details`, `build_http_request`. |
| `conversation_id` | string | no | toolkit executor | Llave usada para cachear specs por conversación. Default `"default"`. |

**Inputs por sub-tool (los rellena el LLM):**

| Sub-tool | Parámetros |
|---|---|
| `load_spec` | `url` (req), `force_reload` (bool, default false) |
| `list_endpoints` | `spec_url` (req), `tag` (str), `limit` (1-200, default 50), `offset` (default 0) |
| `search_endpoint` | `spec_url` (req), `query` (req), `method` (str), `max_results` (1-50, default 10) |
| `get_endpoint_details` | `spec_url` (req), `operation_id` (req) |
| `build_http_request` | `spec_url` (req), `operation_id` (req), `params` (object opt, default `{}`), `auth_secret_ref` (opt) |

**Outputs (`output`):** envelope JSON específico por sub-tool. `load_spec` devuelve `{ spec_url_input, resolved_url, original_format, internal_format, title, version, description, server_url, endpoints_count, tags, security_schemes, cached }`. Los demás devuelven la representación documental directa (`{ endpoints, total }`, `{ query, results }`, detalles del endpoint, o el objeto `http_request`-shaped). Ver el spec C para el contrato exacto.

**Resolución de `$ref`:** los schemas de `request_body` y `responses[].content[]` que devuelve `get_endpoint_details` tienen los `{"$ref": "#/components/schemas/X"}` **inlinados** desde `components.schemas` (con detección de ciclos via path tracking; cycles dejan `{"x-cycle-to": "X"}`, refs desconocidas dejan `{"x-unresolved-ref": "X"}`). Esto es crítico para Gemini — su validador estricto rechaza strings que empiezan con `#/` —, y de paso el LLM ve la forma real del schema sin tener que seguir referencias.

**Errores recuperables (entregados al LLM como JSON):** `rate_limit`, `timeout`, `upstream`, `spec_parse_failed`, `unsupported_spec_format`, `endpoint_not_found` (con `did_you_mean`), `missing_required_params`, `invalid_param_type`, `missing_auth`, `spec_not_loaded`, `unexpected_html_response`, `swagger2_conversion_failed`. Errores de configuración (`InvalidConfig`, `AdapterInit`, `SpecTooLarge`) crashean el DAG.

**Lifecycle:** El nodo mantiene un `SessionRegistry<Arc<SpecCache>>` indexado por `conversation_id` (default `"default"` cuando el dispatch viene por toolkit path — el toolkit executor no inyecta `conversation_id`). La eviction es **pasiva por TTL** (sweeper cada 60s, idle 15min, max lifetime 1h) más muerte del proceso. El `ConversationLifecycleBus` fue eliminado en commit `8a6a17a` (no había productor real). Ver [`developer_guide/25_web_nodes.md` → Cache lifecycle](../developer_guide/25_web_nodes.md).
