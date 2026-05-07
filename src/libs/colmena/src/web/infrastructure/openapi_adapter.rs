//! Reqwest-backed implementation of `ApiSpecPort`.
//!
//! Pipeline (see Spec C §Architecture):
//!   1. Normalize URL (Git-forge blob → raw).
//!   2. GET with streamed body; abort if size > cap.
//!   3. Detect HTML-instead-of-spec responses.
//!   4. Detect JSON vs YAML.
//!   5. Detect OpenAPI 3.x vs Swagger 2.0 by root keys.
//!   6. Convert Swagger 2.0 → OpenAPI 3.0 if needed.
//!   7. Parse via `oas3` → map to `ParsedSpec`.
//!   8. Honor ETag / Last-Modified (304).
//!
//! This module contains Tasks 5–7's incremental slices. Tests in this
//! file are structured by pipeline stage so later tasks can add more
//! without breaking earlier ones.

use crate::web::application::url_normalizer::{normalize_forge_url, NormalizedUrl};
use crate::web::domain::{
    ApiKeyLocation, ApiSpecPort, Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec,
    RequestBodySpec, ResponseSpec, SecurityRequirement, SecurityScheme, SpecFetchResult,
    SpecFormat, WebDomainError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;

/// Caps and limits shared across the pipeline stages.
#[derive(Debug, Clone)]
pub struct OpenApiAdapterConfig {
    pub max_bytes: u64,
    pub timeout: Duration,
}

impl Default for OpenApiAdapterConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

pub struct OpenApiAdapter {
    client: reqwest::Client,
    config: OpenApiAdapterConfig,
}

impl OpenApiAdapter {
    pub fn new(config: OpenApiAdapterConfig) -> Result<Self, WebDomainError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent("colmena-api-explorer/0.1")
            .build()
            .map_err(|e| WebDomainError::AdapterInit(format!("reqwest client init: {e}")))?;
        Ok(Self { client, config })
    }

    /// Lower-level fetch stage. Returns the raw bytes plus response
    /// metadata (content-type, ETag, Last-Modified, final URL). Used by
    /// Task 6/7's parsing layer.
    pub(crate) async fn fetch_raw(
        &self,
        input_url: &str,
        if_none_match: Option<&str>,
        if_modified_since: Option<&str>,
    ) -> Result<FetchRawResult, WebDomainError> {
        let NormalizedUrl { resolved, rewritten: _ } = normalize_forge_url(input_url);

        let mut req = self.client.get(&resolved);
        if let Some(etag) = if_none_match {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = if_modified_since {
            req = req.header("If-Modified-Since", lm);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                WebDomainError::Timeout { ms: self.config.timeout.as_millis() as u64 }
            } else {
                WebDomainError::Upstream {
                    status: 0,
                    body: format!("fetch error: {e}"),
                }
            }
        })?;

        let status = resp.status();
        if status.as_u16() == 304 {
            return Ok(FetchRawResult::NotModified);
        }
        if !status.is_success() {
            return Err(WebDomainError::Upstream {
                status: status.as_u16(),
                body: format!("HTTP {} from {resolved}", status.as_u16()),
            });
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let last_modified = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Reject obvious HTML before spending bytes on download.
        if let Some(ct) = &content_type {
            if ct.starts_with("text/html") {
                return Err(WebDomainError::UnexpectedHtmlResponse {
                    url: input_url.to_string(),
                    resolved_url: resolved.clone(),
                });
            }
        }

        // Stream body with size cap. reqwest's content-length hint is best-effort;
        // we count bytes as they arrive and abort past the cap.
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| WebDomainError::Upstream {
                status: 0,
                body: format!("stream error: {e}"),
            })?;
            if (buf.len() as u64) + (chunk.len() as u64) > self.config.max_bytes {
                return Err(WebDomainError::SpecTooLarge {
                    size_bytes: buf.len() as u64 + chunk.len() as u64,
                    limit_bytes: self.config.max_bytes,
                });
            }
            buf.extend_from_slice(&chunk);
        }

        // If content-type was absent, sniff body: HTML bodies usually start with '<'.
        if content_type.is_none() {
            let first = buf
                .iter()
                .copied()
                .find(|b| !b.is_ascii_whitespace());
            if matches!(first, Some(b'<')) {
                return Err(WebDomainError::UnexpectedHtmlResponse {
                    url: input_url.to_string(),
                    resolved_url: resolved.clone(),
                });
            }
        }

        Ok(FetchRawResult::Fresh {
            body: buf,
            content_type,
            etag,
            last_modified,
            resolved_url: resolved,
        })
    }
}

#[derive(Debug)]
pub(crate) enum FetchRawResult {
    Fresh {
        body: Vec<u8>,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        resolved_url: String,
    },
    NotModified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyFormat {
    Json,
    Yaml,
}

pub(crate) fn detect_body_format(body: &[u8]) -> BodyFormat {
    let first = body.iter().copied().find(|b| !b.is_ascii_whitespace());
    match first {
        Some(b'{') | Some(b'[') => BodyFormat::Json,
        _ => BodyFormat::Yaml,
    }
}

pub(crate) fn detect_spec_kind(v: &serde_json::Value) -> Result<SpecFormat, WebDomainError> {
    if let Some(openapi) = v.get("openapi").and_then(|x| x.as_str()) {
        if openapi.starts_with("3.") {
            return Ok(SpecFormat::OpenApi3x);
        }
        return Err(WebDomainError::UnsupportedSpecFormat {
            detected: format!("openapi {openapi}"),
        });
    }
    if let Some(swagger) = v.get("swagger").and_then(|x| x.as_str()) {
        if swagger.starts_with("2.") {
            return Ok(SpecFormat::Swagger20);
        }
        return Err(WebDomainError::UnsupportedSpecFormat {
            detected: format!("swagger {swagger}"),
        });
    }
    let hint = ["asyncapi", "raml", "info", "definitions"]
        .iter()
        .find(|k| v.get(*k).is_some())
        .copied()
        .unwrap_or("(none)");
    Err(WebDomainError::UnsupportedSpecFormat {
        detected: format!("root key '{hint}' not recognized as OpenAPI or Swagger"),
    })
}

/// Parse raw bytes of an OpenAPI 3.x document into `ParsedSpec`.
/// Swagger 2.0 dispatch is added in Task 7.
pub(crate) fn parse_body_to_spec(
    body: &[u8],
    content_type: Option<&str>,
    input_url: &str,
    resolved_url: &str,
) -> Result<ParsedSpec, WebDomainError> {
    let fmt = match content_type {
        Some(ct) if ct.contains("json") => BodyFormat::Json,
        Some(ct) if ct.contains("yaml") || ct.contains("yml") => BodyFormat::Yaml,
        _ => detect_body_format(body),
    };

    let as_value: serde_json::Value = match fmt {
        BodyFormat::Json => serde_json::from_slice(body).map_err(|e| {
            WebDomainError::SpecParseFailed {
                details: format!("json parse: {e}"),
            }
        })?,
        BodyFormat::Yaml => {
            let y: serde_yaml::Value = serde_yaml::from_slice(body).map_err(|e| {
                WebDomainError::SpecParseFailed {
                    details: format!("yaml parse: {e}"),
                }
            })?;
            serde_json::to_value(y).map_err(|e| WebDomainError::SpecParseFailed {
                details: format!("yaml→json: {e}"),
            })?
        }
    };

    let kind = detect_spec_kind(&as_value)?;
    match kind {
        SpecFormat::OpenApi3x => {
            parse_oas3_value(as_value, SpecFormat::OpenApi3x, input_url, resolved_url)
        }
        SpecFormat::Swagger20 => {
            let converted =
                crate::web::application::swagger2_to_oas3::convert_swagger2_to_openapi3(
                    &as_value,
                )?;
            parse_oas3_value(converted, SpecFormat::Swagger20, input_url, resolved_url)
        }
    }
}

fn parse_oas3_value(
    v: serde_json::Value,
    original_format: SpecFormat,
    input_url: &str,
    resolved_url: &str,
) -> Result<ParsedSpec, WebDomainError> {
    let spec: oas3::OpenApiV3Spec =
        serde_json::from_value(v.clone()).map_err(|e| WebDomainError::SpecParseFailed {
            details: format!("oas3 decode: {e}"),
        })?;

    let title = spec.info.title.clone();
    let version = spec.info.version.clone();
    let description = spec.info.description.clone();
    let internal_format = format!("openapi-{}", spec.openapi);
    let tags = spec.tags.iter().map(|t| t.name.clone()).collect();
    let servers = spec
        .servers
        .iter()
        .map(|s| s.url.clone())
        .collect::<Vec<_>>();

    let security_schemes = extract_security_schemes(&v);
    let endpoints = extract_endpoints(&v)?;
    let components_schemas = v
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(|s| s.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    Ok(ParsedSpec {
        resolved_url: resolved_url.to_string(),
        input_url: input_url.to_string(),
        original_format,
        internal_format,
        title,
        version,
        description,
        servers,
        endpoints,
        security_schemes,
        tags,
        components_schemas,
    })
}

/// We walk the raw JSON rather than `oas3`'s typed view so refs we don't
/// inline still carry through — the LLM sees whatever the spec author
/// wrote, even if it's a `$ref` to a component we haven't resolved.
fn extract_endpoints(root: &serde_json::Value) -> Result<Vec<Endpoint>, WebDomainError> {
    let mut out: Vec<Endpoint> = Vec::new();
    let Some(paths) = root.get("paths").and_then(|v| v.as_object()) else {
        return Ok(out);
    };

    for (path, item) in paths {
        let Some(item_obj) = item.as_object() else {
            continue;
        };

        // Path-level parameters apply to every operation under the path.
        let path_level_params: Vec<serde_json::Value> = item_obj
            .get("parameters")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for (method_key, op_val) in item_obj {
            let Some(method) = HttpMethod::parse(method_key) else {
                continue;
            };
            let Some(op) = op_val.as_object() else {
                continue;
            };

            let operation_id = op
                .get("operationId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| generate_operation_id(method, path));

            let summary = op.get("summary").and_then(|v| v.as_str()).map(String::from);
            let description = op
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from);
            let tags = op
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();

            // Merge path-level + op-level parameters; op-level wins on name conflict.
            let mut combined: Vec<serde_json::Value> = path_level_params.clone();
            if let Some(Some(arr)) = op.get("parameters").map(|v| v.as_array()) {
                for p in arr {
                    combined.retain(|existing| {
                        existing.get("name") != p.get("name")
                            || existing.get("in") != p.get("in")
                    });
                    combined.push(p.clone());
                }
            }

            let mut path_params = Vec::new();
            let mut query_params = Vec::new();
            let mut header_params = Vec::new();
            for p in combined {
                let Some(p_obj) = p.as_object() else { continue };
                let spec = parameter_from_json(p_obj);
                match p_obj.get("in").and_then(|v| v.as_str()) {
                    Some("path") => path_params.push(spec),
                    Some("query") => query_params.push(spec),
                    Some("header") => header_params.push(spec),
                    _ => {} // cookie ignored for v1
                }
            }

            let request_body = op.get("requestBody").and_then(request_body_from_json);
            let responses = op
                .get("responses")
                .and_then(|v| v.as_object())
                .map(|map| {
                    let mut out = HashMap::new();
                    for (code, resp) in map {
                        out.insert(code.clone(), response_spec_from_json(resp));
                    }
                    out
                })
                .unwrap_or_default();
            let security = op
                .get("security")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().flat_map(security_requirements_from_array).collect())
                .unwrap_or_default();

            out.push(Endpoint {
                operation_id,
                method,
                path: path.clone(),
                summary,
                description,
                tags,
                path_params,
                query_params,
                header_params,
                request_body,
                responses,
                security,
            });
        }
    }

    Ok(out)
}

fn generate_operation_id(method: HttpMethod, path: &str) -> String {
    // Derive a stable ID when the author didn't provide one.
    // e.g. (POST, /pet/{petId}/uploadImage) → Post_pet_petId_uploadImage
    let cleaned: String = path
        .chars()
        .map(|c| if c == '/' || c == '{' || c == '}' || c == '-' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches('_');
    format!(
        "{}{}",
        match method {
            HttpMethod::Get => "Get_",
            HttpMethod::Put => "Put_",
            HttpMethod::Post => "Post_",
            HttpMethod::Delete => "Delete_",
            HttpMethod::Options => "Options_",
            HttpMethod::Head => "Head_",
            HttpMethod::Patch => "Patch_",
            HttpMethod::Trace => "Trace_",
        },
        cleaned
    )
}

fn parameter_from_json(p: &serde_json::Map<String, serde_json::Value>) -> ParameterSpec {
    let name = p
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default();
    let description = p.get("description").and_then(|v| v.as_str()).map(String::from);
    let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
    let schema = p
        .get("schema")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let param_type = classify_schema(&schema);
    let style = p.get("style").and_then(|v| v.as_str()).map(String::from);
    let explode = p.get("explode").and_then(|v| v.as_bool());
    ParameterSpec {
        name,
        description,
        required,
        param_type,
        style,
        explode,
    }
}

fn classify_schema(schema: &serde_json::Value) -> ParamType {
    let Some(schema) = schema.as_object() else {
        return ParamType::Unknown;
    };
    match schema.get("type").and_then(|v| v.as_str()) {
        Some("string") => ParamType::String,
        Some("integer") => ParamType::Integer,
        Some("number") => ParamType::Number,
        Some("boolean") => ParamType::Boolean,
        Some("array") => {
            let items = schema.get("items").cloned().unwrap_or(serde_json::Value::Null);
            ParamType::Array(Box::new(classify_schema(&items)))
        }
        Some("object") => ParamType::Object,
        _ => {
            if schema.contains_key("$ref") {
                ParamType::Object
            } else {
                ParamType::Unknown
            }
        }
    }
}

fn request_body_from_json(rb: &serde_json::Value) -> Option<RequestBodySpec> {
    let rb_obj = rb.as_object()?;
    let required = rb_obj.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
    let content = rb_obj.get("content").and_then(|v| v.as_object())?;
    // Prefer JSON, fall back to form-urlencoded, multipart, then first key.
    let preferred = ["application/json", "application/x-www-form-urlencoded", "multipart/form-data"];
    let content_type = preferred
        .iter()
        .find(|k| content.contains_key(**k))
        .map(|s| s.to_string())
        .or_else(|| content.keys().next().cloned())?;
    let media = content.get(&content_type)?.as_object()?;
    let schema = media.get("schema").cloned().unwrap_or(serde_json::Value::Null);
    Some(RequestBodySpec {
        content_type,
        required,
        schema,
    })
}

fn response_spec_from_json(resp: &serde_json::Value) -> ResponseSpec {
    let description = resp
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);
    let mut content = HashMap::new();
    if let Some(cmap) = resp.get("content").and_then(|v| v.as_object()) {
        for (ct, media) in cmap {
            if let Some(media_obj) = media.as_object() {
                if let Some(schema) = media_obj.get("schema") {
                    content.insert(ct.clone(), schema.clone());
                }
            }
        }
    }
    ResponseSpec { description, content }
}

fn security_requirements_from_array(v: &serde_json::Value) -> Vec<SecurityRequirement> {
    let Some(obj) = v.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .map(|(scheme, scopes)| SecurityRequirement {
            scheme: scheme.clone(),
            scopes: scopes
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_default(),
        })
        .collect()
}

fn extract_security_schemes(root: &serde_json::Value) -> HashMap<String, SecurityScheme> {
    let mut out = HashMap::new();
    let Some(comps) = root.get("components").and_then(|v| v.as_object()) else {
        return out;
    };
    let Some(ss) = comps.get("securitySchemes").and_then(|v| v.as_object()) else {
        return out;
    };
    for (name, raw) in ss {
        let Some(scheme_obj) = raw.as_object() else { continue };
        let ty = scheme_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let scheme = match ty {
            "http" => SecurityScheme::Http {
                scheme: scheme_obj
                    .get("scheme")
                    .and_then(|v| v.as_str())
                    .unwrap_or("bearer")
                    .to_string(),
                bearer_format: scheme_obj
                    .get("bearerFormat")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            },
            "apiKey" => {
                let location = match scheme_obj.get("in").and_then(|v| v.as_str()).unwrap_or("header") {
                    "query" => ApiKeyLocation::Query,
                    "cookie" => ApiKeyLocation::Cookie,
                    _ => ApiKeyLocation::Header,
                };
                SecurityScheme::ApiKey {
                    name: scheme_obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    location,
                }
            }
            "oauth2" => SecurityScheme::OAuth2 {
                flows: scheme_obj
                    .get("flows")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            },
            "openIdConnect" => SecurityScheme::OpenIdConnect {
                openid_connect_url: scheme_obj
                    .get("openIdConnectUrl")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
            _ => continue,
        };
        out.insert(name.clone(), scheme);
    }
    out
}

#[async_trait]
impl ApiSpecPort for OpenApiAdapter {
    async fn fetch_and_parse(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<SpecFetchResult, WebDomainError> {
        match self.fetch_raw(url, etag, last_modified).await? {
            FetchRawResult::NotModified => Ok(SpecFetchResult::NotModified),
            FetchRawResult::Fresh {
                body,
                content_type,
                etag,
                last_modified,
                resolved_url,
            } => {
                let spec = parse_body_to_spec(&body, content_type.as_deref(), url, &resolved_url)?;
                Ok(SpecFetchResult::Fresh {
                    spec,
                    etag,
                    last_modified,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests_parse_openapi3 {
    use super::*;
    use crate::web::domain::{HttpMethod, SpecFormat};

    fn petstore_yaml() -> &'static str {
        include_str!("../../../tests/fixtures/specs/petstore-3.0.yaml")
    }

    #[test]
    fn detect_format_json_braces() {
        let b = b"  { \"openapi\": \"3.0.3\" }";
        assert!(matches!(
            super::super::openapi_adapter::detect_body_format(b),
            BodyFormat::Json
        ));
    }

    #[test]
    fn detect_format_yaml_fallback() {
        let b = b"openapi: 3.0.3\ninfo: ...";
        assert!(matches!(
            super::super::openapi_adapter::detect_body_format(b),
            BodyFormat::Yaml
        ));
    }

    #[test]
    fn detect_spec_kind_openapi_3x() {
        let v: serde_json::Value = serde_json::from_str(r#"{"openapi": "3.0.3"}"#).unwrap();
        assert_eq!(super::super::openapi_adapter::detect_spec_kind(&v).unwrap(), SpecFormat::OpenApi3x);
    }

    #[test]
    fn detect_spec_kind_swagger_2() {
        let v: serde_json::Value = serde_json::from_str(r#"{"swagger": "2.0"}"#).unwrap();
        assert_eq!(super::super::openapi_adapter::detect_spec_kind(&v).unwrap(), SpecFormat::Swagger20);
    }

    #[test]
    fn detect_spec_kind_rejects_asyncapi() {
        let v: serde_json::Value = serde_json::from_str(r#"{"asyncapi": "2.4.0"}"#).unwrap();
        let err = super::super::openapi_adapter::detect_spec_kind(&v).unwrap_err();
        assert!(matches!(err, WebDomainError::UnsupportedSpecFormat { .. }));
    }

    #[tokio::test]
    async fn parse_petstore_yaml_succeeds() {
        let body = petstore_yaml().as_bytes().to_vec();
        let parsed = super::super::openapi_adapter::parse_body_to_spec(
            &body,
            Some("application/yaml"),
            "https://example.test/petstore.yaml",
            "https://example.test/petstore.yaml",
        )
        .unwrap();
        assert_eq!(parsed.title, "Swagger Petstore");
        assert_eq!(parsed.original_format, SpecFormat::OpenApi3x);
        assert!(parsed.endpoints.len() >= 3);
        // find POST /pet
        let post_pet = parsed
            .endpoints
            .iter()
            .find(|e| e.method == HttpMethod::Post && e.path == "/pet")
            .expect("POST /pet should exist");
        assert!(post_pet.request_body.is_some());
        let rb = post_pet.request_body.as_ref().unwrap();
        assert!(rb.content_type.starts_with("application/"));
    }
}

#[cfg(test)]
mod tests_fetch {
    use super::*;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn adapter_at(config: OpenApiAdapterConfig) -> OpenApiAdapter {
        OpenApiAdapter::new(config).unwrap()
    }

    fn small_yaml() -> &'static str {
        "openapi: 3.0.3\ninfo:\n  title: T\n  version: '1'\npaths: {}\n"
    }

    #[tokio::test]
    async fn fetch_raw_returns_yaml_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"abc\"")
                    .set_body_raw(small_yaml(), "application/yaml"),
            )
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let res = adapter.fetch_raw(&url, None, None).await.unwrap();
        match res {
            FetchRawResult::Fresh { body, content_type, etag, .. } => {
                assert!(std::str::from_utf8(&body).unwrap().contains("openapi: 3.0.3"));
                assert_eq!(content_type.as_deref(), Some("application/yaml"));
                assert_eq!(etag.as_deref(), Some("\"abc\""));
            }
            FetchRawResult::NotModified => panic!("expected Fresh"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_rejects_html_by_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<!DOCTYPE html><html>...", "text/html; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let url = format!("{}/", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::UnexpectedHtmlResponse { resolved_url, .. } => {
                assert_eq!(resolved_url, url);
            }
            other => panic!("expected UnexpectedHtmlResponse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_rejects_html_body_when_content_type_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"   <!DOCTYPE html>...".as_ref()))
            .mount(&server)
            .await;

        let url = format!("{}/", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        assert!(matches!(err, WebDomainError::UnexpectedHtmlResponse { .. }));
    }

    #[tokio::test]
    async fn fetch_raw_enforces_size_cap() {
        let server = MockServer::start().await;
        let big = "x".repeat(10_000);
        Mock::given(method("GET"))
            .and(path("/big.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(big, "application/yaml"),
            )
            .mount(&server)
            .await;

        let url = format!("{}/big.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig {
            max_bytes: 1024,
            ..OpenApiAdapterConfig::default()
        });
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::SpecTooLarge { limit_bytes, .. } => {
                assert_eq!(limit_bytes, 1024);
            }
            other => panic!("expected SpecTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_propagates_if_none_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .and(header_exists("If-None-Match"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let res = adapter
            .fetch_raw(&url, Some("\"abc\""), None)
            .await
            .unwrap();
        assert!(matches!(res, FetchRawResult::NotModified));
    }

    #[tokio::test]
    async fn fetch_raw_maps_500_to_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::Upstream { status: 500, .. } => {}
            other => panic!("expected Upstream(500), got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests_parse_swagger2 {
    use super::*;
    use crate::web::domain::{ApiKeyLocation, HttpMethod, SecurityScheme, SpecFormat};

    fn petstore2_yaml() -> &'static str {
        include_str!("../../../tests/fixtures/specs/petstore-2.0.yaml")
    }

    #[test]
    fn parse_swagger2_petstore_roundtrips() {
        let body = petstore2_yaml().as_bytes().to_vec();
        let parsed = super::super::openapi_adapter::parse_body_to_spec(
            &body,
            Some("application/yaml"),
            "https://example.test/petstore2.yaml",
            "https://example.test/petstore2.yaml",
        )
        .unwrap();

        assert_eq!(parsed.original_format, SpecFormat::Swagger20);
        assert!(parsed.internal_format.starts_with("openapi-3.0"));
        assert_eq!(parsed.title, "Swagger Petstore 2.0");
        assert_eq!(
            parsed.servers,
            vec!["https://petstore.swagger.io/v2".to_string()]
        );
        assert!(!parsed.endpoints.is_empty());

        // POST /pet exists, with a requestBody that came from the body parameter.
        let post_pet = parsed
            .endpoints
            .iter()
            .find(|e| e.method == HttpMethod::Post && e.path == "/pet")
            .expect("POST /pet");
        let rb = post_pet.request_body.as_ref().expect("request body");
        assert_eq!(rb.content_type, "application/json");

        // ApiKeyAuth security scheme survived the conversion.
        let sec = parsed.security_schemes.get("ApiKeyAuth").expect("ApiKeyAuth");
        match sec {
            SecurityScheme::ApiKey { name, location } => {
                assert_eq!(name, "X-API-Key");
                assert_eq!(*location, ApiKeyLocation::Header);
            }
            other => panic!("expected ApiKey, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_and_parse_uses_conditional_get_roundtrip() {
        use wiremock::matchers::{header_exists, method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // First GET → 200 + ETag.
        let body = petstore2_yaml();
        Mock::given(method("GET"))
            .and(wm_path("/ps.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"v1\"")
                    .set_body_raw(body.as_bytes().to_vec(), "application/yaml"),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Second GET with If-None-Match → 304.
        Mock::given(method("GET"))
            .and(wm_path("/ps.yaml"))
            .and(header_exists("If-None-Match"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let url = format!("{}/ps.yaml", server.uri());
        let adapter = OpenApiAdapter::new(OpenApiAdapterConfig::default()).unwrap();
        let first = adapter.fetch_and_parse(&url, None, None).await.unwrap();
        let etag = match first {
            crate::web::domain::SpecFetchResult::Fresh { etag, .. } => etag,
            _ => panic!("expected Fresh"),
        };
        assert_eq!(etag.as_deref(), Some("\"v1\""));

        let second = adapter
            .fetch_and_parse(&url, etag.as_deref(), None)
            .await
            .unwrap();
        assert!(matches!(
            second,
            crate::web::domain::SpecFetchResult::NotModified
        ));
    }
}
