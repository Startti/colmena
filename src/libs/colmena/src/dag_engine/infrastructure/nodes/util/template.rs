use serde_json::Value;

/// Resolve `{{key}}` and `{{key.nested.path}}` placeholders found in `template`
/// against `lookup`, a root-key lookup function.
///
/// The body was moved verbatim out of `LlmNode::resolve_template_vars` so that
/// more than one node can share the same template semantics instead of each
/// keeping its own copy. The only thing a caller chooses is where a root key
/// is looked up; `llm_call` looks it up in its `inputs`.
///
/// A string value resolves to its own content; any other JSON value resolves
/// to its JSON text representation. A missing root key or a missing nested
/// path both resolve to an empty string, never an error (a miss is logged at
/// debug level, path only, never the value). An unterminated `{{` is left as
/// literal text. A substituted value is never re-scanned for further
/// `{{...}}` occurrences (the cursor only advances through the source
/// string).
pub fn render_template<'a>(template: &str, lookup: impl Fn(&str) -> Option<&'a Value>) -> String {
    let mut result = String::new();
    let mut last_end = 0;

    while let Some(start) = template[last_end..].find("{{") {
        let absolute_start = last_end + start;
        result.push_str(&template[last_end..absolute_start]);

        if let Some(end) = template[absolute_start..].find("}}") {
            let absolute_end = absolute_start + end + 1; // points to the last }
            let var_path = template[absolute_start + 2..absolute_end - 1].trim();

            let parts: Vec<&str> = var_path.splitn(2, '.').collect();
            let val_str = if parts.is_empty() || parts[0].is_empty() {
                String::new()
            } else {
                let root_key = parts[0];
                if let Some(root_val) = lookup(root_key) {
                    if parts.len() == 1 {
                        match root_val {
                            Value::String(s) => s.clone(),
                            _ => serde_json::to_string(root_val).unwrap_or_default(),
                        }
                    } else {
                        let json_pointer = format!("/{}", parts[1].replace('.', "/"));
                        if let Some(nested_val) = root_val.pointer(&json_pointer) {
                            match nested_val {
                                Value::String(s) => s.clone(),
                                _ => serde_json::to_string(nested_val).unwrap_or_default(),
                            }
                        } else {
                            tracing::debug!(
                                target: "colmena::dag_engine::template",
                                path = var_path,
                                "template variable not found"
                            );
                            String::new()
                        }
                    }
                } else {
                    tracing::debug!(
                        target: "colmena::dag_engine::template",
                        path = var_path,
                        "template variable not found"
                    );
                    String::new()
                }
            };

            result.push_str(&val_str);
            last_end = absolute_end + 1;
        } else {
            result.push_str(&template[absolute_start..]);
            last_end = template.len();
            break;
        }
    }
    result.push_str(&template[last_end..]);
    result
}

#[cfg(test)]
mod tests {
    use super::render_template;
    use serde_json::json;
    use serde_json::Value;
    use std::collections::HashMap;

    fn lookup_from<'a>(map: &'a HashMap<String, Value>) -> impl Fn(&str) -> Option<&'a Value> + 'a {
        move |k: &str| map.get(k)
    }

    #[test]
    fn flat_key_resolves() {
        let mut map = HashMap::new();
        map.insert("nombre".to_string(), json!("Ana"));
        assert_eq!(
            render_template("Hola {{nombre}}", lookup_from(&map)),
            "Hola Ana"
        );
    }

    #[test]
    fn dot_path_resolves() {
        let mut map = HashMap::new();
        map.insert("usuario".to_string(), json!({"nombre": "Ana"}));
        assert_eq!(
            render_template("Hola {{usuario.nombre}}", lookup_from(&map)),
            "Hola Ana"
        );
    }

    #[test]
    fn non_string_value_renders_as_json_text() {
        let mut map = HashMap::new();
        map.insert("usuario".to_string(), json!({"nombre": "Ana"}));
        assert_eq!(
            render_template("{{usuario}}", lookup_from(&map)),
            "{\"nombre\":\"Ana\"}"
        );
    }

    #[test]
    fn missing_key_renders_empty_string() {
        let map: HashMap<String, Value> = HashMap::new();
        assert_eq!(render_template("[{{no_existe}}]", lookup_from(&map)), "[]");
    }

    #[test]
    fn unterminated_double_brace_is_left_literal() {
        let mut map = HashMap::new();
        map.insert("nombre".to_string(), json!("Ana"));
        assert_eq!(
            render_template("Hola {{nombre", lookup_from(&map)),
            "Hola {{nombre"
        );
    }
}
