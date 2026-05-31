use serde_json::{json, Value};

/// Converts an inline-required schema to standard JSON Schema.
///
/// Inline form: `{ field_name: { type, required?, description? } }`.
/// Standard form: `{ type: "object", properties: {...}, required: [...] }`.
pub fn inline_to_json_schema(inline: &Value) -> Result<Value, String> {
    let obj = inline
        .as_object()
        .ok_or_else(|| "inline schema must be a JSON object".to_string())?;
    if obj.is_empty() {
        return Err("inline schema must declare at least one field".to_string());
    }

    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();

    for (field_name, field_def) in obj {
        let def_obj = field_def
            .as_object()
            .ok_or_else(|| format!("field '{}' must be a JSON object", field_name))?;

        let type_str = def_obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("field '{}' missing 'type'", field_name))?;

        if !matches!(
            type_str,
            "string" | "number" | "integer" | "boolean" | "array" | "object"
        ) {
            return Err(format!(
                "field '{}' has invalid type '{}'",
                field_name, type_str
            ));
        }

        let mut prop = serde_json::Map::new();
        prop.insert("type".to_string(), json!(type_str));
        if let Some(desc) = def_obj.get("description") {
            prop.insert("description".to_string(), desc.clone());
        }
        properties.insert(field_name.clone(), Value::Object(prop));

        if def_obj
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            required.push(json!(field_name));
        }
    }

    Ok(json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": Value::Array(required)
    }))
}

/// Validates a JSON value against an inline schema.
/// Checks: required fields present (and not null) + type matches per declared field.
/// Returns Err with a human-readable message on first violation.
pub fn validate_against_inline_schema(value: &Value, inline: &Value) -> Result<(), String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "value must be a JSON object".to_string())?;
    let schema_obj = inline
        .as_object()
        .ok_or_else(|| "schema must be a JSON object".to_string())?;

    for (field_name, field_def) in schema_obj {
        let def_obj = field_def
            .as_object()
            .ok_or_else(|| format!("schema field '{}' must be a JSON object", field_name))?;

        let required = def_obj
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let field_value = obj.get(field_name);

        if required && field_value.map_or(true, |v| v.is_null()) {
            return Err(format!("required field '{}' is missing or null", field_name));
        }

        if let Some(v) = field_value {
            if v.is_null() {
                continue;
            }
            let expected_type = def_obj
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("schema field '{}' missing 'type'", field_name))?;
            let actual_ok = match expected_type {
                "string" => v.is_string(),
                "number" => v.is_number(),
                "integer" => v.is_i64() || v.is_u64(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                _ => false,
            };
            if !actual_ok {
                return Err(format!(
                    "field '{}' expected type '{}', got '{}'",
                    field_name,
                    expected_type,
                    type_label(v)
                ));
            }
        }
    }

    Ok(())
}

fn type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_single_required_string_field() {
        let inline = json!({
            "intent": { "type": "string", "required": true, "description": "user intent" }
        });
        let out = inline_to_json_schema(&inline).unwrap();
        assert_eq!(
            out,
            json!({
                "type": "object",
                "properties": { "intent": { "type": "string", "description": "user intent" } },
                "required": ["intent"]
            })
        );
    }

    #[test]
    fn converter_rejects_non_object_root() {
        let err = inline_to_json_schema(&json!("not an object")).unwrap_err();
        assert!(err.contains("must be a JSON object"));
    }

    #[test]
    fn converter_rejects_empty_object() {
        let err = inline_to_json_schema(&json!({})).unwrap_err();
        assert!(err.contains("at least one field"));
    }

    #[test]
    fn converter_rejects_field_with_invalid_type() {
        let inline = json!({ "x": { "type": "weird" } });
        let err = inline_to_json_schema(&inline).unwrap_err();
        assert!(err.contains("invalid type 'weird'"));
    }

    #[test]
    fn converter_omits_required_array_when_no_field_is_required() {
        let inline = json!({ "x": { "type": "string" } });
        let out = inline_to_json_schema(&inline).unwrap();
        assert_eq!(out["required"], json!([]));
    }

    #[test]
    fn converter_preserves_multiple_fields_and_descriptions() {
        let inline = json!({
            "a": { "type": "string", "required": true, "description": "alpha" },
            "b": { "type": "number" }
        });
        let out = inline_to_json_schema(&inline).unwrap();
        assert_eq!(out["properties"]["a"]["description"], json!("alpha"));
        assert_eq!(out["properties"]["b"]["type"], json!("number"));
        assert_eq!(out["required"], json!(["a"]));
    }

    #[test]
    fn validator_accepts_value_matching_schema() {
        let schema = json!({
            "intent": { "type": "string", "required": true },
            "confidence": { "type": "number" }
        });
        let value = json!({ "intent": "sales", "confidence": 0.9 });
        assert!(validate_against_inline_schema(&value, &schema).is_ok());
    }

    #[test]
    fn validator_rejects_missing_required_field() {
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let err = validate_against_inline_schema(&json!({}), &schema).unwrap_err();
        assert!(err.contains("required field 'intent'"));
    }

    #[test]
    fn validator_rejects_null_required_field() {
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let err =
            validate_against_inline_schema(&json!({ "intent": null }), &schema).unwrap_err();
        assert!(err.contains("required field 'intent'"));
    }

    #[test]
    fn validator_accepts_null_for_optional_field() {
        let schema = json!({ "x": { "type": "string" } });
        assert!(validate_against_inline_schema(&json!({ "x": null }), &schema).is_ok());
    }

    #[test]
    fn validator_rejects_wrong_type() {
        let schema = json!({ "n": { "type": "number" } });
        let err =
            validate_against_inline_schema(&json!({ "n": "not a number" }), &schema).unwrap_err();
        assert!(err.contains("expected type 'number'"));
        assert!(err.contains("got 'string'"));
    }

    #[test]
    fn validator_rejects_non_object_root() {
        let schema = json!({ "x": { "type": "string" } });
        let err = validate_against_inline_schema(&json!("scalar"), &schema).unwrap_err();
        assert!(err.contains("must be a JSON object"));
    }
}
