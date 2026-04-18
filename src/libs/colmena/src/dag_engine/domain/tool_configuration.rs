//! Tool configuration types for exposing DAG nodes as LLM-callable tools.
//!
//! ## Three approaches (in priority order)
//!
//! 1. **`node_schema`** (RECOMMENDED) — Unified approach via [`NodeSchema`]. A flat map where
//!    each key is a node field (e.g. `base_url`, `query_params`, `body`). Values can be:
//!    - `fixed`: hidden from the LLM, always applied as-is.
//!    - LLM-visible: typed, optionally required, with description and pattern constraints.
//!      Container fields (e.g. `body`, `query_params`) support nested `properties`, allowing
//!      mixed fixed/dynamic sub-fields. Use this for all non-trivial tool configurations.
//!
//! 2. **`$DYNAMIC` placeholders** — Simpler alternative. Use `fixed_config` with specific
//!    values set to the string literal `"$DYNAMIC"` (see [`DYNAMIC_PLACEHOLDER`]).
//!    The executor auto-exposes those fields as required `string` parameters to the LLM.
//!    **Limitation:** only works one level deep inside a container object
//!    (e.g. `body.title` works; `body.metadata.author.name` does NOT).
//!    Use only for simple cases with a few flat dynamic fields.
//!
//! 3. **Deprecated fallback** — `field_mapping` + `mergeable_fields` + `exposed_inputs`.
//!    Still executed for backward compatibility but must not be used in new configurations.
//!    All deprecated fields carry `#[deprecated(since = "0.3.0")]`.
//!
//! The execution priority in `DagToolExecutor` is: `node_schema` → `$DYNAMIC` → deprecated.

use crate::llm::domain::ParameterProperty;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Marker string used as a placeholder value in `fixed_config` to indicate that a field
/// should be provided by the LLM at call time.
///
/// ## Usage
/// Set any string value inside `fixed_config` to exactly `"$DYNAMIC"` (case-sensitive):
///
/// ```json
/// "fixed_config": {
///   "base_url": "https://api.example.com",
///   "body": { "author": "fixed-author", "title": "$DYNAMIC" }
/// }
/// ```
///
/// The executor detects these markers and automatically creates a required `string` parameter
/// for each one, named after the field. At execution time, the LLM-provided value replaces
/// the `"$DYNAMIC"` string in the final request.
///
/// ## Limitations
/// - All inferred parameters are typed as `string` and marked `required`. There is no way
///   to declare optional or non-string `$DYNAMIC` fields.
/// - Only works **one level deep** inside a container object. For example, `body.title`
///   is detected, but `body.metadata.author.name` is NOT — use `node_schema` instead
///   for deep nesting or complex type requirements.
pub const DYNAMIC_PLACEHOLDER: &str = "$DYNAMIC";

/// Configuration for exposing a DAG node as an LLM-callable tool.
///
/// Defined inside `tool_configurations` of an `llm_call` node. The executor uses this
/// struct to generate the tool definition sent to the LLM and to execute the node when
/// the LLM invokes the tool. See module-level docs for the three configuration approaches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfiguration {
    /// Name of the tool (shown to LLM)
    pub name: String,

    /// Human-readable description for the LLM
    pub description: String,

    /// Node type to execute
    pub node_type: String,

    /// Static configuration values never exposed to the LLM.
    ///
    /// When used with [`DYNAMIC_PLACEHOLDER`] values (e.g. `"title": "$DYNAMIC"`),
    /// the executor auto-exposes those fields to the LLM as required `string` parameters
    /// and replaces them at call time. This is the `$DYNAMIC` approach — simpler than
    /// `node_schema` but limited to flat, `string`-typed dynamic fields.
    ///
    /// For full control (types, optional fields, nested structures), use `node_schema` instead.
    #[serde(default)]
    pub fixed_config: HashMap<String, Value>,

    /// Which input parameters to expose to the LLM
    /// If None, expose all inputs not in fixed_config
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none")]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub exposed_inputs: Option<Vec<String>>,

    /// Optional JSON Schema for parameters to override node schema
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none")]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub parameters: Option<Value>,

    /// Fields where fixed + dynamic values should be merged (not overridden).
    /// Example: ["headers", "query_params", "body"]
    /// When merging a field listed here, the fixed object is the base
    /// and the dynamic (LLM-provided) object overlays it.
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub mergeable_fields: Option<Vec<String>>,

    /// Maps each LLM parameter to its destination container field.
    /// The parameter value is moved into that container under its own key.
    /// Example: {"title" → "body", "x_request_id" → "headers"}
    /// Parameters not listed in this map are kept at the top level.
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub field_mapping: Option<HashMap<String, String>>,

    /// Unified schema defining all node fields in one place. **This is the recommended approach.**
    ///
    /// A flat map where each key is a node field (e.g. `base_url`, `query_params`, `body`).
    /// Each entry is a [`NodeSchemaField`] that can be:
    /// - **Fixed** (`fixed` present): hidden from LLM, always applied.
    /// - **LLM-visible** (`fixed` absent): exposed to the LLM with type, description, and optional constraints.
    /// - **Container** (`properties` present): a nested object where children can be individually
    ///   fixed or LLM-visible. The fixed children are merged as base values; the LLM fills the rest.
    ///
    /// Takes priority over `fixed_config` + `$DYNAMIC` if both are present (though mixing is not recommended).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub node_schema: Option<NodeSchema>,
}

/// A single field entry within a node_schema object or nested properties map.
/// Handles both leaf fields (with `fixed` or `required`) and container fields (with `properties`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSchemaField {
    /// JSON Schema type: "string", "number", "boolean", "object", "array"
    #[serde(rename = "type")]
    pub field_type: String,

    /// If present, this field is hidden from the LLM and always set to this value.
    /// Supports runtime template syntax like "${context.foo}" (resolved elsewhere).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<Value>,

    /// Whether the LLM must supply this field (only meaningful when `fixed` is absent).
    /// If absent or false, the field is optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Human-readable description passed to the LLM in the tool definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Regex pattern constraint (e.g., "^\\d{4}-\\d{2}-\\d{2}$"). Passed through to ParameterProperty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Nested properties — makes this field a container (type = "object").
    /// The executor collects LLM params from children and merges them into this container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, NodeSchemaField>>,
}

/// The top-level node_schema value: a flat map of field name → NodeSchemaField.
/// Example top-level keys: "base_url", "bearer_token", "query_params".
pub type NodeSchema = HashMap<String, NodeSchemaField>;

/// Output of parsing a NodeSchema for use by the executor.
pub struct ParsedNodeSchema {
    /// Values that are always fixed (LLM never sees them).
    /// Key = top-level field name. Value = the fixed Value (string, number, object, etc.).
    pub fixed_values: HashMap<String, Value>,

    /// LLM-visible parameter name → ParameterProperty (for ToolDefinition generation).
    pub llm_properties: HashMap<String, ParameterProperty>,

    /// Required parameter names (subset of llm_properties keys).
    pub required_params: Vec<String>,

    /// Maps each LLM param name → the container field it should be merged into.
    /// None means it goes to the top level of inputs.
    /// This replaces field_mapping for node_schema configs.
    pub param_to_container: HashMap<String, String>,
}

/// Parse a [`NodeSchema`] into the components needed by `generate_tool_definition()` and `execute()`.
///
/// Iterates over each top-level entry and handles three cases:
/// - **Fixed top-level field** (`fixed` present): added directly to `fixed_values`.
/// - **Container field** (`properties` present): child fields with `fixed` are collected into a
///   base object stored in `fixed_values`; LLM-visible children go into `llm_properties` and
///   `param_to_container` (mapped to this container key).
/// - **LLM-visible top-level field** (no `fixed`, no `properties`): added to `llm_properties`
///   at the top level (no container mapping).
pub fn parse_node_schema(schema: &NodeSchema) -> ParsedNodeSchema {
    let mut fixed_values: HashMap<String, Value> = HashMap::new();
    let mut llm_properties: HashMap<String, ParameterProperty> = HashMap::new();
    let mut required_params: Vec<String> = Vec::new();
    let mut param_to_container: HashMap<String, String> = HashMap::new();

    // Collected LLM-visible children from containers (for two-pass collision detection).
    // Each entry: (child_key, container_key, ParameterProperty, is_required)
    let mut container_children: Vec<(String, String, ParameterProperty, bool)> = Vec::new();

    for (top_key, top_field) in schema {
        // Case 1: Top-level field with fixed value
        if let Some(fixed_val) = &top_field.fixed {
            fixed_values.insert(top_key.clone(), fixed_val.clone());
        }
        // Case 2: Container field (has properties)
        else if let Some(properties) = &top_field.properties {
            let mut container_fixed: serde_json::Map<String, Value> = serde_json::Map::new();

            for (child_key, child_field) in properties {
                if let Some(fixed_val) = &child_field.fixed {
                    // Child has fixed value
                    container_fixed.insert(child_key.clone(), fixed_val.clone());
                } else if let Some(nested_properties) = &child_field.properties {
                    // Child is a nested container (e.g., "edge" inside "payload").
                    // Collect its fixed sub-properties into a fixed sub-object so the
                    // executor can deep-merge them with the LLM-provided object.
                    let mut nested_fixed: serde_json::Map<String, Value> = serde_json::Map::new();
                    for (nested_key, nested_field) in nested_properties {
                        if let Some(fixed_val) = &nested_field.fixed {
                            nested_fixed.insert(nested_key.clone(), fixed_val.clone());
                        }
                        // LLM-visible nested sub-properties are not individually exposed;
                        // the LLM provides them as part of the child object.
                    }
                    if !nested_fixed.is_empty() {
                        container_fixed.insert(child_key.clone(), Value::Object(nested_fixed));
                    }

                    // Collect as LLM-visible (deferred to pass 2 for collision detection)
                    let mut prop = ParameterProperty::new(
                        child_field.field_type.clone(),
                        child_field.description.clone().unwrap_or_default(),
                    );
                    if let Some(pattern) = &child_field.pattern {
                        prop = prop.with_pattern(pattern.clone());
                    }
                    container_children.push((
                        child_key.clone(),
                        top_key.clone(),
                        prop,
                        child_field.required == Some(true),
                    ));
                } else {
                    // Child is LLM-visible (deferred to pass 2 for collision detection)
                    let mut prop = ParameterProperty::new(
                        child_field.field_type.clone(),
                        child_field.description.clone().unwrap_or_default(),
                    );

                    if let Some(pattern) = &child_field.pattern {
                        prop = prop.with_pattern(pattern.clone());
                    }

                    container_children.push((
                        child_key.clone(),
                        top_key.clone(),
                        prop,
                        child_field.required == Some(true),
                    ));
                }
            }

            // If container has fixed children, store them as a base object
            if !container_fixed.is_empty() {
                fixed_values.insert(top_key.clone(), Value::Object(container_fixed));
            }
        }
        // Case 3: Top-level LLM-visible field (no fixed, no properties)
        else {
            let mut prop = ParameterProperty::new(
                top_field.field_type.clone(),
                top_field.description.clone().unwrap_or_default(),
            );

            if let Some(pattern) = &top_field.pattern {
                prop = prop.with_pattern(pattern.clone());
            }

            llm_properties.insert(top_key.clone(), prop);

            // Check if required
            if top_field.required == Some(true) {
                required_params.push(top_key.clone());
            }
        }
    }

    // Pass 2: Detect collisions and insert container children with conditional dot-prefix.
    // Count how many containers each child_key appears in.
    let mut key_count: HashMap<String, usize> = HashMap::new();
    for (child_key, _, _, _) in &container_children {
        *key_count.entry(child_key.clone()).or_insert(0) += 1;
    }

    for (child_key, container_key, prop, is_required) in container_children {
        let effective_key = if key_count.get(&child_key).copied().unwrap_or(0) > 1 {
            format!("{}.{}", container_key, child_key)
        } else {
            child_key
        };

        llm_properties.insert(effective_key.clone(), prop);
        if is_required {
            required_params.push(effective_key.clone());
        }
        param_to_container.insert(effective_key, container_key);
    }

    ParsedNodeSchema {
        fixed_values,
        llm_properties,
        required_params,
        param_to_container,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_node_schema_fixed_only() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "base_url": { "type": "string", "fixed": "https://api.example.com" },
            "method": { "type": "string", "fixed": "GET" }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        assert_eq!(parsed.fixed_values.len(), 2);
        assert_eq!(parsed.llm_properties.len(), 0);
        assert_eq!(parsed.required_params.len(), 0);
        assert_eq!(
            parsed.fixed_values.get("base_url").unwrap().as_str(),
            Some("https://api.example.com")
        );
    }

    #[test]
    fn test_parse_node_schema_required_implicit_false() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "title": { "type": "string", "required": true, "description": "Required title" },
            "tags": { "type": "string", "description": "Optional tags" }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        assert_eq!(parsed.llm_properties.len(), 2);
        assert_eq!(parsed.required_params.len(), 1);
        assert!(parsed.required_params.contains(&"title".to_string()));
        assert!(!parsed.required_params.contains(&"tags".to_string()));
    }

    #[test]
    fn test_parse_node_schema_nested_container() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "base_url": { "type": "string", "fixed": "https://api.example.com" },
            "query_params": {
                "type": "object",
                "properties": {
                    "max": { "type": "string", "fixed": "5" },
                    "originLocationCode": { "type": "string", "required": true, "description": "Origin code" },
                    "destinationLocationCode": { "type": "string", "required": true, "description": "Destination code" },
                    "children": { "type": "string", "description": "Optional children count" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        // base_url is fixed at top level
        assert_eq!(parsed.fixed_values.len(), 2);
        assert!(parsed.fixed_values.contains_key("base_url"));
        assert!(parsed.fixed_values.contains_key("query_params"));

        // Check query_params fixed content
        let query_params = parsed.fixed_values.get("query_params").unwrap();
        assert!(query_params.is_object());
        assert_eq!(query_params.get("max").unwrap().as_str(), Some("5"));

        // LLM properties should include the 3 non-fixed children
        assert_eq!(parsed.llm_properties.len(), 3);
        assert!(parsed.llm_properties.contains_key("originLocationCode"));
        assert!(parsed
            .llm_properties
            .contains_key("destinationLocationCode"));
        assert!(parsed.llm_properties.contains_key("children"));

        // Required params check
        assert_eq!(parsed.required_params.len(), 2);
        assert!(parsed
            .required_params
            .contains(&"originLocationCode".to_string()));
        assert!(parsed
            .required_params
            .contains(&"destinationLocationCode".to_string()));

        // Param to container mapping
        assert_eq!(
            parsed.param_to_container.get("originLocationCode"),
            Some(&"query_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("children"),
            Some(&"query_params".to_string())
        );
    }

    #[test]
    fn test_parse_node_schema_body_container() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "method": { "type": "string", "fixed": "POST" },
            "body": {
                "type": "object",
                "properties": {
                    "userId": { "type": "string", "fixed": "1" },
                    "title": { "type": "string", "required": true, "description": "Post title" },
                    "content": { "type": "string", "required": true, "description": "Post content" },
                    "tags": { "type": "string", "description": "Optional tags" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        // body should be in fixed_values with userId
        assert!(parsed.fixed_values.contains_key("body"));
        let body = parsed.fixed_values.get("body").unwrap();
        assert_eq!(body.get("userId").unwrap().as_str(), Some("1"));

        // LLM properties: title, content, tags
        assert_eq!(parsed.llm_properties.len(), 3);
        assert_eq!(parsed.required_params.len(), 2); // title and content

        // All should map to body container
        assert_eq!(
            parsed.param_to_container.get("title"),
            Some(&"body".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("content"),
            Some(&"body".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("tags"),
            Some(&"body".to_string())
        );
    }

    #[test]
    fn test_parse_node_schema_pattern_passthrough() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "departureDate": {
                "type": "string",
                "required": true,
                "description": "Date in YYYY-MM-DD format",
                "pattern": "^\\d{4}-\\d{2}-\\d{2}$"
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        assert_eq!(parsed.llm_properties.len(), 1);
        let prop = parsed.llm_properties.get("departureDate").unwrap();
        assert_eq!(prop.pattern.as_deref(), Some("^\\d{4}-\\d{2}-\\d{2}$"));
    }

    #[test]
    fn test_parse_node_schema_deeply_nested_container() {
        // Simulates the create_edge payload structure:
        // payload.properties.environmentId (fixed) + payload.properties.edge (nested container
        // with its own fixed and LLM-visible sub-properties).
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "url": { "type": "string", "fixed": "https://api.example.com" },
            "payload": {
                "type": "object",
                "properties": {
                    "environmentId": { "type": "string", "fixed": "env-123" },
                    "edge": {
                        "type": "object",
                        "required": true,
                        "description": "Edge object",
                        "properties": {
                            "id": { "type": "string", "required": true, "description": "Edge ID" },
                            "source": { "type": "string", "required": true, "description": "Source node" },
                            "target": { "type": "string", "required": true, "description": "Target node" },
                            "type": { "type": "string", "fixed": "default" },
                            "animated": { "type": "boolean", "fixed": true },
                            "environmentId": { "type": "string", "fixed": "env-123" }
                        }
                    }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        // url is fixed at top level
        assert!(parsed.fixed_values.contains_key("url"));

        // payload should contain fixed values for both environmentId and edge
        assert!(parsed.fixed_values.contains_key("payload"));
        let payload = parsed.fixed_values.get("payload").unwrap();
        assert_eq!(payload.get("environmentId").unwrap(), "env-123");

        // edge's fixed sub-properties should be collected
        let edge_fixed = payload.get("edge").unwrap();
        assert!(edge_fixed.is_object());
        assert_eq!(edge_fixed.get("type").unwrap(), "default");
        assert_eq!(edge_fixed.get("animated").unwrap(), true);
        assert_eq!(edge_fixed.get("environmentId").unwrap(), "env-123");

        // edge should be exposed as an LLM-visible object parameter mapped to payload
        assert!(parsed.llm_properties.contains_key("edge"));
        assert!(parsed.required_params.contains(&"edge".to_string()));
        assert_eq!(
            parsed.param_to_container.get("edge"),
            Some(&"payload".to_string())
        );

        // The LLM-visible sub-properties (id, source, target) should NOT be individually
        // exposed — the LLM provides them as part of the edge object
        assert!(!parsed.llm_properties.contains_key("id"));
        assert!(!parsed.llm_properties.contains_key("source"));
        assert!(!parsed.llm_properties.contains_key("target"));
    }

    #[test]
    fn test_parse_node_schema_collision_prefixed() {
        // Two containers with children that share the same key names ("name", "id").
        // The parser should prefix them as "source_params.name", "target_params.name", etc.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "source_params": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "required": true, "description": "Source name" },
                    "id": { "type": "string", "required": true, "description": "Source ID" }
                }
            },
            "target_params": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "required": true, "description": "Target name" },
                    "id": { "type": "string", "description": "Target ID" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        // All 4 children should be present (no overwrites)
        assert_eq!(parsed.llm_properties.len(), 4);

        // Keys should be dot-prefixed
        assert!(parsed.llm_properties.contains_key("source_params.name"));
        assert!(parsed.llm_properties.contains_key("source_params.id"));
        assert!(parsed.llm_properties.contains_key("target_params.name"));
        assert!(parsed.llm_properties.contains_key("target_params.id"));

        // Original un-prefixed keys should NOT be present
        assert!(!parsed.llm_properties.contains_key("name"));
        assert!(!parsed.llm_properties.contains_key("id"));

        // param_to_container should map prefixed keys to the correct container
        assert_eq!(
            parsed.param_to_container.get("source_params.name"),
            Some(&"source_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("target_params.name"),
            Some(&"target_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("source_params.id"),
            Some(&"source_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("target_params.id"),
            Some(&"target_params".to_string())
        );

        // Required: source_params.name, source_params.id, target_params.name (3 total)
        assert_eq!(parsed.required_params.len(), 3);
        assert!(parsed
            .required_params
            .contains(&"source_params.name".to_string()));
        assert!(parsed
            .required_params
            .contains(&"source_params.id".to_string()));
        assert!(parsed
            .required_params
            .contains(&"target_params.name".to_string()));
        // target_params.id is NOT required
        assert!(!parsed
            .required_params
            .contains(&"target_params.id".to_string()));
    }

    #[test]
    fn test_parse_node_schema_no_collision_no_prefix() {
        // Two containers with unique child names — no collision, no prefix needed.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "query_params": {
                "type": "object",
                "properties": {
                    "city": { "type": "string", "required": true, "description": "City name" },
                    "limit": { "type": "string", "description": "Result limit" }
                }
            },
            "headers": {
                "type": "object",
                "properties": {
                    "x_request_id": { "type": "string", "description": "Request ID" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema);

        // Keys should remain flat (no dot prefix)
        assert_eq!(parsed.llm_properties.len(), 3);
        assert!(parsed.llm_properties.contains_key("city"));
        assert!(parsed.llm_properties.contains_key("limit"));
        assert!(parsed.llm_properties.contains_key("x_request_id"));

        // No dotted keys should exist
        assert!(!parsed.llm_properties.contains_key("query_params.city"));
        assert!(!parsed.llm_properties.contains_key("headers.x_request_id"));

        // Container mappings
        assert_eq!(
            parsed.param_to_container.get("city"),
            Some(&"query_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("x_request_id"),
            Some(&"headers".to_string())
        );
    }
}
