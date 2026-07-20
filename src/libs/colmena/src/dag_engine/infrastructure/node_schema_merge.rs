//! Pure merge of LLM/row args into a `node_schema`, extracted from
//! `DagToolExecutor::execute_inner` so `for_each` reuses identical semantics.

use crate::dag_engine::domain::tool_configuration::{parse_node_schema, NodeSchema};
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use serde_json::Value;
use std::collections::HashMap;

/// Merge caller-supplied args (LLM tool args, or a `for_each` row) into a
/// parsed `node_schema`: seed all `fixed` values, place each arg via
/// `param_to_container`, refuse to override fixed fields, then resolve
/// `${VAR}` templates. Identical to `execute_inner`'s PATH 0.
pub(crate) fn merge_args_into_schema(
    node_schema: &Value,
    args: HashMap<String, Value>,
) -> Result<HashMap<String, Value>, String> {
    let node_schema: NodeSchema = serde_json::from_value(node_schema.clone())
        .map_err(|e| format!("Invalid node_schema: {e}"))?;
    let parsed = parse_node_schema(&node_schema)
        .map_err(|e| format!("Invalid node_schema: {e}"))?;
    let mut result: HashMap<String, Value> = HashMap::new();

    for (k, v) in &parsed.fixed_values {
        result.insert(k.clone(), v.clone());
    }

    for (param_name, param_value) in &args {
        if let Some(container) = parsed.param_to_container.get(param_name) {
            let entry = result
                .entry(container.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(map) = entry {
                let real_key = if let Some(dot_pos) = param_name.find('.') {
                    &param_name[dot_pos + 1..]
                } else {
                    param_name.as_str()
                };
                if let (Some(Value::Object(existing)), Value::Object(incoming)) =
                    (map.get(real_key), param_value)
                {
                    let mut merged = existing.clone();
                    for (k, v) in incoming {
                        merged.insert(k.clone(), v.clone());
                    }
                    map.insert(real_key.to_string(), Value::Object(merged));
                } else {
                    map.insert(real_key.to_string(), param_value.clone());
                }
            }
        } else if parsed.fixed_values.contains_key(param_name) {
            // A supplied arg must NEVER override an operator-declared `fixed` field.
            eprintln!(
                "⚠️ [node_schema_merge] Ignoring arg '{param_name}' — collides with a fixed field."
            );
        } else {
            result.insert(param_name.clone(), param_value.clone());
        }
    }

    let resolved = result
        .iter()
        .map(|(k, v)| (k.clone(), DagToolExecutor::resolve_value_templates(v, &result)))
        .collect::<HashMap<String, Value>>();
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn places_fixed_and_row_args() {
        let schema = json!({
            "base_url": { "fixed": "https://api.example.com" },
            "user_id":  { "type": "number", "required": true, "description": "id" }
        });
        let mut row = HashMap::new();
        row.insert("user_id".to_string(), json!(42));
        let out = merge_args_into_schema(&schema, row).unwrap();
        assert_eq!(out.get("base_url").unwrap(), &json!("https://api.example.com"));
        assert_eq!(out.get("user_id").unwrap(), &json!(42));
    }

    #[test]
    fn row_arg_cannot_override_fixed() {
        let schema = json!({ "secret": { "fixed": "keep" }, "x": { "type": "number", "required": true } });
        let mut row = HashMap::new();
        row.insert("secret".to_string(), json!("evil"));
        row.insert("x".to_string(), json!(1));
        let out = merge_args_into_schema(&schema, row).unwrap();
        assert_eq!(out.get("secret").unwrap(), &json!("keep"));
    }
}
