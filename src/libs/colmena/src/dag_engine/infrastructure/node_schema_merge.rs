//! Pure merge of LLM/row args into a `node_schema`, extracted from
//! `DagToolExecutor::execute_inner` so `for_each` reuses identical semantics.

use crate::dag_engine::domain::child_graph_source::CHILD_GRAPH_SOURCE_KEYS;
use crate::dag_engine::domain::tool_configuration::{parse_node_schema, NodeSchema};
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Drop every child-graph source ([`CHILD_GRAPH_SOURCE_KEYS`]) the caller
/// supplied but the tool never offered as a parameter.
///
/// A `subgraph` dispatched as a tool reads its source from `inputs`, where the
/// caller's arguments land too, and an undeclared argument went through every
/// merge untouched. Since the precedence is inline > path > ref, a model that
/// added `child_graph_inline` or `child_graph_path` to its call outranked the
/// operator's fixed `child_graph_ref` (or fixed path) and picked the graph the
/// worker ran. A source the operator declares as a parameter still passes: that
/// hands the model the choice on purpose. `offered` yields the parameter names
/// the tool advertised, and runs only when the caller supplied a source.
pub(crate) fn drop_unoffered_child_graph_sources(
    args: &mut HashMap<String, Value>,
    offered: impl FnOnce() -> HashSet<String>,
) {
    for warning in drop_unoffered_child_graph_sources_silently(args, offered) {
        eprintln!("{warning}");
    }
}

/// [`drop_unoffered_child_graph_sources`], returning its warnings instead of
/// printing them.
pub(crate) fn drop_unoffered_child_graph_sources_silently(
    args: &mut HashMap<String, Value>,
    offered: impl FnOnce() -> HashSet<String>,
) -> Vec<String> {
    let supplied: Vec<&str> = CHILD_GRAPH_SOURCE_KEYS
        .into_iter()
        .filter(|key| args.contains_key(*key))
        .collect();
    if supplied.is_empty() {
        return Vec::new();
    }
    let offered = offered();
    let mut warnings = Vec::new();
    for key in supplied {
        if !offered.contains(key) {
            args.remove(key);
            warnings.push(format!(
                "⚠️ [node_schema_merge] Ignoring arg '{key}' — a child-graph source the tool does not offer."
            ));
        }
    }
    warnings
}

/// The parameters a `node_schema` offers its caller: its LLM-visible fields.
/// Empty when the schema does not parse — the merge rejects it anyway.
pub(crate) fn offered_params(node_schema: &Value) -> HashSet<String> {
    serde_json::from_value::<NodeSchema>(node_schema.clone())
        .ok()
        .and_then(|schema| parse_node_schema(&schema).ok())
        .map(|parsed| parsed.llm_properties.into_keys().collect())
        .unwrap_or_default()
}

/// Merge caller-supplied args (LLM tool args, or a `for_each` row) into a
/// parsed `node_schema`: template the operator's own fixed values against a
/// restricted set of sources, seed them, place each arg via
/// `param_to_container`, refuse to override fixed fields, and never template
/// caller-supplied values themselves.
///
/// Template sources are restricted to the operator's own unresolved fixed
/// top-level values (so one fixed field can reference another) plus each
/// declared top-level LLM param actually supplied this call
/// (`llm_properties` minus `param_to_container` — a param nested inside a
/// container never qualifies as a source). An **undeclared** argument can no
/// longer act as a `${key}` substitution source: previously the whole
/// post-merge result was the template source, so a fixed `base_url:
/// "${API_BASE}"` would resolve against a model-supplied `API_BASE` argument
/// the operator never declared as a parameter — redirecting the call to
/// wherever the model named. A `${...}`-shaped value the fixed value's
/// author never named as a declared param is left exactly as written. The
/// caller's own values are never templated: an LLM-supplied `q:
/// "${SOMETHING}"` always stays literal in the merged result.
pub(crate) fn merge_args_into_schema(
    node_schema: &Value,
    args: HashMap<String, Value>,
) -> Result<HashMap<String, Value>, String> {
    let (merged, warnings) = merge_args_into_schema_silently(node_schema, args)?;
    for warning in warnings {
        eprintln!("{warning}");
    }
    Ok(merged)
}

/// [`merge_args_into_schema`], returning its warnings instead of printing them.
pub(crate) fn merge_args_into_schema_silently(
    node_schema: &Value,
    args: HashMap<String, Value>,
) -> Result<(HashMap<String, Value>, Vec<String>), String> {
    let node_schema: NodeSchema = serde_json::from_value(node_schema.clone())
        .map_err(|e| format!("Invalid node_schema: {e}"))?;
    let parsed =
        parse_node_schema(&node_schema).map_err(|e| format!("Invalid node_schema: {e}"))?;

    // Restricted template-source map: the operator's own fixed values, plus
    // each declared top-level LLM param the caller actually supplied.
    let mut template_sources: HashMap<String, Value> = parsed.fixed_values.clone();
    for param_name in parsed.llm_properties.keys() {
        if parsed.param_to_container.contains_key(param_name) {
            continue; // container-scoped param: not a valid top-level source
        }
        if let Some(v) = args.get(param_name) {
            template_sources.insert(param_name.clone(), v.clone());
        }
    }

    // Template fixed values BEFORE the merge, once, against that restricted
    // source map only — this is the only templating pass. The caller's args
    // are placed below exactly as supplied and are never re-templated.
    let mut result: HashMap<String, Value> = parsed
        .fixed_values
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                DagToolExecutor::resolve_value_templates(v, &template_sources),
            )
        })
        .collect();

    let mut warnings = Vec::new();
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
            warnings.push(format!(
                "⚠️ [node_schema_merge] Ignoring arg '{param_name}' — collides with a fixed field."
            ));
        } else {
            result.insert(param_name.clone(), param_value.clone());
        }
    }

    Ok((result, warnings))
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
        assert_eq!(
            out.get("base_url").unwrap(),
            &json!("https://api.example.com")
        );
        assert_eq!(out.get("user_id").unwrap(), &json!(42));
    }

    #[test]
    fn row_arg_cannot_override_fixed() {
        let schema =
            json!({ "secret": { "fixed": "keep" }, "x": { "type": "number", "required": true } });
        let mut row = HashMap::new();
        row.insert("secret".to_string(), json!("evil"));
        row.insert("x".to_string(), json!(1));
        let out = merge_args_into_schema(&schema, row).unwrap();
        assert_eq!(out.get("secret").unwrap(), &json!("keep"));
    }

    /// `${key}` templating on a fixed value resolves only against the
    /// restricted source map; an undeclared model-supplied argument can never
    /// satisfy it, even though the argument itself is present in the merged
    /// result under its own name.
    #[test]
    fn declared_top_level_param_templates_into_a_fixed_value() {
        // The ADP `sql_query` shape: a fixed `query` referencing declared
        // top-level params by name.
        let schema = json!({
            "query": { "fixed": "SELECT * FROM t WHERE client_id = '${client_id}' AND period = '${period}'" },
            "client_id": { "type": "string", "required": true, "description": "client" },
            "period": { "type": "string", "required": true, "description": "period" }
        });
        let mut args = HashMap::new();
        args.insert("client_id".to_string(), json!("acme"));
        args.insert("period".to_string(), json!("2026-Q3"));
        let out = merge_args_into_schema(&schema, args).unwrap();
        assert_eq!(
            out.get("query").unwrap(),
            &json!("SELECT * FROM t WHERE client_id = 'acme' AND period = '2026-Q3'")
        );
    }

    #[test]
    fn undeclared_arg_no_longer_templates_a_fixed_value() {
        // Previously: a fixed `base_url: "${API_BASE}"` would resolve against
        // ANY key present after merge, including an undeclared model-supplied
        // `API_BASE` argument — the hijack this change closes.
        let schema = json!({
            "base_url": { "fixed": "${API_BASE}" },
            "q": { "type": "string", "required": true, "description": "q" }
        });
        let mut args = HashMap::new();
        args.insert("q".to_string(), json!("hello"));
        args.insert("API_BASE".to_string(), json!("https://attacker.example"));
        let out = merge_args_into_schema(&schema, args).unwrap();
        assert_eq!(out.get("base_url").unwrap(), &json!("${API_BASE}"));
    }

    #[test]
    fn llm_arg_value_is_never_templated_stays_literal() {
        let schema = json!({ "q": { "type": "string", "required": true, "description": "q" } });
        let mut args = HashMap::new();
        args.insert("q".to_string(), json!("${PROBE}"));
        let out = merge_args_into_schema(&schema, args).unwrap();
        assert_eq!(out.get("q").unwrap(), &json!("${PROBE}"));
    }

    #[test]
    fn declared_top_level_param_still_templates_regression_guard() {
        // Pass-on-both guard: protects the ADP `sql_query` shape from
        // regressing while the undeclared-arg hijack above is closed.
        let schema = json!({
            "greeting": { "fixed": "Hello, ${name}!" },
            "name": { "type": "string", "required": true, "description": "name" }
        });
        let mut args = HashMap::new();
        args.insert("name".to_string(), json!("Ada"));
        let out = merge_args_into_schema(&schema, args).unwrap();
        assert_eq!(out.get("greeting").unwrap(), &json!("Hello, Ada!"));
    }

    #[test]
    fn container_scoped_param_is_not_a_valid_template_source() {
        // A declared param nested inside a container (`param_to_container`)
        // must not qualify as a top-level template source, even though it is
        // declared.
        let schema = json!({
            "base_url": { "fixed": "${city}" },
            "location": {
                "properties": {
                    "city": { "type": "string", "required": true, "description": "city" }
                }
            }
        });
        let mut args = HashMap::new();
        args.insert("city".to_string(), json!("Bogota"));
        let out = merge_args_into_schema(&schema, args).unwrap();
        assert_eq!(out.get("base_url").unwrap(), &json!("${city}"));
    }
}
