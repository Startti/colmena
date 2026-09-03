use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::llm::domain::ProviderKind;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use super::util::extract_with_schema::{extract_with_schema, ExtractInput};
use super::util::inline_schema::inline_to_json_schema;
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};

const DEFAULT_SYSTEM_MSG: &str = include_str!("../../../../text/prompts/extraction_system.md");

pub struct OutputParserNode;

impl OutputParserNode {
    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("Environment variable {} not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }

    fn is_empty_input(v: &Value) -> bool {
        match v {
            Value::Null => true,
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        }
    }
}

#[async_trait]
impl ExecutableNode for OutputParserNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("OutputParser: missing 'provider' in config")?;
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("OutputParser: invalid provider '{}'", provider_str).into()),
        };
        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("OutputParser: missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let inline_schema = config
            .get("schema")
            .ok_or("OutputParser: missing 'schema' in config")?;
        // Validate the inline schema by attempting conversion now (init-time check).
        let json_schema = inline_to_json_schema(inline_schema)
            .map_err(|e| format!("OutputParser config error: {}", e))?;

        let input_raw = inputs.get("input").cloned().unwrap_or(Value::Null);
        if Self::is_empty_input(&input_raw) {
            return Err("OutputParserRuntimeError: missing input — nothing to parse".into());
        }

        let user_text = match &input_raw {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other)?,
        };

        let instructions = config
            .get("instructions")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let instructions_section = if instructions.is_empty() {
            String::new()
        } else {
            format!("\n\nContext/Rules for extraction:\n{}\n", instructions)
        };
        let system_message = DEFAULT_SYSTEM_MSG
            .replace("{user_instructions}", &instructions_section)
            .replace("{schema}", &serde_json::to_string_pretty(&json_schema)?);

        let temperature = config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32);

        extract_with_schema(ExtractInput {
            provider_kind,
            api_key,
            model,
            system_message,
            user_text,
            inline_schema,
            temperature,
            observer,
        })
        .await
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Parses unstructured text (typically the output of an LLM or agent) into a JSON \
             object matching the provided inline schema. Thin wrapper around the extraction \
             engine with a single 'input' port and inline-required schema declaration.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "output_parser",
            "config": {
                "provider": "string (openai | google | anthropic)",
                "api_key": "string",
                "model": "string (optional)",
                "schema": "inline schema: { field: { type, required?, description? } }",
                "instructions": "string (optional)",
                "temperature": "number (optional, default 0.1)"
            },
            "inputs": {
                "input": "any — text or value to parse"
            },
            "outputs": {
                "<schema fields>": "extracted JSON"
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
                .with_field("instructions", FieldSpec::of_type("string"))
                .with_field("temperature", FieldSpec::of_type("number")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_inputs(input: Value) -> NodeInputs {
        let mut m = NodeInputs::new();
        m.insert("input".to_string(), input);
        m
    }

    #[tokio::test]
    async fn fails_when_input_is_null() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(Value::Null), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_string() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!("   ")), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_array() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!([])), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_object() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!({})), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_schema_is_invalid_inline() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "weird" } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!("hello")), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid type 'weird'"));
    }
}
