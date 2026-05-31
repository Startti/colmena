//! The `load_skill` synthetic tool — builds its ToolDefinition from a SkillRepository's
//! catalog and dispatches tool calls to the repository, formatting output for the LLM.

use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use crate::skills::domain::{SkillError, SkillRepository, SkillSource};
use std::collections::HashMap;
use std::sync::Arc;

pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";

/// Apply layer 1/2/3 visibility rules to produce the `load_skill` catalog
/// for a single LLM request.
///
/// - Layer 1 (`node_type` set) → excluded (auto-folded via `tool_description_supplement`).
/// - Layer 2 (name in any `tool_configuration.skills`) → included only
///   if that parent tool is in `discovered_set`.
/// - Layer 3 (not in any tool's `skills`, `node_type` is None) → always included
///   (free-standing skills declared directly on the llm_call node).
///
/// # Arguments
/// - `catalog` — full list from `repo.list_available()`.
/// - `tool_scoped_names` — flat list of all skill names referenced in any
///   `tool_configuration.skills` (used for membership check).
/// - `free_standing_names` — skill names that belong to the llm_call node
///   directly and are **not** scoped to any tool.
/// - `discovered_set` — tool names already surfaced to the LLM this request
///   (via `describe_tool` or direct invocation in message history).
/// - `scoped_by_tool` — map of `tool_name → Vec<skill_name>` built from
///   `tool_configurations`; used to check whether a scoped skill's parent
///   tool has been discovered.
pub fn filter_visible_skills(
    catalog: &[crate::skills::domain::skill_repository::SkillCatalogEntry],
    tool_scoped_names: &[String],
    free_standing_names: &[String],
    discovered_set: &std::collections::HashSet<String>,
    scoped_by_tool: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<crate::skills::domain::skill_repository::SkillCatalogEntry> {
    let scoped_set: std::collections::HashSet<&str> =
        tool_scoped_names.iter().map(String::as_str).collect();
    let free_set: std::collections::HashSet<&str> =
        free_standing_names.iter().map(String::as_str).collect();

    let mut out = Vec::new();
    for entry in catalog {
        // Layer 1 — auto-folded into tool descriptions, never exposed via load_skill.
        if entry.node_type.is_some() {
            continue;
        }

        if free_set.contains(entry.name.as_str()) {
            // Layer 3 — always included.
            out.push(entry.clone());
            continue;
        }

        if scoped_set.contains(entry.name.as_str()) {
            // Layer 2 — included only if the parent tool has been discovered.
            let visible_now = scoped_by_tool.iter().any(|(tool, skills)| {
                discovered_set.contains(tool) && skills.iter().any(|s| s == &entry.name)
            });
            if visible_now {
                out.push(entry.clone());
            }
        }
    }
    out
}

/// Build the `ToolDefinition` for `load_skill` from an already-filtered slice
/// of catalog entries. This is the shared inner builder used by both the
/// per-request rebuild path (layer-filtered) and the public wrapper.
///
/// Returns `None` when `entries` is empty (callers should skip pushing the
/// tool entirely rather than advertising an empty enum).
pub fn build_load_skill_tool_definition_with_catalog(
    entries: &[crate::skills::domain::skill_repository::SkillCatalogEntry],
) -> Option<ToolDefinition> {
    if entries.is_empty() {
        return None;
    }

    let mut names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    names.sort();
    names.dedup();

    let catalog_lines: Vec<String> = {
        let mut sorted = entries.to_vec();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        sorted
            .iter()
            .map(|e| format!("- {}: {}", e.name, e.description))
            .collect()
    };

    let description = format!(
        "Load a specialized knowledge skill on demand when the user's task benefits from it. \
Call this tool BEFORE responding when you identify that one of the skills below applies. \
You may call it multiple times to load several skills or to load a skill's reference material.\n\n\
Available skills:\n{}\n\n\
After loading a skill, if its content lists available references, you may call load_skill \
again with the `reference` parameter to load that additional material.",
        catalog_lines.join("\n")
    );

    let mut properties: HashMap<String, ParameterProperty> = HashMap::new();
    properties.insert(
        "name".to_string(),
        ParameterProperty::new(
            "string".to_string(),
            "The name of the skill to load".to_string(),
        )
        .with_enum(names),
    );
    properties.insert(
        "reference".to_string(),
        ParameterProperty::new(
            "string".to_string(),
            "Optional name of a reference file within the skill. Only use after loading the \
skill and seeing it declares this reference."
                .to_string(),
        ),
    );

    Some(ToolDefinition {
        name: LOAD_SKILL_TOOL_NAME.to_string(),
        description,
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["name".to_string()],
        },
        input_schema_override: None,
    })
}

/// Build the `ToolDefinition` for `load_skill`. The catalog (skill names +
/// descriptions) is embedded directly in the tool description and the `name`
/// parameter's `enum`, keeping the system_message untouched.
///
/// This is a thin wrapper around [`build_load_skill_tool_definition_with_catalog`]
/// that reads the full unfiltered catalog from the repository. Use the
/// with_catalog variant directly when layer rules should be applied.
pub fn build_load_skill_tool_definition(repository: &Arc<dyn SkillRepository>) -> ToolDefinition {
    let catalog = repository.list_available();
    // Safety: a SkillRepository is never empty at the call site (callers guard
    // with `if let Some(repo) = skill_repo`). Unwrap is safe here; if the
    // catalog somehow is empty we fall back to an empty-description definition.
    build_load_skill_tool_definition_with_catalog(&catalog).unwrap_or_else(|| ToolDefinition {
        name: LOAD_SKILL_TOOL_NAME.to_string(),
        description: "Load a specialized knowledge skill on demand.".to_string(),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties: {
                let mut p = HashMap::new();
                p.insert(
                    "name".to_string(),
                    ParameterProperty::new(
                        "string".to_string(),
                        "The name of the skill to load".to_string(),
                    ),
                );
                p
            },
            required: vec!["name".to_string()],
        },
        input_schema_override: None,
    })
}

/// Dispatch a `load_skill` tool call. Returns the output string to surface to the
/// LLM as a tool result. Also returns observability metadata for event emission.
#[derive(Debug)]
pub struct LoadSkillDispatchResult {
    pub output: String,
    pub skill_name: String,
    pub reference: Option<String>,
    pub source: SkillSource,
    pub size_bytes: usize,
}

pub async fn dispatch_load_skill(
    tool_call: &ToolCall,
    repository: &Arc<dyn SkillRepository>,
) -> Result<LoadSkillDispatchResult, LlmError> {
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
            LlmError::InvalidToolCall {
                reason: format!("load_skill: invalid arguments JSON: {}", e),
            }
        })?;

    let name =
        args.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| LlmError::InvalidToolCall {
                reason: "load_skill: missing required parameter 'name'".to_string(),
            })?;
    let reference = args
        .get("reference")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match reference {
        None => {
            let skill = match repository.load_skill(name).await {
                Ok(s) => s,
                Err(e) => return Ok(skill_error_as_result(name, None, e)),
            };

            let mut output = skill.body.clone();
            if !skill.references.is_empty() {
                output.push_str("\n\n---\n\nAvailable references for this skill:\n");
                for r in &skill.references {
                    output.push_str(&format!("- {}: {}\n", r.name, r.description));
                }
                output.push_str(
                    "\nTo load a reference, call load_skill again with the `reference` parameter.",
                );
            }

            let size_bytes = output.len();
            Ok(LoadSkillDispatchResult {
                output,
                skill_name: name.to_string(),
                reference: None,
                source: skill.source,
                size_bytes,
            })
        }
        Some(ref_name) => {
            let reference_body = match repository.load_reference(name, &ref_name).await {
                Ok(r) => r,
                Err(e) => return Ok(skill_error_as_result(name, Some(&ref_name), e)),
            };
            // Determine source by looking up the main skill (cheap because repos are in-memory).
            let source = repository
                .load_skill(name)
                .await
                .map(|s| s.source)
                .unwrap_or(SkillSource::Builtin);
            let size_bytes = reference_body.body.len();
            Ok(LoadSkillDispatchResult {
                output: reference_body.body,
                skill_name: name.to_string(),
                reference: Some(ref_name),
                source,
                size_bytes,
            })
        }
    }
}

/// Convert a SkillError into a LoadSkillDispatchResult whose `output` is a plain
/// error string the LLM can read. Errors are intentionally returned as tool output
/// (not propagated as LlmError) so the ReAct loop continues and the LLM can recover.
fn skill_error_as_result(
    name: &str,
    reference: Option<&str>,
    err: SkillError,
) -> LoadSkillDispatchResult {
    let output = format!("Error: {}", err);
    let size_bytes = output.len();
    LoadSkillDispatchResult {
        output,
        skill_name: name.to_string(),
        reference: reference.map(|s| s.to_string()),
        source: SkillSource::Builtin, // placeholder; the actual source doesn't matter on error
        size_bytes,
    }
}

/// Convenience: turn a LoadSkillDispatchResult into a ToolResult for the ReAct loop.
pub fn into_tool_result(call_id: &str, r: &LoadSkillDispatchResult) -> ToolResult {
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
    use crate::llm::domain::tools::{FunctionCall, ToolCall};
    use crate::skills::domain::{
        Skill, SkillCatalogEntry, SkillError, SkillReference, SkillReferenceMeta, SkillSource,
    };
    use async_trait::async_trait;

    struct FakeRepo;

    #[async_trait]
    impl SkillRepository for FakeRepo {
        fn list_available(&self) -> Vec<SkillCatalogEntry> {
            vec![
                SkillCatalogEntry {
                    name: "python-expert".to_string(),
                    description: "Python stuff".to_string(),
                    source: SkillSource::Builtin,
                    node_type: None,
                },
                SkillCatalogEntry {
                    name: "sql-optimizer".to_string(),
                    description: "SQL stuff".to_string(),
                    source: SkillSource::Builtin,
                    node_type: None,
                },
            ]
        }
        async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
            match name {
                "python-expert" => Ok(Skill {
                    name: "python-expert".to_string(),
                    description: "Python stuff".to_string(),
                    body: "# Python expert\nuse type hints\n".to_string(),
                    references: vec![SkillReferenceMeta {
                        name: "frameworks".to_string(),
                        description: "Django + FastAPI".to_string(),
                    }],
                    source: SkillSource::Builtin,
                    node_type: None,
                }),
                _ => Err(SkillError::SkillNotFound(name.to_string())),
            }
        }
        async fn load_reference(
            &self,
            skill_name: &str,
            reference_name: &str,
        ) -> Result<SkillReference, SkillError> {
            if skill_name == "python-expert" && reference_name == "frameworks" {
                Ok(SkillReference {
                    skill_name: "python-expert".to_string(),
                    reference_name: "frameworks".to_string(),
                    body: "Django is a web framework.".to_string(),
                })
            } else {
                Err(SkillError::ReferenceNotDeclared {
                    skill: skill_name.to_string(),
                    reference: reference_name.to_string(),
                    available: "frameworks".to_string(),
                })
            }
        }
    }

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: LOAD_SKILL_TOOL_NAME.to_string(),
                arguments: args.to_string(),
            },
            response: None,
        }
    }

    #[test]
    fn tool_definition_includes_all_skill_names_in_enum() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let td = build_load_skill_tool_definition(&repo);
        let name_prop = td.parameters.properties.get("name").unwrap();
        let enum_values = name_prop.enum_values.as_ref().unwrap();
        assert!(enum_values.contains(&"python-expert".to_string()));
        assert!(enum_values.contains(&"sql-optimizer".to_string()));
    }

    #[test]
    fn tool_definition_description_lists_each_skill() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let td = build_load_skill_tool_definition(&repo);
        assert!(td.description.contains("python-expert"));
        assert!(td.description.contains("Python stuff"));
        assert!(td.description.contains("sql-optimizer"));
    }

    #[tokio::test]
    async fn dispatch_load_by_name_returns_body_and_references_block() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({"name": "python-expert"}));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert!(r.output.contains("use type hints"));
        assert!(r.output.contains("Available references"));
        assert!(r.output.contains("frameworks"));
        assert_eq!(r.reference, None);
        assert!(matches!(r.source, SkillSource::Builtin));
    }

    #[tokio::test]
    async fn dispatch_load_reference_returns_reference_body() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({
            "name": "python-expert",
            "reference": "frameworks"
        }));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert_eq!(r.output, "Django is a web framework.");
        assert_eq!(r.reference, Some("frameworks".to_string()));
    }

    #[tokio::test]
    async fn dispatch_missing_skill_returns_error_output() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({"name": "does-not-exist"}));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert!(r.output.starts_with("Error:"));
    }

    #[tokio::test]
    async fn dispatch_undeclared_reference_returns_error_output() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({
            "name": "python-expert",
            "reference": "nope"
        }));
        let r = dispatch_load_skill(&call, &repo).await.unwrap();
        assert!(r.output.starts_with("Error:"));
        assert!(r.output.contains("frameworks"));
    }

    #[tokio::test]
    async fn dispatch_missing_name_parameter_is_invalid_tool_call() {
        let repo: Arc<dyn SkillRepository> = Arc::new(FakeRepo);
        let call = mk_call(serde_json::json!({"reference": "x"}));
        let err = dispatch_load_skill(&call, &repo).await.unwrap_err();
        assert!(matches!(err, LlmError::InvalidToolCall { .. }));
    }

    #[test]
    fn into_tool_result_success_on_normal_output() {
        let r = LoadSkillDispatchResult {
            output: "content".to_string(),
            skill_name: "x".to_string(),
            reference: None,
            source: SkillSource::Builtin,
            size_bytes: 7,
        };
        let tr = into_tool_result("call_1", &r);
        assert!(tr.success);
    }

    #[test]
    fn into_tool_result_failure_on_error_prefix() {
        let r = LoadSkillDispatchResult {
            output: "Error: something".to_string(),
            skill_name: "x".to_string(),
            reference: None,
            source: SkillSource::Builtin,
            size_bytes: 16,
        };
        let tr = into_tool_result("call_1", &r);
        assert!(!tr.success);
    }

    // ---- filter_visible_skills tests -----------------------------------------------

    #[test]
    fn build_filtered_definition_excludes_layer1_guides() {
        use crate::skills::domain::skill_repository::SkillCatalogEntry;
        let catalog = vec![
            SkillCatalogEntry {
                name: "guide".into(),
                description: "g".into(),
                source: SkillSource::Builtin,
                node_type: Some("sql_query".into()),
            },
            SkillCatalogEntry {
                name: "free".into(),
                description: "f".into(),
                source: SkillSource::Builtin,
                node_type: None,
            },
        ];
        let visible = filter_visible_skills(
            &catalog,
            /*tool_scoped*/ &[],
            /*free_standing*/ &["free".to_string()],
            /*discovered_set*/ &std::collections::HashSet::new(),
            /*scoped_by_tool*/ &std::collections::HashMap::new(),
        );
        let names: Vec<&str> = visible.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["free"]);
    }

    #[test]
    fn build_filtered_definition_includes_scoped_only_after_discovery() {
        use crate::skills::domain::skill_repository::SkillCatalogEntry;
        let catalog = vec![SkillCatalogEntry {
            name: "sales".into(),
            description: "s".into(),
            source: SkillSource::Builtin,
            node_type: None,
        }];
        let mut scoped_by_tool = std::collections::HashMap::new();
        scoped_by_tool.insert("consultar_ventas".to_string(), vec!["sales".to_string()]);

        let pre_discovery = filter_visible_skills(
            &catalog,
            &["sales".to_string()],
            &[],
            &std::collections::HashSet::new(),
            &scoped_by_tool,
        );
        assert!(pre_discovery.is_empty());

        let mut discovered = std::collections::HashSet::new();
        discovered.insert("consultar_ventas".to_string());
        let post_discovery = filter_visible_skills(
            &catalog,
            &["sales".to_string()],
            &[],
            &discovered,
            &scoped_by_tool,
        );
        assert_eq!(post_discovery.len(), 1);
        assert_eq!(post_discovery[0].name, "sales");
    }

    #[test]
    fn build_load_skill_tool_definition_with_catalog_none_on_empty() {
        let result = build_load_skill_tool_definition_with_catalog(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn build_load_skill_tool_definition_with_catalog_builds_correct_enum() {
        use crate::skills::domain::skill_repository::SkillCatalogEntry;
        let entries = vec![
            SkillCatalogEntry {
                name: "beta".into(),
                description: "Beta skill".into(),
                source: SkillSource::Builtin,
                node_type: None,
            },
            SkillCatalogEntry {
                name: "alpha".into(),
                description: "Alpha skill".into(),
                source: SkillSource::Builtin,
                node_type: None,
            },
        ];
        let td = build_load_skill_tool_definition_with_catalog(&entries).unwrap();
        let name_prop = td.parameters.properties.get("name").unwrap();
        let enum_values = name_prop.enum_values.as_ref().unwrap();
        // sorted
        assert_eq!(enum_values, &vec!["alpha".to_string(), "beta".to_string()]);
    }
}
