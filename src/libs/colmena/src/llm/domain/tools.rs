use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a tool/function definition that can be passed to an LLM
///
/// This follows the JSON Schema format used by OpenAI and other providers.
/// Tools allow LLMs to request execution of specific functions/actions.
///
/// # Example
/// ```rust
/// use colmena::llm::domain::tools::{ToolDefinition, ToolParameters, ParameterProperty};
///
/// let params = ToolParameters::new()
///     .with_property(
///         "a".to_string(),
///         ParameterProperty::new("number".to_string(), "First number".to_string()),
///     )
///     .with_property(
///         "b".to_string(),
///         ParameterProperty::new("number".to_string(), "Second number".to_string()),
///     )
///     .with_required("a".to_string())
///     .with_required("b".to_string());
///
/// let tool = ToolDefinition::new(
///     "add".to_string(),
///     "Add two numbers together".to_string(),
///     params,
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    /// The name of the tool (e.g., "add", "http_request")
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: String,

    /// Short (≤ 200 char) one-line summary surfaced in lazy-tool-loading
    /// catalogs. When `None`, the catalog falls back to a truncated
    /// `description`. Every synthetic tool MUST set this; DAG nodes used as
    /// tools may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// JSON Schema for the tool's parameters
    pub parameters: ToolParameters,

    /// Raw JSON Schema override. When `Some`, providers (OpenAI/Anthropic/Gemini)
    /// send this object verbatim as the tool's input schema and ignore
    /// `parameters`. Lets synthetic tools expose schemars-derived schemas with
    /// nested objects, tagged unions and arrays — shapes that don't fit the
    /// flat `ParameterProperty` model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema_override: Option<serde_json::Value>,
}

impl ToolDefinition {
    /// Create a new tool definition
    pub fn new(name: String, description: String, parameters: ToolParameters) -> Self {
        Self {
            name,
            description,
            summary: None,
            parameters,
            input_schema_override: None,
        }
    }

    /// Builder: attach a one-line summary for lazy catalogs.
    pub fn with_summary(mut self, summary: String) -> Self {
        self.summary = Some(summary);
        self
    }

    /// Builder: attach a raw JSON Schema that providers send verbatim.
    pub fn with_input_schema_override(mut self, schema: serde_json::Value) -> Self {
        self.input_schema_override = Some(schema);
        self
    }

    /// Validate that the tool definition is well-formed
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Tool name cannot be empty".to_string());
        }

        if self.description.is_empty() {
            return Err("Tool description cannot be empty".to_string());
        }

        // When a raw override is provided, structured `parameters` validation
        // is bypassed — the override is the source of truth for the schema.
        if self.input_schema_override.is_some() {
            return Ok(());
        }

        if self.parameters.schema_type != "object" {
            return Err("Parameters schema type must be 'object'".to_string());
        }

        // Validate that all required fields exist in properties
        for required_field in &self.parameters.required {
            if !self.parameters.properties.contains_key(required_field) {
                return Err(format!(
                    "Required field '{}' not found in properties",
                    required_field
                ));
            }
        }

        Ok(())
    }
}

/// JSON Schema definition for tool parameters
///
/// Describes the input schema for a tool using JSON Schema format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolParameters {
    /// Always "object" for function parameters
    #[serde(rename = "type")]
    pub schema_type: String,

    /// Properties/fields of the parameters
    pub properties: HashMap<String, ParameterProperty>,

    /// List of required parameter names
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub required: Vec<String>,
}

impl ToolParameters {
    /// Create a new tool parameters schema
    pub fn new() -> Self {
        Self {
            schema_type: "object".to_string(),
            properties: HashMap::new(),
            required: Vec::new(),
        }
    }

    /// Add a property to the schema
    pub fn with_property(mut self, name: String, property: ParameterProperty) -> Self {
        self.properties.insert(name, property);
        self
    }

    /// Mark a property as required
    pub fn with_required(mut self, name: String) -> Self {
        if !self.required.contains(&name) {
            self.required.push(name);
        }
        self
    }
}

impl Default for ToolParameters {
    fn default() -> Self {
        Self::new()
    }
}

/// Definition of a single parameter property
///
/// Describes a single field in the tool's parameter schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParameterProperty {
    /// JSON Schema type (e.g., "string", "number", "boolean", "object", "array")
    #[serde(rename = "type")]
    pub property_type: String,

    /// Human-readable description of the parameter
    pub description: String,

    /// Optional list of allowed values (for enum types)
    #[serde(skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,

    /// Optional regex pattern constraint for string validation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Item type for array properties. Required by both OpenAI and Gemini's
    /// strict JSON Schema validators when `property_type == "array"`;
    /// Anthropic accepts arrays without `items` but it's good practice
    /// everywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<ParameterProperty>>,
}

impl ParameterProperty {
    /// Create a new parameter property
    pub fn new(property_type: String, description: String) -> Self {
        Self {
            property_type,
            description,
            enum_values: None,
            pattern: None,
            items: None,
        }
    }

    /// Add enum values to the property
    pub fn with_enum(mut self, values: Vec<String>) -> Self {
        self.enum_values = Some(values);
        self
    }

    /// Add a regex pattern constraint to the property
    pub fn with_pattern(mut self, pattern: String) -> Self {
        self.pattern = Some(pattern);
        self
    }

    /// Set the item type for array properties (required by OpenAI)
    pub fn with_items(mut self, item_type: String) -> Self {
        self.items = Some(Box::new(ParameterProperty::new(item_type, String::new())));
        self
    }
}

/// Represents a tool call requested by the LLM
///
/// When an LLM decides to use a tool, it returns a tool call with
/// the function name and arguments to execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Unique identifier for this tool call (provider-generated)
    pub id: String,

    /// The type (usually "function")
    #[serde(rename = "type")]
    pub call_type: String,

    /// The function being called
    pub function: FunctionCall,

    /// The response generated by the function execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<serde_json::Value>,

    /// Opaque, provider-specific signature that MUST be replayed verbatim when
    /// this tool call is sent back in the conversation history. Currently used
    /// by Gemini thinking models (`thoughtSignature`): the API rejects the next
    /// request with HTTP 400 if a previously-returned function call is replayed
    /// without its signature. `None` for providers/models that don't emit one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_signature: Option<String>,
}

impl ToolCall {
    /// Create a new tool call
    pub fn new(id: String, function: FunctionCall) -> Self {
        Self {
            id,
            call_type: "function".to_string(),
            function,
            response: None,
            provider_signature: None,
        }
    }
}

/// The actual function call details
///
/// Contains the function name and JSON-encoded arguments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    /// Name of the function to call
    pub name: String,

    /// JSON string of arguments
    pub arguments: String,
}

impl FunctionCall {
    /// Create a new function call
    pub fn new(name: String, arguments: String) -> Self {
        Self { name, arguments }
    }

    /// Parse the arguments as JSON
    pub fn parse_arguments<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(&self.arguments)
    }
}

/// Result of executing a tool
///
/// Contains the outcome of a tool execution, including success/failure
/// status and the output or error message.
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

impl ToolResult {
    /// Create a successful tool result
    pub fn success(tool_call_id: String, output: String) -> Self {
        Self {
            tool_call_id,
            success: true,
            output,
            error: None,
        }
    }

    /// Create a failed tool result
    pub fn failure(tool_call_id: String, error: String) -> Self {
        Self {
            tool_call_id,
            success: false,
            output: String::new(),
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition_creation() {
        let mut properties = HashMap::new();
        properties.insert(
            "x".to_string(),
            ParameterProperty::new("number".to_string(), "First number".to_string()),
        );
        properties.insert(
            "y".to_string(),
            ParameterProperty::new("number".to_string(), "Second number".to_string()),
        );

        let params = ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["x".to_string(), "y".to_string()],
        };

        let tool = ToolDefinition::new("add".to_string(), "Add two numbers".to_string(), params);

        assert_eq!(tool.name, "add");
        assert_eq!(tool.description, "Add two numbers");
        assert_eq!(tool.parameters.required.len(), 2);
    }

    #[test]
    fn test_tool_definition_validation_success() {
        let params = ToolParameters::new()
            .with_property(
                "a".to_string(),
                ParameterProperty::new("number".to_string(), "Number A".to_string()),
            )
            .with_required("a".to_string());

        let tool = ToolDefinition::new("test".to_string(), "Test tool".to_string(), params);

        assert!(tool.validate().is_ok());
    }

    #[test]
    fn test_tool_definition_validation_empty_name() {
        let params = ToolParameters::new();
        let tool = ToolDefinition::new("".to_string(), "Description".to_string(), params);

        assert!(tool.validate().is_err());
        assert!(tool
            .validate()
            .unwrap_err()
            .contains("name cannot be empty"));
    }

    #[test]
    fn test_tool_definition_validation_missing_required_property() {
        let params = ToolParameters::new().with_required("missing_field".to_string());

        let tool = ToolDefinition::new("test".to_string(), "Test".to_string(), params);

        assert!(tool.validate().is_err());
        assert!(tool
            .validate()
            .unwrap_err()
            .contains("not found in properties"));
    }

    #[test]
    fn test_parameter_property_with_enum() {
        let prop =
            ParameterProperty::new("string".to_string(), "HTTP method".to_string()).with_enum(
                vec!["GET".to_string(), "POST".to_string(), "PUT".to_string()],
            );

        assert_eq!(prop.property_type, "string");
        assert!(prop.enum_values.is_some());
        assert_eq!(prop.enum_values.unwrap().len(), 3);
    }

    #[test]
    fn test_tool_call_creation() {
        let function = FunctionCall::new("add".to_string(), r#"{"a": 5, "b": 3}"#.to_string());

        let tool_call = ToolCall::new("call_123".to_string(), function);

        assert_eq!(tool_call.id, "call_123");
        assert_eq!(tool_call.call_type, "function");
        assert_eq!(tool_call.function.name, "add");
    }

    #[test]
    fn test_function_call_parse_arguments() {
        #[derive(Deserialize)]
        struct Args {
            a: i32,
            b: i32,
        }

        let function = FunctionCall::new("add".to_string(), r#"{"a": 5, "b": 3}"#.to_string());

        let args: Args = function.parse_arguments().unwrap();
        assert_eq!(args.a, 5);
        assert_eq!(args.b, 3);
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("call_123".to_string(), "42".to_string());

        assert_eq!(result.tool_call_id, "call_123");
        assert!(result.success);
        assert_eq!(result.output, "42");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_failure() {
        let result = ToolResult::failure("call_123".to_string(), "Division by zero".to_string());

        assert_eq!(result.tool_call_id, "call_123");
        assert!(!result.success);
        assert!(result.output.is_empty());
        assert_eq!(result.error.unwrap(), "Division by zero");
    }

    #[test]
    fn test_array_property_serializes_with_items() {
        let prop = ParameterProperty::new("array".to_string(), "List of domains".to_string())
            .with_items("string".to_string());

        let json = serde_json::to_value(&prop).unwrap();

        assert_eq!(json["type"], "array");
        assert_eq!(json["items"]["type"], "string");
    }

    #[test]
    fn with_summary_sets_field_and_chains() {
        let td = ToolDefinition::new(
            "demo".to_string(),
            "Does a demo thing".to_string(),
            ToolParameters::new(),
        )
        .with_summary("Run a demo".to_string());
        assert_eq!(td.summary.as_deref(), Some("Run a demo"));
        assert_eq!(td.name, "demo");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let params = ToolParameters::new()
            .with_property(
                "url".to_string(),
                ParameterProperty::new("string".to_string(), "The URL to fetch".to_string()),
            )
            .with_required("url".to_string());

        let tool = ToolDefinition::new(
            "fetch".to_string(),
            "Fetch data from URL".to_string(),
            params,
        );

        let json = serde_json::to_string(&tool).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(tool, deserialized);
    }
}
