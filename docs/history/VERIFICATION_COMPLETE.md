# Secure Values + Node Schema - COMPLETE & VERIFIED ✅

## Executive Summary

**Fixed the token injection problem** in the secure values + node_schema system. The LLM node now properly resolves `${...}` templates in tool configurations before passing them to the tool executor, enabling secure multi-step workflows with real APIs.

### Status
- ✅ **Both user concerns addressed**
- ✅ **Security verified**: LLM isolation confirmed
- ✅ **Real-world tested**: Amadeus flight search works end-to-end

---

## Addressing User Concerns

### Concern 1: "Does the resolver handle ALL variables, not just context?"

**Answer: YES - Fixed ✅**

#### Changed Pattern Matching
```rust
// BEFORE: Only matched ${context.*}
while let Some(start) = value[last_end..].find("${context.") {

// AFTER: Matches ANY ${...} pattern
while let Some(start) = value[last_end..].find("${") {
```

#### Now Resolves All Patterns
- ✅ `${context.amadeus_token}` → context inputs
- ✅ `${trigger.api_key}` → trigger node outputs  
- ✅ `${get_token.body.access_token}` → any upstream node
- ✅ Any variable in flattened inputs dictionary

### Concern 2: "Does the LLM actually NOT see the real token values?"

**Answer: YES - Verified ✅**

#### Evidence from Test Output

**Actual token from Amadeus API:**
```json
"access_token": "eI1yeHGNGztyOOOR4zOApuGs5MuL"
```

**What LLM node receives (config):**
```json
"access_token": "<value_6>"
```

**What LLM tool definition shows:**
- `bearer_token` field is **HIDDEN** from LLM (has `fixed` value)
- LLM never sees `bearer_token` as a parameter to fill
- LLM only sees the required parameters: `originLocationCode`, `destinationLocationCode`, `departureDate`, `adults`

**Verification:**
1. ✅ Real token appears only in HTTP response debug logs
2. ✅ Real token does NOT appear in `tool-input-available` events
3. ✅ LLM input shows only hash `<value_6>`, never the real token
4. ✅ Auto-injection happens at HTTP node execution, not at LLM level

---

## Implementation Details

### File Changed
`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

### Change 1: Generic Template Resolution (Line 49)
```rust
/// Resolve all ${var} placeholders (context, trigger, node outputs, etc.)
/// Matches ${anything.with.dots} and looks it up in inputs
fn resolve_context_vars(value: &str, inputs: &NodeInputs) -> String {
    // ... pattern matching now looks for "$({"  instead of "${ context."
}
```

### Change 2: Recursive Node Schema Resolver (Lines 85-109)
```rust
fn resolve_context_in_node_schema(
    schema: &mut NodeSchema,
    inputs: &NodeInputs,
) {
    // Handles top-level fixed fields
    // Recursively handles nested properties in containers
    // Applies generic template resolution to each field
}
```

### Change 3: Tool Config Initialization (Lines 453-469)
```rust
// Resolve BOTH deprecated fixed_config AND new node_schema
for tool_cfg in tool_configurations.values_mut() {
    // Legacy path: fixed_config
    for val in tool_cfg.fixed_config.values_mut() { ... }
    
    // New path: node_schema (recursive)
    if let Some(node_schema) = tool_cfg.node_schema.as_mut() {
        Self::resolve_context_in_node_schema(node_schema, inputs);
    }
}
```

---

## Data Flow (Complete)

```
Step 1: OAuth2 Token Request
┌──────────────────────────────┐
│ HTTP: get_amadeus_token      │
│ Returns: access_token = "..." │
│ Status: 200 ✅               │
└──────────────┬───────────────┘
               │ secure: true
               ↓
┌──────────────────────────────┐
│ SecureValueService.hash()    │
│ "<secret_token>" → <value_6> │
│ Store encrypted in DB        │
└──────────────┬───────────────┘
               │
               ↓
Step 2: Edge Passes Hash to LLM
┌──────────────────────────────────────┐
│ Edge: get_amadeus_token.access_token │
│ → travel_agent.context.amadeus_token │
│ Value: <value_6> (hash)              │
└──────────────┬──────────────────────┘
               │
               ↓
Step 3: LLM Node Resolves Templates [THIS WAS BROKEN]
┌─────────────────────────────────────────┐
│ Tool Config: bearer_token:              │
│   "${context.amadeus_token}"            │
│                                         │
│ LLM Node RESOLVES:                      │
│ ${context.amadeus_token} → <value_6>    │
│                                         │
│ Tool Config becomes:                    │
│   bearer_token: "<value_6>"             │
└──────────────┬────────────────────────┘
               │ [NOW WORKING ✅]
               ↓
Step 4: LLM Calls Tool
┌──────────────────────────┐
│ Tool: search_flights     │
│ Params: origin, dest,    │
│         date, adults     │
│ (NO bearer_token param   │
│  - it's hidden/fixed)    │
└──────────────┬───────────┘
               │
               ↓
Step 5: Tool Execution
┌──────────────────────────────────┐
│ HTTP Node receives:              │
│ bearer_token: "<value_6>" (hash) │
└──────────────┬───────────────────┘
               │
               ↓
┌──────────────────────────────────────┐
│ SecureValueService.inject_secrets()  │
│ Find: <value_6> in database          │
│ Decrypt: real token "eI1yeHGNGzty..." │
│ Replace in inputs                    │
└──────────────┬──────────────────────┘
               │
               ↓
┌──────────────────────────────────────┐
│ HTTP Request sent:                   │
│ Authorization: Bearer eI1yeHGNGzty.. │
│ Status: 200 ✅                       │
│ Response: 27 flight offers           │
└──────────────────────────────────────┘
```

---

## Test Results

### Real-World Test
**Graph**: `tests/graphs/security/amadeus_secure_gemini_agent_test.json`

#### Before Fix
```
Step 1 (Get Token):   ✅ Status 200
Step 2 (Search Flights): ❌ Status 401 Unauthorized
  "Invalid access token"
  Reason: bearer_token still showed literal "${context.amadeus_token}"
```

#### After Fix
```
Step 1 (Get Token):   ✅ Status 200 
Step 2 (Search Flights): ✅ Status 200
  "Found 27 flight offers"
  LLM Analysis: "Best option is flight IB423 departing at 19:05..."
```

### Run Test
```bash
cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_agent_test.json
```

---

## Security Properties - VERIFIED

### 1. LLM Isolation ✅
- LLM receives **hash only** in context inputs: `<value_6>`
- LLM never receives actual token value: `eI1yeHGNGzty...`
- `bearer_token` field is hidden from LLM (has `fixed` value in schema)
- Tool definition shows only required user-supplied params

### 2. Tool Injection ✅
- Tools receive hash: `<value_6>`
- Before execution, SecureValueService replaces hash with real value
- HTTP request includes Authorization header with real token
- API authentication succeeds with 200 response

### 3. Template Flexibility ✅
- Supports any `${pattern}` in fixed values
- Not limited to context-only variables
- Can reference trigger outputs, upstream nodes, etc.
- Recursive resolution in nested properties

### 4. Backward Compatibility ✅
- Legacy `fixed_config` still works
- Old test graphs unchanged
- Graceful upgrade to new `node_schema`
- No breaking changes to API

---

## Code Changes Summary

**1 file modified**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

**3 changes**:
1. Generic `${...}` pattern matching (was context-only)
2. Recursive node_schema resolver function (new)
3. Tool config initialization for both fixed_config and node_schema (updated)

**Lines changed**: ~60 lines (additions + comments)
**Complexity**: Low - focused changes to resolver logic
**Impact**: Enables secure multi-step agent orchestration

---

## What's Now Possible

With this fix, you can now:

✅ **Multi-step OAuth workflows**
- Get token from OAuth2 endpoint
- Pass hash to LLM (LLM sees only hash)
- LLM calls tools that use real token (auto-injected)
- Tools make authenticated API calls

✅ **Flexible Variable References**
- `${context.var}` - from edge connections
- `${trigger.field}` - from trigger payload
- `${upstream_node.output.path}` - from any node
- All supported in tool configuration fixed values

✅ **Nested Parameter Organization**
- Fixed values in headers: `${context.api_key}`
- Fixed values in query params: `${context.region}`
- Fixed values in body: `${trigger.correlation_id}`
- Works at any nesting level

✅ **LLM Safety**
- LLM controls which tool params to fill
- LLM never sees sensitive values
- Secure injection happens behind the scenes
- No credential exposure in LLM context

---

## Files to Reference

- `SECURE_VALUES_DESIGN.md` - Architecture overview (documento borrado en `9dea7419`; vigente: [Secure Values — diseño](../dds/SECURE_VALUES_DISEÑO.md))
- `CREDENTIALS_AND_SECRETS_STRATEGY.md` - All credential approaches (documento borrado en `9dea7419`; vigente: [Security Strategy](../developer_guide/13_security_strategy.md))
- [SECURE_VALUES_QUICK_REFERENCE.md](SECURE_VALUES_QUICK_REFERENCE.md) - Usage examples
- `LLM_NODE_COMPLETE_GUIDE.md` - Tool config reference (documento borrado en `9dea7419`; vigente: [LLM Deep Dive](../developer_guide/14_llm_deep_dive.md))

---

**Status**: ✅ COMPLETE & VERIFIED  
**Date**: 2026-04-05  
**Impact**: Enables production-ready secure agent orchestration  
**Breaking Changes**: None
