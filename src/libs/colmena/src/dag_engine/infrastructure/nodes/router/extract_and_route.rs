//! Mode B (extract + rules) — the LLM extracts a JSON object against the
//! user's schema; declarative `when` rules walk branches in declaration
//! order and return the first match.

use serde_json::Value;
use std::sync::Arc;

use super::config::RouterConfig;
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::util::extract_with_schema::{
    extract_with_schema, ExtractInput,
};
use crate::dag_engine::infrastructure::nodes::util::inline_schema::inline_to_json_schema;
use crate::llm::domain::ProviderKind;

const EXTRACTION_SYSTEM_MSG: &str = include_str!("../../../../../text/prompts/extraction_system.md");

/// Returns (branch_index, extracted_json) when a branch matches.
/// On no-match, returns an error that includes the extracted JSON for diagnostics.
pub async fn pick_branch(
    cfg: &RouterConfig,
    provider_kind: ProviderKind,
    api_key: String,
    model: Option<String>,
    user_text: String,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<(usize, Value), Box<dyn std::error::Error + Send + Sync>> {
    let inline_schema = cfg.inline_schema.as_ref().ok_or(
        "Router(mode B): inline schema missing — config validation should have caught this",
    )?;
    let json_schema = inline_to_json_schema(inline_schema)?;

    let instructions_section = match &cfg.instructions {
        Some(s) if !s.is_empty() => format!("\n\nContext/Rules for extraction:\n{}\n", s),
        _ => String::new(),
    };
    let system_message = EXTRACTION_SYSTEM_MSG
        .replace("{user_instructions}", &instructions_section)
        .replace("{schema}", &serde_json::to_string_pretty(&json_schema)?);

    let extracted = extract_with_schema(ExtractInput {
        provider_kind,
        api_key,
        model,
        system_message,
        user_text,
        inline_schema,
        temperature: Some(0.1),
        observer,
    })
    .await?;

    for (idx, b) in cfg.branches.iter().enumerate() {
        if let Some(rule) = &b.when {
            if rule.evaluate(&extracted) {
                return Ok((idx, extracted));
            }
        }
    }

    Err(format!(
        "RouterRuntimeError: no branch matched. extracted: {}",
        serde_json::to_string(&extracted).unwrap_or_default()
    )
    .into())
}
