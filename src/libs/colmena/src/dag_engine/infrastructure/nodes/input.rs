use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::infrastructure::nodes::util::template::render_template;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct InputNode;

/// Resolves `{{key}}` and `{{key.nested.path}}` templates found anywhere in a
/// JSON value (including nested objects and arrays), using the same
/// resolver `llm_call` uses (`nodes::util::template::render_template`).
/// Only string values are substituted; the walk itself recurses into object
/// values and array items, leaving keys and non-string leaves untouched.
///
/// Lookup order is STATE first, then INPUTS as a fallback. This node used to
/// read `state` only, and a key can hold different values in the two sources
/// (e.g. an edge-delivered `session_id`), so reading `state` first keeps every
/// previously resolving key rendering the same bytes. Keys an edge delivers
/// (including node-qualified paths like `{{origen.plano}}` when the edge names
/// a target port) resolve from `inputs`. A key missing from both sources renders `""`, never an error.
fn resolve_templates(value: Value, state: &Value, inputs: &NodeInputs) -> Value {
    match value {
        Value::String(s) => {
            let resolved = render_template(&s, |key| state.get(key).or_else(|| inputs.get(key)));
            Value::String(resolved)
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, resolve_templates(v, state, inputs)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| resolve_templates(v, state, inputs))
                .collect(),
        ),
        other => other,
    }
}

#[async_trait::async_trait]
impl ExecutableNode for InputNode {
    /// Outputs the static `data` defined in its config, with `{{key}}` /
    /// `{{key.nested.path}}` templates resolved against `state` first, then
    /// `inputs` as a fallback (see `resolve_templates` above), before any
    /// non-empty edge-delivered input overrides a resolved field by matching
    /// its config key. `__payload__` is read from CONFIG (not inputs) and
    /// returned verbatim, bypassing template resolution entirely.
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
        let mut result = resolve_templates(base, state, inputs);
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

// Characterization tests: they passed against the original flat `state`
// lookup and must keep passing, pinning what already resolved.
#[cfg(test)]
mod characterization_tests {
    use super::*;
    use std::collections::HashMap;

    async fn run(config: Value, inputs: HashMap<String, Value>, mut state: Value) -> Value {
        InputNode
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect("input node execution should not fail")
    }

    #[tokio::test]
    async fn session_id_resolves_via_flat_state_lookup() {
        let config = json!({"data": {"greeting": "hola {{session_id}}"}});
        let state = json!({"session_id": "abc123"});
        let out = run(config, HashMap::new(), state).await;
        assert_eq!(out["greeting"], json!("hola abc123"));
    }

    #[tokio::test]
    async fn a_key_present_only_in_state_still_resolves() {
        // Encodes the compatibility constraint before any lookup-order change:
        // today's code resolves exclusively against `state`, so a key that
        // exists only there (never in `inputs`) must resolve.
        let config = json!({"data": {"greeting": "hola {{solo_en_state}}"}});
        let state = json!({"solo_en_state": "valor_de_state"});
        let out = run(config, HashMap::new(), state).await;
        assert_eq!(out["greeting"], json!("hola valor_de_state"));
    }
}

// Tests for the state-first / inputs-fallback resolution. The ones that read
// edge-delivered values or traverse a dot path fail against the original flat
// `state.get` lookup; the rest guard behavior that must not change.
//
// Config keys are deliberately chosen to NEVER equal an input/template root
// key used in the same test, because the post-resolution override step
// (`if let Some(input_val) = inputs.get(k)`) would otherwise mask whether the
// template itself actually resolved.
#[cfg(test)]
mod new_behavior_tests {
    use super::*;
    use std::collections::HashMap;

    async fn run(config: Value, inputs: HashMap<String, Value>, mut state: Value) -> Value {
        InputNode
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect("input node execution should not fail")
    }

    fn inputs_of(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[tokio::test]
    async fn flat_upstream_value_resolves() {
        let config = json!({"data": {"saludo": "{{plano}}"}});
        let inputs = inputs_of(&[("plano", json!("VALOR_PLANO"))]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["saludo"], json!("VALOR_PLANO"));
    }

    #[tokio::test]
    async fn dot_path_traversal_resolves() {
        let config = json!({"data": {"saludo_usuario": "{{usuario.nombre}}"}});
        let inputs = inputs_of(&[("usuario", json!({"nombre": "Ana"}))]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["saludo_usuario"], json!("Ana"));
    }

    #[tokio::test]
    async fn array_index_traversal_resolves() {
        let config = json!({"data": {"primero": "{{items.0}}"}});
        let inputs = inputs_of(&[("items", json!(["a", "b"]))]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["primero"], json!("a"));
    }

    #[tokio::test]
    async fn non_string_values_render_as_json_text() {
        let config = json!({"data": {
            "campo_obj": "{{obj}}",
            "campo_num": "{{num}}",
            "campo_bool": "{{flag}}",
            "campo_null": "{{nada}}",
        }});
        let inputs = inputs_of(&[
            ("obj", json!({"a": 1})),
            ("num", json!(42)),
            ("flag", json!(true)),
            ("nada", json!(null)),
        ]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["campo_obj"], json!("{\"a\":1}"));
        assert_eq!(out["campo_num"], json!("42"));
        assert_eq!(out["campo_bool"], json!("true"));
        assert_eq!(out["campo_null"], json!("null"));
    }

    #[tokio::test]
    async fn template_nested_inside_object_inside_array_resolves() {
        let config = json!({"data": {
            "lista": [{"campo": "{{plano}}"}]
        }});
        let inputs = inputs_of(&[("plano", json!("VALOR_PLANO"))]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["lista"][0]["campo"], json!("VALOR_PLANO"));
    }

    #[tokio::test]
    async fn missing_key_renders_empty_without_failing() {
        let config = json!({"data": {"saludo": "{{no_existe}}"}});
        let out = run(config, HashMap::new(), json!({})).await;
        assert_eq!(out["saludo"], json!(""));
    }

    #[tokio::test]
    async fn state_only_key_still_resolves_after_wiring() {
        let config = json!({"data": {"saludo": "{{solo_en_state}}"}});
        let state = json!({"solo_en_state": "valor_de_state"});
        let out = run(config, HashMap::new(), state).await;
        assert_eq!(out["saludo"], json!("valor_de_state"));
    }

    #[tokio::test]
    async fn name_clash_between_state_and_inputs_renders_state_value() {
        // State wins on a collision, so every key that resolved before keeps
        // rendering the same bytes.
        let config = json!({"data": {"saludo": "{{clash}}"}});
        let inputs = inputs_of(&[("clash", json!("from_inputs"))]);
        let state = json!({"clash": "from_state"});
        let out = run(config, inputs, state).await;
        assert_eq!(out["saludo"], json!("from_state"));
    }

    #[tokio::test]
    async fn named_port_pattern_keeps_same_named_keys_separate() {
        // Models the shape a named-port edge delivers (`"to": "<node>.<port>"`):
        // each source's payload arrives intact under its own port key.
        let config = json!({"data": {
            "saludo_cliente": "{{cliente.nombre}}",
            "saludo_vendedor": "{{vendedor.nombre}}",
        }});
        let inputs = inputs_of(&[
            ("cliente", json!({"nombre": "Ana"})),
            ("vendedor", json!({"nombre": "Luis"})),
        ]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["saludo_cliente"], json!("Ana"));
        assert_eq!(out["saludo_vendedor"], json!("Luis"));
    }

    #[tokio::test]
    async fn resolved_value_is_not_rescanned_for_further_templates() {
        let config = json!({"data": {"campo": "{{x}}"}});
        let inputs = inputs_of(&[("x", json!("{{y}}")), ("y", json!("should_not_appear"))]);
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["campo"], json!("{{y}}"));
    }
}

// Regression guards for existing behavior that must NOT change. These are expected to
// PASS unchanged against both the old and the new lookup, since they do not
// depend on which source template resolution reads from.
#[cfg(test)]
mod passthrough_regression_tests {
    use super::*;
    use std::collections::HashMap;

    async fn run(config: Value, inputs: HashMap<String, Value>, mut state: Value) -> Value {
        InputNode
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect("input node execution should not fail")
    }

    #[tokio::test]
    async fn edge_delivered_value_still_overrides_a_templated_field() {
        let config = json!({"data": {"saludo": "{{plano}}"}});
        let mut inputs = HashMap::new();
        inputs.insert("plano".to_string(), json!("VALOR_PLANO"));
        // The edge delivers a value under the CONFIG KEY itself ("saludo"),
        // which is the override path — distinct from the template root key.
        inputs.insert("saludo".to_string(), json!("override_value"));
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out["saludo"], json!("override_value"));
    }

    #[tokio::test]
    async fn empty_config_passthrough_is_unchanged() {
        let config = json!({});
        let mut inputs = HashMap::new();
        inputs.insert("a".to_string(), json!(1));
        inputs.insert("b".to_string(), json!(2));
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out, json!({"a": 1, "b": 2}));
    }

    #[tokio::test]
    async fn payload_early_return_reads_from_config_not_inputs() {
        // The spec's "__payload__ in an inputs map" scenario text does not
        // match the code, which reads config.get("__payload__"). Test the
        // actual code path: CONFIG carries __payload__, not inputs.
        let config = json!({"__payload__": {"foo": "bar"}});
        let mut inputs = HashMap::new();
        inputs.insert("__payload__".to_string(), json!({"should": "be_ignored"}));
        let out = run(config, inputs, json!({})).await;
        assert_eq!(out, json!({"foo": "bar"}));
    }
}
