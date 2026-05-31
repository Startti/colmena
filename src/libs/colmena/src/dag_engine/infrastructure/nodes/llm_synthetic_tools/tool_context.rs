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

/// Compose the effective fixed config for a tool: `fixed_config` values merged
/// with any `node_schema` fields that carry a `fixed` value.
///
/// This is the canonical config object passed to
/// [`ExecutableNode::tool_description_supplement`] so policy generation sees
/// all operator-set values regardless of which config path was used.
///
/// Shared by [`build_tool_context_block`], `generate_tool_definition`, and the
/// `tool_context_blocks` summary in `llm.rs`.
pub fn build_effective_fixed(cfg: &ToolConfiguration) -> Value {
    let mut map = serde_json::Map::new();
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
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    name, ty, required, desc
                ));
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

/// Build a JSON object summarising which layered context was injected for each
/// tool. Used by `llm.rs` to populate `extra_info["tool_context_blocks"]`.
///
/// Each key is a tool alias. The value object may contain:
/// - `"node_guide"` — name of the node-type guide skill that was attached (if
///   the skill repo resolved one for the tool's `node_type`).
/// - `"policy_lines"` — line count of the policy text emitted by
///   `tool_description_supplement` (present only when the node returns one).
/// - `"scoped_skills"` — array of skill names declared in the tool's `skills`
///   field (present only when non-empty).
///
/// Entries with no interesting information are omitted. The outer object is
/// absent from `extra_info` when no tool has any context to report.
pub fn build_tool_context_blocks_summary(
    tool_configurations: &std::collections::HashMap<String, ToolConfiguration>,
    registry: &dyn crate::dag_engine::application::ports::NodeRegistryPort,
    skill_repo: Option<&dyn SkillRepository>,
) -> serde_json::Value {
    let mut blocks: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

    for (alias, cfg) in tool_configurations {
        let mut entry: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        // node_guide — did the skill repo find a node-type guide?
        if let Some(repo) = skill_repo {
            if let Some(guide) = repo.find_by_node_type(&cfg.node_type) {
                entry.insert(
                    "node_guide".to_string(),
                    serde_json::Value::String(guide.name),
                );
            }
        }

        // policy_lines — how many lines did the policy supplement emit?
        if let Some(node) = registry.get_node(&cfg.node_type) {
            let fixed = build_effective_fixed(cfg);
            if let Some(policy) = node.tool_description_supplement(&fixed) {
                let count = policy.lines().count();
                entry.insert(
                    "policy_lines".to_string(),
                    serde_json::Value::Number(count.into()),
                );
            }
        }

        // scoped_skills — tool-level skill list (layer 2)
        if !cfg.skills.is_empty() {
            entry.insert(
                "scoped_skills".to_string(),
                serde_json::Value::Array(
                    cfg.skills
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }

        if !entry.is_empty() {
            blocks.insert(alias.clone(), serde_json::Value::Object(entry));
        }
    }

    serde_json::Value::Object(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::application::ports::NodeRegistryPort;
    use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
    use crate::dag_engine::domain::observer::ExecutionObserver;
    use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
    use crate::skills::domain::skill_repository::{SkillCatalogEntry, SkillRepository};
    use crate::skills::domain::{Skill, SkillError, SkillReference, SkillSource};
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
        let block =
            build_tool_context_block(&c, &node, &json!({}), None, BlockVariant::EagerOrNonLazy);
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

    // ── Tests for build_tool_context_blocks_summary ──────────────────────────

    /// Minimal registry: always returns the same node for any node_type.
    struct StubRegistry {
        node: Arc<dyn ExecutableNode>,
    }

    impl NodeRegistryPort for StubRegistry {
        fn get_node(&self, _node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            Some(self.node.clone())
        }

        fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    /// Minimal skill repo: returns a single catalog entry keyed by node_type.
    struct StubSkillRepo {
        entry: SkillCatalogEntry,
    }

    #[async_trait::async_trait]
    impl SkillRepository for StubSkillRepo {
        fn list_available(&self) -> Vec<SkillCatalogEntry> {
            vec![self.entry.clone()]
        }

        async fn load_skill(&self, _name: &str) -> Result<Skill, SkillError> {
            Err(SkillError::SkillNotFound("stub".into()))
        }

        async fn load_reference(
            &self,
            _skill_name: &str,
            _reference_name: &str,
        ) -> Result<SkillReference, SkillError> {
            Err(SkillError::SkillNotFound("stub".into()))
        }
    }

    #[test]
    fn summary_empty_when_no_tools() {
        let registry = StubRegistry {
            node: Arc::new(NoopNode { supp: None }),
        };
        let tools: HashMap<String, ToolConfiguration> = HashMap::new();
        let summary = build_tool_context_blocks_summary(&tools, &registry, None);
        // Empty tool map → empty object
        assert_eq!(summary, json!({}));
    }

    #[test]
    fn summary_omits_entry_when_no_context() {
        // A tool with no policy, no guide, no scoped skills produces no entry.
        let registry = StubRegistry {
            node: Arc::new(NoopNode { supp: None }),
        };
        let mut tools = HashMap::new();
        tools.insert("plain_tool".to_string(), cfg("plain_tool", "noop", "desc"));
        let summary = build_tool_context_blocks_summary(&tools, &registry, None);
        assert_eq!(summary, json!({}));
    }

    #[test]
    fn summary_includes_policy_lines_when_supplement_present() {
        let policy = "line1\nline2\nline3";
        let registry = StubRegistry {
            node: Arc::new(NoopNode {
                supp: Some(policy.to_string()),
            }),
        };
        let mut tools = HashMap::new();
        tools.insert("query_db".to_string(), cfg("query_db", "sql_query", "SQL"));
        let summary = build_tool_context_blocks_summary(&tools, &registry, None);
        let policy_lines = summary["query_db"]["policy_lines"].as_u64().unwrap();
        assert_eq!(policy_lines, 3);
    }

    #[test]
    fn summary_includes_node_guide_name_when_repo_matches() {
        let registry = StubRegistry {
            node: Arc::new(NoopNode { supp: None }),
        };
        let skill_repo = StubSkillRepo {
            entry: SkillCatalogEntry {
                name: "sql_query-guide".to_string(),
                description: "Guide for sql_query".to_string(),
                source: SkillSource::Builtin,
                node_type: Some("sql_query".to_string()),
            },
        };
        let mut tools = HashMap::new();
        tools.insert("query_db".to_string(), cfg("query_db", "sql_query", "SQL"));
        let summary = build_tool_context_blocks_summary(&tools, &registry, Some(&skill_repo));
        let guide = summary["query_db"]["node_guide"].as_str().unwrap();
        assert_eq!(guide, "sql_query-guide");
    }

    #[test]
    fn summary_includes_scoped_skills_array() {
        let registry = StubRegistry {
            node: Arc::new(NoopNode { supp: None }),
        };
        let mut c = cfg("query_db", "sql_query", "SQL");
        c.skills = vec!["sales-analysis".to_string(), "expense-analysis".to_string()];
        let mut tools = HashMap::new();
        tools.insert("query_db".to_string(), c);
        let summary = build_tool_context_blocks_summary(&tools, &registry, None);
        let skills = summary["query_db"]["scoped_skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(skills.contains(&"sales-analysis"));
        assert!(skills.contains(&"expense-analysis"));
    }

    #[test]
    fn summary_all_three_fields_combined() {
        let policy = "ALLOW SELECT\nDENY DROP";
        let registry = StubRegistry {
            node: Arc::new(NoopNode {
                supp: Some(policy.to_string()),
            }),
        };
        let skill_repo = StubSkillRepo {
            entry: SkillCatalogEntry {
                name: "sql_query-guide".to_string(),
                description: "Best practices".to_string(),
                source: SkillSource::Builtin,
                node_type: Some("sql_query".to_string()),
            },
        };
        let mut c = cfg("query_db", "sql_query", "SQL tool");
        c.skills = vec!["sales-analysis".to_string()];
        let mut tools = HashMap::new();
        tools.insert("query_db".to_string(), c);
        let summary = build_tool_context_blocks_summary(&tools, &registry, Some(&skill_repo));
        assert_eq!(
            summary["query_db"]["node_guide"].as_str().unwrap(),
            "sql_query-guide"
        );
        assert_eq!(summary["query_db"]["policy_lines"].as_u64().unwrap(), 2);
        let skills = summary["query_db"]["scoped_skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].as_str().unwrap(), "sales-analysis");
    }
}
