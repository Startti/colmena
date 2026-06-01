//! `when` DSL — parser + evaluator.

use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum WhenRule {
    Equals { field: String, value: Value },
    NotEquals { field: String, value: Value },
    In { field: String, values: Vec<Value> },
    Contains { field: String, value: Value },
    Gt { field: String, value: f64 },
    Lt { field: String, value: f64 },
    Gte { field: String, value: f64 },
    Lte { field: String, value: f64 },
    Matches { field: String, regex: Regex },
    Exists { field: String },
    All(Vec<WhenRule>),
    Any(Vec<WhenRule>),
    Not(Box<WhenRule>),
}

impl WhenRule {
    pub fn parse(when: &Value, inline_schema: &Value) -> Result<Self, String> {
        let obj = when
            .as_object()
            .ok_or_else(|| "'when' must be a JSON object".to_string())?;

        if let Some(rules_val) = obj.get("all") {
            let arr = rules_val
                .as_array()
                .ok_or_else(|| "'all' must be an array".to_string())?;
            let rules = arr
                .iter()
                .map(|r| WhenRule::parse(r, inline_schema))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(WhenRule::All(rules));
        }
        if let Some(rules_val) = obj.get("any") {
            let arr = rules_val
                .as_array()
                .ok_or_else(|| "'any' must be an array".to_string())?;
            let rules = arr
                .iter()
                .map(|r| WhenRule::parse(r, inline_schema))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(WhenRule::Any(rules));
        }
        if let Some(inner) = obj.get("not") {
            let rule = WhenRule::parse(inner, inline_schema)?;
            return Ok(WhenRule::Not(Box::new(rule)));
        }

        let field = obj
            .get("field")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "'when' requires 'field' (or all/any/not)".to_string())?
            .to_string();

        // Validate the top-level (first segment) field is declared in the schema.
        let first = field.split('.').next().unwrap_or(&field);
        if let Some(schema_obj) = inline_schema.as_object() {
            if !schema_obj.contains_key(first) {
                return Err(format!("'when' references unknown field '{}'", first));
            }
        }

        if let Some(v) = obj.get("equals") {
            return Ok(WhenRule::Equals {
                field,
                value: v.clone(),
            });
        }
        if let Some(v) = obj.get("not_equals") {
            return Ok(WhenRule::NotEquals {
                field,
                value: v.clone(),
            });
        }
        if let Some(v) = obj.get("in") {
            let values = v
                .as_array()
                .ok_or_else(|| "'in' must be an array".to_string())?
                .clone();
            return Ok(WhenRule::In { field, values });
        }
        if let Some(v) = obj.get("contains") {
            return Ok(WhenRule::Contains {
                field,
                value: v.clone(),
            });
        }
        if let Some(v) = obj.get("gt") {
            let n = v
                .as_f64()
                .ok_or_else(|| "'gt' must be a number".to_string())?;
            return Ok(WhenRule::Gt { field, value: n });
        }
        if let Some(v) = obj.get("lt") {
            let n = v
                .as_f64()
                .ok_or_else(|| "'lt' must be a number".to_string())?;
            return Ok(WhenRule::Lt { field, value: n });
        }
        if let Some(v) = obj.get("gte") {
            let n = v
                .as_f64()
                .ok_or_else(|| "'gte' must be a number".to_string())?;
            return Ok(WhenRule::Gte { field, value: n });
        }
        if let Some(v) = obj.get("lte") {
            let n = v
                .as_f64()
                .ok_or_else(|| "'lte' must be a number".to_string())?;
            return Ok(WhenRule::Lte { field, value: n });
        }
        if let Some(v) = obj.get("matches") {
            let s = v
                .as_str()
                .ok_or_else(|| "'matches' must be a string".to_string())?;
            let regex = Regex::new(s).map_err(|e| format!("invalid regex: {}", e))?;
            return Ok(WhenRule::Matches { field, regex });
        }
        if let Some(v) = obj.get("exists") {
            let b = v
                .as_bool()
                .ok_or_else(|| "'exists' must be a boolean".to_string())?;
            if !b {
                return Err(
                    "'exists: false' is not supported — use 'not: { ..., exists: true }'"
                        .to_string(),
                );
            }
            return Ok(WhenRule::Exists { field });
        }

        Err("'when' has no operator (equals/in/gt/.../exists)".to_string())
    }

    pub fn evaluate(&self, extracted: &Value) -> bool {
        match self {
            WhenRule::All(rs) => rs.iter().all(|r| r.evaluate(extracted)),
            WhenRule::Any(rs) => rs.iter().any(|r| r.evaluate(extracted)),
            WhenRule::Not(r) => !r.evaluate(extracted),
            WhenRule::Equals { field, value } => {
                resolve(field, extracted).is_some_and(|v| &v == value)
            }
            WhenRule::NotEquals { field, value } => {
                // Missing field is considered "not equal" (returns true).
                resolve(field, extracted).is_none_or(|v| &v != value)
            }
            WhenRule::In { field, values } => {
                resolve(field, extracted).is_some_and(|v| values.contains(&v))
            }
            WhenRule::Contains { field, value } => match resolve(field, extracted) {
                Some(Value::String(s)) => match value {
                    Value::String(needle) => s.contains(needle.as_str()),
                    _ => false,
                },
                Some(Value::Array(a)) => a.contains(value),
                _ => false,
            },
            WhenRule::Gt { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n > *value),
            WhenRule::Lt { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n < *value),
            WhenRule::Gte { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n >= *value),
            WhenRule::Lte { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n <= *value),
            WhenRule::Matches { field, regex } => resolve(field, extracted)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .is_some_and(|s| regex.is_match(&s)),
            WhenRule::Exists { field } => resolve(field, extracted).is_some_and(|v| !v.is_null()),
        }
    }
}

fn resolve(path: &str, root: &Value) -> Option<Value> {
    let mut cur = root;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get(seg)?,
            _ => return None,
        };
    }
    Some(cur.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "intent":     { "type": "string", "required": true },
            "urgency":    { "type": "string" },
            "confidence": { "type": "number" },
            "user":       { "type": "object" }
        })
    }

    fn parse(when: Value) -> WhenRule {
        WhenRule::parse(&when, &schema()).unwrap()
    }

    #[test]
    fn equals_string() {
        let r = parse(json!({ "field": "intent", "equals": "sales" }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(!r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({})));
    }

    #[test]
    fn equals_is_type_strict() {
        let r = parse(json!({ "field": "confidence", "equals": 5 }));
        assert!(r.evaluate(&json!({ "confidence": 5 })));
        assert!(!r.evaluate(&json!({ "confidence": "5" })));
    }

    #[test]
    fn not_equals() {
        let r = parse(json!({ "field": "intent", "not_equals": "sales" }));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "sales" })));
        // missing field is considered "not equal"
        assert!(r.evaluate(&json!({})));
    }

    #[test]
    fn in_operator() {
        let r = parse(json!({ "field": "intent", "in": ["sales", "support"] }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "billing" })));
    }

    #[test]
    fn contains_string_substring() {
        let r = parse(json!({ "field": "intent", "contains": "ale" }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(!r.evaluate(&json!({ "intent": "support" })));
    }

    #[test]
    fn gt_lt_gte_lte() {
        let gt = parse(json!({ "field": "confidence", "gt": 0.5 }));
        let lt = parse(json!({ "field": "confidence", "lt": 0.5 }));
        let gte = parse(json!({ "field": "confidence", "gte": 0.5 }));
        let lte = parse(json!({ "field": "confidence", "lte": 0.5 }));
        assert!(gt.evaluate(&json!({ "confidence": 0.9 })));
        assert!(!gt.evaluate(&json!({ "confidence": 0.5 })));
        assert!(lt.evaluate(&json!({ "confidence": 0.1 })));
        assert!(gte.evaluate(&json!({ "confidence": 0.5 })));
        assert!(lte.evaluate(&json!({ "confidence": 0.5 })));
    }

    #[test]
    fn matches_regex() {
        let r = parse(json!({ "field": "intent", "matches": "^sa.*s$" }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(!r.evaluate(&json!({ "intent": "support" })));
    }

    #[test]
    fn exists_true() {
        let r = parse(json!({ "field": "urgency", "exists": true }));
        assert!(r.evaluate(&json!({ "urgency": "high" })));
        assert!(!r.evaluate(&json!({})));
        assert!(!r.evaluate(&json!({ "urgency": null })));
    }

    #[test]
    fn all_combinator() {
        let r = parse(json!({
            "all": [
                { "field": "intent", "equals": "sales" },
                { "field": "urgency", "equals": "high" }
            ]
        }));
        assert!(r.evaluate(&json!({ "intent": "sales", "urgency": "high" })));
        assert!(!r.evaluate(&json!({ "intent": "sales", "urgency": "low" })));
    }

    #[test]
    fn any_combinator() {
        let r = parse(json!({
            "any": [
                { "field": "intent", "equals": "sales" },
                { "field": "intent", "equals": "support" }
            ]
        }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "billing" })));
    }

    #[test]
    fn not_combinator() {
        let r = parse(json!({ "not": { "field": "intent", "equals": "sales" } }));
        assert!(!r.evaluate(&json!({ "intent": "sales" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
    }

    #[test]
    fn dotted_field_path() {
        let schema = json!({ "user": { "type": "object" } });
        let r =
            WhenRule::parse(&json!({ "field": "user.tier", "equals": "gold" }), &schema).unwrap();
        assert!(r.evaluate(&json!({ "user": { "tier": "gold" } })));
        assert!(!r.evaluate(&json!({ "user": { "tier": "silver" } })));
        assert!(!r.evaluate(&json!({})));
    }

    #[test]
    fn rejects_unknown_field_at_parse_time() {
        let err =
            WhenRule::parse(&json!({ "field": "unknown", "equals": "x" }), &schema()).unwrap_err();
        assert!(err.contains("unknown field 'unknown'"));
    }

    #[test]
    fn rejects_when_with_no_operator() {
        let err = WhenRule::parse(&json!({ "field": "intent" }), &schema()).unwrap_err();
        assert!(err.contains("no operator"));
    }

    #[test]
    fn rejects_invalid_regex() {
        let err =
            WhenRule::parse(&json!({ "field": "intent", "matches": "[" }), &schema()).unwrap_err();
        assert!(err.contains("invalid regex"));
    }

    #[test]
    fn nested_combinators() {
        let r = parse(json!({
            "any": [
                { "all": [
                    { "field": "intent", "equals": "sales" },
                    { "field": "urgency", "equals": "high" }
                ]},
                { "field": "intent", "equals": "support" }
            ]
        }));
        assert!(r.evaluate(&json!({ "intent": "sales", "urgency": "high" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "sales", "urgency": "low" })));
    }
}
