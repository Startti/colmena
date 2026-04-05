# Pending Issues: node_schema Implementation

## Status
✅ **MOSTLY COMPLETE** - Core `node_schema` functionality working
⏳ **ONE BLOCKER** - Token injection in tool execution

---

## What's Working ✅

1. **node_schema Parser** - Full implementation complete
   - `parse_node_schema()` correctly extracts fixed, required, optional fields
   - All 5 unit tests passing
   - 4 integration tests passing
   - 79 total tests passing

2. **Tool Definition Generation** - Verified working
   - `generate_tool_definition()` correctly uses `node_schema` 
   - LLM receives proper tool definitions with required/optional params
   - LLM successfully calls tools with correct parameters

3. **node_schema JSON Format** - Fully functional
   - Fixed values work (`"fixed": "value"`)
   - Required params work (`"required": true`)
   - Optional params work (no `required` or `"required": false`)
   - Pattern validation works (`"pattern": "^....$"`)
   - Nested containers work (`query_params`, `body`, `headers`)

4. **Real-World Test** - Partially working
   - Amadeus OAuth2 token retrieval: ✅ Works with secure values
   - LLM tool calling: ✅ Works - LLM calls `search_flights` with correct params
   - HTTP request execution: ❌ **TOKEN NOT INJECTED**

---

## The Problem 🚨

**Location**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` - `execute()` method

**Issue**: When a tool is executed, fixed values containing `${context.var}` placeholders are NOT resolved.

### Example
```json
"bearer_token": {
  "type": "string",
  "fixed": "${context.amadeus_token}"
}
```

Expected flow:
1. Edge passes token from `get_amadeus_token.body.access_token` → `travel_agent.context.amadeus_token`
2. Token value (hasheado como `<value_6>`) is available in `inputs`
3. When `search_flights` tool executes, `${context.amadeus_token}` should resolve to the token value
4. HTTP request includes `Authorization: Bearer <actual_token>`

Actual flow:
1. ✅ Edge passes token correctly
2. ✅ Token is in inputs
3. ❌ Template NOT resolved - stays as literal `"${context.amadeus_token}"`
4. ❌ HTTP request sent without Authorization header → 401 Unauthorized

### Error Evidence
```
DEBUG: Response Status: 401
DEBUG: Response Body: {"errors":[{"code":38190,"title":"Invalid access token","detail":"The access token provided in the Authorization header is invalid"}]}
```

---

## Attempted Solution

Added template resolution functions in `dag_tool_executor.rs`:
- `resolve_template_string()` - Uses regex to replace `${context.key}` → value from inputs
- `resolve_value_templates()` - Recursively resolves templates in nested Values

Applied after building final inputs in PATH 0 (node_schema):
```rust
let resolved_result = result
    .iter()
    .map(|(k, v)| (k.clone(), Self::resolve_value_templates(v, &result)))
    .collect::<HashMap<String, Value>>();
```

**Result**: Still not working - token still not resolved.

---

## Root Cause Analysis

**Hypothesis**: The token value in `inputs` might be:
1. Under a different key than expected (not `amadeus_token`)
2. Still hashed as `<value_6>` instead of the actual token value
3. Not present in the `inputs` HashMap when `execute()` is called

**What needs investigation**:
1. Add debug logging to see actual `inputs` HashMap contents when tool executes
2. Verify key name - is it `amadeus_token` or something else?
3. Check if secure values auto-injection happens AFTER `execute()` is called
4. Verify `resolve_value_templates()` is actually being called and what it receives

---

## Code Locations

### Files Modified
- `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`
  - Added `#[serde(default)]` to `fixed_config`
  - ✅ COMPLETE

- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
  - Added `resolve_template_string()` function
  - Added `resolve_value_templates()` function  
  - Modified PATH 0 to call `resolve_value_templates()`
  - ⏳ **NEEDS DEBUGGING**

- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
  - No changes needed (already resolves context vars correctly)

- `Cargo.toml`
  - Added `regex = "1.10"` dependency

### Test Files
- `tests/graphs/security/amadeus_secure_gemini_agent_test.json`
  - ✅ Migrated to `node_schema` format
  - ✅ LLM successfully calls tool
  - ❌ HTTP request fails (401 - no token)

---

## Next Steps for Other Agent

### Debug Phase
1. Add detailed logging in `execute()` method:
   - Print `inputs` HashMap before resolution
   - Print each key/value pair
   - Verify token is present and its value

2. Log after `resolve_value_templates()`:
   - Print resolved `inputs` HashMap
   - Verify `${context.amadeus_token}` was replaced

3. Check if token should be resolved as `<value_6>` (hash) or actual token value
   - If hash: auto-injection might happen at HTTP node level (check `http.rs`)
   - If actual token: resolution in `dag_tool_executor.rs` should work

### Fix Phase
Once debug logging reveals the issue:
- Adjust key names if needed
- Check if auto-injection logic exists elsewhere
- Verify template resolution is working correctly
- May need to move resolution to HTTP node level instead

---

## Test Command
```bash
cargo run --bin dag_engine -- run tests/graphs/security/amadeus_secure_gemini_agent_test.json
```

Look for:
- `DEBUG: Response Status: 401` → Token not injected ❌
- `DEBUG: Response Status: 200` → Success ✅

---

## Files to Reference
- `docs/LLM_NODE_COMPLETE_GUIDE.md` - node_schema documentation
- `docs/CREDENTIALS_AND_SECRETS_STRATEGY.md` - Secure values approach
- `tests/graphs/security/amadeus_secure_gemini_agent_test.json` - Real test case
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` - HTTP node implementation (may have auto-injection logic)

---

**Created**: 2026-04-05
**Status**: ⏳ Ready for next agent to debug and fix
**Effort**: ~2-3 hours estimated for complete fix
