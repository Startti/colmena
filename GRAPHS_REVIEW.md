# DAG Test Graphs - Comprehensive Review & Optimization Plan

**Date:** 2026-04-04  
**Total Graphs:** 50  
**Status:** Under Review

---

## 📊 Executive Summary

The test graph collection has **significant redundancy**:
- **13 files** are nearly identical (3-node LLM+tool patterns)
- **6 debug files** with limited educational value
- **Missing:** Amadeus with $DYNAMIC, comprehensive edge resolution examples
- **Opportunity:** Consolidate to ~25 strategic examples with better coverage

---

## 🔴 HIGH-PRIORITY ISSUES

### 1. **Redundant 3-Node LLM+Tool Patterns** (9 files)
These are structurally identical except for small configuration differences:
- `agent_with_tools.json` (Math operations)
- `agent_with_tools_postgres.json` (Database persistence)
- `agent_with_tools_postgres_recall.json` (Database + recall)
- `agent_with_tools_stream.json` (Streaming output)
- `agent_with_tools_gemini.json` (Alternative LLM provider)
- `llm_call.json` (Simple webhook+LLM+log)
- `llm_stream_tool.json` (Streaming variant)
- `llm_gemini_stream_tool.json` (Gemini variant)
- `http_tool_configured.json` (HTTP tool variant)

**Action:** Consolidate to 2-3 exemplars with clear use case labels

---

### 2. **Debug Files with Test Suffixes** (5 files)
These files appear to be debugging aids, not reference examples:
- `debug_amadeus_token_only.json` (Token retrieval only)
- `debug_amadeus_flight_no_llm.json` (Manual flight search)
- `debug_amadeus_auth_flight.json` (Auth + flight + LLM)
- `http_tool_dynamic_placeholder_test.json` (Test variant)
- `http_tool_field_mapping_test.json` (Test variant)

**Action:** Move to separate `tests/graphs/debug/` folder or delete if superseded

---

### 3. **Missing $DYNAMIC Examples**
Only 1 file uses the new `$DYNAMIC` placeholder:
- ✅ `http_tool_dynamic_placeholder_test.json`

**Missing:**
- Amadeus flight search WITH $DYNAMIC (LLM generates search params)
- Header merging with $DYNAMIC
- Query parameter $DYNAMIC
- Complex body $DYNAMIC with nested objects

**Action:** Create comprehensive $DYNAMIC examples

---

### 4. **Incomplete Edge Resolution Coverage**
6 edge_resolution test files exist, but:
- Mostly simple 2-node patterns
- Limited real-world complexity
- Could be better documented with "before/after" examples

**Action:** Enhance with multi-step workflows

---

## 📁 Current Structure Analysis

```
tests/graphs/
├── advanced/        (7 files)  - Complex orchestration
├── agents/          (15 files) - LLM + tools (HIGHLY REDUNDANT)
├── basic/           (10 files) - Simple operations
├── edge_resolution/ (6 files)  - Port resolution test cases
├── examples/        (1 file)   - LLM chain example
├── external/        (6 files)  - HTTP, Amadeus (SOME DUPLICATES)
├── media/           (3 files)  - Vision/PDF (NEARLY IDENTICAL)
└── memory/          (2 files)  - Persistence examples
```

---

## ✅ RECOMMENDED CONSOLIDATION PLAN

### **TIER 1: KEEP - Strategic Examples** (15 files)

| File | Purpose | Features |
|------|---------|----------|
| **Basic Operations** | | |
| `basic/trigger.json` | Simplest possible graph | webhook → log |
| `basic/input_example.json` | Default input ports | input → log |
| `basic/power.json` | Basic math operation | exponential function |
| `basic/python_simple_graph.json` | Python script execution | python_script node |
| **Edge Resolution** | | |
| `edge_resolution/test_case_1_1_implicit_with_defaults.json` | Default ports (implicit) | ✅ new feature |
| `edge_resolution/test_case_1_4_fully_explicit.json` | Explicit port mapping | |
| `edge_resolution/test_case_2_2_explicit_required_add.json` | Required field addition | |
| `edge_resolution/test_case_4_1_smart_extraction.json` | Smart field extraction | |
| **LLM + Tools** | | |
| `agents/agent_with_tools.json` | Math agent with tools | Basic tool calling |
| `agents/agent_with_tools_stream.json` | Streaming output | Stream mode |
| `agents/http_tool_dynamic_placeholder_test.json` | $DYNAMIC parameters | ✅ new feature |
| **External Services** | | |
| `external/http_request.json` | Plain HTTP request | No auth |
| `external/debug_amadeus_token_only.json` | OAuth2 flow (token) | Bearer token auth |
| **Media** | | |
| `media/image_path.json` | Vision (image URL) | LLM vision input |
| `media/pdf_path.json` | Document processing | File path input |
| **Memory** | | |
| `memory/memory_sqlite_example.json` | SQLite persistence | Session storage |

**Subtotal: 15 files**

---

### **TIER 2: CONSOLIDATE - Merge Duplicates** (20 → 5 files)

#### **A. Database Persistence Variants** (3 → 1)
- ❌ `agent_with_tools_postgres.json` 
- ❌ `agent_with_tools_postgres_recall.json`
- ✅ **KEEP:** `memory/memory_postgres_example.json`
  - Already covers "recall" use case clearly

#### **B. LLM + HTTP Tool Variants** (9 → 1)
Keep ONE exemplar per provider/mode combination:
- ✅ `agents/agent_with_tools.json` (OpenAI, basic)
- ✅ `agents/agent_with_tools_stream.json` (OpenAI, streaming)
- ✅ `agents/agent_with_tools_gemini.json` (Gemini provider)
- ❌ Remove: `llm_call.json`, `llm_stream_tool.json`, `llm_stream_dag.json`, `llm_gemini_stream_tool.json`, `llm_local_test.json`

**Rationale:** Sufficiently covered by tier 1 + streaming + Gemini variants

#### **C. Basic HTTP Patterns** (3 → 1)
- ❌ `dynamic_http.json`
- ❌ `http_request.json` (move to tier 1)
- ✅ **KEEP & RENAME:** `external/http_request.json` (basic pattern)

#### **D. Media Variants** (3 → 2)
- ✅ `media/image_path.json` (URL input)
- ❌ `media/pdf_base64.json` (redundant encoding variant)
- ✅ `media/pdf_path.json` (file path input)

#### **E. Advanced LLM Chains** (3 → 1)
- ❌ `advanced/llm_tools_memory_continuation.json`
- ❌ `advanced/llm_tools_memory_test.json`
- ✅ **KEEP:** `examples/llm_chain_birthday.json` (clear educational purpose)

**Subtotal: ~5 consolidated files**

---

### **TIER 3: DELETE - Test/Debug Artifacts** (10 files)

These are clearly debug/test files with limited reference value:
```
❌ agents/http_tool_field_mapping_test.json (deprecated, superseded by $DYNAMIC)
❌ advanced/test_orchestrator.json (incomplete example)
❌ advanced/test_suspend.json (edge case test)
❌ advanced/test_suspend_manual.json (edge case test) [if exists]
❌ advanced/trip_planner.json (v1, superseded by v2/Amadeus)
❌ advanced/trip_planner_v2.json (incomplete)
❌ basic/test_cyclic_graph.json (edge case)
❌ basic/test_cyclic_early_stop.json (edge case)
❌ basic/test_loop.json (duplicate of test_loop_direct.json)
❌ agents/extraction_example.json (unclear purpose)
❌ agents/planner_test.json (incomplete)
❌ agents/python_llm_graph.json (redundant with python_simple_graph.json)
```

**Action:** Move to `tests/graphs/deprecated/` archive for reference, or delete if git history preserves them

---

### **TIER 4: CREATE - Missing Examples** (6 new files)

These examples showcase new/important features:

#### **NEW 1: Amadeus Flight Search with $DYNAMIC**
**File:** `external/amadeus_flight_search_dynamic.json`
```json
{
  "trigger": { "type": "trigger_webhook" },
  "get_token": { "type": "http_request", /* Amadeus OAuth2 */ },
  "agent_search": {
    "type": "llm_call",
    "tools": [{
      "name": "search_flights",
      "node_type": "http_request",
      "fixed_config": {
        "base_url": "https://api.amadeus.com",
        "endpoint": "/v2/shopping/flight-offers",
        "method": "GET",
        "bearer_token": "${context.amadeus_token}",
        "query_params": {
          "originLocationCode": "$DYNAMIC",
          "destinationLocationCode": "$DYNAMIC",
          "departureDate": "$DYNAMIC",
          "adults": "$DYNAMIC"
        }
      }
    }]
  }
}
```
**Features:** $DYNAMIC, query params, OAuth2, LLM generates specifics

---

#### **NEW 2: HTTP Header Merging with $DYNAMIC**
**File:** `external/http_headers_dynamic.json`
**Features:** $DYNAMIC in headers, custom authentication, LLM generates auth values

---

#### **NEW 3: Complex Body Structure with $DYNAMIC**
**File:** `external/http_body_nested_dynamic.json`
**Features:** Nested JSON objects, $DYNAMIC at multiple levels, field_mapping with mergeable_fields

---

#### **NEW 4: Default Ports with Chained Operations**
**File:** `edge_resolution/default_ports_chain.json`
**Features:** Multi-step flow using only default ports (no explicit edge config)

---

#### **NEW 5: Default Output Ports (Named Outputs)**
**File:** `edge_resolution/default_output_ports.json`
**Features:** Nodes with named outputs automatically routed without explicit port specification

---

#### **NEW 6: Edge Resolution with Smart Extraction**
**File:** `edge_resolution/smart_extraction_complex.json`
**Features:** Multi-field matching, automatic flattening, array handling

---

## 📊 Final Structure

```
tests/graphs/
├── basic/                                    (5 files)
│   ├── trigger.json
│   ├── input_example.json
│   ├── power.json
│   └── python_simple_graph.json
│   └── power_webhook.json [KEEP - webhook variant]
│
├── agents/                                   (4 files)
│   ├── agent_with_tools.json [OpenAI basic]
│   ├── agent_with_tools_stream.json [OpenAI streaming]
│   ├── agent_with_tools_gemini.json [Gemini provider]
│   └── http_tool_dynamic_placeholder_test.json [NEW: $DYNAMIC showcase]
│
├── edge_resolution/                          (6 files)
│   ├── test_case_1_1_implicit_with_defaults.json
│   ├── test_case_1_4_fully_explicit.json
│   ├── test_case_2_2_explicit_required_add.json
│   ├── test_case_4_1_smart_extraction.json
│   ├── default_ports_chain.json [NEW]
│   └── default_output_ports.json [NEW]
│
├── examples/                                 (1 file)
│   └── llm_chain_birthday.json
│
├── external/                                 (4 files)
│   ├── http_request.json [Basic HTTP]
│   ├── debug_amadeus_token_only.json [OAuth2 auth]
│   ├── amadeus_flight_search_dynamic.json [NEW: $DYNAMIC + Amadeus]
│   ├── http_headers_dynamic.json [NEW: Headers + $DYNAMIC]
│   └── http_body_nested_dynamic.json [NEW: Nested body + $DYNAMIC]
│
├── media/                                    (2 files)
│   ├── image_path.json
│   └── pdf_path.json
│
├── memory/                                   (2 files)
│   ├── memory_sqlite_example.json
│   └── memory_postgres_example.json
│
└── advanced/                                 (1 file)
    └── travel_agent_amadeus.json [Complex orchestration example]
```

**Total: 25 files** (down from 50)

---

## 🔧 Implementation Checklist

- [ ] **Phase 1: Consolidation**
  - [ ] Delete 10 test/debug files
  - [ ] Merge 6 variants into exemplars
  - [ ] Archive deprecated files

- [ ] **Phase 2: New Examples**
  - [ ] Create Amadeus + $DYNAMIC example
  - [ ] Create headers + $DYNAMIC example
  - [ ] Create nested body + $DYNAMIC example
  - [ ] Create default ports chain example
  - [ ] Create default output ports example
  - [ ] Create complex extraction example

- [ ] **Phase 3: Validation**
  - [ ] Run all 25 remaining graphs with `cargo run --bin dag_engine -- run`
  - [ ] Verify $DYNAMIC examples with mocked LLM responses
  - [ ] Check all Amadeus examples for token flow correctness
  - [ ] Confirm edge resolution examples demonstrate intended behavior

- [ ] **Phase 4: Documentation**
  - [ ] Update `docs/developer_guide/` with graph usage guide
  - [ ] Add comments to each example explaining key features
  - [ ] Create quick reference: "Which example for X?"

---

## 📝 Example Structure Template

Each example JSON should include:
```json
{
  "comment": "Clear 1-line description of what this example demonstrates",
  "metadata": {
    "category": "agents|external|memory|edge_resolution",
    "features": ["$DYNAMIC", "OAuth2", "streaming"],
    "provider": "openai|gemini|anthropic|none",
    "difficulty": "beginner|intermediate|advanced"
  },
  "nodes": { ... },
  "edges": [ ... ]
}
```

---

## ❓ Questions for Review

1. **Keep `travel_agent_amadeus.json`?** (Advanced, ~80 lines)
   - Currently only realistic end-to-end example
   - Could be good reference OR redundant with simpler examples
   - **Recommendation:** KEEP (educational value for complex flows)

2. **Archive vs. Delete debug files?**
   - Current: test files clutter the "examples" directory
   - **Recommendation:** Move to `tests/graphs/deprecated/` with README explaining why

3. **Default ports feature maturity:**
   - Several edge_resolution examples exist, are they all still valid?
   - **Recommendation:** Validate against current implementation

4. **Media examples completeness:**
   - Only 3 media examples (image path, PDF path, PDF base64)
   - Missing: image base64, video, complex media in agent context
   - **Recommendation:** Keep current 2, consider adding more if needed

---

## 🎯 Success Criteria

✅ **After Consolidation:**
- [ ] All graphs are unique and exemplary (no near-duplicates)
- [ ] Each graph demonstrates ≥1 distinct feature
- [ ] All new features ($DYNAMIC, default ports) have clear examples
- [ ] Graphs are organized by educational level + feature
- [ ] All graphs pass validation (`cargo run --bin dag_engine -- run`)
- [ ] Documentation clearly maps: "I want to learn X" → "Read file Y"

---

