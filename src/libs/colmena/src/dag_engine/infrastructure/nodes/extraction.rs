use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use super::task_mutations;
use crate::llm::domain::ProviderKind;

/// Default system prompt template for ExtractionNode.
/// Uses `{user_instructions}` and `{schema}` as placeholders.
const DEFAULT_EXTRACTION_SYSTEM_MSG: &str =
    include_str!("../../../../text/prompts/extraction_system.md");

pub struct ExtractionNode {
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}

impl ExtractionNode {
    pub fn new(
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
    ) -> Self {
        Self { task_memory_repo }
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with("}") {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("Environment variable {} not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[async_trait]
impl ExecutableNode for ExtractionNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // --- 1. Resolve Provider Configuration ---
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'provider' in config")?;

        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("Invalid provider '{}'.", provider_str).into()),
        };

        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;

        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Verbose flag for debugging.
        let verbose = config
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // --- 2. Resolve Schema & Build System Message ---
        let schema = config.get("schema").ok_or("Missing 'schema' in config")?;

        let user_system_message = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
            .unwrap_or("");

        let user_system_message_section = if !user_system_message.is_empty() {
            format!(
                "\n\nContext/Rules for extraction:\n{}\n",
                user_system_message
            )
        } else {
            String::new()
        };

        let system_message = DEFAULT_EXTRACTION_SYSTEM_MSG
            .replace("{user_instructions}", &user_system_message_section)
            .replace("{schema}", &serde_json::to_string_pretty(schema)?);

        // --- 3. Gather and Format Texts ---
        let mut formatted_texts = String::new();

        // The DAG engine flattens input paths (e.g. from edge.to = "extract_info.texts.slack_message")
        // So we look for any input key that starts with "texts."
        for (key, val) in inputs {
            if let Some(text_name) = key.strip_prefix("texts.") {
                let text_str = match val {
                    Value::String(s) => s.clone(),
                    Value::Null => continue, // Ignore explicitly null values
                    _ => val.to_string(),    // Serialize objects or numbers to string
                };

                // If it serialized a string with quotes (e.g., "Hello"), strip them
                let clean_text = if text_str.starts_with('"') && text_str.ends_with('"') {
                    text_str[1..text_str.len() - 1].to_string()
                } else {
                    text_str
                };

                formatted_texts.push_str(&format!("# {}\n\n{}\n\n", text_name, clean_text));
            }
        }

        // Also support static config
        if let Some(texts_obj) = config.get("texts").and_then(|v| v.as_object()) {
            for (key, val) in texts_obj {
                if let Some(text_str) = val.as_str() {
                    formatted_texts.push_str(&format!("# {}\n\n{}\n\n", key, text_str));
                }
            }
        }

        if formatted_texts.is_empty() {
            colmena_log!(
                "⚠️ [ExtractionNode] Skipped execution because 'texts' input was missing or empty."
            );
            return Ok(Value::Null);
        }

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🔍 [ExtractionNode] VERBOSE — System Prompt:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", system_message);
            colmena_log!("───────────────────────────────────────");
            colmena_log!("Texts:\n{}", formatted_texts);
            colmena_log!("═══════════════════════════════════════\n");
        }

        // --- 4 + 5. Call LLM and parse via shared helper ---
        use crate::dag_engine::infrastructure::nodes::util::extract_with_schema::{
            extract_with_schema, ExtractInput,
        };
        let empty_schema = json!({});
        let parsed_json = extract_with_schema(ExtractInput {
            provider_kind: provider_kind.clone(),
            api_key: api_key.clone(),
            model: model.clone(),
            system_message: system_message.clone(),
            user_text: formatted_texts.clone(),
            // ExtractionNode does not validate against an inline schema; pass
            // an empty schema object so the validator is a no-op.
            inline_schema: &empty_schema,
            temperature: Some(0.1),
            observer: _observer.clone(),
        })
        .await?;

        // Structured event: safe metadata only — never the raw parsed
        // output (LLM-generated structured data). See
        // docs/developer_guide/50_logging_and_observability.md. The raw
        // parsed JSON goes through `payload_trace!`, double-gated
        // (EnvFilter directive AND `COLMENA_LOG_PAYLOADS`) — unconditional
        // on `verbose` now that `tracing`'s own `EnvFilter` governs
        // visibility.
        tracing::debug!(
            target: crate::dag_engine::log_policy::T_EXTRACTION,
            field_count = parsed_json.as_object().map(|o| o.len()).unwrap_or(0),
            "extraction node parsed the llm output"
        );
        crate::dag_engine::log_policy::payload_trace!(
            extraction_result,
            parsed = %serde_json::to_string_pretty(&parsed_json).unwrap_or_default()
        );
        if verbose {
            colmena_log!(
                "🔍 [ExtractionNode] VERBOSE — parsed output ({} fields)",
                parsed_json.as_object().map(|o| o.len()).unwrap_or(0)
            );
        }

        let session_id = _state
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_run")
            .to_string();

        if let Some(repo) = &self.task_memory_repo {
            // Apply the critic's modifications, then read the updated list back
            // for the next nodes. Both steps propagate their failures: a task
            // that was not written, or a list that could not be read, must never
            // reach the orchestrator dressed up as a successful empty result.
            let skipped_deletes = task_mutations::apply_critic_mutations(
                repo,
                &session_id,
                parsed_json.get("add_tasks"),
                parsed_json.get("delete_tasks"),
            )
            .await?;
            if !skipped_deletes.is_empty() {
                tracing::warn!(
                    target: "colmena::extraction",
                    skipped_deletes = ?skipped_deletes,
                    "critic asked to delete task ids that are not valid identifiers"
                );
            }

            let all_tasks_json = task_mutations::fetch_session_tasks(repo, &session_id).await?;

            // Check if we need to suspend
            let suspend = parsed_json
                .get("suspend")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if suspend {
                let mut extra_info = json!({
                    "__colmena_status": "SUSPENDED",
                    "all_tasks": all_tasks_json
                });
                if !skipped_deletes.is_empty() {
                    extra_info["skipped_deletes"] = json!(skipped_deletes);
                }
                return Ok(json!({
                    "result": parsed_json.clone(),
                    "extra_info": extra_info
                }));
            }

            if !skipped_deletes.is_empty() {
                return Ok(json!({
                    "result": parsed_json,
                    "extra_info": { "skipped_deletes": skipped_deletes }
                }));
            }
        }

        Ok(json!({
            "result": parsed_json,
            "extra_info": {}
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Extracts structured information from unstructured text based on a provided JSON schema using an LLM.")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "information_extraction",
            "config": {
                "provider": "string",
                "api_key": "string",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "schema": "object"
            },
            "inputs": {
                "texts": "object mapping names to text strings",
                "system_message": "string (optional)"
            },
            "outputs": {
                "result": "object — the extracted fields, shaped by the configured `schema` (also the default_output)",
                "extra_info": "object — empty on the normal path; when the extraction requests suspension it carries `__colmena_status: \"SUSPENDED\"` and `all_tasks` (the updated task list); carries `skipped_deletes` (array of ids) when the critic named a `delete_tasks` id that is not a valid identifier"
            }
        })
    }
}
