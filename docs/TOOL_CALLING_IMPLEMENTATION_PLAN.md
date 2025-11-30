# Tool Calling (Function Calling) Implementation Plan

## Executive Summary

This document outlines the complete implementation plan for adding tool calling capabilities to the Colmena DAG Engine, enabling LLM nodes to autonomously execute other DAG nodes as tools in a ReAct (Reasoning + Acting) pattern.

**Key Innovation**: Any registered DAG node automatically becomes a tool available to AI agents, enabling composition and reusability.

---

## Current Architecture Analysis

### ✅ What We Have
- **Hexagonal Architecture**: Clear separation between Domain, Application, and Infrastructure layers
- **LLM Module**: Working with OpenAI, Gemini, and Anthropic adapters
- **Memory System**: Persistent conversation history with SQLite and PostgreSQL
- **DAG Engine**: Node registry system with ExecutableNode trait
- **Working Nodes**: Mock, Log, Math (Add, Subtract, Multiply, Divide, Exponential), HTTP, Trigger, and LLM nodes

### 🎯 What We Need
- **Tool Definitions**: Domain models for representing tools/functions
- **Tool Call Handling**: Support in LlmRequest/LlmResponse for tools and tool_calls
- **Provider Adapters**: Update OpenAI, Anthropic, and Gemini adapters for function calling
- **ReAct Loop**: Agent service to orchestrate reasoning and tool execution
- **Tool Executor**: Bridge between LLM module and DAG Engine nodes

---

## Phase 1: Planning & Research (Days 1-2)

### 1.1 Research Provider APIs ✓ (Ready to Start)

**Tasks:**
- [ ] Review OpenAI Function Calling API documentation
  - Understand `tools` parameter format (JSON Schema)
  - Understand `tool_calls` response format
  - Study `tool_choice` parameter options
- [ ] Review Anthropic Tool Use API documentation
  - Understand Claude's tool/function calling format
  - Study differences from OpenAI format
- [ ] Review Gemini Function Calling API documentation
  - Understand Google's function declaration format
  - Study response structure for tool calls
- [ ] Document format differences and create compatibility matrix

**Deliverable**: `docs/research/PROVIDER_TOOL_FORMATS.md` with comparison table

### 1.2 Design Domain Model

**Tasks:**
- [ ] Design `ToolDefinition` struct (JSON Schema based)
- [ ] Design `ToolCall` struct (function name + arguments)
- [ ] Design `ToolResult` struct (success/error + output)
- [ ] Design `ToolExecutor` trait abstraction
- [ ] Review with existing `ExecutableNode` trait for compatibility

**Deliverable**: Updated UML diagrams in this document

---

## Phase 2: Domain Layer - Tool Abstractions (Days 3-5)

### 2.1 Create Tool Domain Models

**Location**: `src/llm/domain/tools.rs`

**Structs to Implement**:

```rust
/// Represents a tool/function definition that can be passed to an LLM
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    /// The name of the tool (e.g., "add", "http_request")
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: String,

    /// JSON Schema for the tool's parameters
    pub parameters: ToolParameters,
}

/// JSON Schema definition for tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolParameters {
    /// Always "object" for function parameters
    #[serde(rename = "type")]
    pub schema_type: String,

    /// Properties/fields of the parameters
    pub properties: HashMap<String, ParameterProperty>,

    /// List of required parameter names
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

/// Definition of a single parameter property
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParameterProperty {
    #[serde(rename = "type")]
    pub property_type: String,

    pub description: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

/// Represents a tool call requested by the LLM
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Unique identifier for this tool call (provider-generated)
    pub id: String,

    /// The type (usually "function")
    #[serde(rename = "type")]
    pub call_type: String,

    /// The function being called
    pub function: FunctionCall,
}

/// The actual function call details
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    /// Name of the function to call
    pub name: String,

    /// JSON string of arguments
    pub arguments: String,
}

/// Result of executing a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool call ID this result corresponds to
    pub tool_call_id: String,

    /// Whether execution succeeded
    pub success: bool,

    /// The output/result as JSON string
    pub output: String,

    /// Error message if success = false
    pub error: Option<String>,
}
```

**Tasks**:
- [ ] Create `src/llm/domain/tools.rs`
- [ ] Implement all structs above with proper derives
- [ ] Add builder methods for ergonomic construction
- [ ] Add validation methods
- [ ] Write comprehensive unit tests
- [ ] Add to `src/llm/domain/mod.rs` exports

### 2.2 Update LlmRequest and LlmResponse

**Location**:
- `src/llm/domain/llm_request.rs`
- `src/llm/domain/llm_response.rs`

**Changes to LlmRequest**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    id: LlmRequestId,
    messages: Vec<LlmMessage>,
    config: LlmConfig,
    stream: bool,

    // NEW: Optional tools available for this request
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,

    // NEW: Control how the model uses tools ("auto", "none", specific function)
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

impl LlmRequest {
    // NEW methods
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_tool_choice(mut self, choice: String) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    pub fn tools(&self) -> Option<&[ToolDefinition]> {
        self.tools.as_deref()
    }

    pub fn tool_choice(&self) -> Option<&str> {
        self.tool_choice.as_deref()
    }
}
```

**Changes to LlmResponse**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    id: LlmResponseId,
    request_id: LlmRequestId,
    message: LlmMessage,
    usage: Option<LlmUsage>,
    provider: LlmProvider,
    timestamp: DateTime<Utc>,
    finish_reason: Option<String>,

    // NEW: Tool calls requested by the LLM
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
}

impl LlmResponse {
    // NEW methods
    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = Some(tool_calls);
        self
    }

    pub fn tool_calls(&self) -> Option<&[ToolCall]> {
        self.tool_calls.as_deref()
    }

    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
    }
}
```

**Tasks**:
- [ ] Update `LlmRequest` struct and implementation
- [ ] Update `LlmResponse` struct and implementation
- [ ] Update existing tests to pass with new fields
- [ ] Add new tests for tool-related functionality
- [ ] Update serialization/deserialization tests

### 2.3 Create ToolExecutor Trait

**Location**: `src/llm/domain/tool_executor.rs`

```rust
use async_trait::async_trait;
use super::{ToolCall, ToolResult, LlmError};

/// Trait for executing tools requested by LLMs
///
/// This abstraction allows the LLM module to request tool execution
/// without knowing the implementation details (e.g., DAG nodes).
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool call and return the result
    ///
    /// # Arguments
    /// * `tool_call` - The tool call to execute
    ///
    /// # Returns
    /// * `Ok(ToolResult)` - Successful execution with output
    /// * `Err(LlmError)` - Execution failed
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError>;

    /// Get list of available tools
    ///
    /// # Returns
    /// Vector of tool definitions that can be passed to LLM
    async fn available_tools(&self) -> Vec<ToolDefinition>;
}
```

**Tasks**:
- [ ] Create `src/llm/domain/tool_executor.rs`
- [ ] Define `ToolExecutor` trait
- [ ] Add trait to `src/llm/domain/mod.rs` exports
- [ ] Add documentation with examples

---

## Phase 3: Infrastructure Layer - Provider Adapters (Days 6-10)

### 3.1 Update OpenAI Adapter

**Location**: `src/llm/infrastructure/openai_adapter.rs`

**Tasks**:
- [ ] Update `build_request_body()` to include tools if present
  ```rust
  if let Some(tools) = request.tools() {
      body["tools"] = json!(tools.iter().map(|t| json!({
          "type": "function",
          "function": {
              "name": t.name,
              "description": t.description,
              "parameters": t.parameters
          }
      })).collect::<Vec<_>>());
  }

  if let Some(choice) = request.tool_choice() {
      body["tool_choice"] = json!(choice);
  }
  ```
- [ ] Update response parsing to extract `tool_calls`
  ```rust
  let tool_calls = response_json["choices"][0]["message"]["tool_calls"]
      .as_array()
      .map(|arr| {
          arr.iter()
              .filter_map(|tc| serde_json::from_value(tc.clone()).ok())
              .collect()
      });
  ```
- [ ] Handle message role "tool" for tool results
- [ ] Update tests with OpenAI tool calling examples
- [ ] Test with real OpenAI API (gpt-4, gpt-3.5-turbo)

### 3.2 Update Anthropic Adapter

**Location**: `src/llm/infrastructure/anthropic_adapter.rs`

**Tasks**:
- [ ] Study Anthropic's tool format (different from OpenAI)
- [ ] Convert `ToolDefinition` to Anthropic format
  ```rust
  fn convert_tools_to_anthropic(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
      tools.iter().map(|t| json!({
          "name": t.name,
          "description": t.description,
          "input_schema": t.parameters
      })).collect()
  }
  ```
- [ ] Update `build_request_body()` to include tools
- [ ] Parse Anthropic tool use blocks from response
- [ ] Handle `tool_use` content blocks
- [ ] Update tests with Claude examples
- [ ] Test with Claude API (claude-3-opus, claude-3-sonnet)

### 3.3 Update Gemini Adapter

**Location**: `src/llm/infrastructure/gemini_adapter.rs`

**Tasks**:
- [ ] Study Gemini's function calling format
- [ ] Convert `ToolDefinition` to Gemini format
  ```rust
  fn convert_tools_to_gemini(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
      tools.iter().map(|t| json!({
          "function_declarations": [{
              "name": t.name,
              "description": t.description,
              "parameters": t.parameters
          }]
      })).collect()
  }
  ```
- [ ] Update request building
- [ ] Parse function call responses
- [ ] Handle Gemini-specific response structure
- [ ] Update tests with Gemini examples
- [ ] Test with Gemini API (gemini-pro, gemini-1.5-pro)

### 3.4 Update Mock Adapter (for Testing)

**Location**: `src/llm/infrastructure/mock_adapter.rs`

**Tasks**:
- [ ] Add tool call simulation
- [ ] Return predefined tool calls for testing
- [ ] Support configurable mock behaviors
- [ ] Add tests for tool calling scenarios

---

## Phase 4: Application Layer - Agent Service (Days 11-14)

### 4.1 Create Agent Service

**Location**: `src/llm/application/agent_service.rs`

```rust
use crate::llm::domain::{
    LlmRepository, ConversationRepository, ThreadId,
    LlmRequest, LlmResponse, LlmMessage, LlmConfig, LlmError,
    ToolExecutor, ToolDefinition, ToolCall, ToolResult,
    MessageRole,
};
use std::sync::Arc;
use async_trait::async_trait;

/// Agent service implementing the ReAct (Reasoning + Acting) pattern
///
/// This service orchestrates the LLM reasoning loop:
/// 1. LLM thinks and may request tool execution
/// 2. Tools are executed via ToolExecutor
/// 3. Results are fed back to LLM
/// 4. Loop continues until LLM provides final answer
pub struct AgentService {
    llm_repository: Arc<dyn LlmRepository>,
    conversation_repository: Arc<dyn ConversationRepository>,
}

impl AgentService {
    pub fn new(
        llm_repository: Arc<dyn LlmRepository>,
        conversation_repository: Arc<dyn ConversationRepository>,
    ) -> Self {
        Self {
            llm_repository,
            conversation_repository,
        }
    }

    /// Run the agent with tool execution capabilities
    ///
    /// # Arguments
    /// * `thread_id` - Conversation thread for memory
    /// * `prompt` - User's prompt/request
    /// * `config` - LLM configuration
    /// * `tools` - List of tools available to the agent (from enabled_tools config)
    /// * `tool_executor` - Implementation that executes tools
    /// * `max_iterations` - Safety limit for ReAct loop (default: 10)
    ///
    /// # Returns
    /// Final response from the LLM after tool execution
    pub async fn run(
        &self,
        thread_id: &ThreadId,
        prompt: String,
        config: LlmConfig,
        tools: Vec<ToolDefinition>,
        tool_executor: &dyn ToolExecutor,
        max_iterations: Option<usize>,
    ) -> Result<LlmResponse, LlmError> {
        let max_iter = max_iterations.unwrap_or(10);

        // 1. Load conversation history
        let conversation = self.conversation_repository.get_by_id(thread_id).await?;
        let mut messages = conversation.messages;

        // 2. Add user prompt
        let user_message = LlmMessage::user(prompt)?;
        messages.push(user_message.clone());
        self.conversation_repository.add_message(thread_id, user_message).await?;

        // 3. Tools are passed in from LlmNode (based on enabled_tools config)
        // No need to call tool_executor.available_tools()

        // 4. ReAct Loop
        for iteration in 0..max_iter {
            // A. Call LLM with tools
            let request = LlmRequest::new(messages.clone(), config.clone(), false)?
                .with_tools(tools.clone());

            let response = self.llm_repository.call(request).await?;

            // B. Save assistant response to memory
            self.conversation_repository.add_message(thread_id, response.message().clone()).await?;
            messages.push(response.message().clone());

            // C. Check if LLM wants to use tools
            if let Some(tool_calls) = response.tool_calls() {
                if tool_calls.is_empty() {
                    // No tool calls, return response
                    return Ok(response);
                }

                // D. Execute each tool call
                for tool_call in tool_calls {
                    let result = tool_executor.execute(tool_call).await?;

                    // E. Create tool result message
                    let tool_message = LlmMessage::tool(
                        result.tool_call_id.clone(),
                        result.output.clone(),
                    )?;

                    // F. Add to conversation
                    messages.push(tool_message.clone());
                    self.conversation_repository.add_message(thread_id, tool_message).await?;
                }

                // Continue loop - LLM will see tool results
                continue;
            } else {
                // No tool calls - final response
                return Ok(response);
            }
        }

        // Safety: Max iterations reached
        Err(LlmError::MaxIterationsReached { max: max_iter })
    }
}
```

**Tasks**:
- [ ] Create `src/llm/application/agent_service.rs`
- [ ] Implement ReAct loop logic
- [ ] Add proper error handling
- [ ] Add logging for debugging
- [ ] Add iteration counter for safety
- [ ] Handle edge cases (empty tool calls, parsing errors)
- [ ] Write unit tests with mock dependencies
- [ ] Add integration tests

### 4.2 Update LlmMessage for Tool Messages

**Location**: `src/llm/domain/llm_message.rs`

**Tasks**:
- [ ] Add `Tool` variant to `MessageRole` enum
- [ ] Add `tool_call_id` field for tool messages
- [ ] Add `LlmMessage::tool()` constructor
- [ ] Update serialization to handle tool messages
- [ ] Add tests for tool message creation

### 4.3 Add New Error Types

**Location**: `src/llm/domain/llm_error.rs`

**Tasks**:
- [ ] Add `ToolExecutionFailed` error variant
- [ ] Add `MaxIterationsReached` error variant
- [ ] Add `InvalidToolCall` error variant
- [ ] Add `ToolNotFound` error variant

---

## Phase 5: DAG Engine Integration (Days 15-18)

### 5.1 Create DagToolExecutor

**Location**: `src/dag_engine/infrastructure/tool_executor.rs`

```rust
use colmena::llm::domain::{
    ToolExecutor, ToolDefinition, ToolCall, ToolResult, ToolParameters,
    ParameterProperty, LlmError,
};
use crate::application::ports::NodeRegistryPort;
use crate::domain::node::NodeInputs;
use serde_json::{json, Value};
use std::sync::Arc;
use async_trait::async_trait;
use std::collections::HashMap;

/// Adapts DAG Engine nodes to be executable as LLM tools
pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
}

impl DagToolExecutor {
    pub fn new(registry: Arc<dyn NodeRegistryPort>) -> Self {
        Self { registry }
    }

    /// Convert a DAG node schema to a ToolDefinition
    fn node_schema_to_tool(node_type: &str, schema: &Value) -> Option<ToolDefinition> {
        // Extract description from schema
        let description = schema.get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("No description available")
            .to_string();

        // Extract inputs schema and convert to parameters
        let inputs = schema.get("inputs")?;
        let properties = Self::extract_properties(inputs);
        let required = Self::extract_required(inputs);

        Some(ToolDefinition {
            name: node_type.to_string(),
            description,
            parameters: ToolParameters {
                schema_type: "object".to_string(),
                properties,
                required,
            },
        })
    }

    fn extract_properties(inputs: &Value) -> HashMap<String, ParameterProperty> {
        let mut properties = HashMap::new();

        if let Some(obj) = inputs.as_object() {
            for (key, value) in obj {
                if let Some(prop) = Self::value_to_property(value) {
                    properties.insert(key.clone(), prop);
                }
            }
        }

        properties
    }

    fn value_to_property(value: &Value) -> Option<ParameterProperty> {
        Some(ParameterProperty {
            property_type: value.get("type")?
                .as_str()?
                .to_string(),
            description: value.get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string(),
            enum_values: value.get("enum")
                .and_then(|e| e.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                }),
        })
    }

    fn extract_required(inputs: &Value) -> Vec<String> {
        inputs.get("required")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait]
impl ToolExecutor for DagToolExecutor {
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
        let function_name = &tool_call.function.name;

        // Get the node from registry
        let node = self.registry.get_node(function_name)
            .ok_or_else(|| LlmError::ToolNotFound {
                name: function_name.clone()
            })?;

        // Parse arguments
        let args: Value = serde_json::from_str(&tool_call.function.arguments)
            .map_err(|e| LlmError::InvalidToolCall {
                reason: format!("Failed to parse arguments: {}", e)
            })?;

        // Convert to NodeInputs
        let inputs = NodeInputs::from_value(&args);

        // Execute the node
        let mut state = json!({});
        match node.execute(&inputs, &json!({}), &mut state).await {
            Ok(output) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: true,
                output: serde_json::to_string(&output).unwrap_or_default(),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }

    async fn available_tools(&self) -> Vec<ToolDefinition> {
        // NOTE: This will be called with a filtered list from LlmNode
        // based on the "enabled_tools" config in the DAG JSON
        // This method returns ALL available tools from registry for now

        // Get all registered nodes and convert to tools
        // In practice, this will be filtered by LlmNode based on config
        vec![] // Implementation will be updated when called with specific tool list
    }

    /// Get specific tools by name (from enabled_tools config)
    pub fn get_tools(&self, tool_names: &[String]) -> Vec<ToolDefinition> {
        tool_names.iter()
            .filter_map(|node_type| {
                self.registry.get_node(node_type)
                    .and_then(|node| {
                        let schema = node.schema();
                        Self::node_schema_to_tool(node_type, &schema)
                    })
            })
            .collect()
    }

    /// Get all available tools from the registry
    /// Used when enabled_tools contains "*"
    pub fn get_all_available_tools(&self) -> Vec<ToolDefinition> {
        // All registered nodes that can be used as tools
        let all_tool_nodes = vec![
            "add", "subtract", "multiply", "divide", "exponential",
            "http_request", "log",
            // Add more as they become available
        ];

        self.get_tools(&all_tool_nodes.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
}
```

**Tasks**:
- [ ] Create `src/dag_engine/infrastructure/tool_executor.rs`
- [ ] Implement `DagToolExecutor`
- [ ] Implement schema-to-tool conversion logic
- [ ] Handle argument parsing and validation
- [ ] Add error handling for node execution
- [ ] Write unit tests
- [ ] Add integration tests with real nodes

### 5.2 Update LlmNode to Use AgentService

**Location**: `src/dag_engine/infrastructure/nodes/llm.rs`

**Configuration Options**:
```rust
// In DAG JSON config:
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "model": "gpt-4",
    // NEW: List of tool names to enable for this LLM node
    "enabled_tools": ["add", "multiply", "http_request"],
    // OR enable all available tools:
    "enabled_tools": ["*"],
    // Optional: max iterations for ReAct loop
    "max_iterations": 10
  }
}
```

**Implementation Approach**:
```rust
impl ExecutableNode for LlmNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
    ) -> Result<Value, Box<dyn Error>> {
        // ... existing code for provider, api_key, prompt, etc ...

        // NEW: Check if tools are enabled
        let enabled_tools = config.get("enabled_tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<String>>()
            });

        if let Some(tool_names) = enabled_tools {
            // Tools are enabled - use AgentService
            let tool_executor = DagToolExecutor::new(self.registry.clone());

            // Get specific tools based on config
            let tools = if tool_names.contains(&"*".to_string()) {
                // "*" means all available tools
                tool_executor.get_all_available_tools()
            } else {
                // Specific list of tools
                tool_executor.get_tools(&tool_names)
            };

            let max_iterations = config.get("max_iterations")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);

            // Use AgentService for ReAct loop
            let agent_service = AgentService::new(
                llm_repo.clone(),
                conversation_repo.clone(),
            );

            let response = agent_service.run(
                &thread_id,
                prompt.to_string(),
                llm_config,
                tools, // Pass the filtered tools list
                &tool_executor,
                max_iterations,
            ).await?;

            // Return agent response
            Ok(json!({
                "output": {
                    "content": response.content(),
                    "usage": response.usage()
                }
            }))
        } else {
            // No tools - standard LLM call (existing behavior)
            let request = LlmRequest::new(messages, llm_config, false)?;
            let response = llm_repo.call(request).await?;

            // ... existing memory save logic ...

            Ok(json!({
                "output": {
                    "content": response.content(),
                    "usage": response.usage()
                }
            }))
        }
    }
}
```

**Tasks**:
- [ ] Add `enabled_tools` config parsing (array of strings)
- [ ] Support `"*"` wildcard for all tools
- [ ] Add `max_iterations` config option
- [ ] Instantiate `AgentService` when tools are enabled
- [ ] Create `DagToolExecutor` instance
- [ ] Filter tools based on `enabled_tools` list
- [ ] Pass filtered tools to agent service
- [ ] Keep backward compatibility (no tools = standard LLM call)
- [ ] Update schema to document tool options
- [ ] Add validation for tool names (ensure they exist in registry)
- [ ] Add tests with various tool configurations
- [ ] Add example DAG JSON files

### 5.3 Update Node Schemas for Tool Discovery

**Tasks**:
- [ ] Review and update all node schemas to have proper descriptions
- [ ] Ensure all input parameters have clear descriptions
- [ ] Add `toolEnabled: true` flag to node schemas that should be tools
- [ ] Document schema requirements in developer guide

---

## Phase 6: Testing & Validation (Days 19-22)

### 6.1 Unit Tests

**Tasks**:
- [ ] Test `ToolDefinition` creation and validation
- [ ] Test `ToolCall` parsing
- [ ] Test `ToolResult` serialization
- [ ] Test `AgentService` ReAct loop with mocks
- [ ] Test `DagToolExecutor` node execution
- [ ] Test provider adapter tool serialization
- [ ] Achieve >80% code coverage

### 6.2 Integration Tests

**Tasks**:
- [ ] Create "Mathematical Agent" test DAG
  - User asks: "What is (5 + 3) * 2?"
  - Agent should use `add` then `multiply` nodes
- [ ] Create "Web Research Agent" test DAG
  - User asks: "What's the weather in London?"
  - Agent should use `http_request` node
- [ ] Test with real provider APIs
- [ ] Test memory persistence with tool usage
- [ ] Test error handling (invalid tool calls, execution failures)
- [ ] Test max iterations safety limit

### 6.3 Example DAGs

**Create these examples in** `examples/dags/agents/`:

**Example 1: math_agent.json**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/math-agent",
        "method": "POST",
        "test_payload": {
          "question": "What is (15 + 27) * 3?"
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4",
        "system_message": "You are a helpful math assistant. Use the available tools to solve math problems step by step.",
        "enabled_tools": ["add", "subtract", "multiply", "divide"],
        "max_iterations": 10,
        "thread_id": "math-session-1",
        "connection_url": "${DATABASE_URL}"
      }
    },
    "result": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "trigger.output.question",
      "to": "agent.prompt"
    },
    {
      "from": "agent.output.content",
      "to": "result.input"
    }
  ]
}
```

**Example 2: research_agent.json**
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/research",
        "method": "POST",
        "test_payload": {
          "topic": "latest news about AI"
        }
      }
    },
    "researcher": {
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "api_key": "${ANTHROPIC_API_KEY}",
        "model": "claude-3-sonnet-20240229",
        "system_message": "You are a research assistant. Use http_request to fetch information from APIs.",
        "enabled_tools": ["http_request", "log"],
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
      "to": "researcher.prompt"
    },
    {
      "from": "researcher.output",
      "to": "output.input"
    }
  ]
}
```

**Example 3: general_agent.json** (Using wildcard)
```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/general-agent",
        "method": "POST",
        "test_payload": {
          "task": "Calculate 10 + 5, then make an HTTP GET request to https://api.example.com/data"
        }
      }
    },
    "general_agent": {
      "type": "llm_call",
      "config": {
        "provider": "gemini",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-1.5-pro",
        "system_message": "You are a general purpose assistant with access to multiple tools. Use them as needed.",
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

**Tasks**:
- [ ] Create example DAGs
- [ ] Test each example end-to-end
- [ ] Document expected behavior
- [ ] Add to usage examples documentation

---

## Phase 7: Documentation (Days 23-24)

### 7.1 Update Technical Documentation

**Tasks**:
- [ ] Update `docs/dds/MODULO_LLM_DISEÑO.md` with tool calling
- [ ] Update `docs/dds/DISEÑO_AGENTES_Y_TOOLS.md`
- [ ] Update `docs/developer_guide/12_dag_engine_guide.md`
- [ ] Create `docs/guides/TOOL_CALLING_GUIDE.md`
- [ ] Update API reference documentation

### 7.2 Update User Documentation

**Tasks**:
- [ ] Update `docs/USAGE_EXAMPLES.md` with agent examples
- [ ] Update `docs/PYTHON_USAGE_EXAMPLES.md`
- [ ] Create troubleshooting guide for tool calling
- [ ] Add FAQ section

### 7.3 Update PENDING_TASKS.md

**Tasks**:
- [ ] Mark Phase 2 tasks as complete
- [ ] Mark Phase 3 tasks as complete
- [ ] Document any future enhancements

---

## Implementation Improvements

### Enhancements to Your Original Plan

1. **Added Detailed Code Examples**: Your plan was high-level; I've added actual struct definitions and implementation code

2. **Provider-Specific Adapters**: Detailed the specific format conversions needed for each provider (OpenAI, Anthropic, Gemini)

3. **ReAct Loop Implementation**: Provided complete `AgentService` implementation with safety limits and error handling

4. **DAG Bridge (DagToolExecutor)**: Showed exactly how to convert node schemas to tool definitions

5. **Testing Strategy**: Added specific test scenarios and example DAGs

6. **Phased Approach**: Organized into clear phases with dependencies

7. **Documentation Updates**: Identified all docs that need updating

### Risk Mitigation

| Risk | Mitigation Strategy |
|------|-------------------|
| Provider API changes | Abstract with trait, isolate in adapters |
| Infinite loops in ReAct | Max iterations limit with configurable value |
| Tool execution errors | Robust error handling, return errors to LLM |
| Schema mismatch | Validation layer between LLM and nodes |
| Memory/performance issues | Streaming support, configurable limits |
| Complex debugging | Comprehensive logging at each ReAct step |

---

## Success Criteria

- [ ] ✅ All three providers (OpenAI, Anthropic, Gemini) support tool calling
- [ ] ✅ AgentService successfully executes ReAct loop
- [ ] ✅ DAG nodes are automatically discoverable as tools
- [ ] ✅ Mathematical agent example works end-to-end
- [ ] ✅ Tool execution errors are handled gracefully
- [ ] ✅ Conversation memory persists tool calls and results
- [ ] ✅ Code coverage >80%
- [ ] ✅ All documentation updated
- [ ] ✅ No breaking changes to existing LLM functionality

---

## Timeline Summary

| Phase | Duration | Dependencies |
|-------|----------|--------------|
| 1. Planning & Research | 2 days | None |
| 2. Domain Layer | 3 days | Phase 1 |
| 3. Infrastructure (Adapters) | 5 days | Phase 2 |
| 4. Application (Agent Service) | 4 days | Phase 2 |
| 5. DAG Integration | 4 days | Phase 2, 3, 4 |
| 6. Testing & Validation | 4 days | Phase 5 |
| 7. Documentation | 2 days | Phase 6 |
| **Total** | **24 days** | |

---

## Next Steps

1. **Review this plan** with the team
2. **Set up tracking** in GitHub issues or project board
3. **Start Phase 1** research and documentation
4. **Create feature branch**: `feat/tool-calling`
5. **Begin implementation** following the phase order

---

## Appendix: Architecture Diagrams

### Current Architecture (Before Tool Calling)
```
┌─────────────┐
│   LlmNode   │ (Simple: calls LLM, returns response)
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ LlmRepository│ (OpenAI/Gemini/Anthropic)
└─────────────┘
```

### Target Architecture (With Tool Calling)
```
┌─────────────┐
│   LlmNode   │ (Delegates to AgentService)
└──────┬──────┘
       │
       ▼
┌──────────────────┐         ┌──────────────────┐
│  AgentService    │────────>│ LlmRepository    │
│  (ReAct Loop)    │         │ (OpenAI/etc)     │
└────────┬─────────┘         └──────────────────┘
         │
         │ uses
         ▼
┌──────────────────┐         ┌──────────────────┐
│ DagToolExecutor  │────────>│  NodeRegistry    │
│ (ToolExecutor    │         │  (DAG Nodes)     │
│  implementation) │         └──────────────────┘
└──────────────────┘
```

### ReAct Loop Flow
```
User Prompt
    │
    ▼
┌───────────────────────────┐
│ 1. Load Conversation      │
│    History (Memory)       │
└───────────┬───────────────┘
            │
            ▼
┌───────────────────────────┐
│ 2. Call LLM with Tools    │
└───────────┬───────────────┘
            │
            ▼
       ┌────────┐
       │ Has    │ No
       │ Tool   │────────> Return Final Response
       │ Calls? │
       └────┬───┘
            │ Yes
            ▼
┌───────────────────────────┐
│ 3. Execute Each Tool      │
│    via ToolExecutor       │
└───────────┬───────────────┘
            │
            ▼
┌───────────────────────────┐
│ 4. Add Tool Results to    │
│    Conversation           │
└───────────┬───────────────┘
            │
            │ Loop (max 10x)
            └───────────────> Back to Step 2
```

---

**Document Version**: 1.0
**Last Updated**: 2025-11-29
**Status**: Ready for Implementation
