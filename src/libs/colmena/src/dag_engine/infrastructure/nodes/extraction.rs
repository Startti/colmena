use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use crate::llm::domain::ProviderKind;

/// Default system prompt template for ExtractionNode.
/// Uses `{user_instructions}` and `{schema}` as placeholders.
const DEFAULT_EXTRACTION_SYSTEM_MSG: &str = include_str!("../../../../text/prompts/extraction_system.md");

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

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🔍 [ExtractionNode] VERBOSE — Parsed Output:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!(
                "{}",
                serde_json::to_string_pretty(&parsed_json).unwrap_or_default()
            );
            colmena_log!("═══════════════════════════════════════\n");
        }

        let session_id = _state
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_run")
            .to_string();

        if let Some(repo) = &self.task_memory_repo {
            // Process Critic modifications (Add tasks)
            if let Some(add_array) = parsed_json.get("add_tasks").and_then(|v| v.as_array()) {
                for task_val in add_array {
                    if let Some(task_obj) = task_val.as_object() {
                        let task_name = task_obj
                            .get("task")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown")
                            .to_string();
                        let assigned_to = task_obj
                            .get("assigned_to")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown")
                            .to_string();

                        let new_task = crate::dag_engine::domain::state::DagTask {
                            id: uuid::Uuid::new_v4().to_string(),
                            session_id: session_id.clone(),
                            task_name,
                            assigned_to,
                            completed: false,
                            result: None,
                            phase: 1,
                            parallel: false,
                            context: None,
                            is_bridge: false,
                        };
                        repo.add_task(&new_task).await?;
                    }
                }
            }

            // Process Critic modifications (Delete tasks)
            if let Some(delete_array) = parsed_json.get("delete_tasks").and_then(|v| v.as_array()) {
                for id_val in delete_array {
                    if let Some(id_str) = id_val.as_str() {
                        let _ = repo.delete_task(id_str).await;
                    }
                }
            }

            // Generate updated tasks list for next nodes
            let mut all_tasks_json = Vec::new();
            if let Ok(tasks) = repo.get_tasks_for_run(&session_id).await {
                for t in tasks {
                    all_tasks_json.push(json!({
                        "id": t.id,
                        "task_name": t.task_name,
                        "assigned_to": t.assigned_to,
                        "completed": t.completed,
                        "result": t.result
                    }));
                }
            }

            // Check if we need to suspend
            let suspend = parsed_json
                .get("suspend")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if suspend {
                return Ok(json!({
                    "result": parsed_json.clone(),
                    "extra_info": {
                        "__colmena_status": "SUSPENDED",
                        "all_tasks": all_tasks_json
                    }
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
                "extracted field": "varies according to schema"
            }
        })
    }
}
