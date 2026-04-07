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
| **llm_call** | Language model inference | Calls OpenAI/Gemini/Anthropic; streams tokens; supports tool calling |
| **planner** | Multi-step planning | LLM generates structured plan from inputs |
| **critic** | Quality review | LLM reviews outputs; returns pass/fail assessment |
| **information_extraction** | Schema-based extraction | LLM extracts structured data per schema |
| **reactor** | Summarization & review | LLM summarizes and reviews outputs |
| **orchestrator** | Multi-agent coordination | Manages teams of sub-agents; full lifecycle control |

### **Integration Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **http_request** | HTTP calls | GET/POST/PUT/DELETE to external APIs; supports auth, body, headers |
| **python_script** | Arbitrary code | Executes Python code; injects inputs as variables; requires feature `python` |
| **task_memory_writer** | Persistence | Writes task state to PostgreSQL; for agent memory |

### **Utility Nodes**

| Node | Purpose | Key Behavior |
|---|---|---|
| **mock_input** | Test data | Emits config as-is without transformation |

---

## All Nodes: Defaults Table

| Node Type | `default_input` | `default_output` | Notes |
|---|---|---|---|
| `llm_call` | `prompt` | `result` | LLM node — always maps to/from prompt/result |
| `output` | `input` | `result` | Output node — captures final result |
| `log` | `input` | `output` | Debug logger — pass-through |
| `input` | — | `output` | Static input — reads from config |
| `suspend` | `question` | `answer_received` | Suspend/resume — question→answer flow |
| `loop_controller` | `loop_status` | `output` | Loop control — manages loop state |
| **add** | — | `output` | **Requires explicit `a`, `b` fields** |
| **subtract** | — | `output` | **Requires explicit `a`, `b` fields** |
| **multiply** | — | `output` | **Requires explicit `a`, `b` fields** |
| **divide** | — | `output` | **Requires explicit `a`, `b` fields** |
| `exponential` | `input` | `output` | Power function — single numeric input |
| **http_request** | — | `body` | **Requires explicit `url`, `method`, etc.** |
| `python_script` | — | `output` | **Dynamic inputs** — all inputs flattened as Python variables |
| `planner` | — | `result` | **Dynamic inputs** — any input is treated as text for planning |
| `critic` | — | `result` | **Dynamic inputs** — `texts.*` inputs reviewed by LLM |
| `information_extraction` | — | `result` | **Dynamic inputs** — `texts.*` inputs extracted per schema |
| `reactor` | — | `result` | **Dynamic inputs** — `texts.*` summarized and reviewed |
| `orchestrator` | — | `result` | **Dynamic inputs** — full orchestration lifecycle |
| `task_memory_writer` | — | `result` | **Requires explicit fields** for task management |
| `trigger_webhook` | — | `output` | Webhook trigger — emits payload |
| `mock_input` | — | — | **Raw output** — emits config as-is, no specific field |

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

**3. Resume Phase:** Pass the `session_id` and user's answer to continue:
```bash
cargo run --bin dag_engine -- run graph.json \
  --session-id 6d8928e5-e38c-49c3-a40b-16a1202055f3 \
  --answer "Approved"
```

**4. Internal: Answer Injection:** The DAG engine automatically injects the user's answer into the node's inputs as `__colmena_resume_answer`. This happens before the node executes on resume.

**5. Resume Execution:** The `suspend` node executes with `__colmena_resume_answer` present and produces:
```json
{
  "status": "resumed",
  "answer_received": "Approved"
}
```

**Important:** The `answer_received` field contains the **exact value** passed via `--answer`, not a modified version. If you passed `--answer "Approved"`, then `answer_received = "Approved"`. If you passed `--answer '{"status": "ok"}'`, then `answer_received = {"status": "ok"}` (JSON parsing).

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
    "approval": { "type": "suspend", "config": { "question": "Approve?" } },
    "final_output": { "type": "log" }
  },
  "edges": [
    { "from": "process_node", "to": "approval" },
    { "from": "approval", "to": "final_output" }
  ]
}
```
After user approves with `--answer "yes"`, execution continues to `final_output`.

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
Chain multiple `suspend` nodes for cascading approvals. Resume each one with `--session-id <id> --answer <response>`.

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

# ✅ Correct: with --answer
cargo run --bin dag_engine -- run graph.json --session-id abc123 --answer "yes"
```

### Issue: Answer received as JSON string instead of object

**Cause:** CLI argument is quoted but not parsed.

**Solutions:**
```bash
# If you want a string "yes"
cargo run --bin dag_engine -- run graph.json --session-id abc123 --answer "yes"
# answer_received = "yes"

# If you want JSON object {"approved": true}
cargo run --bin dag_engine -- run graph.json --session-id abc123 --answer '{"approved": true}'
# answer_received = {approved: true} (JSON parsed)
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

