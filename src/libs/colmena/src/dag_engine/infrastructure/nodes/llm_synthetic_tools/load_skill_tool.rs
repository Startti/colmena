//! The `load_skill` synthetic tool — builds its ToolDefinition from a SkillRepository's
//! catalog and dispatches tool calls to the repository, formatting output for the LLM.

use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use crate::skills::domain::{SkillError, SkillRepository, SkillSource};
use std::collections::HashMap;
use std::sync::Arc;

pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";

/// Build the `ToolDefinition` for `load_skill`. The catalog (skill names +
/// descriptions) is embedded directly in the tool description and the `name`
/// parameter's `enum`, keeping the system_message untouched.
pub fn build_load_skill_tool_definition(repository: &Arc<dyn SkillRepository>) -> ToolDefinition {
    let catalog = repository.list_available();

    let mut names: Vec<String> = catalog.iter().map(|e| e.name.clone()).collect();
    names.sort();
    names.dedup();

    let catalog_lines: Vec<String> = {
        let mut sorted = catalog.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        sorted
            .iter()
            .map(|e| format!("- {}: {}", e.name, e.description))
            .collect()
    };

    let description = format!(
        "{}{}",
        crate::text::tool_description(LOAD_SKILL_TOOL_NAME),
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
            "Optional reference name within the skill. Use 'a/b' for nested.".to_string(),
        ),
    );

    ToolDefinition {
        name: LOAD_SKILL_TOOL_NAME.to_string(),
        description,
        summary: Some(crate::text::tool_summary(LOAD_SKILL_TOOL_NAME).to_string()),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["name".to_string()],
        },
        input_schema_override: None,
    }
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
                },
                SkillCatalogEntry {
                    name: "sql-optimizer".to_string(),
                    description: "SQL stuff".to_string(),
                    source: SkillSource::Builtin,
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
                        references: vec![],
                    }],
                    source: SkillSource::Builtin,
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
}
