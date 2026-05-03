//! The `describe_tool` synthetic tool — dispatches catalog lookups and produces
//! curated markdown for the LLM.

use crate::dag_engine::domain::tool_configuration::{NodeSchemaField, ToolConfiguration};
use crate::llm::domain::{LlmError, ToolCall, ToolResult};

pub const DESCRIBE_TOOL_NAME: &str = "describe_tool";

#[derive(Debug)]
pub struct DescribeToolDispatchResult {
    /// id of the originating describe_tool call — surfaced so SSE consumers can
    /// correlate the ToolDescribed event with the surrounding tool-call lifecycle.
    pub tool_call_id: String,
    pub output: String,
    pub tool_name: String,
}

/// Produce the markdown the LLM sees as the result of calling describe_tool.
/// Filters out fields that are LLM-invisible: those marked `fixed` in the schema
/// or already populated by `fixed_config` at the top level.
pub fn generate_tool_markdown(cfg: &ToolConfiguration) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", cfg.name));
    out.push_str(cfg.description.trim());
    out.push_str("\n\n");

    let visible_fields = collect_visible_fields(cfg);
    if visible_fields.is_empty() {
        out.push_str(
            "## Parameters\n\nNo parameter schema declared — pass arguments as a free-form JSON object that matches the tool's expectations.\n\n",
        );
    } else {
        out.push_str("## Parameters\n\n");
        out.push_str("| Name | Type | Required | Description |\n");
        out.push_str("|------|------|----------|-------------|\n");
        for (name, field) in &visible_fields {
            let ty = field.field_type.as_str();
            let required = if field.required.unwrap_or(false) {
                "yes"
            } else {
                "no"
            };
            let desc = field.description.as_deref().unwrap_or("");
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                name, ty, required, desc
            ));
        }
        out.push('\n');
    }

    out.push_str("---\nThe tool `");
    out.push_str(&cfg.name);
    out.push_str("` is now available. Call it directly on your next turn.\n");
    out
}

/// Return only fields that the LLM should see: not `fixed`, and not already
/// shadowed by a top-level `fixed_config` entry.
fn collect_visible_fields(cfg: &ToolConfiguration) -> Vec<(String, &NodeSchemaField)> {
    let Some(schema) = cfg.node_schema.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<(String, &NodeSchemaField)> = Vec::new();
    for (name, field) in schema {
        if field.fixed.is_some() {
            continue;
        }
        if cfg.fixed_config.contains_key(name) {
            continue;
        }
        out.push((name.clone(), field));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Dispatch a `describe_tool` call. `lookup` is the slice of currently-configured
/// `ToolConfiguration` entries. Returns the curated markdown for the requested
/// tool, or an "Error: ..." string if the name is not found.
pub async fn dispatch_describe_tool(
    tool_call: &ToolCall,
    lookup: &[ToolConfiguration],
) -> Result<DescribeToolDispatchResult, LlmError> {
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
            LlmError::InvalidToolCall {
                reason: format!("describe_tool: invalid arguments JSON: {}", e),
            }
        })?;
    let name =
        args.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| LlmError::InvalidToolCall {
                reason: "describe_tool: missing required parameter 'name'".to_string(),
            })?;

    let cfg = lookup.iter().find(|c| c.name == name);
    let output = match cfg {
        Some(c) => generate_tool_markdown(c),
        None => format!("Error: Tool '{}' not found in catalog", name),
    };
    Ok(DescribeToolDispatchResult {
        tool_call_id: tool_call.id.clone(),
        output,
        tool_name: name.to_string(),
    })
}

pub fn into_tool_result(call_id: &str, r: &DescribeToolDispatchResult) -> ToolResult {
    ToolResult {
        tool_call_id: call_id.to_string(),
        output: r.output.clone(),
        success: !r.output.starts_with("Error:"),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::tool_configuration::{NodeSchema, NodeSchemaField};
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_field(field_type: &str, required: bool, description: &str) -> NodeSchemaField {
        NodeSchemaField {
            field_type: field_type.to_string(),
            fixed: None,
            required: Some(required),
            description: Some(description.to_string()),
            pattern: None,
            properties: None,
            items: None,
        }
    }

    fn fixed_field(value: serde_json::Value) -> NodeSchemaField {
        NodeSchemaField {
            field_type: "string".to_string(),
            fixed: Some(value),
            required: None,
            description: None,
            pattern: None,
            properties: None,
            items: None,
        }
    }

    fn cfg_minimal(name: &str, desc: &str) -> ToolConfiguration {
        ToolConfiguration {
            name: name.to_string(),
            description: desc.to_string(),
            node_type: "noop".to_string(),
            fixed_config: HashMap::new(),
            #[allow(deprecated)]
            exposed_inputs: None,
            #[allow(deprecated)]
            parameters: None,
            #[allow(deprecated)]
            mergeable_fields: None,
            #[allow(deprecated)]
            field_mapping: None,
            node_schema: None,
            node_config: None,
            expose_sub_tools: None,
            summary: None,
            eager: false,
        }
    }

    #[test]
    fn markdown_includes_name_description_and_anchor() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let md = generate_tool_markdown(&cfg);
        assert!(md.contains("# search_orders"));
        assert!(md.contains("Search the orders table"));
        assert!(md.contains("now available"));
        assert!(md.contains("next turn"));
    }

    #[test]
    fn markdown_without_node_schema_notes_freeform_args() {
        let cfg = cfg_minimal("send_email", "Send transactional email");
        let md = generate_tool_markdown(&cfg);
        assert!(md.contains("No parameter schema declared"));
        assert!(!md.contains("| Name | Type"));
    }

    #[test]
    fn markdown_with_node_schema_renders_table_for_visible_fields() {
        let mut cfg = cfg_minimal("search_orders", "Search orders");
        let mut schema: NodeSchema = HashMap::new();
        schema.insert(
            "start_date".to_string(),
            empty_field("string", true, "ISO date YYYY-MM-DD"),
        );
        schema.insert(
            "status".to_string(),
            empty_field("string", false, "Order status"),
        );
        cfg.node_schema = Some(schema);
        let md = generate_tool_markdown(&cfg);
        assert!(md.contains("| Name | Type | Required | Description |"));
        assert!(md.contains("| start_date | string | yes | ISO date YYYY-MM-DD |"));
        assert!(md.contains("| status | string | no | Order status |"));
    }

    #[test]
    fn markdown_omits_fixed_fields() {
        let mut cfg = cfg_minimal("http_get", "Make HTTP GET");
        let mut schema: NodeSchema = HashMap::new();
        schema.insert(
            "base_url".to_string(),
            fixed_field(json!("https://api.example.com")),
        );
        schema.insert("path".to_string(), empty_field("string", true, "URL path"));
        cfg.node_schema = Some(schema);
        let md = generate_tool_markdown(&cfg);
        assert!(!md.contains("base_url"));
        assert!(md.contains("path"));
    }

    use crate::llm::domain::tools::{FunctionCall, ToolCall};

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new(DESCRIBE_TOOL_NAME.to_string(), args.to_string()),
        )
    }

    #[tokio::test]
    async fn dispatch_returns_markdown_for_known_tool() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let lookup = vec![cfg];
        let call = mk_call(json!({ "name": "search_orders" }));
        let r = dispatch_describe_tool(&call, &lookup).await.unwrap();
        assert_eq!(r.tool_name, "search_orders");
        assert!(r.output.contains("# search_orders"));
        assert!(r.output.contains("now available"));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_output() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let lookup = vec![cfg];
        let call = mk_call(json!({ "name": "deleted_tool" }));
        let r = dispatch_describe_tool(&call, &lookup).await.unwrap();
        assert!(r.output.starts_with("Error:"));
        assert!(r.output.contains("not found in catalog"));
    }

    #[tokio::test]
    async fn dispatch_missing_name_arg_is_invalid_tool_call() {
        let cfg = cfg_minimal("search_orders", "Search");
        let lookup = vec![cfg];
        let call = mk_call(json!({}));
        let err = dispatch_describe_tool(&call, &lookup).await.unwrap_err();
        assert!(matches!(err, LlmError::InvalidToolCall { .. }));
    }

    #[test]
    fn into_tool_result_marks_failure_when_output_starts_with_error() {
        let r = DescribeToolDispatchResult {
            tool_call_id: "call_1".into(),
            output: "Error: Tool 'X' not found in catalog".into(),
            tool_name: "X".into(),
        };
        let tr = into_tool_result("call_1", &r);
        assert!(!tr.success);
    }

    #[test]
    fn markdown_omits_fields_shadowed_by_fixed_config() {
        let mut cfg = cfg_minimal("http_get", "Make HTTP GET");
        let mut schema: NodeSchema = HashMap::new();
        schema.insert(
            "base_url".to_string(),
            empty_field("string", true, "Base URL"),
        );
        schema.insert("path".to_string(), empty_field("string", true, "URL path"));
        cfg.node_schema = Some(schema);
        cfg.fixed_config
            .insert("base_url".to_string(), json!("https://api.example.com"));
        let md = generate_tool_markdown(&cfg);
        assert!(!md.contains("Base URL"));
        assert!(md.contains("path"));
    }
}
