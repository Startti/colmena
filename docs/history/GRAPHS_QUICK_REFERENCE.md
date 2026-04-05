# Test Graphs - Quick Reference Guide

**Last Updated:** 2026-04-04  
**Total Graphs:** 56 (50 original + 6 new examples)  
**Status:** Under consolidation

---

## 🎯 Find an Example By Feature

### **I want to learn about $DYNAMIC parameters**
- ✅ `agents/http_tool_dynamic_placeholder_test.json` — Body + Headers
- ✅ `external/amadeus_flight_search_dynamic.json` — Query params + OAuth2
- ✅ `external/http_headers_dynamic.json` — Headers only
- ✅ `external/http_body_nested_dynamic.json` — Nested JSON + $DYNAMIC

### **I want to learn about Default Ports**
- ✅ `edge_resolution/test_case_1_1_implicit_with_defaults.json` — Implicit defaults
- ✅ `edge_resolution/default_ports_chain.json` — Multi-step chain (NEW)
- ✅ `edge_resolution/default_output_ports_named.json` — Named outputs (NEW)

### **I want to learn about LLM + Tools**
- ✅ `agents/agent_with_tools.json` — Math operations (OpenAI)
- ✅ `agents/agent_with_tools_stream.json` — Streaming output
- ✅ `agents/agent_with_tools_gemini.json` — Gemini provider

### **I want to learn about Authentication**
- ✅ `external/debug_amadeus_token_only.json` — OAuth2 token retrieval
- ✅ `external/amadeus_flight_search_dynamic.json` — Bearer token in headers

### **I want to learn about Media Processing**
- ✅ `media/image_path.json` — Vision with image URL
- ✅ `media/pdf_path.json` — Document processing

### **I want to learn about Database Persistence**
- ✅ `memory/memory_sqlite_example.json` — SQLite backend
- ✅ `memory/memory_postgres_example.json` — PostgreSQL backend

### **I want to learn about Field Extraction**
- ✅ `edge_resolution/test_case_4_1_smart_extraction.json` — Smart extraction
- ✅ `edge_resolution/smart_extraction_complex.json` — Array flattening (NEW)

### **I want a simple example to start**
- ✅ `basic/trigger.json` — Webhook → Log (2 nodes)
- ✅ `basic/input_example.json` — Input → Log (2 nodes, default ports)

### **I want a complete end-to-end workflow**
- ✅ `advanced/travel_agent_amadeus.json` — Full travel planning agent
- ✅ `external/amadeus_flight_search_dynamic.json` — Flight search with LLM

---

## 📊 Graph Organization by Complexity

### **Beginner** (Learn the basics)
```
basic/
├── trigger.json                          (Webhook trigger)
├── input_example.json                    (Input node + default ports)
├── power.json                            (Math operation)
└── python_simple_graph.json              (Python execution)

edge_resolution/
├── test_case_1_1_implicit_with_defaults.json    (Implicit ports)
└── default_ports_chain.json [NEW]        (Default port chaining)
```

### **Intermediate** (Expand your skills)
```
agents/
├── agent_with_tools.json                 (LLM + tools)
├── agent_with_tools_stream.json          (Streaming)
└── http_tool_dynamic_placeholder_test.json [Feature: $DYNAMIC]

external/
├── http_request.json                     (Plain HTTP)
├── debug_amadeus_token_only.json         (OAuth2)
├── http_headers_dynamic.json [NEW]       (Headers + $DYNAMIC)
└── http_body_nested_dynamic.json [NEW]   (Nested body + $DYNAMIC)

edge_resolution/
├── test_case_1_4_fully_explicit.json     (Explicit mapping)
├── test_case_2_2_explicit_required_add.json
├── test_case_4_1_smart_extraction.json   (Field extraction)
└── smart_extraction_complex.json [NEW]   (Complex arrays)

media/
├── image_path.json                       (Vision processing)
└── pdf_path.json                         (Document processing)

memory/
├── memory_sqlite_example.json            (SQLite persistence)
└── memory_postgres_example.json          (PostgreSQL persistence)
```

### **Advanced** (Master complex workflows)
```
agents/
└── agent_with_tools_gemini.json          (Alternative LLM provider)

external/
└── amadeus_flight_search_dynamic.json [NEW]  (Amadeus + $DYNAMIC + OAuth2)

edge_resolution/
└── default_output_ports_named.json [NEW]     (Multiple outputs)

advanced/
└── travel_agent_amadeus.json             (End-to-end travel planning)

examples/
└── llm_chain_birthday.json               (Multi-step LLM chain)
```

---

## 🚀 Running Examples

### **Run a single graph locally**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
cargo run --bin dag_engine -- run tests/graphs/agents/agent_with_tools.json
```

### **Run as HTTP server**
```bash
cargo run --bin dag_engine -- serve tests/graphs/agents/agent_with_tools.json
# Access: http://localhost:3000
```

### **Test with environment variables**
```bash
export OPENAI_API_KEY="sk-..."
export AMADEUS_CLIENT_ID="..."
export AMADEUS_CLIENT_SECRET="..."
cargo run --bin dag_engine -- run tests/graphs/external/amadeus_flight_search_dynamic.json
```

---

## 📋 Feature Matrix

| Feature | Examples | Status |
|---------|----------|--------|
| **Basic Nodes** | trigger.json, input_example.json | ✅ Ready |
| **Default Ports** | test_case_1_1_implicit_with_defaults.json, default_ports_chain.json | ✅ Ready |
| **$DYNAMIC** | http_tool_dynamic_placeholder_test.json, amadeus_flight_search_dynamic.json, http_headers_dynamic.json, http_body_nested_dynamic.json | ✅ Ready |
| **LLM + Tools** | agent_with_tools.json, agent_with_tools_stream.json, agent_with_tools_gemini.json | ✅ Ready |
| **OAuth2** | debug_amadeus_token_only.json, amadeus_flight_search_dynamic.json | ✅ Ready |
| **Streaming** | agent_with_tools_stream.json, llm_stream_dag.json | ✅ Ready |
| **Vision/Media** | image_path.json, pdf_path.json | ✅ Ready |
| **SQLite** | memory_sqlite_example.json | ✅ Ready |
| **PostgreSQL** | memory_postgres_example.json | ✅ Ready |
| **Field Extraction** | test_case_4_1_smart_extraction.json, smart_extraction_complex.json | ✅ Ready |

---

## 🔧 Environment Variables Required

| Graph | Required Env Vars |
|-------|------------------|
| `agent_with_tools.json` | `OPENAI_API_KEY` |
| `agent_with_tools_stream.json` | `OPENAI_API_KEY` |
| `agent_with_tools_gemini.json` | `GEMINI_API_KEY` |
| `http_tool_dynamic_placeholder_test.json` | `OPENAI_API_KEY` |
| `amadeus_flight_search_dynamic.json` | `OPENAI_API_KEY`, `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET`, `DATABASE_URL` |
| `debug_amadeus_token_only.json` | `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET` |
| `memory_sqlite_example.json` | `OPENAI_API_KEY` |
| `memory_postgres_example.json` | `OPENAI_API_KEY`, `DATABASE_URL` |
| `travel_agent_amadeus.json` | `OPENAI_API_KEY`, `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET`, `DATABASE_URL` |

---

## 📚 Learning Paths

### **Path 1: Getting Started (30 min)**
1. Read: [GRAPHS_REVIEW.md](GRAPHS_REVIEW.md) overview
2. Run: `basic/trigger.json`
3. Run: `basic/input_example.json`
4. Run: `basic/power.json`
5. Understand: How nodes connect via edges

### **Path 2: LLM Fundamentals (1 hour)**
1. Run: `agents/agent_with_tools.json`
2. Read: Node config for `llm_call` type
3. Run: `agents/agent_with_tools_stream.json`
4. Understand: Tool calling and streaming

### **Path 3: $DYNAMIC Parameters (1 hour)**
1. Read: `http_tool_dynamic_placeholder_test.json` comments
2. Understand: How `$DYNAMIC` placeholders work
3. Run: `external/http_headers_dynamic.json`
4. Run: `external/http_body_nested_dynamic.json`
5. Run: `external/amadeus_flight_search_dynamic.json`

### **Path 4: External APIs with Auth (1.5 hours)**
1. Read: `debug_amadeus_token_only.json`
2. Understand: OAuth2 flow and token passing
3. Run: `amadeus_flight_search_dynamic.json` (requires Amadeus credentials)
4. Study: Edge from token to LLM context

### **Path 5: Advanced Features (2 hours)**
1. Default ports: `edge_resolution/default_ports_chain.json`
2. Field extraction: `edge_resolution/smart_extraction_complex.json`
3. Persistence: `memory/memory_sqlite_example.json`
4. Complex orchestration: `advanced/travel_agent_amadeus.json`

---

## ✅ Validation Checklist

Before using examples in production:

- [ ] All environment variables are set
- [ ] Graph runs without errors: `cargo run --bin dag_engine -- run <file>`
- [ ] Output matches expected format
- [ ] External APIs are reachable (if applicable)
- [ ] Database connections work (if applicable)

---

## 🗑️ Deprecated / To Be Consolidated

These files are candidates for removal or consolidation:

```
❌ agents/llm_call.json                         → Superseded by agent_with_tools.json
❌ agents/llm_stream_tool.json                  → Superseded by agent_with_tools_stream.json
❌ agents/llm_stream_dag.json                   → Superseded by agent_with_tools_stream.json
❌ agents/llm_gemini_stream_tool.json           → Superseded by agent_with_tools_gemini.json
❌ agents/llm_local_test.json                   → Incomplete test file
❌ agents/http_tool_field_mapping_test.json     → Superseded by $DYNAMIC approach
❌ agents/extraction_example.json               → Unclear purpose
❌ agents/python_llm_graph.json                 → Superseded by python_simple_graph.json
❌ agents/planner_test.json                     → Incomplete test file
❌ external/dynamic_http.json                   → Superseded by http_request.json
❌ external/debug_amadeus_flight_no_llm.json    → Debug artifact
❌ external/debug_amadeus_auth_flight.json      → Superseded by amadeus_flight_search_dynamic.json
❌ advanced/llm_tools_memory_continuation.json  → Superseded by memory/ examples
❌ advanced/llm_tools_memory_test.json          → Superseded by memory/ examples
❌ advanced/test_orchestrator.json              → Incomplete test file
❌ advanced/test_suspend.json                   → Edge case test
❌ advanced/trip_planner.json                   → v1, superseded by v2/Amadeus
❌ advanced/trip_planner_v2.json                → Incomplete
❌ basic/test_cyclic_graph.json                 → Edge case test
❌ basic/test_cyclic_early_stop.json            → Edge case test
❌ basic/test_loop.json                         → Duplicate of test_loop_direct.json
❌ media/pdf_base64.json                        → Superseded by pdf_path.json
```

---

## 📞 Need Help?

- **Question:** "How do I use $DYNAMIC parameters?"
  - **Answer:** See `external/amadeus_flight_search_dynamic.json` or `external/http_headers_dynamic.json`

- **Question:** "How do default ports work?"
  - **Answer:** See `edge_resolution/default_ports_chain.json`

- **Question:** "How do I integrate with OAuth2?"
  - **Answer:** See `external/debug_amadeus_token_only.json` and `external/amadeus_flight_search_dynamic.json`

- **Question:** "What's a simple starting point?"
  - **Answer:** Start with `basic/trigger.json`, then `basic/input_example.json`

- **Question:** "Can I see a complete workflow?"
  - **Answer:** See `advanced/travel_agent_amadeus.json` or `external/amadeus_flight_search_dynamic.json`

---
