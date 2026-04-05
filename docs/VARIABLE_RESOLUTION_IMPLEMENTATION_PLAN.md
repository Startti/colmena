# Variable Resolution Implementation Plan

## Overview

Implement the new variable resolution specification with full support for:
- Environment variables: `${UPPERCASE_VAR}`
- Node references: `${node_name.field.path}`

## Phase 1: Specification & Documentation (DONE ✅)

- ✅ Variable Resolution Spec created
- ✅ Examples documented
- ✅ Migration guide included

**Status**: Ready for implementation

---

## Phase 2: Code Implementation (TODO)

### 2.1 Create Unified Variable Resolver

**File**: `src/libs/colmena/src/dag_engine/infrastructure/variable_resolver.rs` (NEW)

```rust
/// Unified variable resolver supporting:
/// - ${UPPERCASE_VAR} → environment variables
/// - ${node_name.field.path} → node output references
pub struct VariableResolver;

impl VariableResolver {
    /// Resolve all variables in a string template
    pub fn resolve(template: &str, inputs: &HashMap<String, Value>) -> Result<String, ResolutionError> {
        // 1. Resolve ${UPPERCASE} → process.env
        // 2. Resolve ${lowercase.path} → inputs HashMap
        // 3. Return resolved string or error
    }
    
    /// Recursively resolve variables in a Value (for node_schema)
    pub fn resolve_value(value: &Value, inputs: &HashMap<String, Value>) -> Result<Value, ResolutionError> {
        // Handles: strings, objects, arrays
    }
}

pub enum ResolutionError {
    EnvironmentVariableNotFound(String),
    InvalidTemplate(String),
    // ...
}
```

**Tests**:
- ✅ Simple env var: `${API_KEY}` → "secret123"
- ✅ Missing env var: `${MISSING}` → Error
- ✅ Simple node ref: `${node.field}` → value
- ✅ Deep path: `${node.body.data.id}` → value
- ✅ Missing node ref: `${missing.field}` → unchanged
- ✅ Mixed: `"${API_KEY}-${node.id}"` → "secret-123"

**Effort**: ~3 hours

---

### 2.2 Update HTTP Node to Use Unified Resolver

**File**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

**Changes**:
```rust
// BEFORE: Only resolved env vars
let base_url = Self::resolve_env_vars(raw_url)?;

// AFTER: Use unified resolver
let base_url = VariableResolver::resolve(raw_url, &inputs)?;
```

**Impact**:
- ✅ Can now reference node outputs in HTTP config
- ✅ Can use environment variables as before
- ✅ Both work together seamlessly

**Effort**: ~1 hour

---

### 2.3 Update LLM Node to Use Unified Resolver

**File**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

**Changes**:
```rust
// BEFORE: Special resolve_context_vars for only ${context.*}
Self::resolve_context_in_node_schema(node_schema, inputs);

// AFTER: Use unified resolver
for field in node_schema.values_mut() {
    if let Some(fixed) = field.fixed.as_mut() {
        if let Value::String(s) = fixed {
            *s = VariableResolver::resolve(s, inputs)?;
        }
    }
}
```

**Impact**:
- ✅ Can now reference ANY node output
- ✅ Supports environment variables in tool configs
- ✅ Clearer semantics with new syntax

**Effort**: ~1 hour

---

### 2.4 Add Pattern Validation & Error Messages

**File**: `src/libs/colmena/src/dag_engine/infrastructure/variable_resolver.rs`

**Features**:
```rust
pub fn validate_variable_name(name: &str) -> Result<VariableType, ValidationError> {
    match name {
        // Pattern: [A-Z_][A-Z0-9_]* → environment variable
        env if env.chars().all(|c| c.is_uppercase() || c == '_') => Ok(VariableType::Environment),
        
        // Pattern: [a-z_][a-z0-9_]*(\.[a-z_][a-z0-9_]*)* → node reference
        node if Self::is_valid_node_ref(node) => Ok(VariableType::NodeReference),
        
        _ => Err(ValidationError::InvalidPattern(name.to_string())),
    }
}

pub enum VariableType {
    Environment,
    NodeReference,
}
```

**Error Messages**:
```
❌ Template "${missing_env_var}" references environment variable that doesn't exist
   Expected: Set DATABASE_PASSWORD in your environment
   
❌ Template "${upstream_node.field}" references undefined node or field
   Found nodes: [node1, node2] but not upstream_node
   Available in node1: [status, body, headers]
```

**Effort**: ~2 hours

---

## Phase 3: Test Graphs Refactoring (TODO)

### 3.1 Update Amadeus Test Graph

**File**: `tests/graphs/security/amadeus_secure_gemini_agent_test.json`

**Changes**:
```json
// BEFORE
"fixed": "${context.amadeus_token}"

// AFTER
"fixed": "${get_amadeus_token.body.access_token}"
```

**Why**: Explicit node reference, no ambiguity

**Effort**: ~30 min

---

### 3.2 Update Other Test Graphs

**Files**:
- `tests/graphs/agents/*.json`
- `tests/graphs/advanced/*.json`
- Any graph using `${context.*}` references

**Process**:
1. Find all `${context.` patterns
2. Trace back to find source node
3. Replace with `${source_node.field.path}`

**Effort**: ~2 hours (across all graphs)

---

### 3.3 Create New Examples

**File**: `tests/graphs/examples/multi_llm_pipeline.json`

```json
{
  "comment": "Multi-LLM pipeline demonstrating variable flow",
  "nodes": {
    "research_agent": {
      "type": "llm_call",
      "config": {
        "prompt": "Research: ${trigger.topic}"
      }
    },
    "analysis_agent": {
      "type": "llm_call",
      "config": {
        "prompt": "Analyze this research:\n${research_agent.result}"
      }
    },
    "summary_agent": {
      "type": "llm_call",
      "config": {
        "prompt": "Summarize:\n${analysis_agent.result}"
      }
    }
  }
}
```

**Demonstrates**:
- ✅ Clear variable flow between LLMs
- ✅ No ambiguity about data sources
- ✅ Easy to trace DAG logic

**Effort**: ~1 hour

---

## Phase 4: Documentation Updates (TODO)

### 4.1 Update Main Docs

**Files to update**:
- `docs/LLM_NODE_COMPLETE_GUIDE.md` - Add variable resolution section
- `docs/HTTP_NODE_GUIDE.md` - Add variable examples
- `CLAUDE.md` - Update conventions

**Changes**:
- Add Variable Resolution Spec reference
- Update all examples to use new syntax
- Document migration path

**Effort**: ~2 hours

---

### 4.2 Create Migration Guide

**File**: `docs/MIGRATION_GUIDE_VARIABLE_RESOLUTION.md`

**Content**:
- What changed and why
- How to update existing graphs
- When to use each syntax
- Common mistakes to avoid

**Effort**: ~1 hour

---

## Phase 5: Backward Compatibility (TODO)

### 5.1 Support Both Syntaxes

**Duration**: Transition period (next 3-4 releases)

```rust
// Support both during transition
match template {
    "${context." => {
        // OLD: Map context.X to flattened key "context.X"
        warn!("Using old syntax ${context.X}. Use ${node_name.field} instead");
    },
    "${node" => {
        // NEW: Direct node reference
        // No warning
    },
    // ...
}
```

**Deprecation Plan**:
1. Release 0.3.1: Support both, warn on old syntax
2. Release 0.4.0: Support both, strong deprecation notice
3. Release 0.5.0: Only support new syntax

**Effort**: ~1 hour

---

## Phase 6: Testing & Validation (TODO)

### 6.1 Unit Tests

**File**: `src/libs/colmena/src/dag_engine/infrastructure/variable_resolver.rs`

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_resolve_env_var_success() { }
    
    #[test]
    fn test_resolve_env_var_missing() { }
    
    #[test]
    fn test_resolve_node_ref_simple() { }
    
    #[test]
    fn test_resolve_node_ref_nested() { }
    
    #[test]
    fn test_resolve_node_ref_missing() { }
    
    #[test]
    fn test_resolve_mixed() { }
    
    #[test]
    fn test_validate_env_pattern() { }
    
    #[test]
    fn test_validate_node_pattern() { }
    
    #[test]
    fn test_error_messages_clear() { }
}
```

**Coverage**: 100% of resolver logic

**Effort**: ~3 hours

---

### 6.2 Integration Tests

**File**: `tests/variable_resolution_integration.rs`

```rust
#[tokio::test]
async fn test_http_node_with_env_and_node_refs() { }

#[tokio::test]
async fn test_llm_tool_config_with_node_refs() { }

#[tokio::test]
async fn test_multi_llm_pipeline_variable_flow() { }

#[tokio::test]
async fn test_secure_values_with_new_syntax() { }

#[tokio::test]
async fn test_backward_compat_context_syntax() { }
```

**Effort**: ~4 hours

---

### 6.3 Test All Existing Graphs

Run full test suite:
```bash
cargo test

# All graphs should pass with updated variable syntax
```

**Effort**: ~1 hour

---

## Implementation Timeline

```
Phase 1 (Spec):          DONE ✅
Phase 2 (Code):          ~7 hours
Phase 3 (Test Graphs):   ~3.5 hours
Phase 4 (Docs):          ~3 hours
Phase 5 (Compat):        ~1 hour
Phase 6 (Testing):       ~8 hours

TOTAL: ~21.5 hours
```

**Estimated delivery**: 2-3 days (2-3 person-days)

---

## Rollout Plan

### Week 1: Development
- Phase 2: Implement unified resolver
- Phase 6.1: Unit tests
- Merge to develop branch

### Week 2: Integration
- Phase 3: Refactor test graphs
- Phase 6.2: Integration tests
- Phase 5: Add backward compatibility layer

### Week 3: Documentation
- Phase 4: Update all documentation
- Phase 6.3: Full test suite validation
- Create release notes

### Week 4: Release
- Release 0.3.1 (with deprecation warnings)
- Document migration path
- Gradual rollout to users

---

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Breaking change | Users can't upgrade | Backward compat layer (Phase 5) |
| Parsing complexity | Bugs in resolver | Thorough unit tests (Phase 6.1) |
| Performance impact | Slower execution | Benchmark resolver, optimize if needed |
| Documentation lag | Users confused | Complete docs before release (Phase 4) |

---

## Success Criteria

- ✅ Unified resolver handles both syntaxes
- ✅ All existing graphs work (backward compatible)
- ✅ New syntax is clearer and less ambiguous
- ✅ Documentation is complete and clear
- ✅ 100% test coverage of resolver
- ✅ Zero breaking changes (with compat layer)
- ✅ Users can migrate at their own pace

---

## Next Steps

1. **Approve plan** - Is this the direction you want?
2. **Start Phase 2** - Implement unified resolver
3. **Parallel**: Create unit tests as we go
4. **Integration**: Test against existing graphs
5. **Documentation**: Keep docs updated throughout

Ready to start Phase 2?
