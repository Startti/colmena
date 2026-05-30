//! Tool context block builder — assembles the layered context (description +
//! policy + node-type guide + parameters + scoped skills announcement) into a
//! single markdown string. Used at two injection points:
//!   - `generate_tool_markdown` (lazy describe_tool path) → Lazy variant.
//!   - `generate_tool_definition` (eager / non-lazy path) → EagerOrNonLazy.
//!
//! Pure function — no I/O. Each section is omitted when its input is empty.

use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::tool_configuration::{NodeSchemaField, ToolConfiguration};
use crate::skills::domain::skill_repository::SkillRepository;
use serde_json::Value;

/// Which variant of the block to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockVariant {
    /// For describe_tool (lazy): includes the Parameters table.
    Lazy,
    /// For ToolDefinition.description (eager or non-lazy): omits Parameters
    /// because the schema travels typed.
    EagerOrNonLazy,
}

/// Assemble the layered tool context block for a given tool configuration.
///
/// Sections included (each omitted when its input is empty/None):
/// 1. Header: `# <name>` + description (always present)
/// 2. Access policy (from `node.tool_description_supplement`)
/// 3. Best practices / node-type guide marker (from `skill_repo.find_by_node_type`)
/// 4. Parameters table (Lazy variant only)
/// 5. Related knowledge / scoped skills announcement
pub fn build_tool_context_block(
    cfg: &ToolConfiguration,
    node: &dyn ExecutableNode,
    fixed_config_effective: &Value,
    skill_repo: Option<&dyn SkillRepository>,
    variant: BlockVariant,
) -> String {
    let mut out = String::new();

    // Header: name + description (always)
    out.push_str(&format!("# {}\n\n", cfg.name));
    out.push_str(cfg.description.trim());
    out.push_str("\n\n");

    // Layer 1 — policy (from node)
    if let Some(policy) = node.tool_description_supplement(fixed_config_effective) {
        out.push_str("## Access policy\n\n");
        out.push_str(policy.trim());
        out.push_str("\n\n");
    }

    // Layer 1 — node-type guide (from skill repo)
    if let Some(repo) = skill_repo {
        if let Some(guide_entry) = repo.find_by_node_type(&cfg.node_type) {
            // Body is loaded lazily — for now we expose name + description in
            // the block header and a separator. Full body is fetched async via
            // SkillRepository::load_skill at the call site (since this fn is
            // sync). Implementation note: the caller passes the resolved body
            // via the wrapper used in describe_tool / generate_tool_definition.
            //
            // Here we render the header section and a placeholder marker that
            // the wrapper replaces with the markdown body. Keeps this function
            // sync and pure.
            out.push_str("## Best practices\n\n");
            out.push_str(&format!("<!-- node-type guide: {} -->\n", guide_entry.name));
            out.push_str("{{NODE_GUIDE_BODY}}\n\n");
        }
    }

    // Parameters (Lazy variant only)
    if variant == BlockVariant::Lazy {
        let visible = collect_visible_fields(cfg);
        out.push_str("## Parameters\n\n");
        if visible.is_empty() {
            out.push_str(
                "No parameter schema declared — pass arguments as a free-form JSON object that matches the tool's expectations.\n\n",
            );
        } else {
            out.push_str("| Name | Type | Required | Description |\n");
            out.push_str("|------|------|----------|-------------|\n");
            for (name, field) in &visible {
                let ty = field.field_type.as_deref().unwrap_or("unknown");
                let required = if field.required.unwrap_or(false) {
                    "yes"
                } else {
                    "no"
                };
                let desc = field.description.as_deref().unwrap_or("");
                out.push_str(&format!("| {} | {} | {} | {} |\n", name, ty, required, desc));
            }
            out.push('\n');
        }
    }

    // Layer 2 announcement
    if !cfg.skills.is_empty() {
        out.push_str("## Related knowledge\n\n");
        out.push_str("Load with `load_skill(name)` when your task matches:\n");
        if let Some(repo) = skill_repo {
            for skill_name in &cfg.skills {
                let desc = repo
                    .list_available()
                    .into_iter()
                    .find(|e| e.name == *skill_name)
                    .map(|e| e.description)
                    .unwrap_or_else(|| "(description unavailable)".to_string());
                out.push_str(&format!("- {}: {}\n", skill_name, desc));
            }
        } else {
            for skill_name in &cfg.skills {
                out.push_str(&format!("- {}\n", skill_name));
            }
        }
        out.push('\n');
    }

    out
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
    use crate::dag_engine::domain::observer::ExecutionObserver;
    use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct NoopNode {
        supp: Option<String>,
    }

    #[async_trait::async_trait]
    impl ExecutableNode for NoopNode {
        async fn execute(
            &self,
            _inputs: &NodeInputs,
            _config: &Value,
            _state: &mut Value,
            _observer: Option<Arc<dyn ExecutionObserver>>,
        ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            Ok(json!({}))
        }

        fn schema(&self) -> Value {
            json!({})
        }

        fn tool_description_supplement(&self, _fixed_config: &Value) -> Option<String> {
            self.supp.clone()
        }
    }

    fn cfg(name: &str, node_type: &str, description: &str) -> ToolConfiguration {
        ToolConfiguration {
            name: name.to_string(),
            description: description.to_string(),
            node_type: node_type.to_string(),
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
    fn minimal_block_only_header_and_description() {
        let node = NoopNode { supp: None };
        let block = build_tool_context_block(
            &cfg("t", "noop", "Tool desc"),
            &node,
            &json!({}),
            None,
            BlockVariant::EagerOrNonLazy,
        );
        assert!(block.contains("# t"));
        assert!(block.contains("Tool desc"));
        assert!(!block.contains("Access policy"));
        assert!(!block.contains("Best practices"));
        assert!(!block.contains("Parameters"));
        assert!(!block.contains("Related knowledge"));
    }

    #[test]
    fn policy_section_present_when_supplement_some() {
        let node = NoopNode {
            supp: Some("POLICY".to_string()),
        };
        let block = build_tool_context_block(
            &cfg("t", "noop", "Tool"),
            &node,
            &json!({}),
            None,
            BlockVariant::EagerOrNonLazy,
        );
        assert!(block.contains("## Access policy"));
        assert!(block.contains("POLICY"));
    }

    #[test]
    fn related_knowledge_lists_scoped_skills() {
        let mut c = cfg("consultar_ventas", "sql_query", "Sales tool");
        c.skills = vec!["sales-analysis".to_string()];
        let node = NoopNode { supp: None };
        let block = build_tool_context_block(
            &c,
            &node,
            &json!({}),
            None,
            BlockVariant::EagerOrNonLazy,
        );
        assert!(block.contains("## Related knowledge"));
        assert!(block.contains("sales-analysis"));
    }

    #[test]
    fn parameters_section_only_in_lazy_variant() {
        let node = NoopNode { supp: None };
        let block_lazy = build_tool_context_block(
            &cfg("t", "noop", "T"),
            &node,
            &json!({}),
            None,
            BlockVariant::Lazy,
        );
        let block_eager = build_tool_context_block(
            &cfg("t", "noop", "T"),
            &node,
            &json!({}),
            None,
            BlockVariant::EagerOrNonLazy,
        );
        assert!(block_lazy.contains("## Parameters"));
        assert!(!block_eager.contains("## Parameters"));
    }
}
