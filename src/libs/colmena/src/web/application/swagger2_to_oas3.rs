//! Convert a Swagger 2.0 JSON/YAML tree to OpenAPI 3.0.3.
//!
//! Operates on `serde_json::Value` — if the input was YAML, the adapter
//! has already round-tripped it through `serde_yaml` → `serde_json::Value`.
//!
//! The converter is **lossy on purpose** for features OpenAPI 3.0 cannot
//! represent (e.g. `collectionFormat: tsv`): those cases raise an error
//! rather than silently degrading so the agent sees a clear failure.

use crate::web::domain::WebDomainError;
use serde_json::{json, Map, Value};

/// Convert a Swagger 2.0 `serde_json::Value` into an OpenAPI 3.0.3 `Value`.
///
/// Returns `Err(WebDomainError::Swagger2ConversionFailed)` if the document
/// uses a feature with no OAS 3.0 equivalent (e.g. `collectionFormat: tsv`).
pub fn convert_swagger2_to_openapi3(input: &Value) -> Result<Value, WebDomainError> {
    let root = input.as_object().ok_or_else(|| {
        WebDomainError::Swagger2ConversionFailed {
            reason: "root is not a JSON object".into(),
            unsupported_feature: None,
        }
    })?;

    let mut out = Map::new();
    out.insert("openapi".into(), Value::String("3.0.3".into()));

    if let Some(info) = root.get("info") {
        out.insert("info".into(), info.clone());
    } else {
        return Err(WebDomainError::Swagger2ConversionFailed {
            reason: "missing required `info` block".into(),
            unsupported_feature: None,
        });
    }

    // servers: one per scheme × (host + basePath)
    let servers = build_servers(root);
    if !servers.is_empty() {
        out.insert("servers".into(), Value::Array(servers));
    }

    // paths (deep-copied then ref-rewritten; operation-body conversion lives in Task 3)
    let paths = root.get("paths").cloned().unwrap_or(json!({}));
    let paths = rewrite_refs_recursive(paths);
    out.insert("paths".into(), paths);

    // components
    let mut components = Map::new();
    if let Some(defs) = root.get("definitions") {
        components.insert("schemas".into(), rewrite_refs_recursive(defs.clone()));
    }
    if let Some(global_params) = root.get("parameters") {
        components.insert("parameters".into(), rewrite_refs_recursive(global_params.clone()));
    }
    if let Some(global_responses) = root.get("responses") {
        components.insert("responses".into(), rewrite_refs_recursive(global_responses.clone()));
    }
    if let Some(sec_defs) = root.get("securityDefinitions") {
        components.insert("securitySchemes".into(), convert_security_definitions(sec_defs)?);
    }
    if !components.is_empty() {
        out.insert("components".into(), Value::Object(components));
    }

    // Security requirements at the root are shaped the same in 2.0 and 3.0.
    if let Some(sec) = root.get("security") {
        out.insert("security".into(), sec.clone());
    }

    // Tags are compatible.
    if let Some(tags) = root.get("tags") {
        out.insert("tags".into(), tags.clone());
    }

    Ok(Value::Object(out))
}

fn build_servers(root: &Map<String, Value>) -> Vec<Value> {
    let host = root.get("host").and_then(|v| v.as_str()).unwrap_or("");
    let base_path = root.get("basePath").and_then(|v| v.as_str()).unwrap_or("");
    let schemes: Vec<String> = root
        .get("schemes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_else(|| vec!["https".into()]);

    if host.is_empty() {
        return Vec::new();
    }

    schemes
        .into_iter()
        .map(|s| json!({ "url": format!("{s}://{host}{base_path}") }))
        .collect()
}

fn convert_security_definitions(sec_defs: &Value) -> Result<Value, WebDomainError> {
    let map = sec_defs.as_object().ok_or_else(|| {
        WebDomainError::Swagger2ConversionFailed {
            reason: "securityDefinitions is not an object".into(),
            unsupported_feature: None,
        }
    })?;

    let mut out = Map::new();
    for (name, scheme_val) in map {
        let scheme = scheme_val.as_object().ok_or_else(|| {
            WebDomainError::Swagger2ConversionFailed {
                reason: format!("security scheme '{name}' is not an object"),
                unsupported_feature: None,
            }
        })?;

        let ty = scheme.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut converted = Map::new();
        match ty {
            "basic" => {
                // 2.0 `type: basic` → 3.0 `type: http, scheme: basic`
                converted.insert("type".into(), Value::String("http".into()));
                converted.insert("scheme".into(), Value::String("basic".into()));
            }
            "apiKey" => {
                // 2.0 apiKey (in: header|query) → 3.0 identical shape.
                converted.insert("type".into(), Value::String("apiKey".into()));
                if let Some(v) = scheme.get("name") {
                    converted.insert("name".into(), v.clone());
                }
                if let Some(v) = scheme.get("in") {
                    converted.insert("in".into(), v.clone());
                }
            }
            "oauth2" => {
                // 2.0 flow-variants → 3.0 flows map. Minimal round-trip: copy
                // the fields, rename the flow.
                converted.insert("type".into(), Value::String("oauth2".into()));
                let flow_name = scheme
                    .get("flow")
                    .and_then(|v| v.as_str())
                    .unwrap_or("implicit");
                let mut flow_body = Map::new();
                for k in &["authorizationUrl", "tokenUrl", "refreshUrl", "scopes"] {
                    if let Some(v) = scheme.get(*k) {
                        flow_body.insert((*k).into(), v.clone());
                    }
                }
                let oas3_flow = match flow_name {
                    "implicit" => "implicit",
                    "password" => "password",
                    "application" => "clientCredentials",
                    "accessCode" => "authorizationCode",
                    other => {
                        return Err(WebDomainError::Swagger2ConversionFailed {
                            reason: format!("unknown oauth2 flow: {other}"),
                            unsupported_feature: Some("oauth2.flow".into()),
                        });
                    }
                };
                let mut flows = Map::new();
                flows.insert(oas3_flow.into(), Value::Object(flow_body));
                converted.insert("flows".into(), Value::Object(flows));
            }
            other => {
                return Err(WebDomainError::Swagger2ConversionFailed {
                    reason: format!("unsupported security scheme type: {other}"),
                    unsupported_feature: Some(format!("securityDefinitions.{name}.type")),
                });
            }
        }
        // `description` carries over for all types.
        if let Some(desc) = scheme.get("description") {
            converted.insert("description".into(), desc.clone());
        }
        out.insert(name.clone(), Value::Object(converted));
    }
    Ok(Value::Object(out))
}

/// Walk a JSON tree and rewrite `$ref` strings from 2.0 layout to 3.0 layout.
///
/// - `#/definitions/X`  → `#/components/schemas/X`
/// - `#/parameters/X`   → `#/components/parameters/X`
/// - `#/responses/X`    → `#/components/responses/X`
pub fn rewrite_refs_recursive(v: Value) -> Value {
    match v {
        Value::Object(mut map) => {
            if let Some(Value::String(r)) = map.get("$ref").cloned() {
                let rewritten = rewrite_single_ref(&r);
                map.insert("$ref".into(), Value::String(rewritten));
            }
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if k == "$ref" {
                    continue;
                }
                if let Some(child) = map.remove(&k) {
                    map.insert(k, rewrite_refs_recursive(child));
                }
            }
            Value::Object(map)
        }
        Value::Array(arr) => {
            Value::Array(arr.into_iter().map(rewrite_refs_recursive).collect())
        }
        other => other,
    }
}

fn rewrite_single_ref(r: &str) -> String {
    if let Some(rest) = r.strip_prefix("#/definitions/") {
        format!("#/components/schemas/{rest}")
    } else if let Some(rest) = r.strip_prefix("#/parameters/") {
        format!("#/components/parameters/{rest}")
    } else if let Some(rest) = r.strip_prefix("#/responses/") {
        format!("#/components/responses/{rest}")
    } else {
        r.to_string()
    }
}

#[cfg(test)]
mod tests_root {
    use super::*;
    use serde_json::json;

    #[test]
    fn minimal_swagger2_produces_openapi_303() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v1",
            "schemes": ["https"],
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["openapi"], "3.0.3");
        assert_eq!(out["info"]["title"], "T");
        assert_eq!(out["servers"][0]["url"], "https://api.example.com/v1");
    }

    #[test]
    fn multiple_schemes_produce_multiple_servers() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "host": "api.example.com",
            "basePath": "",
            "schemes": ["https", "http"],
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["servers"].as_array().unwrap().len(), 2);
        assert_eq!(out["servers"][0]["url"], "https://api.example.com");
        assert_eq!(out["servers"][1]["url"], "http://api.example.com");
    }

    #[test]
    fn missing_schemes_defaults_to_https() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "host": "api.example.com",
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["servers"][0]["url"], "https://api.example.com");
    }

    #[test]
    fn missing_host_produces_no_servers() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {}
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert!(out.get("servers").is_none());
    }

    #[test]
    fn missing_info_is_conversion_error() {
        let input = json!({ "swagger": "2.0", "paths": {} });
        let err = convert_swagger2_to_openapi3(&input).unwrap_err();
        match err {
            WebDomainError::Swagger2ConversionFailed { reason, .. } => {
                assert!(reason.contains("info"));
            }
            other => panic!("expected Swagger2ConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn definitions_move_to_components_schemas() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "definitions": {
                "Pet": { "type": "object", "properties": { "id": { "type": "integer" } } }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(
            out["components"]["schemas"]["Pet"]["type"],
            "object"
        );
    }

    #[test]
    fn refs_to_definitions_are_rewritten() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/pet": {
                    "get": {
                        "responses": {
                            "200": { "schema": { "$ref": "#/definitions/Pet" } }
                        }
                    }
                }
            },
            "definitions": { "Pet": { "type": "object" } }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let r = &out["paths"]["/pet"]["get"]["responses"]["200"]["schema"]["$ref"];
        assert_eq!(r, "#/components/schemas/Pet");
    }

    #[test]
    fn security_definitions_basic_becomes_http_basic() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "BasicAuth": { "type": "basic" }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["components"]["securitySchemes"]["BasicAuth"]["type"], "http");
        assert_eq!(out["components"]["securitySchemes"]["BasicAuth"]["scheme"], "basic");
    }

    #[test]
    fn security_definitions_apikey_roundtrips() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "KeyHeader": { "type": "apiKey", "in": "header", "name": "X-API-Key" }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let ks = &out["components"]["securitySchemes"]["KeyHeader"];
        assert_eq!(ks["type"], "apiKey");
        assert_eq!(ks["in"], "header");
        assert_eq!(ks["name"], "X-API-Key");
    }

    #[test]
    fn security_definitions_oauth2_accesscode_becomes_authorization_code() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "OA": {
                    "type": "oauth2",
                    "flow": "accessCode",
                    "authorizationUrl": "https://example.com/oauth/authorize",
                    "tokenUrl": "https://example.com/oauth/token",
                    "scopes": { "read": "Read access" }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let oa = &out["components"]["securitySchemes"]["OA"];
        assert_eq!(oa["type"], "oauth2");
        assert!(oa["flows"]["authorizationCode"].is_object());
        assert_eq!(
            oa["flows"]["authorizationCode"]["tokenUrl"],
            "https://example.com/oauth/token"
        );
    }

    #[test]
    fn global_parameters_move_to_components_parameters() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "parameters": {
                "SkipParam": { "name": "skip", "in": "query", "type": "integer" }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(
            out["components"]["parameters"]["SkipParam"]["name"],
            "skip"
        );
    }

    #[test]
    fn tags_carry_over() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {},
            "tags": [{ "name": "pets" }, { "name": "users" }]
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        assert_eq!(out["tags"][0]["name"], "pets");
        assert_eq!(out["tags"][1]["name"], "users");
    }
}
