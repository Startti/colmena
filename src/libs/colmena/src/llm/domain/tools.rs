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
/// use std::collections::HashMap;
///
/// let mut properties = HashMap::new();
/// properties.insert(
///     "a".to_string(),
///     ParameterProperty {
///         property_type: "number".to_string(),
///         description: "First number".to_string(),
///         enum_values: None,
///     }
/// );
///
/// let tool = ToolDefinition {
///     name: "add".to_string(),
///     description: "Add two numbers together".to_string(),
///     parameters: ToolParameters {
///         schema_type: "object".to_string(),
///         properties,
///         required: vec!["a".to_string(), "b".to_string()],
///     },
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    /// The name of the tool (e.g., "add", "http_request")
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: String,

    /// JSON Schema for the tool's parameters
    pub parameters: ToolParameters,
}

impl ToolDefinition {
    /// Create a new tool definition
    pub fn new(name: String, description: String, parameters: ToolParameters) -> Self {
        Self {
            name,
            description,
            parameters,
        }
    }

    /// Validate that the tool definition is well-formed
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Tool name cannot be empty".to_string());
        }

        if self.description.is_empty() {
            return Err("Tool description cannot be empty".to_string());
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
}

impl ParameterProperty {
    /// Create a new parameter property
    pub fn new(property_type: String, description: String) -> Self {
        Self {
            property_type,
            description,
            enum_values: None,
        }
    }

    /// Add enum values to the property
    pub fn with_enum(mut self, values: Vec<String>) -> Self {
        self.enum_values = Some(values);
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
}

impl ToolCall {
    /// Create a new tool call
    pub fn new(id: String, function: FunctionCall) -> Self {
        Self {
            id,
            call_type: "function".to_string(),
            function,
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
