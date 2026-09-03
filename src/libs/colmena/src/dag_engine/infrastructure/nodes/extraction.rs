use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use super::task_mutations;
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
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

        // Whether the graph author DECLARED a text source at all, independently of
        // what that source resolved to. The two are not the same thing: a correctly
        // wired `texts.` edge can legitimately carry a null (an `http_request` that
        // answers 204, say), and that is a data condition, not a misconfiguration.
        let mut source_declared = config.get("texts").is_some();

        // The DAG engine flattens input paths (e.g. from edge.to = "extract_info.texts.slack_message")
        // So we look for any input key that starts with "texts."
        for (key, val) in inputs {
            if let Some(text_name) = key.strip_prefix("texts.") {
                source_declared = true;
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
            if source_declared {
                // A text source WAS wired, it just resolved to nothing. Keep the
                // pre-existing behaviour: report null and let the engine skip the
                // downstream branch. Failing here would break correctly-wired
                // graphs whose upstream legitimately produced no content, and the
                // wiring advice below would be actively wrong for them.
                colmena_log!(
                    "⚠️ [ExtractionNode] Skipped execution: every declared text source resolved to null or empty."
                );
                return Ok(Value::Null);
            }

            // No source declared at all — a misconfiguration. Fail loudly instead
            // of returning `Ok(Value::Null)`. A null return reported SUCCESS for
            // work that never happened: the engine then emitted `node-skipped`
            // with `reason: upstream_null_output` for the whole downstream branch,
            // and the only trace was a `colmena_log!` invisible without verbose
            // logging. Nothing reached the SSE stream.
            //
            // The list below carries only the keys a graph author actually wrote. The engine also injects
            // `__colmena_*` / `__node_id` / `__graph_nodes` / `session_id` plumbing,
            // and listing those buries the one line the operator needs to act on.
            let mut received: Vec<&str> = inputs
                .keys()
                .filter(|k| !k.starts_with("__") && k.as_str() != "session_id")
                .map(|k| k.as_str())
                .collect();
            received.sort_unstable();
            let received_list = if received.is_empty() {
                "<none>".to_string()
            } else {
                received.join(", ")
            };

            return Err(format!(
                "[information_extraction] no text sources to extract from. This node reads its \
                 documents ONLY from inputs whose key starts with `texts.` (wire the edge as \
                 {{\"from\": \"<source_node>\", \"to\": \"<this_node>.texts.<name>\"}} — a plain \
                 {{\"to\": \"<this_node>\"}} does NOT work) or from a static `config.texts` object. \
                 Input keys received: [{received_list}]. Config `texts` present: {has_config_texts}.",
                received_list = received_list,
                has_config_texts = config.get("texts").is_some(),
            )
            .into());
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

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        Some(
            NodeCatalogEntry::no_config()
                .with_field(
                    "provider",
                    FieldSpec::of_type("string").required().valid_values([
                        "openai".into(),
                        "google".into(),
                        "anthropic".into(),
                    ]),
                )
                .with_field("api_key", FieldSpec::of_type("string").required())
                .with_field("model", FieldSpec::of_type("string"))
                .with_field("schema", FieldSpec::of_type("object").required())
                .with_field("system_message", FieldSpec::of_type("string"))
                .with_field("texts", FieldSpec::of_type("object"))
                .with_field("verbose", FieldSpec::of_type("boolean")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a config that is valid in every respect EXCEPT the text sources,
    /// so `execute` reaches the `formatted_texts.is_empty()` guard without ever
    /// touching the network.
    fn config_without_texts() -> Value {
        json!({
            "provider": "openai",
            "api_key": "sk-not-used-the-guard-fires-first",
            "schema": { "field": { "type": "string" } }
        })
    }

    #[tokio::test]
    async fn errors_instead_of_returning_null_when_no_texts_are_wired() {
        let node = ExtractionNode::new(None);
        let mut inputs = NodeInputs::new();
        // The exact mis-wiring this guards: an edge written as
        // `{"from": "slack_message", "to": "extract_info"}` lands the payload
        // under the bare source name, never under `texts.`.
        inputs.insert("slack_message".to_string(), json!("hello"));
        inputs.insert("email_body".to_string(), json!("world"));
        // Engine-injected plumbing that must not leak into the message.
        inputs.insert("__node_id".to_string(), json!("extract_info"));
        inputs.insert("session_id".to_string(), json!("run-1"));
        let mut state = json!({});

        let err = node
            .execute(&inputs, &config_without_texts(), &mut state, None)
            .await
            .expect_err("a node with no text sources must fail, not report success");

        let msg = err.to_string();
        assert!(
            msg.contains("texts."),
            "the error must name the expected wiring, got: {msg}"
        );
        assert!(
            msg.contains("slack_message") && msg.contains("email_body"),
            "the error must list the input keys actually received, got: {msg}"
        );
        assert!(
            !msg.contains("__node_id") && !msg.contains("session_id"),
            "engine-injected plumbing must not bury the actionable keys, got: {msg}"
        );
    }

    #[tokio::test]
    async fn a_correctly_wired_but_null_text_source_returns_null_instead_of_failing() {
        let node = ExtractionNode::new(None);
        let mut inputs = NodeInputs::new();
        // The edge IS wired correctly. The upstream just produced nothing — an
        // `http_request` answering 204, for instance, resolves its `body` to null
        // and the engine hands it over as `texts.<name> = null`. That is a data
        // condition, not a misconfiguration, and it must not fail the whole run:
        // before the guard existed this path returned null and the engine skipped
        // only the downstream branch.
        inputs.insert("texts.api_response".to_string(), Value::Null);
        let mut state = json!({});

        let out = node
            .execute(&inputs, &config_without_texts(), &mut state, None)
            .await
            .expect("a declared-but-empty source must not fail the run");

        assert_eq!(
            out,
            Value::Null,
            "the pre-existing null return is the contract for a declared-but-empty source"
        );
    }

    #[tokio::test]
    async fn a_declared_but_empty_config_texts_also_returns_null() {
        let node = ExtractionNode::new(None);
        let inputs = NodeInputs::new();
        let mut config = config_without_texts();
        config["texts"] = json!({});
        let mut state = json!({});

        let out = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect("declaring `config.texts` is declaring a source, even when empty");

        assert_eq!(out, Value::Null);
    }
}
