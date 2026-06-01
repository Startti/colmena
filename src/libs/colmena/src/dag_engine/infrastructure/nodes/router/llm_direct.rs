//! Mode A (LLM-direct) — the LLM picks a branch name from the declared
//! descriptions via a synthetic single-field enum schema.

use serde_json::json;
use std::sync::Arc;

use super::config::{BranchConfig, RouterConfig};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::util::extract_with_schema::{
    extract_with_schema, ExtractInput,
};
use crate::llm::domain::ProviderKind;

const ROUTING_SYSTEM_MSG: &str = include_str!("../prompts/routing_classifier_system.md");

/// Picks the winning branch for mode A and returns (branch_index, llm_reason).
pub async fn pick_branch(
    cfg: &RouterConfig,
    provider_kind: ProviderKind,
    api_key: String,
    model: Option<String>,
    user_text: String,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<(usize, String), Box<dyn std::error::Error + Send + Sync>> {
    // Build the enum of valid names + the bullet-list prompt context.
    let names: Vec<String> = cfg.branches.iter().map(|b| b.name.clone()).collect();
    let bullets: String = cfg
        .branches
        .iter()
        .map(|b: &BranchConfig| {
            format!(
                "- {}: {}",
                b.name,
                b.description.as_deref().unwrap_or("(no description)")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Schema fed to extract_with_schema for the LLM's structured reply.
    // We use an inline schema so the same validator catches off-enum answers.
    let inline_schema = json!({
        "branch": {
            "type": "string",
            "required": true,
            "description": format!("must be one of: {}", names.join(", "))
        },
        "reason": { "type": "string", "required": false }
    });
    let json_schema =
        crate::dag_engine::infrastructure::nodes::util::inline_schema::inline_to_json_schema(
            &inline_schema,
        )?;

    let instructions_section = match &cfg.instructions {
        Some(s) if !s.is_empty() => format!("\n\nAdditional rules:\n{}\n", s),
        _ => String::new(),
    };
    let system_message = ROUTING_SYSTEM_MSG
        .replace("{branches}", &bullets)
        .replace("{user_instructions}", &instructions_section)
        .replace("{schema}", &serde_json::to_string_pretty(&json_schema)?);

    let parsed = extract_with_schema(ExtractInput {
        provider_kind,
        api_key,
        model,
        system_message,
        user_text,
        inline_schema: &inline_schema,
        temperature: Some(0.1),
        observer,
    })
    .await?;

    let chosen = parsed
        .get("branch")
        .and_then(|v| v.as_str())
        .ok_or("RouterRuntimeError: llm response missing 'branch' field")?
        .to_string();
    let reason = parsed
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let idx = cfg
        .branches
        .iter()
        .position(|b| b.name == chosen)
        .ok_or_else(|| format!("RouterRuntimeError: llm picked unknown branch '{}'", chosen))?;
    Ok((idx, reason))
}
