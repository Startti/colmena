# Tool Selection Design - DAG JSON Configuration

## Overview

This document describes how users can configure which tools are available to AI agents in their DAG JSON files using the `enabled_tools` configuration option.

---

## Design Principle

**User Control**: The DAG JSON author explicitly specifies which tools the AI agent can use. This provides:
- **Security**: Limit agent capabilities to only what's needed
- **Predictability**: Know exactly what actions the agent can take
- **Debugging**: Easier to trace which tools were available
- **Flexibility**: Different agents can have different tool sets

---

## Configuration Options

### 1. Specific Tools List

Specify exactly which tools the agent can use:

```json
{
  "agent": {
    "type": "llm_call",
    "config": {
      "provider": "openai",
      "model": "gpt-4",
      "enabled_tools": ["add", "multiply", "divide"],
      "max_iterations": 10
    }
  }
}
```

**Use Case**: Math agent that only needs arithmetic operations.

### 2. Wildcard (All Tools)

Enable all available tools using the wildcard `"*"`:

```json
{
  "agent": {
    "type": "llm_call",
    "config": {
      "provider": "anthropic",
      "model": "claude-3-sonnet-20240229",
      "enabled_tools": ["*"],
      "max_iterations": 15
    }
  }
}
```

**Use Case**: General-purpose agent that needs access to everything.

### 3. No Tools (Standard LLM Call)

Omit `enabled_tools` for normal LLM behavior without tool calling:

```json
{
  "simple_llm": {
    "type": "llm_call",
    "config": {
      "provider": "gemini",
      "model": "gemini-pro",
      "system_message": "You are a helpful assistant."
    }
  }
}
```

**Use Case**: Simple question-answering without actions.

---

## Implementation Flow

```
DAG JSON Config
    │
    ├─ enabled_tools: ["add", "multiply"]
    │       │
    │       ▼
    │   LlmNode.execute()
    │       │
    │       ├─ Parse enabled_tools array
    │       │
    │       ├─ Create DagToolExecutor
    │       │
    │       ├─ Call: tool_executor.get_tools(["add", "multiply"])
    │       │       │
    │       │       ├─ Get "add" node from registry
    │       │       ├─ Convert schema to ToolDefinition
    │       │       ├─ Get "multiply" node from registry
    │       │       └─ Convert schema to ToolDefinition
    │       │
    │       ├─ Create AgentService
    │       │
    │       └─ Call: agent_service.run(tools=[add_def, multiply_def])
    │               │
    │               └─ ReAct Loop with only these tools
    │
    ├─ enabled_tools: ["*"]
    │       │
    │       └─ Call: tool_executor.get_all_available_tools()
    │               └─ Returns all registered tools
    │
    └─ No enabled_tools
            └─ Standard LLM call (no AgentService)
```

---

## Available Tools

### Current Tools (Registered Nodes)

| Tool Name | Description | Example Use Case |
|-----------|-------------|------------------|
| `add` | Add two numbers | Math calculations |
| `subtract` | Subtract two numbers | Math calculations |
| `multiply` | Multiply two numbers | Math calculations |
| `divide` | Divide two numbers | Math calculations |
| `exponential` | Raise to power | Advanced math |
| `http_request` | Make HTTP calls | Fetch external data |
| `log` | Log output | Debugging, output display |

### Future Tools (Can be added)

Any new node registered in the DAG Engine automatically becomes available as a tool:
- `send_email` - Send emails
- `query_database` - Run SQL queries
- `read_file` - Read file contents
- `write_file` - Write to files
- `parse_json` - Parse JSON data
- Custom business logic nodes

---

## Examples

### Example 1: Math Agent (Specific Tools)

**Scenario**: Agent that solves math problems step-by-step

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/math",
        "method": "POST",
        "test_payload": {
          "question": "What is (15 + 27) * 3?"
        }
      }
    },
    "math_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4",
        "system_message": "Solve math problems step by step using the available tools.",
        "enabled_tools": ["add", "subtract", "multiply", "divide"],
        "max_iterations": 10,
        "thread_id": "math-session-1",
        "connection_url": "${DATABASE_URL}"
      }
    },
    "output": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "trigger.output.question",
      "to": "math_agent.prompt"
    },
    {
      "from": "math_agent.output.content",
      "to": "output.input"
    }
  ]
}
```

**Agent Behavior**:
1. Receives: "What is (15 + 27) * 3?"
2. Uses `add` tool: add(15, 27) → 42
3. Uses `multiply` tool: multiply(42, 3) → 126
4. Returns: "The answer is 126"

### Example 2: Research Agent (HTTP Only)

**Scenario**: Agent that fetches data from APIs

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/research",
        "method": "POST",
        "test_payload": {
          "topic": "weather in London"
        }
      }
    },
    "research_agent": {
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "api_key": "${ANTHROPIC_API_KEY}",
        "model": "claude-3-sonnet-20240229",
        "system_message": "Fetch information using HTTP requests and summarize.",
        "enabled_tools": ["http_request"],
        "max_iterations": 5
      }
    },
    "output": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "trigger.output.topic",
      "to": "research_agent.prompt"
    },
    {
      "from": "research_agent.output",
      "to": "output.input"
    }
  ]
}
```

**Agent Behavior**:
1. Receives: "weather in London"
2. Uses `http_request`: GET https://api.weather.com/london
3. Parses response
4. Returns: "The weather in London is sunny, 18°C"

### Example 3: General Agent (All Tools)

**Scenario**: Multi-purpose agent with full capabilities

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/general",
        "method": "POST",
        "test_payload": {
          "task": "Calculate 10 + 5, then fetch data from https://api.example.com"
        }
      }
    },
    "general_agent": {
      "type": "llm_call",
      "config": {
        "provider": "gemini",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-1.5-pro",
        "system_message": "You are a general assistant. Use any available tools.",
        "enabled_tools": ["*"],
        "max_iterations": 15,
        "thread_id": "general-session",
        "connection_url": "${DATABASE_URL}"
      }
    },
    "output": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "trigger.output.task",
      "to": "general_agent.prompt"
    },
    {
      "from": "general_agent.output",
      "to": "output.input"
    }
  ]
}
```

**Agent Behavior**:
1. Receives: "Calculate 10 + 5, then fetch data from https://api.example.com"
2. Uses `add`: add(10, 5) → 15
3. Uses `http_request`: GET https://api.example.com
4. Returns combined results

---

## Validation & Error Handling

### Valid Configuration
```json
"enabled_tools": ["add", "multiply"]  // ✅ Specific tools
"enabled_tools": ["*"]                 // ✅ All tools
// No enabled_tools field              // ✅ Standard LLM call
```

### Invalid Configuration
```json
"enabled_tools": "add"                // ❌ Must be array
"enabled_tools": []                   // ❌ Empty array (use no field instead)
"enabled_tools": ["nonexistent"]      // ❌ Tool doesn't exist
```

### Error Messages

**Tool Not Found**:
```
Error: Tool 'nonexistent_tool' not found in registry.
Available tools: add, subtract, multiply, divide, exponential, http_request, log
```

**Invalid Configuration**:
```
Error: 'enabled_tools' must be an array of strings or ["*"]
```

---

## Security Considerations

### Principle of Least Privilege

Only enable tools that are necessary for the task:

```json
// ❌ BAD: Overly permissive
{
  "enabled_tools": ["*"]
}

// ✅ GOOD: Only what's needed
{
  "enabled_tools": ["add", "multiply"]
}
```

### Dangerous Tools

Some tools may have side effects. Be cautious:

```json
// Potentially dangerous - only enable if needed
{
  "enabled_tools": [
    "send_email",        // Can spam
    "write_file",        // Can modify filesystem
    "query_database"     // Can modify data
  ]
}
```

### Sandboxing

Future enhancement: Tool permissions

```json
{
  "enabled_tools": ["http_request"],
  "tool_permissions": {
    "http_request": {
      "allowed_domains": ["api.example.com"],
      "max_requests": 5
    }
  }
}
```

---

## Testing Strategy

### Test Cases

1. **Specific Tools**
```bash
# Config: enabled_tools: ["add", "multiply"]
# Expected: Agent can only use add and multiply
# Verify: LLM doesn't attempt to use other tools
```

2. **Wildcard**
```bash
# Config: enabled_tools: ["*"]
# Expected: Agent has access to all registered tools
# Verify: All tools appear in LLM request
```

3. **No Tools**
```bash
# Config: (no enabled_tools field)
# Expected: Standard LLM call, no tool calling
# Verify: AgentService not instantiated
```

4. **Invalid Tool**
```bash
# Config: enabled_tools: ["fake_tool"]
# Expected: Clear error message
# Verify: Lists available tools
```

5. **Mixed Valid/Invalid**
```bash
# Config: enabled_tools: ["add", "fake_tool", "multiply"]
# Expected: Error or filter to valid only (TBD)
# Verify: Consistent behavior
```

---

## Implementation Checklist

### Phase 1: Core Implementation
- [ ] Parse `enabled_tools` array from config
- [ ] Implement wildcard `"*"` support
- [ ] Create `DagToolExecutor.get_tools(names)`
- [ ] Create `DagToolExecutor.get_all_available_tools()`
- [ ] Pass filtered tools to `AgentService`

### Phase 2: Validation
- [ ] Validate tool names exist in registry
- [ ] Clear error messages for invalid tools
- [ ] Handle empty array
- [ ] Handle non-array values

### Phase 3: Testing
- [ ] Unit tests for each configuration type
- [ ] Integration tests with real agents
- [ ] Error handling tests
- [ ] Documentation examples

### Phase 4: Documentation
- [ ] Update DAG Engine guide
- [ ] Create usage examples
- [ ] Document all available tools
- [ ] Security best practices

---

## Future Enhancements

### 1. Tool Aliases
```json
{
  "enabled_tools": ["math.*", "http.*"]
  // Enables all math tools and HTTP tools
}
```

### 2. Tool Permissions
```json
{
  "enabled_tools": ["http_request"],
  "tool_config": {
    "http_request": {
      "max_retries": 3,
      "timeout": 5000,
      "allowed_methods": ["GET"]
    }
  }
}
```

### 3. Dynamic Tool Discovery
```json
{
  "enabled_tools": ["registry:*"]
  // Auto-discover all tools from registry
}
```

### 4. Tool Categories
```json
{
  "enabled_tools": ["@math", "@network"]
  // @math = add, subtract, multiply, divide, exponential
  // @network = http_request, websocket, etc.
}
```

---

## Summary

**Key Points**:
1. Users specify tools via `enabled_tools` array in DAG JSON
2. Supports specific lists, wildcard `"*"`, or no tools
3. Provides security, predictability, and debugging benefits
4. Tools are filtered at LlmNode level before passing to AgentService
5. Clear error messages for invalid configurations

**Example Configuration**:
```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "model": "gpt-4",
    "enabled_tools": ["add", "multiply", "http_request"],
    "max_iterations": 10
  }
}
```

This design gives users full control over agent capabilities while maintaining simplicity and security.
