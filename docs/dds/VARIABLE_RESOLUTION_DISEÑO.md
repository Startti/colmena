# Variable Resolution Specification

## Overview

Colmena supports two types of variable resolution in DAG configurations:

1. **Environment Variables** - System-level variables from the process environment
2. **Node Output References** - Values from outputs of other nodes in the DAG

## Syntax

### Environment Variables
```
${UPPERCASE_VARIABLE_NAME}
```

**Pattern**: `${[A-Z_0-9]+}`

**Where used**: Anywhere in config - headers, body, endpoints, etc.

**Resolution**: Looks up in `process.env` at node execution time

**Examples**:
```json
{
  "client_id": "${AMADEUS_CLIENT_ID}",
  "api_key": "${OPENAI_API_KEY}",
  "db_password": "${DATABASE_PASSWORD}"
}
```

**Behavior**:
- If variable not found in env → Error (fails fast)
- Resolved by HTTP node, LLM node, and other infrastructure nodes
- Not passed to LLM (LLM never sees actual env values)

---

### Node Output References
```
${node_name.field.path}
```

**Pattern**: `${[a-z_][a-z0-9_]*(\.[a-z_][a-z0-9_]*)*}`

**Where used**: 
- In `inputs` section (edge connections)
- In LLM tool configurations (fixed values in node_schema)
- In prompts and contexts

**Resolution**: Looks up in flattened inputs dictionary at execution time

**Examples**:
```json
{
  "from": "get_token.body.access_token",
  "to": "search_tool.context.auth_token"
}
```

```json
{
  "fixed": "${get_token.body.access_token}",
  "comment": "Reference node output explicitly"
}
```

```json
{
  "prompt": "Previous analysis: ${llm_1.result}\nNow analyze..."
}
```

**Behavior**:
- Resolved by LLM node before tool execution
- If variable not found → keeps original template (doesn't fail)
- Supports arbitrary nesting depth: `${upstream.deeply.nested.value}`
- Can be hashed by SecureValueService if source node has `secure: true`

---

## Resolution Flow

### 1. HTTP Node Execution
```
Config: {"body": "client_id=${AMADEUS_CLIENT_ID}"}
                              ↓
                    resolve_env_vars()
                              ↓
        Looks up: process.env["AMADEUS_CLIENT_ID"]
                              ↓
Result: {"body": "client_id=ABC123"}
```

### 2. LLM Node Tool Configuration
```
Tool Config: {
  "fixed": "${get_token.body.access_token}",
  "comment": "Bearer token from OAuth node"
}
                              ↓
        LLM node resolves at initialization:
        resolve_context_in_node_schema()
                              ↓
   Looks up: inputs["get_token.body.access_token"]
             (flattened from edge connections)
                              ↓
Result: {
  "fixed": "<value_6>"  (if source was secure:true)
  OR
  "fixed": "actual_token_value"  (if source was not secure)
}
```

### 3. Edge-Driven Variable Passing
```
Edge: get_token.body.access_token → llm_agent.context.auth_token

Step 1: get_token node executes
        Output: {"body": {"access_token": "token123"}}

Step 2: Edge extracts path: "token123"

Step 3: LLM node receives flattened input
        inputs["context.auth_token"] = "token123" OR "<value_N>" if secure

Step 4: Tool config resolves
        "${context.auth_token}" → looks up in inputs
        → finds "context.auth_token" key
        → replaces with value
```

---

## Key Rules

### Rule 1: Case Indicates Source
```
${UPPERCASE}     → Environment variable (system)
${lowercase}     → Node output reference (DAG)
```

### Rule 2: Dots Indicate Path
```
${simple}                    → Top-level input
${node_name.field}          → Nested field
${node_name.deep.nested}    → Arbitrary depth
```

### Rule 3: Precedence
1. Environment variables (resolved first, system-level)
2. Node outputs (resolved second, execution-level)
3. If both exist with same name (shouldn't happen) → Environment wins

### Rule 4: LLM Visibility
```
${AMADEUS_API_KEY}              → NOT visible to LLM (env var)
${get_token.body.access_token}  → MAY be visible to LLM as context
                                  (unless source has secure:true)
```

### Rule 5: Security
```
If source node has secure: true:
  ✅ Real value encrypted in DB
  ✅ LLM receives hash: <value_N>
  ✅ Non-LLM nodes get real value auto-injected

If source node has secure: false:
  ✅ Real value passed as-is
  ⚠️  Visible to LLM if referenced in context
```

---

## Examples

### Example 1: OAuth2 Flow with Secure Values

```json
{
  "nodes": {
    "get_amadeus_token": {
      "type": "http_request",
      "config": {
        "endpoint": "/oauth2/token",
        "body": "client_id=${AMADEUS_CLIENT_ID}&secret=${AMADEUS_SECRET}",
        "secure": true
      }
    },
    "travel_agent": {
      "type": "llm_call",
      "config": {
        "prompt": "Help user find flights",
        "tool_configurations": {
          "search_flights": {
            "node_schema": {
              "bearer_token": {
                "type": "string",
                "fixed": "${get_amadeus_token.body.access_token}"
              }
            }
          }
        }
      }
    }
  },
  "edges": [
    {
      "from": "get_amadeus_token.body.access_token",
      "to": "travel_agent.context.auth_token",
      "comment": "Token hashed due to secure:true on source"
    }
  ]
}
```

**Resolution**:
1. ✅ HTTP node: `${AMADEUS_CLIENT_ID}` → resolves from process.env
2. ✅ Secure service: hashes response → `<value_1>`
3. ✅ Edge passes: hash to `travel_agent.context.auth_token`
4. ✅ LLM node: resolves `${get_amadeus_token.body.access_token}` → `<value_1>`
5. ✅ Tool gets: `bearer_token: "<value_1>"`
6. ✅ HTTP tool: SecureValueService auto-injects real token before request

---

### Example 2: Multi-LLM Pipeline

```json
{
  "nodes": {
    "research_llm": {
      "type": "llm_call",
      "config": {
        "prompt": "Research this topic: ${trigger.topic}",
        "model": "gpt-4"
      }
    },
    "analysis_llm": {
      "type": "llm_call",
      "config": {
        "prompt": "Analyze this research:\n${research_llm.result}\n\nFocus on..."
      }
    },
    "summary_llm": {
      "type": "llm_call",
      "config": {
        "prompt": "Summarize the analysis:\n${analysis_llm.result}"
      }
    }
  },
  "edges": [
    {"from": "trigger.topic", "to": "research_llm.prompt"},
    {"from": "research_llm.result", "to": "analysis_llm.analysis_input"},
    {"from": "analysis_llm.result", "to": "summary_llm.analysis"}
  ]
}
```

**Resolution**:
1. ✅ Trigger: `${trigger.topic}` → resolves from trigger payload
2. ✅ Research LLM: gets topic, produces result
3. ✅ Analysis LLM: `${research_llm.result}` → looks up in inputs → gets research output
4. ✅ Summary LLM: `${analysis_llm.result}` → looks up in inputs → gets analysis output

**Clarity**: No ambiguity - each `${node_name.field}` explicitly shows source node

---

### Example 3: Avoiding Confusion

```json
// ❌ CONFUSING (before)
{
  "fixed": "${context.token}"
  // Is this: context from LLM? Or from another node? Or env var?
}

// ✅ CLEAR (after)
{
  "fixed": "${auth_response.body.token}"
  // Obviously from auth_response node, body field, token subfield
}
```

---

## Implementation Notes

### For Variable Resolution Code

```rust
pub fn resolve_variables(template: &str, inputs: &HashMap<String, Value>) -> String {
    let env_pattern = Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}").unwrap();
    let node_pattern = Regex::new(r"\$\{([a-z_][a-z0-9_]*(\.([a-z_][a-z0-9_]*))*)}\}").unwrap();
    
    // First: resolve environment variables
    let after_env = env_pattern.replace_all(template, |caps: &Captures| {
        let var_name = &caps[1];
        std::env::var(var_name).unwrap_or_else(|_| {
            panic!("Environment variable {} not found", var_name)
        })
    });
    
    // Second: resolve node references
    let after_nodes = node_pattern.replace_all(&after_env, |caps: &Captures| {
        let path = &caps[1];
        inputs.get(path)
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("${{{}}}", path)) // Keep if not found
    });
    
    after_nodes.to_string()
}
```

### Resolution Order
1. Environment variables (system-level, fail fast if missing)
2. Node references (dag-level, graceful if missing)
3. This order ensures env vars can't be spoofed by node names

---

## Migration Guide

### From Old Syntax to New

#### Old (Ambiguous)
```json
"fixed": "${context.amadeus_token}"
```

#### New (Clear)
```json
"fixed": "${get_amadeus_token.body.access_token}"
```

### Backward Compatibility
- Both syntaxes supported during transition
- Gradual migration path
- Documentation guides existing users

---

## Testing Strategy

### Test Cases

1. **Environment variables**
   - ✅ Resolution succeeds
   - ✅ Missing var fails with clear error
   - ✅ Multiple vars in same template
   - ✅ Nested in different parts (headers, body, endpoint)

2. **Node references**
   - ✅ Simple top-level: `${node.field}`
   - ✅ Nested: `${node.deep.nested.path}`
   - ✅ Multiple nodes: `${node1.x}` and `${node2.y}`
   - ✅ Missing ref doesn't fail (graceful)

3. **Secure values**
   - ✅ Env vars never hashed
   - ✅ Node refs hashed if source is secure:true
   - ✅ LLM isolation maintained

4. **Multi-node chains**
   - ✅ Node1 → Node2 → Node3 flows
   - ✅ Diamond dependencies (A → B,C → D)
   - ✅ Circular refs detected (invalid DAG)

---

## Security Considerations

### Environment Variables
- ✅ Never passed to LLM
- ✅ Only resolved at execution time
- ✅ Not logged or stored
- ✅ Standard practice (same as shell variables)

### Node References
- ✅ Can be hashed via SecureValueService
- ✅ LLM sees hashes if source is secure:true
- ✅ Non-LLM nodes get real values auto-injected
- ✅ Encryption in database for secure values

### Best Practices
1. Put sensitive values in environment variables: `${API_KEY_PROD}`
2. Use secure:true for sensitive node outputs
3. Don't hardcode secrets in JSON files
4. Use node references for data flow transparency

---

## FAQ

**Q: Can I use both syntaxes in same DAG?**
A: Yes, both `${UPPERCASE}` and `${node.field}` work independently

**Q: What if I typo a node name?**
A: Template is kept as-is (not resolved), likely causing execution error downstream

**Q: Can environment variables have dots?**
A: No - only `[A-Z0-9_]+` allowed. Use underscores: `${MY_VAR_NAME}`

**Q: Can node names have uppercase letters?**
A: Yes, but follow convention: lowercase for clarity. Pattern is `[a-z_][a-z0-9_]*`

**Q: What about nested objects in node output?**
A: Use dots to traverse: `${node.body.deeply.nested.field}`

**Q: Can I use both in same value?**
A: Yes: `"Authorization: Bearer ${OAUTH_PREFIX}-${token_node.value}"`

---

## Status

**Version**: 1.0  
**Date**: 2026-04-05  
**Status**: Final Specification  
**Implementation**: In Progress
