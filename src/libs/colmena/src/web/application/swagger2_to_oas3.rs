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
    let root = input
        .as_object()
        .ok_or_else(|| WebDomainError::Swagger2ConversionFailed {
            reason: "root is not a JSON object".into(),
            unsupported_feature: None,
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

    let global_consumes = root
        .get("consumes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let global_produces = root
        .get("produces")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let paths = root.get("paths").cloned().unwrap_or(json!({}));
    let paths = rewrite_refs_recursive(paths);
    let paths = convert_operations(paths, &global_consumes, &global_produces)?;
    out.insert("paths".into(), paths);

    // components
    let mut components = Map::new();
    if let Some(defs) = root.get("definitions") {
        components.insert("schemas".into(), rewrite_refs_recursive(defs.clone()));
    }
    if let Some(global_params) = root.get("parameters") {
        components.insert(
            "parameters".into(),
            rewrite_refs_recursive(global_params.clone()),
        );
    }
    if let Some(global_responses) = root.get("responses") {
        components.insert(
            "responses".into(),
            rewrite_refs_recursive(global_responses.clone()),
        );
    }
    if let Some(sec_defs) = root.get("securityDefinitions") {
        components.insert(
            "securitySchemes".into(),
            convert_security_definitions(sec_defs)?,
        );
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
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
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
    let map = sec_defs
        .as_object()
        .ok_or_else(|| WebDomainError::Swagger2ConversionFailed {
            reason: "securityDefinitions is not an object".into(),
            unsupported_feature: None,
        })?;

    let mut out = Map::new();
    for (name, scheme_val) in map {
        let scheme =
            scheme_val
                .as_object()
                .ok_or_else(|| WebDomainError::Swagger2ConversionFailed {
                    reason: format!("security scheme '{name}' is not an object"),
                    unsupported_feature: None,
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
        Value::Array(arr) => Value::Array(arr.into_iter().map(rewrite_refs_recursive).collect()),
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

/// Walk each path → method → operation and rewrite 2.0-isms.
fn convert_operations(
    paths: Value,
    global_consumes: &[Value],
    global_produces: &[Value],
) -> Result<Value, WebDomainError> {
    let Value::Object(mut path_map) = paths else {
        return Ok(paths);
    };
    let path_keys: Vec<String> = path_map.keys().cloned().collect();
    for path_key in path_keys {
        let Some(path_item) = path_map.get_mut(&path_key) else {
            continue;
        };
        let Value::Object(item_map) = path_item else {
            continue;
        };
        let method_keys: Vec<String> = item_map.keys().cloned().collect();
        for method_key in method_keys {
            if !is_http_method(&method_key) {
                continue;
            }
            let Some(op_val) = item_map.get_mut(&method_key) else {
                continue;
            };
            let Value::Object(op) = op_val else { continue };
            convert_single_operation(op, global_consumes, global_produces)?;
        }
    }
    Ok(Value::Object(path_map))
}

fn is_http_method(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

fn convert_single_operation(
    op: &mut Map<String, Value>,
    global_consumes: &[Value],
    global_produces: &[Value],
) -> Result<(), WebDomainError> {
    // Determine effective consumes / produces.
    let consumes: Vec<Value> = op
        .get("consumes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_else(|| global_consumes.to_vec());
    let produces: Vec<Value> = op
        .get("produces")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_else(|| global_produces.to_vec());
    op.remove("consumes");
    op.remove("produces");

    // Split parameters: body + formData vs the rest.
    let mut new_params: Vec<Value> = Vec::new();
    let mut body_param: Option<Value> = None;
    let mut form_data: Vec<Value> = Vec::new();

    if let Some(Value::Array(params)) = op.remove("parameters") {
        for p in params {
            let Value::Object(mut p_obj) = p else {
                new_params.push(p);
                continue;
            };
            let kind = p_obj.get("in").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "body" => body_param = Some(Value::Object(p_obj)),
                "formData" => form_data.push(Value::Object(p_obj)),
                _ => {
                    convert_param_collection_format(&mut p_obj)?;
                    new_params.push(Value::Object(p_obj));
                }
            }
        }
    }

    if !new_params.is_empty() {
        op.insert("parameters".into(), Value::Array(new_params));
    }

    // body → requestBody
    if let Some(Value::Object(mut bp)) = body_param {
        let required = bp.remove("required").unwrap_or(Value::Bool(false));
        let description = bp.remove("description");
        let schema = bp.remove("schema").unwrap_or(json!({}));

        let content_type = pick_first_content_type(&consumes, "application/json");
        let mut content = Map::new();
        let mut media = Map::new();
        media.insert("schema".into(), schema);
        content.insert(content_type, Value::Object(media));

        let mut rb = Map::new();
        rb.insert("required".into(), required);
        if let Some(d) = description {
            rb.insert("description".into(), d);
        }
        rb.insert("content".into(), Value::Object(content));
        op.insert("requestBody".into(), Value::Object(rb));
    }

    // formData → requestBody (urlencoded or multipart)
    if !form_data.is_empty() {
        let has_file = form_data
            .iter()
            .any(|p| p.get("type").and_then(|v| v.as_str()) == Some("file"));
        let media_type = if has_file {
            "multipart/form-data"
        } else {
            // Honor the operation's consumes if it names form-urlencoded; otherwise default.
            pick_first_consume_for_form(&consumes).unwrap_or("application/x-www-form-urlencoded")
        }
        .to_string();

        let mut properties = Map::new();
        let mut required_names: Vec<Value> = Vec::new();

        for p in form_data {
            let Value::Object(p_obj) = p else { continue };
            let Some(name) = p_obj.get("name").and_then(|v| v.as_str()).map(String::from) else {
                continue;
            };
            let mut prop = Map::new();
            if p_obj.get("type").and_then(|v| v.as_str()) == Some("file") {
                prop.insert("type".into(), Value::String("string".into()));
                prop.insert("format".into(), Value::String("binary".into()));
            } else if let Some(ty) = p_obj.get("type") {
                prop.insert("type".into(), ty.clone());
                if let Some(fmt) = p_obj.get("format") {
                    prop.insert("format".into(), fmt.clone());
                }
                if let Some(items) = p_obj.get("items") {
                    prop.insert("items".into(), items.clone());
                }
                if let Some(en) = p_obj.get("enum") {
                    prop.insert("enum".into(), en.clone());
                }
            }
            if let Some(desc) = p_obj.get("description") {
                prop.insert("description".into(), desc.clone());
            }
            if p_obj
                .get("required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                required_names.push(Value::String(name.clone()));
            }
            properties.insert(name, Value::Object(prop));
        }

        let mut schema = Map::new();
        schema.insert("type".into(), Value::String("object".into()));
        schema.insert("properties".into(), Value::Object(properties));
        if !required_names.is_empty() {
            schema.insert("required".into(), Value::Array(required_names));
        }

        let mut media = Map::new();
        media.insert("schema".into(), Value::Object(schema));

        let mut content = Map::new();
        content.insert(media_type, Value::Object(media));

        let mut rb = Map::new();
        rb.insert("required".into(), Value::Bool(true));
        rb.insert("content".into(), Value::Object(content));
        op.insert("requestBody".into(), Value::Object(rb));
    }

    // responses.<code>.schema → responses.<code>.content.<produce>.schema
    if let Some(Value::Object(ref mut responses)) = op.get_mut("responses") {
        let codes: Vec<String> = responses.keys().cloned().collect();
        for code in codes {
            let Some(Value::Object(ref mut resp)) = responses.get_mut(&code) else {
                continue;
            };
            if let Some(schema) = resp.remove("schema") {
                let content_type = pick_first_content_type(&produces, "application/json");
                let mut media = Map::new();
                media.insert("schema".into(), schema);
                let mut content = Map::new();
                content.insert(content_type, Value::Object(media));
                resp.insert("content".into(), Value::Object(content));
            }
        }
    }

    Ok(())
}

fn pick_first_content_type(list: &[Value], default: &str) -> String {
    list.iter()
        .find_map(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| default.to_string())
}

fn pick_first_consume_for_form(list: &[Value]) -> Option<&'static str> {
    for v in list {
        match v.as_str() {
            Some("application/x-www-form-urlencoded") => {
                return Some("application/x-www-form-urlencoded")
            }
            Some("multipart/form-data") => return Some("multipart/form-data"),
            _ => {}
        }
    }
    None
}

/// Translate Swagger 2.0 `collectionFormat` on a parameter to OpenAPI 3.0
/// `style` + `explode`. Errors on `tsv` (no 3.0 equivalent).
fn convert_param_collection_format(p: &mut Map<String, Value>) -> Result<(), WebDomainError> {
    let Some(Value::String(cf)) = p.remove("collectionFormat") else {
        return Ok(());
    };
    match cf.as_str() {
        "csv" => {
            p.insert("style".into(), Value::String("form".into()));
            p.insert("explode".into(), Value::Bool(false));
        }
        "multi" => {
            p.insert("style".into(), Value::String("form".into()));
            p.insert("explode".into(), Value::Bool(true));
        }
        "ssv" => {
            p.insert("style".into(), Value::String("spaceDelimited".into()));
        }
        "pipes" => {
            p.insert("style".into(), Value::String("pipeDelimited".into()));
        }
        "tsv" => {
            return Err(WebDomainError::Swagger2ConversionFailed {
                reason: "collectionFormat: tsv has no OpenAPI 3.0 equivalent".into(),
                unsupported_feature: Some("collectionFormat.tsv".into()),
            });
        }
        other => {
            return Err(WebDomainError::Swagger2ConversionFailed {
                reason: format!("unknown collectionFormat: {other}"),
                unsupported_feature: Some(format!("collectionFormat.{other}")),
            });
        }
    }
    Ok(())
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
        assert_eq!(out["components"]["schemas"]["Pet"]["type"], "object");
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
        // After operation conversion, schema moves into content.<media-type>.schema
        let r = &out["paths"]["/pet"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"];
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
        assert_eq!(
            out["components"]["securitySchemes"]["BasicAuth"]["type"],
            "http"
        );
        assert_eq!(
            out["components"]["securitySchemes"]["BasicAuth"]["scheme"],
            "basic"
        );
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
        assert_eq!(out["components"]["parameters"]["SkipParam"]["name"], "skip");
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

#[cfg(test)]
mod tests_operations {
    use super::*;
    use serde_json::json;

    fn petstore_post_with_body() -> Value {
        json!({
            "swagger": "2.0",
            "info": { "title": "Pet", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v2",
            "schemes": ["https"],
            "consumes": ["application/json"],
            "produces": ["application/json"],
            "paths": {
                "/pet": {
                    "post": {
                        "operationId": "addPet",
                        "parameters": [
                            {
                                "in": "body",
                                "name": "body",
                                "required": true,
                                "schema": { "$ref": "#/definitions/Pet" }
                            }
                        ],
                        "responses": {
                            "200": { "description": "ok", "schema": { "$ref": "#/definitions/Pet" } }
                        }
                    }
                }
            },
            "definitions": {
                "Pet": { "type": "object", "properties": { "id": { "type": "integer" } } }
            }
        })
    }

    #[test]
    fn body_param_becomes_request_body() {
        let out = convert_swagger2_to_openapi3(&petstore_post_with_body()).unwrap();
        let op = &out["paths"]["/pet"]["post"];
        assert!(op["parameters"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true));
        let rb = &op["requestBody"];
        assert_eq!(rb["required"], true);
        assert_eq!(
            rb["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Pet"
        );
    }

    #[test]
    fn response_schema_becomes_content_schema() {
        let out = convert_swagger2_to_openapi3(&petstore_post_with_body()).unwrap();
        let resp = &out["paths"]["/pet"]["post"]["responses"]["200"];
        assert!(resp["schema"].is_null() || resp.get("schema").is_none());
        assert_eq!(
            resp["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Pet"
        );
        assert_eq!(resp["description"], "ok");
    }

    #[test]
    fn formdata_becomes_urlencoded_request_body() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/login": {
                    "post": {
                        "parameters": [
                            { "in": "formData", "name": "user",     "type": "string", "required": true },
                            { "in": "formData", "name": "password", "type": "string", "required": true }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let rb = &out["paths"]["/login"]["post"]["requestBody"];
        let schema = &rb["content"]["application/x-www-form-urlencoded"]["schema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["user"].is_object());
        assert!(schema["properties"]["password"].is_object());
        let req = schema["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "user"));
        assert!(req.iter().any(|v| v == "password"));
    }

    #[test]
    fn formdata_with_file_becomes_multipart() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "parameters": [
                            { "in": "formData", "name": "file", "type": "file", "required": true },
                            { "in": "formData", "name": "caption", "type": "string" }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let rb = &out["paths"]["/upload"]["post"]["requestBody"];
        let schema = &rb["content"]["multipart/form-data"]["schema"];
        assert_eq!(schema["properties"]["file"]["type"], "string");
        assert_eq!(schema["properties"]["file"]["format"], "binary");
        assert_eq!(schema["properties"]["caption"]["type"], "string");
    }

    #[test]
    fn collection_format_csv_becomes_form_no_explode() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "csv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "form");
        assert_eq!(p["explode"], false);
        assert!(p.get("collectionFormat").is_none());
    }

    #[test]
    fn collection_format_multi_becomes_form_explode_true() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "multi"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "form");
        assert_eq!(p["explode"], true);
    }

    #[test]
    fn collection_format_ssv_becomes_space_delimited() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "ssv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "spaceDelimited");
    }

    #[test]
    fn collection_format_pipes_becomes_pipe_delimited() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "pipes"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let p = &out["paths"]["/things"]["get"]["parameters"][0];
        assert_eq!(p["style"], "pipeDelimited");
    }

    #[test]
    fn collection_format_tsv_is_conversion_error() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "paths": {
                "/things": {
                    "get": {
                        "parameters": [{
                            "in": "query", "name": "ids",
                            "type": "array", "items": { "type": "string" },
                            "collectionFormat": "tsv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let err = convert_swagger2_to_openapi3(&input).unwrap_err();
        match err {
            WebDomainError::Swagger2ConversionFailed {
                unsupported_feature,
                ..
            } => {
                assert_eq!(unsupported_feature.as_deref(), Some("collectionFormat.tsv"));
            }
            other => panic!("expected Swagger2ConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn operation_consumes_overrides_global_consumes() {
        let input = json!({
            "swagger": "2.0",
            "info": { "title": "T", "version": "1" },
            "consumes": ["application/json"],
            "paths": {
                "/form": {
                    "post": {
                        "consumes": ["application/x-www-form-urlencoded"],
                        "parameters": [
                            { "in": "formData", "name": "x", "type": "string", "required": true }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });
        let out = convert_swagger2_to_openapi3(&input).unwrap();
        let rb = &out["paths"]["/form"]["post"]["requestBody"];
        assert!(rb["content"]["application/x-www-form-urlencoded"].is_object());
        assert!(rb
            .get("content")
            .and_then(|c| c.get("application/json"))
            .is_none());
    }
}
