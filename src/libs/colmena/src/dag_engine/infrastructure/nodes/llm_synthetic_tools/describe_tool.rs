//! The `describe_tool` synthetic tool — dispatches catalog lookups and produces
//! curated markdown for the LLM.

use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::tool_configuration::{NodeSchemaField, ToolConfiguration};
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::tool_context::{
    build_tool_context_block, BlockVariant,
};
use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use crate::skills::domain::skill_repository::SkillRepository;

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
            // visible_fields by construction only contains LLM-visible entries,
            // so `type` SHOULD be present. Fall back to "unknown" defensively
            // rather than panic on malformed inputs.
            let ty = field.field_type.as_deref().unwrap_or("unknown");
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

/// Async wrapper around `build_tool_context_block`: resolves the
/// `{{NODE_GUIDE_BODY}}` placeholder by loading the matched skill's full body,
/// then appends the "now available" footer.
pub async fn generate_tool_markdown_async(
    cfg: &ToolConfiguration,
    node: Option<&dyn ExecutableNode>,
    skill_repo: Option<&dyn SkillRepository>,
) -> String {
    struct NoopNode;
    #[async_trait::async_trait]
    impl ExecutableNode for NoopNode {
        async fn execute(
            &self,
            _inputs: &crate::dag_engine::domain::node::NodeInputs,
            _config: &serde_json::Value,
            _state: &mut serde_json::Value,
            _observer: Option<
                std::sync::Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>,
            >,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            Ok(serde_json::json!({}))
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
    }

    let noop = NoopNode;
    let node_ref: &dyn ExecutableNode = node.unwrap_or(&noop);

    let fixed = effective_fixed_config(cfg);

    let mut block = build_tool_context_block(cfg, node_ref, &fixed, skill_repo, BlockVariant::Lazy);

    if block.contains("{{NODE_GUIDE_BODY}}") {
        if let Some(repo) = skill_repo {
            if let Some(entry) = repo.find_by_node_type(&cfg.node_type) {
                if let Ok(skill) = repo.load_skill(&entry.name).await {
                    block = block.replace("{{NODE_GUIDE_BODY}}", skill.body.trim());
                }
            }
        }
        // If still present (load failed or no repo), strip the marker
        block = block.replace("{{NODE_GUIDE_BODY}}", "(guide unavailable)");
    }

    block.push_str("---\nThe tool `");
    block.push_str(&cfg.name);
    block.push_str("` is now available. Call it directly on your next turn.\n");
    block
}

/// Merge `fixed_config` and `node_schema` fixed values into a single object.
/// This is the effective static configuration the builder uses for policy resolution.
fn effective_fixed_config(cfg: &ToolConfiguration) -> serde_json::Value {
    use serde_json::{Map, Value};
    let mut map = Map::new();
    for (k, v) in &cfg.fixed_config {
        map.insert(k.clone(), v.clone());
    }
    if let Some(schema) = &cfg.node_schema {
        for (k, field) in schema {
            if let Some(v) = &field.fixed {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(map)
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
///
/// `skill_repo` and `registry` are optional — when present, `dispatch_describe_tool`
/// can resolve the layer-1 guide body (via `skill_repo`) and call
/// `ExecutableNode::tool_description_supplement` on the live node (via `registry`).
pub async fn dispatch_describe_tool(
    tool_call: &ToolCall,
    lookup: &[ToolConfiguration],
    skill_repo: Option<&dyn SkillRepository>,
    registry: &dyn crate::dag_engine::application::ports::NodeRegistryPort,
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
        Some(c) => {
            let node = registry.get_node(&c.node_type);
            generate_tool_markdown_async(c, node.as_deref(), skill_repo).await
        }
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
            field_type: Some(field_type.to_string()),
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
            field_type: None, // fixed fields don't need `type` anymore
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
            skills: Vec::new(),
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

    use crate::dag_engine::application::ports::NodeRegistryPort;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::llm::domain::tools::{FunctionCall, ToolCall};

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new(DESCRIBE_TOOL_NAME.to_string(), args.to_string()),
        )
    }

    /// Minimal registry stub for tests that don't need a live node.
    struct NullRegistry;
    impl NodeRegistryPort for NullRegistry {
        fn get_node(&self, _node_type: &str) -> Option<std::sync::Arc<dyn ExecutableNode>> {
            None
        }
        fn get_all_nodes(
            &self,
        ) -> std::collections::HashMap<String, std::sync::Arc<dyn ExecutableNode>> {
            std::collections::HashMap::new()
        }
    }

    #[tokio::test]
    async fn dispatch_returns_markdown_for_known_tool() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let lookup = vec![cfg];
        let call = mk_call(json!({ "name": "search_orders" }));
        let r = dispatch_describe_tool(&call, &lookup, None, &NullRegistry)
            .await
            .unwrap();
        assert_eq!(r.tool_name, "search_orders");
        assert!(r.output.contains("# search_orders"));
        assert!(r.output.contains("now available"));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_output() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let lookup = vec![cfg];
        let call = mk_call(json!({ "name": "deleted_tool" }));
        let r = dispatch_describe_tool(&call, &lookup, None, &NullRegistry)
            .await
            .unwrap();
        assert!(r.output.starts_with("Error:"));
        assert!(r.output.contains("not found in catalog"));
    }

    #[tokio::test]
    async fn dispatch_missing_name_arg_is_invalid_tool_call() {
        let cfg = cfg_minimal("search_orders", "Search");
        let lookup = vec![cfg];
        let call = mk_call(json!({}));
        let err = dispatch_describe_tool(&call, &lookup, None, &NullRegistry)
            .await
            .unwrap_err();
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

    #[tokio::test]
    async fn markdown_includes_guide_body_when_node_type_matches() {
        use crate::skills::infrastructure::builtin_skill_repository::BuiltinSkillRepository;
        use std::sync::Arc;
        let mut cfg = cfg_minimal("query_db", "Query the database");
        cfg.node_type = "sql_query".to_string();
        let repo: Arc<dyn crate::skills::domain::skill_repository::SkillRepository> =
            Arc::new(BuiltinSkillRepository::new(&["sql_query-guide".to_string()]).unwrap());
        let md = generate_tool_markdown_async(&cfg, None, Some(repo.as_ref())).await;
        assert!(md.contains("## Best practices"));
        assert!(md.contains("sql_query — best practices"));
        assert!(!md.contains("{{NODE_GUIDE_BODY}}"));
    }

    /// Integration test: `dispatch_describe_tool` loads the guide body via `skill_repo`
    /// when `node_type` is `sql_query` and the builtin skill is in scope.
    /// Uses `NullRegistry` (no live `SqlNode`) so only the guide-body substitution
    /// path is exercised (the `tool_description_supplement` supplement is skipped).
    #[tokio::test]
    async fn dispatch_loads_node_guide_body_for_sql_query() {
        use crate::skills::infrastructure::builtin_skill_repository::BuiltinSkillRepository;
        use std::sync::Arc;

        let repo: Arc<dyn crate::skills::domain::skill_repository::SkillRepository> =
            Arc::new(BuiltinSkillRepository::new(&["sql_query-guide".to_string()]).unwrap());

        let mut cfg = cfg_minimal("query_db", "Query the database");
        cfg.node_type = "sql_query".to_string();
        let lookup = vec![cfg];
        let call = mk_call(json!({ "name": "query_db" }));

        let r = dispatch_describe_tool(&call, &lookup, Some(repo.as_ref()), &NullRegistry)
            .await
            .unwrap();

        assert_eq!(r.tool_name, "query_db");
        assert!(r.output.contains("## Best practices"));
        assert!(r.output.contains("sql_query — best practices"));
        assert!(!r.output.contains("{{NODE_GUIDE_BODY}}"));
    }
}
