# Node Ports Reference — Default Input/Output System

## Overview

In Colmena's DAG engine, each node has optional **default ports** for input and output. This system simplifies edge definition by automatically mapping fields when you don't specify them explicitly.

**Key principle:** Instead of manually specifying every field in an edge, declare sensible defaults once per node type, then use shorthand edges.

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

