//! HTTP request node — makes outbound HTTP calls from a DAG.
//!
//! ## Standalone use
//! Configure via `config`: `base_url`, `endpoint`, `method`, `headers`, `query_params`,
//! `body`, `bearer_token`, `authorization`. All string values support `${ENV_VAR}` resolution.
//! Input edges override config values (inputs take priority over config).
//!
//! ## As an LLM tool (via `tool_configurations`)
//! When invoked by `DagToolExecutor`, extra non-reserved input keys with primitive values
//! (string, number, boolean) are automatically appended as URL query parameters.
//! This is the mechanism that allows `node_schema` container children and `$DYNAMIC`
//! top-level fields to reach the node as flat inputs.
//!
//! ## Outputs
//! Always returns `{ "status": u16, "body": Value }`.
//! `body` is parsed as JSON; if the response is not valid JSON, `body` is `null`.
//! The default output port is `body`.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use bytes::Bytes;
use futures::Stream;
use reqwest::{Client, Method, Url};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

/// Executes HTTP requests. Implements [`ExecutableNode`]. Stateless — all configuration
/// comes from `inputs` (highest priority) and `config`.
pub struct HttpNode {
    /// Optional storage adapter — used to resolve `$attachment:<id>` placeholders
    /// in the body. When None, placeholders pass through unchanged (logged warn).
    storage: Option<Arc<dyn crate::storage::domain::OutputStorageRepository>>,
}

impl Default for HttpNode {
    fn default() -> Self {
        Self::new()
    }
}

const ATTACHMENT_PLACEHOLDER_PREFIX: &str = "$attachment:";
const URL_HTTP_PREFIX: &str = "http://";
const URL_HTTPS_PREFIX: &str = "https://";

/// A single resolved multipart form part, prior to network I/O. Built by
/// [`HttpNode::parse_multipart_body`] and consumed by the form assembler.
#[derive(Debug, Clone)]
pub(crate) enum PartSpec {
    Url {
        field: String,
        url: String,
        filename_override: Option<String>,
        content_type_override: Option<String>,
    },
    Attachment {
        field: String,
        storage_key: String,
        filename_override: Option<String>,
        content_type_override: Option<String>,
    },
    Text {
        field: String,
        value: String,
        content_type_override: Option<String>,
    },
}

/// Resolution result for a single URL part: a streaming reader + the metadata
/// we'll forward to the downstream multipart form.
pub(crate) struct ResolvedUrlPart {
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    pub size_bytes: u64,
    pub content_type: String,
    pub filename: String,
}

impl std::fmt::Debug for ResolvedUrlPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedUrlPart")
            .field("size_bytes", &self.size_bytes)
            .field("content_type", &self.content_type)
            .field("filename", &self.filename)
            .field("stream", &"<stream>")
            .finish()
    }
}

pub(crate) struct MultipartUrlResolver {
    pub max_file_size_bytes: u64,
    pub timeout_secs: u64,
    pub allow_http_urls: bool,
}

impl MultipartUrlResolver {
    pub(crate) async fn resolve(
        &self,
        url: &str,
    ) -> Result<ResolvedUrlPart, Box<dyn StdError + Send + Sync>> {
        let parsed = Url::parse(url)
            .map_err(|e| format!("UrlValidationFailed: cannot parse '{url}': {e}"))?;
        match parsed.scheme() {
            "https" => {}
            "http" if self.allow_http_urls => {}
            "http" => {
                return Err(format!(
                    "UrlValidationFailed: plain http:// URL '{url}' rejected (set allow_http_urls=true to permit)"
                )
                .into());
            }
            other => {
                return Err(format!(
                    "UrlValidationFailed: scheme '{other}' not supported (only http/https)"
                )
                .into());
            }
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .http1_only()
            .build()?;

        // GET-only: HEAD is intentionally skipped because V4-signed URLs (GCS,
        // S3) are method-specific — a URL signed for GET returns 4xx on HEAD.
        // `send().await?` resolves once response HEADERS arrive (body not
        // consumed yet), so we can validate Content-Length and reject by
        // dropping `resp` BEFORE any body bytes flow into the worker.
        let resp = client
            .get(parsed.clone())
            .send()
            .await
            .map_err(|e| format!("UrlValidationFailed: GET for '{url}' failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "UrlValidationFailed: GET for '{url}' returned status {}",
                resp.status()
            )
            .into());
        }
        // Read Content-Length directly from the response header. Using the raw
        // header (not `resp.content_length()`) sidesteps reqwest's
        // decoded-body size_hint quirks.
        let size_bytes = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .ok_or_else(|| {
                format!("UrlValidationFailed: GET for '{url}' returned no Content-Length")
            })?;
        if size_bytes > self.max_file_size_bytes {
            // Drop `resp` before returning so the TCP connection closes and the
            // upstream stops transmitting. No body bytes ever reach the worker.
            drop(resp);
            return Err(format!(
                "FileTooLarge: '{url}' declared {size_bytes} bytes, max is {}",
                self.max_file_size_bytes
            )
            .into());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let filename =
            filename_from_disposition(resp.headers().get(reqwest::header::CONTENT_DISPOSITION))
                .unwrap_or_else(|| filename_from_url_path(&parsed));
        let stream = resp.bytes_stream();

        Ok(ResolvedUrlPart {
            stream: Box::pin(stream),
            size_bytes,
            content_type,
            filename,
        })
    }
}

/// Parse `Content-Disposition: attachment; filename="report.pdf"` (or unquoted)
/// into the bare filename. Returns None for unrecognized shapes or absent
/// header. RFC 5987 (`filename*=`) is intentionally not handled in v1.
fn filename_from_disposition(header: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    let v = header?.to_str().ok()?;
    let after = v.split(';').find_map(|chunk| {
        let chunk = chunk.trim();
        let lower = chunk.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("filename=") {
            let _ = rest;
            Some(&chunk[("filename=".len())..])
        } else {
            None
        }
    })?;
    let unquoted = after.trim().trim_matches('"').to_string();
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted)
    }
}

/// Last path segment of the URL (after the final `/`), URL-decoded. Falls back
/// to `"file"` for URLs with no usable path component.
fn filename_from_url_path(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut s| {
            s.next_back()
                .filter(|seg| !seg.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "file".to_string())
}

impl HttpNode {
    pub fn new() -> Self {
        Self { storage: None }
    }

    pub fn with_storage(
        mut self,
        storage: Arc<dyn crate::storage::domain::OutputStorageRepository>,
    ) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Recursively walks a JSON value, replacing every string of the form
    /// `$attachment:<storage_key>` with `data:<mime>;base64,<bytes>`. Returns
    /// an error if any placeholder cannot be resolved (no storage adapter,
    /// or storage.read fails).
    async fn resolve_attachment_placeholders(
        &self,
        val: Value,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        use base64::Engine;

        match val {
            Value::String(s) if s.starts_with(ATTACHMENT_PLACEHOLDER_PREFIX) => {
                let id = &s[ATTACHMENT_PLACEHOLDER_PREFIX.len()..];
                let storage = self.storage.as_ref().ok_or_else(|| {
                    format!(
                        "http_request: body contains '{s}' but no OutputStorageRepository is wired"
                    )
                })?;
                let bytes = storage
                    .read(id)
                    .await
                    .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes.bytes);
                Ok(Value::String(format!(
                    "data:{};base64,{}",
                    bytes.mime_type, encoded
                )))
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k, Box::pin(self.resolve_attachment_placeholders(v)).await?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for v in arr {
                    out.push(Box::pin(self.resolve_attachment_placeholders(v)).await?);
                }
                Ok(Value::Array(out))
            }
            other => Ok(other),
        }
    }

    fn resolve_env_vars(input: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut last_end = 0;

        while let Some(start) = input[last_end..].find("${") {
            let absolute_start = last_end + start;
            result.push_str(&input[last_end..absolute_start]);

            if let Some(end) = input[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_name = &input[absolute_start + 2..absolute_end];
                let val = std::env::var(var_name)
                    .map_err(|_| format!("Env var {} not found", var_name))?;
                result.push_str(&val);
                last_end = absolute_end + 1;
            } else {
                result.push_str(&input[absolute_start..]);
                last_end = input.len();
                break;
            }
        }
        result.push_str(&input[last_end..]);
        Ok(result)
    }

    /// Resolve `${ENV_VAR}` in all string values within a JSON Value (recursive).
    fn resolve_env_vars_in_value(val: &Value) -> Value {
        match val {
            Value::String(s) => {
                Value::String(Self::resolve_env_vars(s).unwrap_or_else(|_| s.clone()))
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), Self::resolve_env_vars_in_value(v));
                }
                Value::Object(out)
            }
            Value::Array(arr) => {
                Value::Array(arr.iter().map(Self::resolve_env_vars_in_value).collect())
            }
            other => other.clone(),
        }
    }

    /// Returns `true` when the merged headers map contains a Content-Type
    /// whose MIME type begins with `multipart/`. Header lookup is
    /// case-insensitive per RFC 9110 §5.1.
    pub(crate) fn is_multipart_mode(headers: &serde_json::Map<String, Value>) -> bool {
        for (k, v) in headers {
            if k.eq_ignore_ascii_case("content-type") {
                if let Some(s) = v.as_str() {
                    return s
                        .trim_start()
                        .to_ascii_lowercase()
                        .starts_with("multipart/");
                }
            }
        }
        false
    }

    /// Parse a `body` JSON object into a flat list of `PartSpec`s. Pure logic,
    /// no I/O. The rules match the design spec D2 table.
    ///
    /// Returns an error for malformed bodies (non-object root, unrecognized
    /// explicit object shape, etc.).
    pub(crate) fn parse_multipart_body(
        body: &Value,
    ) -> Result<Vec<PartSpec>, Box<dyn StdError + Send + Sync>> {
        let map = body
            .as_object()
            .ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
                "MultipartConfigError: body must be a JSON object in multipart mode".into()
            })?;

        let mut parts = Vec::new();
        for (field, value) in map {
            Self::push_parts_for_value(field, value, &mut parts)?;
        }
        Ok(parts)
    }

    fn push_parts_for_value(
        field: &str,
        value: &Value,
        out: &mut Vec<PartSpec>,
    ) -> Result<(), Box<dyn StdError + Send + Sync>> {
        match value {
            Value::Null => Ok(()),
            Value::String(s) => {
                out.push(Self::classify_string_part(field, s));
                Ok(())
            }
            Value::Number(n) => {
                out.push(PartSpec::Text {
                    field: field.to_string(),
                    value: n.to_string(),
                    content_type_override: None,
                });
                Ok(())
            }
            Value::Bool(b) => {
                out.push(PartSpec::Text {
                    field: field.to_string(),
                    value: b.to_string(),
                    content_type_override: None,
                });
                Ok(())
            }
            Value::Array(arr) => {
                for item in arr {
                    Self::push_parts_for_value(field, item, out)?;
                }
                Ok(())
            }
            Value::Object(obj) => {
                let filename_override = obj
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let content_type_override = obj
                    .get("content_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                    out.push(PartSpec::Url {
                        field: field.to_string(),
                        url: url.to_string(),
                        filename_override,
                        content_type_override,
                    });
                    Ok(())
                } else if let Some(key) = obj.get("attachment").and_then(|v| v.as_str()) {
                    out.push(PartSpec::Attachment {
                        field: field.to_string(),
                        storage_key: key.to_string(),
                        filename_override,
                        content_type_override,
                    });
                    Ok(())
                } else if let Some(value_s) = obj.get("value").and_then(|v| v.as_str()) {
                    out.push(PartSpec::Text {
                        field: field.to_string(),
                        value: value_s.to_string(),
                        content_type_override,
                    });
                    Ok(())
                } else {
                    Err(format!(
                        "MultipartConfigError: object under field '{field}' has none of \
                         'url', 'attachment', 'value' (unrecognized shape)"
                    )
                    .into())
                }
            }
        }
    }

    fn classify_string_part(field: &str, s: &str) -> PartSpec {
        if let Some(rest) = s.strip_prefix(ATTACHMENT_PLACEHOLDER_PREFIX) {
            PartSpec::Attachment {
                field: field.to_string(),
                storage_key: rest.to_string(),
                filename_override: None,
                content_type_override: None,
            }
        } else if s.starts_with(URL_HTTPS_PREFIX) || s.starts_with(URL_HTTP_PREFIX) {
            PartSpec::Url {
                field: field.to_string(),
                url: s.to_string(),
                filename_override: None,
                content_type_override: None,
            }
        } else {
            PartSpec::Text {
                field: field.to_string(),
                value: s.to_string(),
                content_type_override: None,
            }
        }
    }

    const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 104_857_600; // 100 MiB
    const DEFAULT_MAX_PARTS: usize = 10;
    const DEFAULT_URL_DOWNLOAD_TIMEOUT_SECS: u64 = 30;

    fn limit_u64(config: &Value, key: &str, default: u64) -> u64 {
        config.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
    }
    fn limit_usize(config: &Value, key: &str, default: usize) -> usize {
        config
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(default)
    }
    fn limit_bool(config: &Value, key: &str, default: bool) -> bool {
        config.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    async fn execute_multipart(
        &self,
        full_url: &str,
        method_str: &str,
        merged_headers: &serde_json::Map<String, Value>,
        inputs: &NodeInputs,
        config: &Value,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let body_val = inputs
            .get("body")
            .or_else(|| config.get("body"))
            .ok_or("MultipartConfigError: body is required in multipart mode")?;

        // Apply env-var resolution to string leaves before parsing so that
        // `${VAR}` works inside URLs and text values, matching JSON path.
        let body_resolved = Self::resolve_env_vars_in_value(body_val);

        let parts = Self::parse_multipart_body(&body_resolved)?;

        let max_parts = Self::limit_usize(config, "max_parts", Self::DEFAULT_MAX_PARTS);
        if parts.len() > max_parts {
            return Err(format!(
                "TooManyParts: body produced {} parts, max is {}",
                parts.len(),
                max_parts
            )
            .into());
        }

        let max_file_size_bytes = Self::limit_u64(
            config,
            "max_file_size_bytes",
            Self::DEFAULT_MAX_FILE_SIZE_BYTES,
        );
        let timeout_secs = Self::limit_u64(
            config,
            "url_download_timeout_secs",
            Self::DEFAULT_URL_DOWNLOAD_TIMEOUT_SECS,
        );
        let allow_http_urls = Self::limit_bool(config, "allow_http_urls", false);

        let resolver = MultipartUrlResolver {
            max_file_size_bytes,
            timeout_secs,
            allow_http_urls,
        };

        let parts_count = parts.len();
        let mut form = reqwest::multipart::Form::new();
        for spec in parts {
            form = self
                .add_part_to_form(form, spec, &resolver, max_file_size_bytes)
                .await?;
        }

        // Build the outbound request — same client tuning as JSON path
        let client = reqwest::Client::builder().http1_only().build()?;
        let url = Url::parse(full_url).map_err(|e| format!("Invalid URL '{full_url}': {e}"))?;
        let method = reqwest::Method::from_str(method_str)
            .map_err(|e| format!("Invalid HTTP method '{method_str}': {e}"))?;
        let mut req = client.request(method, url);
        req = req.header("User-Agent", "colmena-http-node/0.1");

        // Forward all headers EXCEPT Content-Type — reqwest will set
        // multipart/form-data; boundary=... itself.
        for (k, v) in merged_headers {
            if k.eq_ignore_ascii_case("content-type") {
                continue;
            }
            if let Some(v_str) = v.as_str() {
                let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                        as Box<dyn StdError + Send + Sync>
                })?;
                req = req.header(k, v_resolved);
            }
        }
        if let Some(token) = inputs.get("bearer_token").and_then(|v| v.as_str()) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(auth) = inputs.get("authorization").and_then(|v| v.as_str()) {
            req = req.header("Authorization", auth);
        }

        println!("[HttpNode] → {method_str} {full_url} (multipart, {parts_count} parts)");

        let response = req.multipart(form).send().await?;
        let status = response.status().as_u16();
        println!("[HttpNode] ← {status} ({full_url})");

        let response_body: Value = match response.json::<Value>().await {
            Ok(json) => json,
            Err(_) => Value::Null,
        };

        Ok(serde_json::json!({
            "status": status,
            "body": response_body
        }))
    }

    async fn add_part_to_form(
        &self,
        form: reqwest::multipart::Form,
        spec: PartSpec,
        resolver: &MultipartUrlResolver,
        max_file_size_bytes: u64,
    ) -> Result<reqwest::multipart::Form, Box<dyn StdError + Send + Sync>> {
        use futures::StreamExt;
        match spec {
            PartSpec::Text {
                field,
                value,
                content_type_override,
            } => {
                let mut part = reqwest::multipart::Part::text(value);
                if let Some(ct) = content_type_override {
                    part = part.mime_str(&ct)?;
                }
                Ok(form.part(field, part))
            }
            PartSpec::Url {
                field,
                url,
                filename_override,
                content_type_override,
            } => {
                let resolved = resolver.resolve(&url).await?;
                let filename = filename_override.unwrap_or(resolved.filename);
                let content_type = content_type_override.unwrap_or(resolved.content_type);
                let mapped = resolved
                    .stream
                    .map(|chunk| chunk.map_err(std::io::Error::other));
                let body = reqwest::Body::wrap_stream(mapped);
                let part = reqwest::multipart::Part::stream_with_length(body, resolved.size_bytes)
                    .file_name(filename)
                    .mime_str(&content_type)?;
                Ok(form.part(field, part))
            }
            PartSpec::Attachment {
                field,
                storage_key,
                filename_override,
                content_type_override,
            } => {
                let storage = self.storage.as_ref().ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
                    format!("AttachmentNotFound: body references '$attachment:{storage_key}' but no OutputStorageRepository is wired").into()
                })?;
                let stored = storage
                    .read_stream(&storage_key)
                    .await
                    .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;
                if stored.size_bytes > max_file_size_bytes {
                    return Err(format!(
                        "FileTooLarge: attachment '{storage_key}' is {} bytes, max is {max_file_size_bytes}",
                        stored.size_bytes
                    )
                    .into());
                }
                let filename = filename_override.unwrap_or(stored.filename);
                let content_type = content_type_override.unwrap_or(stored.mime_type);
                let mapped = stored
                    .stream
                    .map(|chunk| chunk.map_err(std::io::Error::other));
                let body = reqwest::Body::wrap_stream(mapped);
                let part = reqwest::multipart::Part::stream_with_length(body, stored.size_bytes)
                    .file_name(filename)
                    .mime_str(&content_type)?;
                Ok(form.part(field, part))
            }
        }
    }
}

#[async_trait::async_trait]
impl ExecutableNode for HttpNode {
    /// Execute an HTTP request.
    ///
    /// # Priority
    /// For every field (`base_url`, `endpoint`, `method`, `headers`, `query_params`, `body`,
    /// `bearer_token`, `authorization`), the value from `inputs` takes priority over `config`.
    ///
    /// # Env var resolution
    /// All string values in `config` (and input headers) support `${VAR_NAME}` syntax, resolved
    /// via `std::env::var` at call time. This is the primary mechanism for injecting API keys.
    ///
    /// # Extra query params
    /// Any input key not in `reserved_keys` that holds a primitive value (string, number, bool)
    /// is automatically appended as a URL query parameter. When called as an LLM tool, the
    /// executor passes `node_schema` child fields and `$DYNAMIC` replacements as flat inputs,
    /// which this mechanism then routes to query params or body as appropriate.
    ///
    /// # Outputs
    /// Returns `{"status": <u16>, "body": <json_value_or_null>}`. The `body` is the default
    /// output port — downstream nodes without a field selector receive it directly.
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // 1. Parse Configuration (Inputs > Config)
        let base_url_raw = inputs
            .get("base_url")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("base_url").and_then(|v| v.as_str()))
            .unwrap_or("");
        let base_url = Self::resolve_env_vars(base_url_raw).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        let endpoint_raw = inputs
            .get("endpoint")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("endpoint").and_then(|v| v.as_str()))
            .unwrap_or("");
        let endpoint = Self::resolve_env_vars(endpoint_raw).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        let method_str = inputs
            .get("method")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("method").and_then(|v| v.as_str()))
            .unwrap_or("GET");

        // 2. Construct URL
        // Handle trailing/leading slashes to avoid double slashes or missing slashes
        let base = base_url.trim_end_matches('/');
        let path = endpoint.trim_start_matches('/');
        let full_url_str = if path.is_empty() {
            base.to_string()
        } else {
            format!("{}/{}", base, path)
        };

        let url = Url::parse(&full_url_str)
            .map_err(|e| format!("Invalid URL '{}': {}", full_url_str, e))?;
        let method = Method::from_str(method_str)
            .map_err(|e| format!("Invalid HTTP method '{}': {}", method_str, e))?;

        // 3. Prepare Client and Request
        // Build client forcing HTTP/1.1 to avoid HTTP/2 issues with some APIs
        let client = Client::builder().http1_only().build()?;

        println!("[HttpNode] → {} {}", method, url);

        let mut request_builder = client.request(method, url);

        // Add a default User-Agent to improve compatibility with some APIs
        request_builder = request_builder.header("User-Agent", "colmena-http-node/0.1");

        // 4. Headers (Config + Inputs)
        // Config headers
        if let Some(headers) = config.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    request_builder = request_builder.header(k, v_resolved);
                }
            }
        }
        // Input headers (override config)
        if let Some(headers) = inputs.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    request_builder = request_builder.header(k, v_resolved);
                }
            }
        }

        // Handle specific auth inputs
        if let Some(token) = inputs.get("bearer_token").and_then(|v| v.as_str()) {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }
        if let Some(auth) = inputs.get("authorization").and_then(|v| v.as_str()) {
            request_builder = request_builder.header("Authorization", auth);
        }

        // 5. Query Params (Config + Inputs) — resolve ${ENV_VAR} in values
        if let Some(params) = config.get("query_params").and_then(|v| v.as_object()) {
            let mut resolved = serde_json::Map::new();
            for (k, v) in params {
                if let Some(s) = v.as_str() {
                    let s_resolved = Self::resolve_env_vars(s).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    resolved.insert(k.clone(), Value::String(s_resolved));
                } else {
                    resolved.insert(k.clone(), v.clone());
                }
            }
            request_builder = request_builder.query(&resolved);
        } else if let Some(params) = config.get("query_params") {
            request_builder = request_builder.query(params);
        }
        if let Some(params) = inputs.get("query_params").and_then(|v| v.as_object()) {
            let mut resolved = serde_json::Map::new();
            for (k, v) in params {
                if let Some(s) = v.as_str() {
                    let s_resolved = Self::resolve_env_vars(s).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    resolved.insert(k.clone(), Value::String(s_resolved));
                } else {
                    resolved.insert(k.clone(), v.clone());
                }
            }
            request_builder = request_builder.query(&resolved);
        } else if let Some(params) = inputs.get("query_params") {
            request_builder = request_builder.query(params);
        }

        // Collect extra inputs as query params (for tools that flatten params)
        let reserved_keys = [
            "base_url",
            "endpoint",
            "method",
            "headers",
            "body",
            "query_params",     // correct key used throughout the codebase
            "query_parameters", // kept for backward compat
            "bearer_token",
            "authorization",
            "secure", // internal Colmena flag — NEVER send to external APIs
            "__colmena_session_id",
            "__colmena_agent_session_id",
            "__node_id",
            "__colmena_node_id_path",
            "__colmena_resume_answer",
        ];
        let mut extra_params = std::collections::HashMap::new();
        for (k, v) in inputs {
            if !reserved_keys.contains(&k.as_str()) {
                // Only include primitives (String, Number, Boolean)
                match v {
                    serde_json::Value::String(s) => {
                        let s_resolved = Self::resolve_env_vars(s).unwrap_or(s.to_string());
                        extra_params.insert(k, serde_json::Value::String(s_resolved));
                    }
                    serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
                        extra_params.insert(k, v.clone());
                    }
                    _ => {
                        // Ignore Objects, Arrays, Nulls
                    }
                }
            }
        }
        if !extra_params.is_empty() {
            request_builder = request_builder.query(&extra_params);
        }

        // 6. Body (Inputs or Config) — branch on multipart vs JSON/string
        // Build a merged headers map for the multipart detector
        let mut merged_headers = serde_json::Map::new();
        if let Some(h) = config.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in h {
                merged_headers.insert(k.clone(), v.clone());
            }
        }
        if let Some(h) = inputs.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in h {
                merged_headers.insert(k.clone(), v.clone());
            }
        }

        if Self::is_multipart_mode(&merged_headers) {
            return self
                .execute_multipart(&full_url_str, method_str, &merged_headers, inputs, config)
                .await;
        }

        let body_val = inputs.get("body").or_else(|| config.get("body"));

        if let Some(body) = body_val {
            if let Some(s) = body.as_str() {
                let s_resolved = Self::resolve_env_vars(s).map_err(|e| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                        as Box<dyn StdError + Send + Sync>
                })?;
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.body(s_resolved);
            } else {
                // Resolve ${ENV_VAR} in body object string values before sending
                let resolved_body = Self::resolve_env_vars_in_value(body);
                // Then resolve any `$attachment:<id>` placeholders to data: URIs
                // by reading bytes via OutputStorageRepository. This is what
                // lets agents pass generated artifacts to external endpoints
                // without ever seeing the raw bytes in their context.
                let resolved_body = self.resolve_attachment_placeholders(resolved_body).await?;
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.json(&resolved_body);
            }
        }

        // 7. Execute Request
        // Note: Headers are not easily printable from request_builder, but we can print what we added
        // println!("DEBUG: Headers: {:?}", request_builder); // RequestBuilder doesn't implement Debug nicely for headers

        let response = request_builder.send().await?;
        let status = response.status().as_u16();
        println!("[HttpNode] ← {} ({})", status, full_url_str);

        // Try to parse response as JSON, fallback to text/string
        let response_body: Value = match response.json::<Value>().await {
            Ok(json) => {
                // Never log response body — it may contain tokens, keys, or PII
                json
            }
            Err(_) => {
                println!("[HttpNode] Response body is not JSON or is empty");
                Value::Null
            }
        };

        // 8. Return Output
        Ok(json!({
            "status": status,
            "body": response_body
        }))
    }

    /// Human-readable description of this node type, used in LLM tool definitions.
    fn description(&self) -> Option<&str> {
        Some("Make HTTP requests to external APIs. Supports GET, POST, PUT, DELETE methods with custom headers and query parameters.")
    }

    /// The default output port is `body` — the parsed JSON response body.
    fn default_output(&self) -> Option<&str> {
        Some("body")
    }

    /// JSON schema describing the node's config and input/output ports.
    fn schema(&self) -> Value {
        json!({
            "type": "http_request",
            "config": {
                "base_url": "string",
                "endpoint": "string",
                "method": "string (GET, POST, PUT, DELETE, etc.)",
                "headers": "map<string, string> (optional)",
                "query_params": "any (optional)"
            },
            "inputs": {
                "base_url": "string (optional)",
                "endpoint": "string (optional)",
                "method": "string (optional)",
                "body": "any (optional)",
                "headers": "map<string, string> (optional)",
                "query_params": "any (optional)"
            },
            "outputs": {
                "status": "integer",
                "body": "any"
            }
        })
    }
}

#[cfg(test)]
mod attachment_placeholder_tests {
    use super::*;
    use crate::storage::domain::{MockOutputStorageRepository, StoredBytes};
    use std::collections::HashMap;
    use std::sync::Arc;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn body_attachment_placeholder_resolved_to_data_uri() {
        let server = MockServer::start().await;

        // Server expects a JSON body whose `image` field is a data URI for [0xDE, 0xAD]
        // base64 = "3q0=" (4 chars). Use that exact bytes/encoding in the assertion.
        Mock::given(method("POST"))
            .and(path("/upload"))
            .and(body_json(serde_json::json!({
                "image": "data:image/png;base64,3q0="
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_read()
            .times(1)
            .withf(|key: &str| key == "gen-abc")
            .returning(|_| {
                Ok(StoredBytes {
                    bytes: vec![0xDE, 0xAD],
                    mime_type: "image/png".to_string(),
                    filename: "img.png".to_string(),
                })
            });

        let node = HttpNode::new().with_storage(Arc::new(storage));

        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/upload",
            "method": "POST",
            "body": {
                // The placeholder must be resolved BEFORE the body is JSON-serialized
                "image": "$attachment:gen-abc"
            }
        });
        let mut state = serde_json::json!({});
        let out = node
            .execute(&HashMap::<String, Value>::new(), &config, &mut state, None)
            .await
            .expect("execute ok — placeholder resolved + POST succeeded");
        assert_eq!(out["status"], 200);
    }

    #[tokio::test]
    async fn body_without_placeholder_passes_through_unchanged() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/plain"))
            .and(body_json(serde_json::json!({ "hello": "world" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let storage = MockOutputStorageRepository::new(); // never called
        let node = HttpNode::new().with_storage(Arc::new(storage));

        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/plain",
            "method": "POST",
            "body": { "hello": "world" }
        });
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("ok");
        assert_eq!(out["status"], 200);
    }

    #[tokio::test]
    async fn placeholder_without_storage_errors_with_clear_hint() {
        let server = MockServer::start().await;
        // Server should NEVER be hit because resolution must fail first.
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let node = HttpNode::new(); // no storage

        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/upload",
            "method": "POST",
            "body": { "image": "$attachment:gen-abc" }
        });
        let err = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("OutputStorageRepository"),
            "error must mention storage: {err}"
        );
    }

    #[tokio::test]
    async fn placeholder_nested_in_array_resolved() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/batch"))
            .and(body_json(serde_json::json!({
                "items": [
                    { "name": "a", "data": "data:image/png;base64,3q0=" }
                ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read().returning(|_| {
            Ok(StoredBytes {
                bytes: vec![0xDE, 0xAD],
                mime_type: "image/png".to_string(),
                filename: "x.png".to_string(),
            })
        });
        let node = HttpNode::new().with_storage(Arc::new(storage));

        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/batch",
            "method": "POST",
            "body": {
                "items": [
                    { "name": "a", "data": "$attachment:gen-1" }
                ]
            }
        });
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("nested placeholder ok");
        assert_eq!(out["status"], 200);
    }
}

#[cfg(test)]
mod multipart_detection_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_multipart_from_form_data_header_lowercase() {
        let headers = json!({ "content-type": "multipart/form-data" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn detects_multipart_with_boundary_param() {
        let headers = json!({ "Content-Type": "multipart/form-data; boundary=foo" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn detects_other_multipart_subtypes() {
        let headers = json!({ "Content-Type": "multipart/mixed" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn rejects_json_content_type() {
        let headers = json!({ "Content-Type": "application/json" });
        assert!(!HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn rejects_missing_content_type() {
        let headers = json!({ "X-Custom": "yes" });
        assert!(!HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let headers = json!({ "CONTENT-TYPE": "multipart/form-data" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }
}

#[cfg(test)]
mod multipart_body_parser_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_url_becomes_url_part() {
        let body = json!({ "files": "https://example.com/a.pdf" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Url {
                field,
                url,
                filename_override,
                content_type_override,
            } => {
                assert_eq!(field, "files");
                assert_eq!(url, "https://example.com/a.pdf");
                assert!(filename_override.is_none());
                assert!(content_type_override.is_none());
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn string_attachment_becomes_attachment_part() {
        let body = json!({ "files": "$attachment:abc123" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Attachment {
                field,
                storage_key,
                filename_override,
                content_type_override,
            } => {
                assert_eq!(field, "files");
                assert_eq!(storage_key, "abc123");
                assert!(filename_override.is_none());
                assert!(content_type_override.is_none());
            }
            other => panic!("expected Attachment, got {other:?}"),
        }
    }

    #[test]
    fn plain_string_becomes_text_part() {
        let body = json!({ "metadata": "uploaded by agent" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Text {
                field,
                value,
                content_type_override,
            } => {
                assert_eq!(field, "metadata");
                assert_eq!(value, "uploaded by agent");
                assert!(content_type_override.is_none());
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn array_expands_to_multiple_parts_under_same_field() {
        let body = json!({ "files": ["https://a/1", "https://a/2", "$attachment:k"] });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[0], PartSpec::Url { field, .. } if field == "files"));
        assert!(matches!(&parts[1], PartSpec::Url { field, .. } if field == "files"));
        assert!(matches!(&parts[2], PartSpec::Attachment { field, .. } if field == "files"));
    }

    #[test]
    fn explicit_url_object_with_overrides() {
        let body = json!({
            "files": [{
                "url": "https://example.com/x",
                "filename": "report.pdf",
                "content_type": "application/pdf"
            }]
        });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Url {
                url,
                filename_override,
                content_type_override,
                ..
            } => {
                assert_eq!(url, "https://example.com/x");
                assert_eq!(filename_override.as_deref(), Some("report.pdf"));
                assert_eq!(content_type_override.as_deref(), Some("application/pdf"));
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn explicit_attachment_object_with_overrides() {
        let body = json!({
            "files": [{
                "attachment": "key-1",
                "filename": "x.png",
                "content_type": "image/png"
            }]
        });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Attachment {
                storage_key,
                filename_override,
                content_type_override,
                ..
            } => {
                assert_eq!(storage_key, "key-1");
                assert_eq!(filename_override.as_deref(), Some("x.png"));
                assert_eq!(content_type_override.as_deref(), Some("image/png"));
            }
            other => panic!("expected Attachment, got {other:?}"),
        }
    }

    #[test]
    fn explicit_text_object_with_content_type() {
        let body = json!({
            "metadata": { "value": "hello", "content_type": "text/csv" }
        });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        match &parts[0] {
            PartSpec::Text {
                value,
                content_type_override,
                ..
            } => {
                assert_eq!(value, "hello");
                assert_eq!(content_type_override.as_deref(), Some("text/csv"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn number_and_boolean_become_text_parts() {
        let body = json!({ "count": 5, "flag": true });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 2);
        let has_count = parts.iter().any(|p| matches!(p, PartSpec::Text { field, value, .. } if field == "count" && value == "5"));
        let has_flag = parts.iter().any(|p| matches!(p, PartSpec::Text { field, value, .. } if field == "flag" && value == "true"));
        assert!(has_count && has_flag);
    }

    #[test]
    fn null_value_omits_field() {
        let body = json!({ "ignored": null, "kept": "yes" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], PartSpec::Text { field, .. } if field == "kept"));
    }

    #[test]
    fn malformed_object_errors() {
        let body = json!({ "files": [{ "unknown_field": "x" }] });
        let err = HttpNode::parse_multipart_body(&body).unwrap_err();
        assert!(
            err.to_string().contains("MultipartConfigError")
                || err.to_string().contains("unrecognized")
        );
    }

    #[test]
    fn body_must_be_object() {
        let body = json!("just a string");
        let err = HttpNode::parse_multipart_body(&body).unwrap_err();
        assert!(err.to_string().contains("object"));
    }
}

#[cfg(test)]
mod multipart_url_resolution_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn happy_path_resolves_size_and_mime_from_get() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .set_body_bytes(vec![1, 2, 3, 4]),
            )
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true, // wiremock serves http://
        };
        let url = format!("{}/file", server.uri());
        let resolved = resolver.resolve(&url).await.unwrap();
        assert_eq!(resolved.size_bytes, 4);
        assert_eq!(resolved.content_type, "application/pdf");
        assert_eq!(resolved.filename, "file");
    }

    #[tokio::test]
    async fn rejects_when_get_returns_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/missing", server.uri());
        let err = resolver.resolve(&url).await.unwrap_err();
        assert!(err.to_string().contains("404"), "got {err}");
    }

    #[tokio::test]
    async fn rejects_http_when_not_allowed() {
        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: false,
        };
        let err = resolver.resolve("http://example.com/x").await.unwrap_err();
        assert!(err.to_string().contains("http://"), "got {err}");
    }

    #[tokio::test]
    async fn rejects_unknown_scheme() {
        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let err = resolver.resolve("ftp://example.com/x").await.unwrap_err();
        assert!(err.to_string().contains("scheme"));
    }

    #[tokio::test]
    async fn no_head_request_is_issued_to_upstream() {
        // V4-signed URLs (GCS / S3) are method-specific — a HEAD against a
        // URL signed for GET returns 4xx. The resolver MUST NOT issue HEAD.
        // We assert this by mounting a single GET mock; if the resolver ever
        // reintroduces a HEAD pre-flight, the request will 404 against the
        // catch-all wiremock default and the test fails.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/signed"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .set_body_bytes(vec![1, 2, 3, 4]),
            )
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/signed", server.uri());
        let resolved = resolver.resolve(&url).await.unwrap();
        assert_eq!(resolved.size_bytes, 4);
        // Confirm only one request was issued, and it was a GET.
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1, "expected exactly 1 request, got {}", received.len());
        assert_eq!(received[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn rejects_when_oversized() {
        let server = MockServer::start().await;
        // wiremock sets Content-Length automatically from the body length, so
        // a body of 999_999 bytes triggers the cap.
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/octet-stream")
                    .set_body_bytes(vec![0u8; 999_999]),
            )
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/big", server.uri());
        let err = resolver.resolve(&url).await.unwrap_err();
        assert!(err.to_string().contains("too large") || err.to_string().contains("FileTooLarge"));
    }

    #[tokio::test]
    async fn filename_from_content_disposition_overrides_url_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .insert_header("Content-Disposition", "attachment; filename=\"report.pdf\"")
                    .set_body_bytes(vec![0u8; 10]),
            )
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/file", server.uri());
        let resolved = resolver.resolve(&url).await.unwrap();
        assert_eq!(resolved.filename, "report.pdf");
    }
}

#[cfg(test)]
mod multipart_execute_tests {
    use super::*;
    use crate::storage::domain::{MockOutputStorageRepository, StoredStream};
    use bytes::Bytes;
    use futures::stream;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a multipart-routed config that POSTs to `<server>/upload`.
    fn mk_config(server_uri: &str, body: Value) -> Value {
        serde_json::json!({
            "base_url": server_uri,
            "endpoint": "/upload",
            "method": "POST",
            "headers": { "Content-Type": "multipart/form-data" },
            "allow_http_urls": true, // wiremock is http://
            "body": body,
        })
    }

    #[tokio::test]
    async fn multipart_with_two_url_parts_sends_form() {
        let server = MockServer::start().await;

        // Two upstream files — GET only (Content-Length comes from body length).
        Mock::given(method("GET"))
            .and(path("/u1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .set_body_bytes(vec![1, 2, 3, 4]),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/u2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .set_body_bytes(vec![5, 6, 7]),
            )
            .mount(&server)
            .await;

        // Downstream upload — capture the multipart body and assert basic shape
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let url1 = format!("{}/u1", server.uri());
        let url2 = format!("{}/u2", server.uri());
        let body = serde_json::json!({ "files": [url1, url2] });
        let config = mk_config(&server.uri(), body);

        let node = HttpNode::new();
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("execute ok");
        assert_eq!(out["status"], 200);

        // Verify the downstream actually got multipart by inspecting recorded requests
        let received = server.received_requests().await.unwrap();
        let upload_req = received
            .iter()
            .find(|r| r.url.path() == "/upload")
            .expect("upload request received");
        let ct = upload_req
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("multipart/form-data"), "got {ct}");
        assert!(
            ct.contains("boundary="),
            "boundary should be present in {ct}"
        );
    }

    #[tokio::test]
    async fn multipart_with_attachment_part_streams_via_storage() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_read_stream()
            .times(1)
            .withf(|key: &str| key == "k-abc")
            .returning(|_| {
                let chunk: Result<Bytes, crate::storage::domain::StorageError> =
                    Ok(Bytes::from_static(b"hello"));
                Ok(StoredStream {
                    stream: Box::pin(stream::once(async move { chunk })),
                    size_bytes: 5,
                    mime_type: "text/plain".to_string(),
                    filename: "hello.txt".to_string(),
                })
            });

        let node = HttpNode::new().with_storage(std::sync::Arc::new(storage));
        let body = serde_json::json!({ "files": "$attachment:k-abc" });
        let config = mk_config(&server.uri(), body);
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("execute ok");
        assert_eq!(out["status"], 200);
    }

    #[tokio::test]
    async fn multipart_with_too_many_parts_errors_before_upload() {
        let server = MockServer::start().await;
        // No upstream mocks needed — should fail before any HEAD.
        let body = serde_json::json!({
            "files": [
                "https://a/1", "https://a/2", "https://a/3", "https://a/4",
                "https://a/5", "https://a/6", "https://a/7", "https://a/8",
                "https://a/9", "https://a/10", "https://a/11"
            ]
        });
        let mut config = mk_config(&server.uri(), body);
        config["max_parts"] = serde_json::json!(10);

        let node = HttpNode::new();
        let err = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("TooManyParts"), "got {err}");
    }

    #[tokio::test]
    async fn existing_json_path_unaffected_without_multipart_header() {
        let server = MockServer::start().await;
        use wiremock::matchers::body_json;
        Mock::given(method("POST"))
            .and(path("/json"))
            .and(body_json(serde_json::json!({ "hi": "there" })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;
        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/json",
            "method": "POST",
            "body": { "hi": "there" }
        });
        let node = HttpNode::new();
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("ok");
        assert_eq!(out["status"], 200);
    }
}
