//! Router node — declarative branching with LLM-direct and extract+rules modes.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::llm::domain::ProviderKind;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::error::Error;
use std::sync::Arc;

use super::config::{parse_and_validate, RouterMode};
use super::llm_direct::pick_branch as pick_llm_direct;

pub struct RouterNode;

impl RouterNode {
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
impl ExecutableNode for RouterNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // 1. Parse + validate config (re-runs every execute; cheap).
        let cfg = parse_and_validate(config)
            .map_err(|e| -> Box<dyn Error + Send + Sync> { e.into() })?;

        // 2. Read input.
        let input_raw = inputs.get("input").cloned().unwrap_or(Value::Null);
        if Self::is_empty_input(&input_raw) {
            return Err("RouterRuntimeError: missing input — nothing to route".into());
        }
        let user_text = match &input_raw {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other)?,
        };

        // 3. Resolve LLM provider config (shared by both modes).
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("Router: missing 'provider' in config")?;
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("Router: invalid provider '{}'", provider_str).into()),
        };
        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("Router: missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)
            .map_err(|e| -> Box<dyn Error + Send + Sync> { e.into() })?;
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // 4. Pick a branch.
        let (idx, reason, extracted): (usize, String, Option<Value>) = match cfg.mode {
            RouterMode::LlmDirect => {
                let (i, r) = pick_llm_direct(
                    &cfg,
                    provider_kind,
                    api_key,
                    model,
                    user_text,
                    observer.clone(),
                )
                .await?;
                (i, r, None)
            }
            RouterMode::ExtractAndRoute => {
                let (i, ex) = super::extract_and_route::pick_branch(
                    &cfg,
                    provider_kind,
                    api_key,
                    model,
                    user_text,
                    observer.clone(),
                )
                .await?;
                (i, String::new(), Some(ex))
            }
        };

        let selected = &cfg.branches[idx];
        let payload = match &extracted {
            Some(e) => json!({ "input": input_raw, "extracted": e }),
            None => json!({ "input": input_raw }),
        };

        // 5. Emit __decision + one payload per port (null for non-selected).
        let mut out = Map::new();
        out.insert(
            "__decision".to_string(),
            json!({
                "selected_branch": selected.name,
                "reason": reason,
                "extracted": extracted
            }),
        );
        for (i, b) in cfg.branches.iter().enumerate() {
            out.insert(
                b.name.clone(),
                if i == idx { payload.clone() } else { Value::Null },
            );
        }

        Ok(Value::Object(out))
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Routes the input to one of N branches. Mode 'llm_direct' lets an LLM pick the \
             branch by name from descriptions. Mode 'extract_and_route' extracts a JSON object \
             against a schema and applies declarative 'when' rules to pick the branch. \
             Fails fast if no branch matches.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "router",
            "config": {
                "mode": "string (llm_direct | extract_and_route)",
                "provider": "string",
                "api_key": "string",
                "model": "string (optional)",
                "schema": "inline schema (mode B only)",
                "branches": "array of branch configs"
            },
            "inputs": { "input": "any" },
            "outputs": {
                "<branch_name>": "object — non-null only on selected branch",
                "__decision": "object — { selected_branch, reason, extracted }"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Value {
        json!({
            "mode": "llm_direct",
            "provider": "google",
            "api_key": "fake",
            "branches": [
                { "name": "a", "description": "x" },
                { "name": "b", "description": "y" }
            ]
        })
    }

    fn inputs(v: Value) -> NodeInputs {
        let mut m = NodeInputs::new();
        m.insert("input".to_string(), v);
        m
    }

    #[tokio::test]
    async fn fails_when_input_is_null() {
        let node = RouterNode;
        let mut state = json!({});
        let err = node
            .execute(&inputs(Value::Null), &cfg(), &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_string() {
        let node = RouterNode;
        let mut state = json!({});
        let err = node
            .execute(&inputs(json!("  ")), &cfg(), &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_on_invalid_config_at_runtime() {
        let node = RouterNode;
        let mut state = json!({});
        let err = node
            .execute(
                &inputs(json!("anything")),
                &json!({ "mode": "weird", "branches": [] }),
                &mut state,
                None,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid mode"));
    }

    #[tokio::test]
    async fn extract_and_route_requires_schema_at_runtime() {
        let node = RouterNode;
        let mut state = json!({});
        let cfg = json!({
            "mode": "extract_and_route",
            "provider": "google",
            "api_key": "fake",
            "branches": [ { "name": "a", "when": { "field": "x", "equals": "y" } } ]
        });
        let err = node
            .execute(&inputs(json!("anything")), &cfg, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("requires schema"));
    }
}
