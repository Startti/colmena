use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct InputNode;

/// Resolves `{{key}}` and `{{key.nested}}` templates in a JSON value using a flat
/// key/value lookup from `state`. Only string values are substituted.
fn resolve_templates(value: Value, state: &Value) -> Value {
    match value {
        Value::String(s) => {
            let mut result = s.clone();
            let mut search_from = 0;
            while let Some(start) = result[search_from..].find("{{") {
                let abs_start = search_from + start;
                let Some(end) = result[abs_start..].find("}}") else {
                    break;
                };
                let abs_end = abs_start + end;
                let key = result[abs_start + 2..abs_end].trim();
                let replacement = state
                    .get(key)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                result.replace_range(abs_start..abs_end + 2, &replacement);
                search_from = abs_start + replacement.len();
            }
            Value::String(result)
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, resolve_templates(v, state)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| resolve_templates(v, state))
                .collect(),
        ),
        other => other,
    }
}

#[async_trait::async_trait]
impl ExecutableNode for InputNode {
    /// The Input node ignores upstream inputs and payload, and just outpus the static `data` defined in its config.
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // If a state payload was injected (e.g. from a loop), yield it directly
        // to prevent double-nesting the graph output state.
        if let Some(p) = config.get("__payload__") {
            return Ok(p.clone());
        }

        // If config declares keys, use them as the schema and let injected inputs override.
        // If config is empty, pass through all injected inputs (minus internal keys) directly.
        let base = if let Some(data) = config.get("data") {
            data.clone()
        } else {
            config.clone()
        };

        let config_is_empty = base.as_object().map(|o| o.is_empty()).unwrap_or(false);

        if config_is_empty {
            // Passthrough: return all user-meaningful injected inputs
            let passthrough: serde_json::Map<String, Value> = inputs
                .iter()
                .filter(|(k, _)| !k.starts_with("__") && k.as_str() != "session_id")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            return Ok(Value::Object(passthrough));
        }

        // Config has declared keys — resolve templates then let injected values override.
        let mut result = resolve_templates(base, state);
        if let Some(obj) = result.as_object_mut() {
            for (k, v) in obj.iter_mut() {
                if let Some(input_val) = inputs.get(k) {
                    match input_val {
                        Value::Null => {}
                        Value::Object(o) if o.is_empty() => {}
                        Value::String(s) if s.is_empty() => {}
                        other => *v = other.clone(),
                    }
                }
            }
        }
        Ok(result)
    }

    fn description(&self) -> Option<&str> {
        Some("Input node that outputs hardcoded data from its configuration.")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "input",
            "config": {
                "data": "any (the static data to output)"
            },
            "outputs": {
                "output": "any"
            }
        })
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Emits its config as the downstream payload, so any key is valid data.
        // `data` and `__payload__` are the two keys it also gives meaning to.
        Some(
            NodeCatalogEntry::open_config()
                .with_field("data", FieldSpec::of_type("any"))
                .with_field("__payload__", FieldSpec::of_type("any")),
        )
    }
}
