//! `ApiSpecUseCase` — orchestrates fetch / cache / search / build for a
//! single conversation.

use crate::web::domain::{
    ApiSpecPort, ParsedSpec, SessionKey, SessionRegistry, SpecFetchResult, WebDomainError,
};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Tunables for the use case. Defaults match the Spec C "minimal config" values.
#[derive(Debug, Clone)]
pub struct ApiSpecUseCaseConfig {
    pub enable_cache: bool,
    pub cache_ttl: Duration,
    pub max_cached_specs: usize,
    pub fuzzy_match_threshold: f32,
    pub default_base_url_override: Option<String>,
}

impl Default for ApiSpecUseCaseConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_ttl: Duration::from_secs(86_400),
            max_cached_specs: 100,
            fuzzy_match_threshold: 0.1,
            default_base_url_override: None,
        }
    }
}

/// Per-conversation cache of parsed specs.
pub struct SpecCache {
    specs: Mutex<LruCache<String, CachedSpec>>,
}

impl SpecCache {
    pub fn new(max: usize) -> Self {
        Self {
            specs: Mutex::new(LruCache::new(
                NonZeroUsize::new(max.max(1)).unwrap(),
            )),
        }
    }
}

#[derive(Clone)]
pub struct CachedSpec {
    pub parsed: Arc<ParsedSpec>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cached_at: Instant,
}

pub struct ApiSpecUseCase {
    port: Arc<dyn ApiSpecPort>,
    registry: Arc<SessionRegistry<Arc<SpecCache>>>,
    config: ApiSpecUseCaseConfig,
}

impl ApiSpecUseCase {
    pub fn new(
        port: Arc<dyn ApiSpecPort>,
        registry: Arc<SessionRegistry<Arc<SpecCache>>>,
        config: ApiSpecUseCaseConfig,
    ) -> Self {
        Self { port, registry, config }
    }

    /// Fetch-or-reuse a spec for a given conversation.
    ///
    /// Returns `(entry, was_cached)`:
    /// * `was_cached = true` when the in-memory cache satisfied the request
    ///   without contacting the port.
    /// * `was_cached = false` for a fresh fetch, a forced reload, or a 304
    ///   revalidation (the network was hit either way).
    pub async fn fetch_spec(
        &self,
        conversation_id: &str,
        input_url: &str,
        force_reload: bool,
    ) -> Result<(CachedSpec, bool), WebDomainError> {
        let key = SessionKey::new(conversation_id, "api_explorer");

        // Get-or-create the per-conversation cache.
        let cache = self
            .registry
            .with_entry(&key, |c| c.clone())
            .await
            .unwrap_or_else(|| Arc::new(SpecCache::new(self.config.max_cached_specs)));
        // Insert if absent so future lookups share the Arc.
        self.registry.insert(key.clone(), cache.clone()).await;

        if self.config.enable_cache && !force_reload {
            if let Some(hit) = cache
                .specs
                .lock()
                .await
                .get(input_url)
                .cloned()
            {
                if hit.cached_at.elapsed() < self.config.cache_ttl {
                    // Fresh enough — skip revalidation to avoid network.
                    return Ok((hit, true));
                }
            }
        }

        // Either no cache entry, stale, or forced reload. Re-fetch.
        let previous = cache.specs.lock().await.get(input_url).cloned();
        let etag = previous.as_ref().and_then(|c| c.etag.clone());
        let last_modified = previous.as_ref().and_then(|c| c.last_modified.clone());

        match self
            .port
            .fetch_and_parse(input_url, etag.as_deref(), last_modified.as_deref())
            .await?
        {
            SpecFetchResult::Fresh {
                spec,
                etag,
                last_modified,
            } => {
                let resolved = spec.resolved_url.clone();
                let entry = CachedSpec {
                    parsed: Arc::new(spec),
                    etag,
                    last_modified,
                    cached_at: Instant::now(),
                };
                let mut locked = cache.specs.lock().await;
                locked.put(input_url.to_string(), entry.clone());
                // Also index by resolved_url so the LLM can use either form.
                if resolved != input_url {
                    locked.put(resolved, entry.clone());
                }
                Ok((entry, false))
            }
            SpecFetchResult::NotModified => {
                let mut prev = previous.ok_or_else(|| WebDomainError::AdapterInit(
                    "got 304 Not Modified without a cached spec".into(),
                ))?;
                prev.cached_at = Instant::now();
                cache
                    .specs
                    .lock()
                    .await
                    .put(input_url.to_string(), prev.clone());
                Ok((prev, false))
            }
        }
    }

    /// Look up a previously-fetched spec for this conversation.
    ///
    /// Returns [`WebDomainError::SpecNotLoaded`] when no entry exists. This
    /// is the recoverable shape that the LLM sees when it calls a handler
    /// (`list_endpoints`, `search_endpoint`, …) before `load_spec` has
    /// populated the cache.
    pub async fn lookup_cached(
        &self,
        conversation_id: &str,
        spec_url: &str,
    ) -> Result<Arc<ParsedSpec>, WebDomainError> {
        let key = SessionKey::new(conversation_id, "api_explorer");
        let cache = self.registry.with_entry(&key, |c| c.clone()).await;
        let cache = match cache {
            Some(c) => c,
            None => {
                return Err(WebDomainError::SpecNotLoaded {
                    spec_url: spec_url.to_string(),
                });
            }
        };
        let entry = cache.specs.lock().await.get(spec_url).cloned();
        match entry {
            Some(e) => Ok(e.parsed.clone()),
            None => Err(WebDomainError::SpecNotLoaded {
                spec_url: spec_url.to_string(),
            }),
        }
    }

    /// Conversation-scoped wrapper around [`list_endpoints`].
    pub async fn list_endpoints(
        &self,
        conversation_id: &str,
        spec_url: &str,
        tag: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<EndpointListPage, WebDomainError> {
        let spec = self.lookup_cached(conversation_id, spec_url).await?;
        Ok(list_endpoints(&spec, tag, limit, offset))
    }

    /// Conversation-scoped wrapper around [`search_endpoint`].
    pub async fn search_endpoint(
        &self,
        conversation_id: &str,
        spec_url: &str,
        query: &str,
        method: Option<&str>,
        max_results: usize,
    ) -> Result<Vec<EndpointSearchHit>, WebDomainError> {
        let spec = self.lookup_cached(conversation_id, spec_url).await?;
        Ok(search_endpoint(
            &spec,
            query,
            method,
            max_results,
            self.config.fuzzy_match_threshold,
        ))
    }

    /// Conversation-scoped wrapper around [`get_endpoint_details`].
    pub async fn get_endpoint_details(
        &self,
        conversation_id: &str,
        spec_url: &str,
        operation_id: &str,
    ) -> Result<Value, WebDomainError> {
        let spec = self.lookup_cached(conversation_id, spec_url).await?;
        get_endpoint_details(&spec, operation_id)
    }

    /// Conversation-scoped wrapper around [`build_http_request`].
    pub async fn build_http_request(
        &self,
        conversation_id: &str,
        spec_url: &str,
        operation_id: &str,
        params: &Value,
        auth_secret_ref: Option<&str>,
    ) -> Result<Value, WebDomainError> {
        let spec = self.lookup_cached(conversation_id, spec_url).await?;
        build_http_request(&spec, operation_id, params, auth_secret_ref)
    }

    /// Public accessor for tests + later tasks that need the registry's
    /// Arc (e.g. lifecycle subscription).
    pub fn registry(&self) -> Arc<SessionRegistry<Arc<SpecCache>>> {
        self.registry.clone()
    }
}

use crate::web::domain::{Endpoint, HttpMethod, ParamType, ParameterSpec};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct EndpointListPage {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub endpoints: Vec<EndpointSummary>,
}

#[derive(Debug, Clone)]
pub struct EndpointSummary {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
}

impl From<&Endpoint> for EndpointSummary {
    fn from(e: &Endpoint) -> Self {
        Self {
            operation_id: e.operation_id.clone(),
            method: e.method.as_str().to_string(),
            path: e.path.clone(),
            summary: e.summary.clone(),
            tags: e.tags.clone(),
        }
    }
}

pub fn list_endpoints(
    spec: &ParsedSpec,
    tag: Option<&str>,
    limit: usize,
    offset: usize,
) -> EndpointListPage {
    let filtered: Vec<&Endpoint> = spec
        .endpoints
        .iter()
        .filter(|e| match tag {
            None => true,
            Some(t) => e.tags.iter().any(|candidate| candidate == t),
        })
        .collect();
    let total = filtered.len();
    let slice: Vec<EndpointSummary> = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(EndpointSummary::from)
        .collect();
    let returned = slice.len();
    EndpointListPage {
        total,
        returned,
        offset,
        endpoints: slice,
    }
}

#[derive(Debug, Clone)]
pub struct EndpointSearchHit {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub score: f32,
    pub match_reason: String,
}

/// Fuzzy-search endpoints by a free-text query. The input is tokenized
/// and each token is scored independently against a concatenated
/// searchable string; the final score is the normalized sum.
pub fn search_endpoint(
    spec: &ParsedSpec,
    query: &str,
    method_filter: Option<&str>,
    max_results: usize,
    threshold: f32,
) -> Vec<EndpointSearchHit> {
    use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
    use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32Str};

    let method_filter = method_filter.and_then(HttpMethod::parse);

    let mut matcher = Matcher::new(NucleoConfig::DEFAULT);

    let candidates: Vec<(&Endpoint, String)> = spec
        .endpoints
        .iter()
        .filter(|e| match method_filter {
            None => true,
            Some(m) => e.method == m,
        })
        .map(|e| {
            let mut haystack = String::new();
            haystack.push_str(&e.path);
            haystack.push(' ');
            haystack.push_str(&e.operation_id);
            haystack.push(' ');
            if let Some(s) = &e.summary {
                haystack.push_str(s);
                haystack.push(' ');
            }
            if let Some(d) = &e.description {
                haystack.push_str(d);
                haystack.push(' ');
            }
            for t in &e.tags {
                haystack.push_str(t);
                haystack.push(' ');
            }
            (e, haystack)
        })
        .collect();

    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    // Score each haystack with nucleo.
    let mut scored: Vec<(f32, &Endpoint, String)> = Vec::new();
    let mut hay_buf = Vec::new();
    for (ep, hay) in &candidates {
        hay_buf.clear();
        let hay_u32 = Utf32Str::new(hay, &mut hay_buf);
        if let Some(raw) = pattern.score(hay_u32, &mut matcher) {
            let normalized = (raw as f32) / (hay.chars().count().max(1) as f32 * 16.0);
            if normalized >= threshold {
                let reason = explain_match(query, ep);
                scored.push((normalized.min(1.0), ep, reason));
            }
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(max_results)
        .map(|(score, ep, reason)| EndpointSearchHit {
            operation_id: ep.operation_id.clone(),
            method: ep.method.as_str().to_string(),
            path: ep.path.clone(),
            summary: ep.summary.clone(),
            score,
            match_reason: reason,
        })
        .collect()
}

/// Look up a single endpoint by `operation_id` and return a verbose JSON
/// description of its parameters, request body, responses, and security.
///
/// Returns [`WebDomainError::EndpointNotFound`] with fuzzy `did_you_mean`
/// suggestions when the `operation_id` is not found.
pub fn get_endpoint_details(
    spec: &ParsedSpec,
    operation_id: &str,
) -> Result<Value, WebDomainError> {
    let ep = match spec.endpoints.iter().find(|e| e.operation_id == operation_id) {
        Some(e) => e,
        None => {
            // Return top-3 fuzzy suggestions.
            let hits = search_endpoint(spec, operation_id, None, 3, 0.0);
            return Err(WebDomainError::EndpointNotFound {
                searched_for: operation_id.to_string(),
                did_you_mean: hits.into_iter().map(|h| h.operation_id).collect(),
            });
        }
    };

    fn params_to_json(params: &[ParameterSpec]) -> Value {
        Value::Array(
            params
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "type": param_type_str(&p.param_type),
                        "required": p.required,
                        "description": p.description,
                        "style": p.style,
                        "explode": p.explode,
                    })
                })
                .collect(),
        )
    }

    fn param_type_str(t: &ParamType) -> String {
        match t {
            ParamType::String => "string".into(),
            ParamType::Integer => "integer".into(),
            ParamType::Number => "number".into(),
            ParamType::Boolean => "boolean".into(),
            ParamType::Array(_) => "array".into(),
            ParamType::Object => "object".into(),
            ParamType::Unknown => "string".into(),
        }
    }

    let request_body = ep.request_body.as_ref().map(|rb| {
        json!({
            "content_type": rb.content_type,
            "required": rb.required,
            "schema": rb.schema,
        })
    });

    let responses = {
        let mut m = serde_json::Map::new();
        for (code, r) in &ep.responses {
            let content: serde_json::Map<String, Value> = r
                .content
                .iter()
                .map(|(ct, schema)| (ct.clone(), schema.clone()))
                .collect();
            m.insert(
                code.clone(),
                json!({
                    "description": r.description,
                    "content": Value::Object(content),
                }),
            );
        }
        Value::Object(m)
    };

    let security = Value::Array(
        ep.security
            .iter()
            .map(|s| {
                json!({ "scheme": s.scheme, "scopes": s.scopes })
            })
            .collect(),
    );

    Ok(json!({
        "operation_id": ep.operation_id,
        "method": ep.method.as_str(),
        "path": ep.path,
        "summary": ep.summary,
        "description": ep.description,
        "path_parameters": params_to_json(&ep.path_params),
        "query_parameters": params_to_json(&ep.query_params),
        "header_parameters": params_to_json(&ep.header_params),
        "request_body": request_body,
        "responses": responses,
        "security": security,
    }))
}

fn explain_match(query: &str, ep: &Endpoint) -> String {
    let q = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    if ep.path.to_ascii_lowercase().contains(&q) {
        hits.push(format!("path matches '{}'", q));
    }
    if let Some(s) = &ep.summary {
        if s.to_ascii_lowercase().contains(&q) {
            hits.push(format!("summary matches '{}'", q));
        }
    }
    if ep.operation_id.to_ascii_lowercase().contains(&q) {
        hits.push(format!("operation_id matches '{}'", q));
    }
    if hits.is_empty() {
        "fuzzy match across path/summary/description/tags".into()
    } else {
        hits.join("; ")
    }
}

use crate::web::domain::{ApiKeyLocation, RequestBodySpec, SecurityScheme};

/// Given an endpoint + parameter map + optional auth secret reference,
/// emit the JSON object the `http_request` node consumes.
///
/// # Errors
/// - [`WebDomainError::EndpointNotFound`] — `operation_id` not in spec.
/// - [`WebDomainError::MissingRequiredParams`] — one or more required params absent.
/// - [`WebDomainError::InvalidParamType`] — value cannot be coerced to declared type.
/// - [`WebDomainError::MissingAuth`] — endpoint requires auth but `auth_secret_ref` is `None`.
/// - [`WebDomainError::InvalidConfig`] — spec is internally inconsistent (missing security scheme
///   definition, no `servers[]`, unsupported content-type).
pub fn build_http_request(
    spec: &ParsedSpec,
    operation_id: &str,
    params: &Value,
    auth_secret_ref: Option<&str>,
) -> Result<Value, WebDomainError> {
    let ep = spec
        .endpoints
        .iter()
        .find(|e| e.operation_id == operation_id)
        .ok_or_else(|| WebDomainError::EndpointNotFound {
            searched_for: operation_id.to_string(),
            did_you_mean: search_endpoint(spec, operation_id, None, 3, 0.0)
                .into_iter()
                .map(|h| h.operation_id)
                .collect(),
        })?;

    let params_obj = params
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);

    // ---- Path parameters ----
    let mut url_path = ep.path.clone();
    let mut missing: Vec<String> = Vec::new();
    for p in &ep.path_params {
        match params_obj.get(&p.name) {
            Some(v) => {
                let coerced = coerce_scalar(v, &p.param_type, &p.name)?;
                url_path = url_path.replace(&format!("{{{}}}", p.name), &coerced);
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }

    // ---- Query parameters ----
    let mut query_params = serde_json::Map::new();
    for p in &ep.query_params {
        match params_obj.get(&p.name) {
            Some(v) => {
                query_params.insert(p.name.clone(), encode_param(v, p)?);
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }

    // ---- Header parameters ----
    let mut headers = serde_json::Map::new();
    for p in &ep.header_params {
        match params_obj.get(&p.name) {
            Some(v) => {
                let coerced = coerce_scalar(v, &p.param_type, &p.name)?;
                headers.insert(p.name.clone(), Value::String(coerced));
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }

    // ---- Body ----
    let mut body_value: Value = Value::Null;
    if let Some(rb) = &ep.request_body {
        let (body, body_missing) = build_body(rb, &params_obj)?;
        if rb.required {
            for m in body_missing {
                missing.push(m);
            }
        }
        body_value = body;
        headers.insert("Content-Type".into(), Value::String(rb.content_type.clone()));
    }

    if !missing.is_empty() {
        return Err(WebDomainError::MissingRequiredParams {
            missing,
            hints: Some(format!(
                "The listed parameters are required for {}. Check get_endpoint_details for descriptions.",
                ep.operation_id
            )),
        });
    }

    // ---- Security ----
    if !ep.security.is_empty() {
        let required = &ep.security[0]; // use the first security option
        let scheme = spec.security_schemes.get(&required.scheme).ok_or_else(|| {
            WebDomainError::InvalidConfig(format!(
                "endpoint declares security scheme '{}' but the spec has no matching entry in components.securitySchemes",
                required.scheme
            ))
        })?;
        let secret_ref = auth_secret_ref.ok_or_else(|| WebDomainError::MissingAuth {
            scheme: required.scheme.clone(),
            message: format!(
                "endpoint '{}' requires auth scheme '{}' but no auth_secret_ref was provided",
                ep.operation_id, required.scheme
            ),
        })?;
        apply_security_scheme(scheme, secret_ref, &mut headers, &mut query_params);
    }

    // ---- Base URL ----
    let base = spec
        .servers
        .first()
        .cloned()
        .ok_or_else(|| WebDomainError::InvalidConfig(
            "spec has no servers[]; set default_base_url_override in the node config".into(),
        ))?;
    let url = format!("{}{}", base.trim_end_matches('/'), url_path);

    Ok(json!({
        "url": url,
        "method": ep.method.as_str(),
        "headers": headers,
        "query_params": query_params,
        "body": body_value,
    }))
}

fn coerce_scalar(v: &Value, ty: &ParamType, name: &str) -> Result<String, WebDomainError> {
    match (v, ty) {
        (Value::String(s), ParamType::Integer) => {
            s.parse::<i64>()
                .map(|n| n.to_string())
                .map_err(|_| WebDomainError::InvalidParamType {
                    param: name.into(),
                    expected_type: "integer".into(),
                    got: format!("\"{s}\""),
                })
        }
        (Value::String(s), ParamType::Number) => {
            s.parse::<f64>()
                .map(|n| n.to_string())
                .map_err(|_| WebDomainError::InvalidParamType {
                    param: name.into(),
                    expected_type: "number".into(),
                    got: format!("\"{s}\""),
                })
        }
        (Value::Number(n), ParamType::Integer) => Ok(n.as_i64()
            .map(|i| i.to_string())
            .unwrap_or_else(|| n.to_string())),
        (Value::Number(n), ParamType::Number) => Ok(n.to_string()),
        (Value::Bool(b), ParamType::Boolean) => Ok(b.to_string()),
        (Value::String(s), ParamType::Boolean) => match s.as_str() {
            "true" | "false" => Ok(s.clone()),
            other => Err(WebDomainError::InvalidParamType {
                param: name.into(),
                expected_type: "boolean".into(),
                got: format!("\"{other}\""),
            }),
        },
        (Value::String(s), _) => Ok(s.clone()),
        (Value::Number(n), _) => Ok(n.to_string()),
        (Value::Bool(b), _) => Ok(b.to_string()),
        (Value::Null, _) => Err(WebDomainError::InvalidParamType {
            param: name.into(),
            expected_type: format!("{ty:?}"),
            got: "null".into(),
        }),
        (other, _) => Err(WebDomainError::InvalidParamType {
            param: name.into(),
            expected_type: format!("{ty:?}"),
            got: format!("{other:?}"),
        }),
    }
}

fn encode_param(v: &Value, p: &ParameterSpec) -> Result<Value, WebDomainError> {
    if let ParamType::Array(inner) = &p.param_type {
        let arr = v.as_array().ok_or_else(|| WebDomainError::InvalidParamType {
            param: p.name.clone(),
            expected_type: "array".into(),
            got: format!("{v:?}"),
        })?;
        let strs: Result<Vec<String>, WebDomainError> = arr
            .iter()
            .map(|e| coerce_scalar(e, inner, &p.name))
            .collect();
        let strs = strs?;
        let style = p.style.as_deref().unwrap_or("form");
        let explode = p.explode.unwrap_or(true);
        let encoded = match (style, explode) {
            ("form", false) => strs.join(","),
            // explode=true is consumed as an array by the http_request node; CSV is a safe transport default
            ("form", true) => strs.join(","),
            ("spaceDelimited", _) => strs.join(" "),
            ("pipeDelimited", _) => strs.join("|"),
            _ => strs.join(","),
        };
        Ok(Value::String(encoded))
    } else {
        Ok(Value::String(coerce_scalar(v, &p.param_type, &p.name)?))
    }
}

fn build_body(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> Result<(Value, Vec<String>), WebDomainError> {
    match rb.content_type.as_str() {
        "application/json" => Ok(build_body_json(rb, params)),
        "application/x-www-form-urlencoded" => Ok(build_body_form_urlencoded(rb, params)),
        "multipart/form-data" => Ok(build_body_multipart(rb, params)),
        other => Err(WebDomainError::InvalidConfig(format!(
            "unsupported request body content_type: {other}"
        ))),
    }
}

fn required_fields_from_schema(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn schema_property_names(schema: &Value) -> Vec<String> {
    schema
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn build_body_json(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> (Value, Vec<String>) {
    let required = required_fields_from_schema(&rb.schema);
    let mut missing = Vec::new();
    for name in &required {
        if !params.contains_key(name) {
            missing.push(name.clone());
        }
    }
    let props = schema_property_names(&rb.schema);
    let mut body = serde_json::Map::new();
    for name in props {
        if let Some(v) = params.get(&name) {
            body.insert(name, v.clone());
        }
    }
    // Any extra params not in the schema are still allowed (many specs are
    // partial); include them verbatim.
    for (k, v) in params {
        if !body.contains_key(k) {
            body.insert(k.clone(), v.clone());
        }
    }
    (Value::Object(body), missing)
}

fn build_body_form_urlencoded(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> (Value, Vec<String>) {
    let required = required_fields_from_schema(&rb.schema);
    let mut missing = Vec::new();
    for name in &required {
        if !params.contains_key(name) {
            missing.push(name.clone());
        }
    }
    let pairs: Vec<String> = params
        .iter()
        .map(|(k, v)| {
            let vs = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}={}", percent_encode(k), percent_encode(&vs))
        })
        .collect();
    (Value::String(pairs.join("&")), missing)
}

fn build_body_multipart(
    rb: &RequestBodySpec,
    params: &serde_json::Map<String, Value>,
) -> (Value, Vec<String>) {
    let required = required_fields_from_schema(&rb.schema);
    let mut missing = Vec::new();
    for name in &required {
        if !params.contains_key(name) {
            missing.push(name.clone());
        }
    }
    let mut out = params.clone();
    out.insert("__multipart".into(), Value::Bool(true));
    (Value::Object(out), missing)
}

fn percent_encode(s: &str) -> String {
    // RFC 3986 unreserved: A-Z a-z 0-9 - . _ ~
    // application/x-www-form-urlencoded also permits +/= but we stay strict
    // and percent-encode everything else (uppercase hex per RFC 3986).
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == '~' {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn apply_security_scheme(
    scheme: &SecurityScheme,
    secret_ref: &str,
    headers: &mut serde_json::Map<String, Value>,
    query_params: &mut serde_json::Map<String, Value>,
) {
    let placeholder = format!("${{SECURE:{secret_ref}}}");
    match scheme {
        SecurityScheme::Http { scheme: s, .. } => {
            let prefix = if s.eq_ignore_ascii_case("basic") { "Basic" } else { "Bearer" };
            headers.insert(
                "Authorization".into(),
                Value::String(format!("{prefix} {placeholder}")),
            );
        }
        SecurityScheme::ApiKey { name, location } => match location {
            ApiKeyLocation::Header => {
                headers.insert(name.clone(), Value::String(placeholder));
            }
            ApiKeyLocation::Query => {
                query_params.insert(name.clone(), Value::String(placeholder));
            }
            ApiKeyLocation::Cookie => {
                headers.insert(
                    "Cookie".into(),
                    Value::String(format!("{name}={placeholder}")),
                );
            }
        },
        SecurityScheme::OAuth2 { .. } | SecurityScheme::OpenIdConnect { .. } => {
            headers.insert(
                "Authorization".into(),
                Value::String(format!("Bearer {placeholder}")),
            );
        }
    }
}

#[cfg(test)]
mod tests_build_http_request {
    use super::*;
    use crate::web::domain::{
        ApiKeyLocation, Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec,
        RequestBodySpec, SecurityRequirement, SecurityScheme, SpecFormat,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn spec_stripe_like() -> ParsedSpec {
        let mut security_schemes = HashMap::new();
        security_schemes.insert(
            "BearerAuth".to_string(),
            SecurityScheme::Http {
                scheme: "bearer".into(),
                bearer_format: None,
            },
        );
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "Stripe".into(),
            version: "x".into(),
            description: None,
            servers: vec!["https://api.stripe.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "PostSubscriptions".into(),
                method: HttpMethod::Post,
                path: "/v1/subscriptions".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: Some(RequestBodySpec {
                    content_type: "application/x-www-form-urlencoded".into(),
                    required: true,
                    schema: json!({
                        "type": "object",
                        "required": ["customer"],
                        "properties": {
                            "customer": { "type": "string" },
                            "items[0][price]": { "type": "string" }
                        }
                    }),
                }),
                responses: HashMap::new(),
                security: vec![SecurityRequirement {
                    scheme: "BearerAuth".into(),
                    scopes: Vec::new(),
                }],
            }],
            security_schemes,
            tags: Vec::new(),
        }
    }

    fn spec_pet_by_id() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "Pet".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "getPetById".into(),
                method: HttpMethod::Get,
                path: "/pet/{petId}".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: vec![ParameterSpec {
                    name: "petId".into(),
                    description: None,
                    required: true,
                    param_type: ParamType::Integer,
                    style: None,
                    explode: None,
                }],
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    fn spec_search_with_array_query() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "S".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "search".into(),
                method: HttpMethod::Get,
                path: "/items".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: vec![ParameterSpec {
                    name: "ids".into(),
                    description: None,
                    required: false,
                    param_type: ParamType::Array(Box::new(ParamType::String)),
                    style: Some("form".into()),
                    explode: Some(false),
                }],
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn happy_path_stripe_post_subscriptions() {
        let spec = spec_stripe_like();
        let req = build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({
                "customer": "cus_ABC",
                "items[0][price]": "price_XYZ"
            }),
            Some("stripe_key"),
        )
        .unwrap();
        assert_eq!(req["url"], "https://api.stripe.com/v1/subscriptions");
        assert_eq!(req["method"], "POST");
        assert_eq!(
            req["headers"]["Authorization"],
            "Bearer ${SECURE:stripe_key}"
        );
        assert_eq!(
            req["headers"]["Content-Type"],
            "application/x-www-form-urlencoded"
        );
        let body = req["body"].as_str().unwrap();
        assert!(body.contains("customer=cus_ABC"));
        assert!(body.contains("items%5B0%5D%5Bprice%5D=price_XYZ"));
    }

    #[test]
    fn missing_required_body_field_returns_error() {
        let spec = spec_stripe_like();
        let err = build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "items[0][price]": "price_XYZ" }),
            Some("stripe_key"),
        )
        .unwrap_err();
        match err {
            WebDomainError::MissingRequiredParams { missing, .. } => {
                assert!(missing.contains(&"customer".to_string()));
            }
            other => panic!("expected MissingRequiredParams, got {other:?}"),
        }
    }

    #[test]
    fn missing_auth_when_endpoint_requires_it() {
        let spec = spec_stripe_like();
        let err = build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus_ABC" }),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WebDomainError::MissingAuth { .. }));
    }

    #[test]
    fn path_parameter_is_substituted() {
        let spec = spec_pet_by_id();
        let req = build_http_request(
            &spec,
            "getPetById",
            &json!({ "petId": 42 }),
            None,
        )
        .unwrap();
        assert_eq!(req["url"], "https://api.example.com/pet/42");
        assert_eq!(req["method"], "GET");
    }

    #[test]
    fn missing_required_path_parameter_returns_error() {
        let spec = spec_pet_by_id();
        let err = build_http_request(
            &spec,
            "getPetById",
            &json!({}),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WebDomainError::MissingRequiredParams { .. }));
    }

    #[test]
    fn integer_param_accepts_string_coercion() {
        let spec = spec_pet_by_id();
        let req = build_http_request(
            &spec,
            "getPetById",
            &json!({ "petId": "42" }),
            None,
        )
        .unwrap();
        assert_eq!(req["url"], "https://api.example.com/pet/42");
    }

    #[test]
    fn integer_param_rejects_non_numeric_string() {
        let spec = spec_pet_by_id();
        let err = build_http_request(
            &spec,
            "getPetById",
            &json!({ "petId": "not-a-number" }),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WebDomainError::InvalidParamType { .. }));
    }

    #[test]
    fn array_query_csv_serializes_comma_separated() {
        let spec = spec_search_with_array_query();
        let req = build_http_request(
            &spec,
            "search",
            &json!({ "ids": ["a", "b", "c"] }),
            None,
        )
        .unwrap();
        assert_eq!(req["query_params"]["ids"], "a,b,c");
    }

    #[test]
    fn missing_auth_scheme_definition_is_error() {
        // Endpoint declares "GhostAuth" but the spec has no matching security scheme.
        let mut spec = spec_stripe_like();
        spec.security_schemes.clear();
        let err = build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus" }),
            Some("key"),
        )
        .unwrap_err();
        match err {
            WebDomainError::InvalidConfig(msg) => assert!(msg.contains("BearerAuth")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn api_key_scheme_with_header_location() {
        let mut spec = spec_stripe_like();
        spec.security_schemes.clear();
        spec.security_schemes.insert(
            "KeyHdr".into(),
            SecurityScheme::ApiKey {
                name: "X-API-Key".into(),
                location: ApiKeyLocation::Header,
            },
        );
        spec.endpoints[0].security = vec![SecurityRequirement {
            scheme: "KeyHdr".into(),
            scopes: Vec::new(),
        }];
        let req = build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus_ABC" }),
            Some("my_key"),
        )
        .unwrap();
        assert_eq!(req["headers"]["X-API-Key"], "${SECURE:my_key}");
    }

    #[test]
    fn api_key_scheme_with_query_location() {
        let mut spec = spec_stripe_like();
        spec.security_schemes.clear();
        spec.security_schemes.insert(
            "KeyQ".into(),
            SecurityScheme::ApiKey {
                name: "api_key".into(),
                location: ApiKeyLocation::Query,
            },
        );
        spec.endpoints[0].security = vec![SecurityRequirement {
            scheme: "KeyQ".into(),
            scopes: Vec::new(),
        }];
        let req = build_http_request(
            &spec,
            "PostSubscriptions",
            &json!({ "customer": "cus_ABC" }),
            Some("my_key"),
        )
        .unwrap();
        assert_eq!(req["query_params"]["api_key"], "${SECURE:my_key}");
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::domain::{
        Endpoint, HttpMethod, ParsedSpec, SpecFormat, TtlConfig,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingPort {
        calls: AtomicU32,
        respond_with: Mutex<Option<SpecFetchResult>>,
    }

    #[async_trait]
    impl ApiSpecPort for CountingPort {
        async fn fetch_and_parse(
            &self,
            _url: &str,
            _etag: Option<&str>,
            _lm: Option<&str>,
        ) -> Result<SpecFetchResult, WebDomainError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Return whatever was prepared; default to a fresh tiny spec.
            let prepared = self.respond_with.lock().await.take();
            if let Some(r) = prepared {
                return Ok(r);
            }
            Ok(SpecFetchResult::Fresh {
                spec: tiny_spec(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            })
        }
    }

    fn tiny_spec() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "https://ex/s.yaml".into(),
            input_url: "https://ex/s.yaml".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "getThing".into(),
                method: HttpMethod::Get,
                path: "/thing".into(),
                summary: None,
                description: None,
                tags: Vec::new(),
                path_params: Vec::new(),
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    fn use_case_with(port: Arc<CountingPort>) -> ApiSpecUseCase {
        let registry: Arc<SessionRegistry<Arc<SpecCache>>> =
            SessionRegistry::new(TtlConfig::default());
        ApiSpecUseCase::new(port, registry, ApiSpecUseCaseConfig::default())
    }

    #[tokio::test]
    async fn first_fetch_hits_the_port() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        let (_entry, was_cached) = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", false)
            .await
            .unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
        assert!(!was_cached);
    }

    #[tokio::test]
    async fn second_fetch_within_ttl_does_not_hit_the_port() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        let (_e1, c1) = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", false)
            .await
            .unwrap();
        let (_e2, c2) = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", false)
            .await
            .unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
        assert!(!c1);
        assert!(c2);
    }

    #[tokio::test]
    async fn force_reload_bypasses_cache() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        let (_entry, was_cached) = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", true)
            .await
            .unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
        assert!(!was_cached);
    }

    #[tokio::test]
    async fn different_conversations_have_separate_caches() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-a", "https://ex/s.yaml", false).await.unwrap();
        uc.fetch_spec("conv-b", "https://ex/s.yaml", false).await.unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn not_modified_refreshes_existing_cached_entry() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        *port.respond_with.lock().await = Some(SpecFetchResult::NotModified);
        let (entry, was_cached) = uc
            .fetch_spec("conv-1", "https://ex/s.yaml", true)
            .await
            .unwrap();
        assert_eq!(port.calls.load(Ordering::SeqCst), 2);
        assert_eq!(entry.etag.as_deref(), Some("\"v1\""));
        assert!(!was_cached);
    }

    #[tokio::test]
    async fn lookup_cached_returns_spec_not_loaded_when_missing() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        let err = uc
            .lookup_cached("conv-1", "https://ex/s.yaml")
            .await
            .unwrap_err();
        match err {
            WebDomainError::SpecNotLoaded { spec_url } => {
                assert_eq!(spec_url, "https://ex/s.yaml");
            }
            other => panic!("expected SpecNotLoaded, got {other:?}"),
        }
        assert_eq!(port.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn lookup_cached_returns_arc_after_fetch() {
        let port = Arc::new(CountingPort {
            calls: AtomicU32::new(0),
            respond_with: Mutex::new(None),
        });
        let uc = use_case_with(port.clone());
        uc.fetch_spec("conv-1", "https://ex/s.yaml", false).await.unwrap();
        let spec = uc
            .lookup_cached("conv-1", "https://ex/s.yaml")
            .await
            .unwrap();
        assert_eq!(spec.title, "T");
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod tests_list_and_search {
    use crate::web::domain::{Endpoint, HttpMethod, ParsedSpec, SpecFormat};
    use std::collections::HashMap;

    fn spec_with(endpoints: Vec<Endpoint>) -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints,
            security_schemes: HashMap::new(),
            tags: vec!["pet".into(), "store".into()],
        }
    }

    fn ep(
        op: &str,
        method: HttpMethod,
        path: &str,
        summary: &str,
        tag: &str,
    ) -> Endpoint {
        Endpoint {
            operation_id: op.into(),
            method,
            path: path.into(),
            summary: Some(summary.into()),
            description: None,
            tags: vec![tag.into()],
            path_params: Vec::new(),
            query_params: Vec::new(),
            header_params: Vec::new(),
            request_body: None,
            responses: HashMap::new(),
            security: Vec::new(),
        }
    }

    fn sample() -> ParsedSpec {
        spec_with(vec![
            ep("listPets", HttpMethod::Get, "/pet", "List pets", "pet"),
            ep("addPet", HttpMethod::Post, "/pet", "Add a new pet", "pet"),
            ep("getPet", HttpMethod::Get, "/pet/{id}", "Get pet by ID", "pet"),
            ep("listStores", HttpMethod::Get, "/store", "List stores", "store"),
            ep("createSubscription", HttpMethod::Post, "/subscription", "Create a subscription", "billing"),
        ])
    }

    #[test]
    fn list_all_returns_all_endpoints() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, None, 50, 0);
        assert_eq!(page.total, 5);
        assert_eq!(page.returned, 5);
        assert_eq!(page.endpoints.len(), 5);
    }

    #[test]
    fn list_paginates() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, None, 2, 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.returned, 2);
        assert_eq!(page.endpoints[0].operation_id, "getPet");
    }

    #[test]
    fn list_filters_by_tag() {
        let spec = sample();
        let page = super::super::api_spec_use_case::list_endpoints(&spec, Some("billing"), 50, 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.endpoints[0].operation_id, "createSubscription");
    }

    #[test]
    fn search_finds_by_summary() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "create subscription",
            None,
            10,
            0.1,
        );
        assert!(!results.is_empty());
        assert_eq!(results[0].operation_id, "createSubscription");
    }

    #[test]
    fn search_filters_by_method() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "pet",
            Some("GET"),
            10,
            0.1,
        );
        for r in &results {
            assert_eq!(r.method, "GET");
        }
    }

    #[test]
    fn search_respects_threshold() {
        let spec = sample();
        let tight = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "completely unrelated nonsense xyz",
            None,
            10,
            0.99,
        );
        assert!(tight.is_empty());
    }

    #[test]
    fn search_returns_top_n() {
        let spec = sample();
        let results = super::super::api_spec_use_case::search_endpoint(
            &spec,
            "pet",
            None,
            2,
            0.1,
        );
        assert!(results.len() <= 2);
    }
}

#[cfg(test)]
mod tests_details {
    use super::*;
    use crate::web::domain::{
        Endpoint, HttpMethod, ParamType, ParameterSpec, ParsedSpec, RequestBodySpec,
        ResponseSpec, SpecFormat,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn sample() -> ParsedSpec {
        ParsedSpec {
            resolved_url: "u".into(),
            input_url: "u".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://api.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "createSubscription".into(),
                method: HttpMethod::Post,
                path: "/v1/subscriptions".into(),
                summary: Some("Create a subscription".into()),
                description: None,
                tags: vec!["billing".into()],
                path_params: Vec::new(),
                query_params: vec![ParameterSpec {
                    name: "expand".into(),
                    description: Some("Fields to expand".into()),
                    required: false,
                    param_type: ParamType::Array(Box::new(ParamType::String)),
                    style: Some("form".into()),
                    explode: Some(false),
                }],
                header_params: Vec::new(),
                request_body: Some(RequestBodySpec {
                    content_type: "application/x-www-form-urlencoded".into(),
                    required: true,
                    schema: json!({
                        "type": "object",
                        "required": ["customer"],
                        "properties": {
                            "customer": { "type": "string" },
                            "items": { "type": "array" }
                        }
                    }),
                }),
                responses: {
                    let mut m = HashMap::new();
                    m.insert(
                        "200".to_string(),
                        ResponseSpec {
                            description: Some("Success".into()),
                            content: {
                                let mut c = HashMap::new();
                                c.insert(
                                    "application/json".into(),
                                    json!({ "$ref": "#/components/schemas/Subscription" }),
                                );
                                c
                            },
                        },
                    );
                    m
                },
                security: Vec::new(),
            }],
            security_schemes: HashMap::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn happy_path_returns_expected_shape() {
        let spec = sample();
        let details = super::super::api_spec_use_case::get_endpoint_details(
            &spec,
            "createSubscription",
        )
        .unwrap();
        assert_eq!(details["operation_id"], "createSubscription");
        assert_eq!(details["method"], "POST");
        assert_eq!(details["path"], "/v1/subscriptions");
        assert_eq!(details["path_parameters"].as_array().unwrap().len(), 0);
        assert_eq!(details["query_parameters"][0]["name"], "expand");
        assert_eq!(details["query_parameters"][0]["type"], "array");
        assert_eq!(
            details["request_body"]["content_type"],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(details["responses"]["200"]["description"], "Success");
    }

    #[test]
    fn missing_operation_suggests_candidates() {
        let spec = sample();
        let err = super::super::api_spec_use_case::get_endpoint_details(
            &spec,
            "createSubscrpition", // typo
        )
        .unwrap_err();
        match err {
            WebDomainError::EndpointNotFound { did_you_mean, .. } => {
                assert!(did_you_mean.iter().any(|s| s == "createSubscription"));
            }
            other => panic!("expected EndpointNotFound, got {other:?}"),
        }
    }
}
