//! HTTP request node — makes outbound HTTP calls from a DAG.
//!
//! ## Standalone use
//! Configure via `config`: `base_url`, `endpoint`, `method`, `headers`, `query_params`,
//! `body`, `bearer_token`, `authorization`. `config` string values support `${ENV_VAR}`
//! resolution; values arriving over edges never do (they may be a webhook payload or a
//! model's output). Input edges override config values, except that `base_url`, `method`,
//! `headers`, `bearer_token` and `authorization` only come over an edge that names them
//! (`to: "<node>.base_url"`) — see [`ExecutableNode::author_owned_inputs`].
//!
//! ## As an LLM tool (via `tool_configurations`)
//! When invoked by `DagToolExecutor`, extra non-reserved input keys with primitive values
//! (string, number, boolean) are automatically appended as URL query parameters.
//! This is the mechanism that allows `node_schema` container children and `$DYNAMIC`
//! top-level fields to reach the node as flat inputs.
//! Engine-internal inputs (`__colmena_*`, `__node*`) are excluded by prefix — they are
//! bookkeeping for the engine and must never reach an external API.
//!
//! ## Outputs
//! Always returns `{ "status": u16, "body": Value }`.
//! `body` is parsed as JSON; if the response is not valid JSON, `body` is `null`.
//! The default output port is `body`.

use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::infrastructure::env_provenance::{escape_pointer_segment, EnvPolicy};
use crate::dag_engine::infrastructure::nodes::util::session_attachment::read_session_attachment;
use crate::llm::domain::{BoxedByteStream, LlmError};
use crate::llm::infrastructure::files::SignedUrlDownloader;
use reqwest::{Method, Url};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::str::FromStr;
use std::sync::Arc;

/// Executes HTTP requests. Implements [`ExecutableNode`]. Stateless — all configuration
/// comes from `inputs` (highest priority) and `config`.
pub struct HttpNode {
    /// Optional storage adapter — used to resolve `$attachment:<id>` placeholders
    /// in the body. When None, placeholders pass through unchanged (logged warn).
    storage: Option<Arc<dyn crate::storage::domain::OutputStorageRepository>>,
    /// Plan A: optional resolver for `$attachment:<document_id>` placeholders.
    /// When `Some`, every placeholder (JSON and multipart) must be a
    /// document_id of the session; when `None`, the id is a storage_key.
    attachment_resolver: Option<Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>>,
    /// Shared OAuth provider cache. When set, a config `auth` block authenticates
    /// via the refresh_token grant, reusing one token per credential fingerprint.
    oauth_cache: Option<Arc<crate::google_oauth::infrastructure::OAuthProviderCache>>,
    /// Fetches multipart URL parts: public addresses only.
    url_parts: SignedUrlDownloader,
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
    pub stream: BoxedByteStream,
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
    /// The guarded client ([`SignedUrlDownloader`]).
    pub fetcher: SignedUrlDownloader,
}

impl MultipartUrlResolver {
    pub(crate) async fn resolve(
        &self,
        url: &str,
    ) -> Result<ResolvedUrlPart, Box<dyn StdError + Send + Sync>> {
        // Errors name the URL without its query (a signed URL's signature).
        let shown = url.split(['?', '#']).next().unwrap_or(url);
        let parsed = Url::parse(url)
            .map_err(|e| format!("UrlValidationFailed: cannot parse '{shown}': {e}"))?;
        match parsed.scheme() {
            "https" => {}
            "http" if self.allow_http_urls => {}
            "http" => {
                return Err(format!(
                    "UrlValidationFailed: plain http:// URL '{shown}' rejected (set allow_http_urls=true to permit)"
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

        // GET-only: HEAD is intentionally skipped because V4-signed URLs (GCS,
        // S3) are method-specific — a URL signed for GET returns 4xx on HEAD.
        // The guarded client dials public addresses only and returns once the
        // response HEADERS arrive (body not consumed yet), so we can validate
        // Content-Length and reject by dropping `resp` BEFORE any body bytes
        // flow into the worker; a body past the cap ends its stream.
        let fetcher = self.fetcher.clone().capped_at(self.max_file_size_bytes);
        let fetcher = fetcher.with_timeout(std::time::Duration::from_secs(self.timeout_secs));
        let resp = fetcher.fetch(url).await.map_err(|e| match e {
            LlmError::AttachmentTooLarge { limit } => {
                format!("FileTooLarge: '{shown}' is larger than {limit} bytes")
            }
            e => format!("UrlValidationFailed: GET for '{shown}' failed: {e}"),
        })?;
        // Read Content-Length directly from the response header. Using the raw
        // header (not `resp.content_length()`) sidesteps reqwest's
        // decoded-body size_hint quirks.
        let size_bytes = resp
            .headers
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .ok_or_else(|| {
                format!("UrlValidationFailed: GET for '{shown}' returned no Content-Length")
            })?;
        if size_bytes > self.max_file_size_bytes {
            // Drop `resp` before returning so the TCP connection closes and the
            // upstream stops transmitting. No body bytes ever reach the worker.
            drop(resp);
            return Err(format!(
                "FileTooLarge: '{shown}' declared {size_bytes} bytes, max is {}",
                self.max_file_size_bytes
            )
            .into());
        }
        let content_type = resp
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let filename =
            filename_from_disposition(resp.headers.get(reqwest::header::CONTENT_DISPOSITION))
                .unwrap_or_else(|| filename_from_url_path(&parsed));

        Ok(ResolvedUrlPart {
            stream: resp.body,
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
    /// Keys this node consumes itself; they must never travel as query params.
    const RESERVED_KEYS: [&'static str; 11] = [
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
        // The run's id: global state hands it to every node. An API that needs a
        // `session_id` param gets it through `query_params`.
        "session_id",
    ];

    /// True for engine-injected bookkeeping inputs — the domain's
    /// [`crate::dag_engine::domain::node::is_engine_key`] (`__colmena*`, `__node*`).
    ///
    /// Matched by PREFIX rather than listed in [`Self::RESERVED_KEYS`]: that list has to be
    /// extended by hand every time the engine adds an internal input, and the one that gets
    /// forgotten leaks silently into the outbound query string. `__colmena_subgraph_depth`
    /// did exactly that — `DagToolExecutor` injects it into every tool call assuming it is
    /// "harmless for nodes that ignore this key", but this node forwards unknown primitives
    /// as query params. APIs that ignore unknown params hid the leak; one that validates
    /// them rejected the request outright (HTTP 400, the param echoed back as an unexpected
    /// filter), breaking every tool call of an agent built against it.
    fn is_engine_internal(key: &str) -> bool {
        crate::dag_engine::domain::node::is_engine_key(key)
    }

    /// Collects the leftover inputs that should travel as query params.
    ///
    /// Only primitives (string, number, bool) qualify — objects/arrays/nulls are ignored.
    fn collect_extra_query_params<'a>(
        inputs: &'a NodeInputs,
        policy: &EnvPolicy,
    ) -> std::collections::HashMap<&'a str, Value> {
        let mut extra_params = std::collections::HashMap::new();
        for (k, v) in inputs {
            if Self::RESERVED_KEYS.contains(&k.as_str()) || Self::is_engine_internal(k.as_str()) {
                continue;
            }
            match v {
                Value::String(s) => {
                    let pointer = format!("/{}", escape_pointer_segment(k));
                    let s_resolved =
                        Self::expand_if_trusted(s, &pointer, policy).unwrap_or(s.to_string());
                    extra_params.insert(k.as_str(), Value::String(s_resolved));
                }
                Value::Number(_) | Value::Bool(_) => {
                    extra_params.insert(k.as_str(), v.clone());
                }
                _ => {
                    // Ignore Objects, Arrays, Nulls
                }
            }
        }
        extra_params
    }

    pub fn new() -> Self {
        Self {
            storage: None,
            attachment_resolver: None,
            oauth_cache: None,
            url_parts: SignedUrlDownloader::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_url_parts(mut self, fetcher: SignedUrlDownloader) -> Self {
        self.url_parts = fetcher;
        self
    }

    pub fn with_storage(
        mut self,
        storage: Arc<dyn crate::storage::domain::OutputStorageRepository>,
    ) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Plan A: wire an `AttachmentStreamResolver`. When present, every
    /// `$attachment:<id>` (JSON body and multipart) must be a `document_id`
    /// of the calling session; raw storage_keys are rejected.
    pub fn with_attachment_resolver(
        mut self,
        resolver: Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>,
    ) -> Self {
        self.attachment_resolver = Some(resolver);
        self
    }

    /// Wire the shared OAuth provider cache so config `auth` blocks
    /// authenticate via the refresh_token grant.
    pub fn with_oauth_cache(
        mut self,
        cache: Arc<crate::google_oauth::infrastructure::OAuthProviderCache>,
    ) -> Self {
        self.oauth_cache = Some(cache);
        self
    }

    /// Resolve the OAuth provider for this request, if an `auth` block is
    /// configured. Returns Ok(None) when there is no `auth` block.
    fn resolve_oauth_provider(
        &self,
        config: &Value,
        inputs: &NodeInputs,
    ) -> Result<
        Option<Arc<crate::google_oauth::infrastructure::OAuthRefreshTokenProvider>>,
        Box<dyn StdError + Send + Sync>,
    > {
        let spec = match crate::dag_engine::infrastructure::nodes::http_oauth::parse_oauth_auth(
            config, inputs,
        )
        .map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })? {
            Some(s) => s,
            None => return Ok(None),
        };
        let cache = self.oauth_cache.as_ref().ok_or_else(|| {
            Box::new(std::io::Error::other(
                "http_request: `auth` block set but no OAuthProviderCache wired",
            )) as Box<dyn StdError + Send + Sync>
        })?;
        let resolve = |s: &str| -> Result<String, Box<dyn StdError + Send + Sync>> {
            Self::resolve_env_vars(s).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                    as Box<dyn StdError + Send + Sync>
            })
        };
        let token_url = resolve(&spec.token_url)?;
        let client_id = resolve(&spec.client_id)?;
        let client_secret = resolve(&spec.client_secret)?;
        let refresh_token = resolve(&spec.refresh_token)?;
        Ok(Some(cache.get_or_create(
            &token_url,
            &client_id,
            &client_secret,
            &refresh_token,
        )))
    }

    /// Recursively walks a JSON value, replacing every string of the form
    /// `$attachment:<id>` with `data:<mime>;base64,<bytes>`. With a resolver,
    /// `<id>` is a document_id of `agent_session_id` of at most `max_bytes`;
    /// without one, a storage_key. Any unresolved placeholder is an error.
    async fn resolve_attachment_placeholders(
        &self,
        val: Value,
        agent_session_id: Option<&str>,
        max_bytes: u64,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        use base64::Engine;

        match val {
            Value::String(s) if s.starts_with(ATTACHMENT_PLACEHOLDER_PREFIX) => {
                let id = &s[ATTACHMENT_PLACEHOLDER_PREFIX.len()..];
                let bytes = if let Some(resolver) = self.attachment_resolver.as_ref() {
                    read_session_attachment(resolver.as_ref(), agent_session_id, id, max_bytes)
                        .await?
                } else {
                    let storage = self.storage.as_ref().ok_or_else(|| {
                        format!(
                            "http_request: body contains '{s}' but no OutputStorageRepository is wired"
                        )
                    })?;
                    storage
                        .read(id)
                        .await
                        .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?
                };
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes.bytes);
                Ok(Value::String(format!(
                    "data:{};base64,{}",
                    bytes.mime_type, encoded
                )))
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    let v = self.resolve_attachment_placeholders(v, agent_session_id, max_bytes);
                    out.insert(k, Box::pin(v).await?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for v in arr {
                    let v = self.resolve_attachment_placeholders(v, agent_session_id, max_bytes);
                    out.push(Box::pin(v).await?);
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
    /// Unconditional — only for `config`-sourced values, which are always trusted.
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

    // --- Env-provenance gating, both body paths (see env_provenance.rs).
    // An `inputs` value expands `${VAR}` only if trusted; `config` always expands.

    /// Wraps a `resolve_env_vars`-family `String` error as the boxed error
    /// type `execute()` returns — a one-liner replacing a repeated 4-line closure.
    fn io_err(e: String) -> Box<dyn StdError + Send + Sync> {
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }

    /// Header names that never carry a credential. Any other header the author
    /// sets counts as one for [`Self::carries_author_credentials`].
    pub(crate) const NON_CREDENTIAL_HEADERS: &'static [&'static str] = &[
        "accept",
        "accept-encoding",
        "accept-language",
        "cache-control",
        "content-type",
        "user-agent",
    ];

    /// The author's value for `key`: from `config`, or a tool's `fixed` value.
    pub(crate) fn author_value<'a>(
        inputs: &'a NodeInputs,
        config: &'a Value,
        key: &str,
    ) -> Option<&'a Value> {
        config.get(key).or_else(|| {
            inputs.get(key).filter(|_| {
                crate::dag_engine::infrastructure::env_provenance::is_authored_input(inputs, key)
            })
        })
    }

    /// Whether `v` holds an env template anywhere.
    pub(crate) fn has_template(v: &Value) -> bool {
        match v {
            Value::String(s) => s.contains("${"),
            Value::Object(m) => m.values().any(Self::has_template),
            Value::Array(a) => a.iter().any(Self::has_template),
            _ => false,
        }
    }

    /// Whether a `headers` object holds a credential: a header other than
    /// [`Self::NON_CREDENTIAL_HEADERS`], or any value with an env template.
    pub(crate) fn headers_carry_credentials(headers: &Value) -> bool {
        headers.as_object().is_some_and(|h| {
            h.iter().any(|(k, v)| {
                !Self::NON_CREDENTIAL_HEADERS.contains(&k.to_ascii_lowercase().as_str())
                    || Self::has_template(v)
            })
        })
    }

    /// Whether the author's `fixed` leaves (`__colmena_authored_leaves`) hold a
    /// credential: a leaf under one of `credential_fields`, or under `headers`
    /// outside [`Self::NON_CREDENTIAL_HEADERS`], or one that `extra` accepts
    /// by its top-level key.
    pub(crate) fn authored_leaves_carry_credentials(
        inputs: &NodeInputs,
        credential_fields: &[&str],
        extra: impl Fn(&str) -> bool,
    ) -> bool {
        use crate::dag_engine::infrastructure::env_provenance::{
            listed_pointers, AUTHORED_LEAVES_KEY,
        };
        listed_pointers(inputs, AUTHORED_LEAVES_KEY)
            .iter()
            .any(|pointer| {
                let mut segments = pointer.trim_start_matches('/').splitn(3, '/');
                let (top, child) = (segments.next().unwrap_or(""), segments.next());
                match (top, child) {
                    ("headers", Some(name)) => {
                        !Self::NON_CREDENTIAL_HEADERS.contains(&name.to_ascii_lowercase().as_str())
                    }
                    (top, _) if credential_fields.contains(&top) => true,
                    (top, None) => extra(top),
                    _ => false,
                }
            })
    }

    /// Whether the request carries credentials the author configured, in
    /// `config` or as a tool's `fixed` value (whole or one leaf of a container
    /// the caller also filled): a `bearer_token`/`authorization`, a header
    /// other than [`Self::NON_CREDENTIAL_HEADERS`], any query param (in
    /// `query_params` or a top-level extra one), an `endpoint`/`body` with an
    /// env template, any value a dispatcher vouched for as the author's
    /// `${VAR}`, or a `config` leaf the engine filled with a secure value.
    fn carries_author_credentials(inputs: &NodeInputs, config: &Value) -> bool {
        let author = |key: &str| Self::author_value(inputs, config, key);
        crate::dag_engine::infrastructure::env_provenance::carries_env_or_secret(inputs)
            || author("bearer_token").is_some()
            || author("authorization").is_some()
            || author("headers").is_some_and(Self::headers_carry_credentials)
            || author("query_params").is_some_and(|q| q.as_object().is_none_or(|m| !m.is_empty()))
            || author("endpoint").is_some_and(Self::has_template)
            || author("body").is_some_and(Self::has_template)
            || Self::authored_leaves_carry_credentials(
                inputs,
                &["bearer_token", "authorization", "query_params"],
                |key| !Self::RESERVED_KEYS.contains(&key) && !Self::is_engine_internal(key),
            )
    }

    /// Same scheme, host and port.
    fn same_origin(a: &Url, b: &Url) -> bool {
        a.scheme() == b.scheme()
            && a.host_str() == b.host_str()
            && a.port_or_known_default() == b.port_or_known_default()
    }

    /// The author's credentials go only to the author's origin (the `base_url`
    /// the author set) or to a host listed in the author's `allowed_hosts`
    /// (`"host"` or `"host:port"`), mirroring the `auth` block's rule in
    /// `http_oauth.rs`. `url` is where the request is about to go.
    fn check_credential_destination(
        url: &Url,
        author_base_url: Option<&str>,
        inputs: &NodeInputs,
        config: &Value,
    ) -> Result<(), String> {
        if !Self::carries_author_credentials(inputs, config) {
            return Ok(());
        }
        let allowed = Self::author_value(inputs, config, "allowed_hosts");
        Self::credential_destination_allowed(url, author_base_url, allowed).map_err(|host| {
            format!(
                "http_request: the credentials configured for this node are sent only to \
                     its base_url's origin or to a host in `allowed_hosts`; '{host}' is neither"
            )
        })
    }

    /// `Ok` when `url` shares the origin of `author_base_url` or its host
    /// (`"host"` or `"host:port"`) is in `allowed_hosts`; otherwise
    /// `Err("host:port")`. Shared with `socketio_request`.
    pub(crate) fn credential_destination_allowed(
        url: &Url,
        author_base_url: Option<&str>,
        allowed_hosts: Option<&Value>,
    ) -> Result<(), String> {
        let author_origin = author_base_url.and_then(|b| Url::parse(b).ok());
        if author_origin.is_some_and(|a| Self::same_origin(&a, url)) {
            return Ok(());
        }
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let host_port = format!("{host}:{}", url.port_or_known_default().unwrap_or_default());
        let allowed = allowed_hosts
            .and_then(|v| v.as_array())
            .is_some_and(|hosts| {
                hosts.iter().filter_map(|h| h.as_str()).any(|h| {
                    let h = h.to_ascii_lowercase();
                    h == host || h == host_port
                })
            });
        if allowed {
            Ok(())
        } else {
            Err(host_port)
        }
    }

    /// A client whose redirects never take the author's credentials to
    /// another origin: with credentials on the request, only a same-origin
    /// redirect is followed (a cross-origin one is returned as is).
    fn client_for(
        credentials: bool,
        builder: reqwest::ClientBuilder,
    ) -> reqwest::Result<reqwest::Client> {
        if !credentials {
            return builder.build();
        }
        builder
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 10 {
                    attempt.error("too many redirects")
                } else if attempt
                    .previous()
                    .first()
                    .is_some_and(|first| Self::same_origin(first, attempt.url()))
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
    }

    /// Expand `${VAR}` in `raw` only if `pointer` is trusted by `policy`.
    fn expand_if_trusted(raw: &str, pointer: &str, policy: &EnvPolicy) -> Result<String, String> {
        if policy.may_expand(pointer) {
            Self::resolve_env_vars(raw)
        } else {
            Ok(raw.to_string())
        }
    }

    /// Read a string field with inputs-over-config priority, gated per the rule above.
    fn resolve_priority_str(
        inputs: &NodeInputs,
        config: &Value,
        key: &str,
        pointer: &str,
        policy: &EnvPolicy,
        default: &str,
    ) -> Result<String, String> {
        if let Some(s) = inputs.get(key).and_then(|v| v.as_str()) {
            Self::expand_if_trusted(s, pointer, policy)
        } else if let Some(s) = config.get(key).and_then(|v| v.as_str()) {
            Self::resolve_env_vars(s)
        } else {
            Ok(default.to_string())
        }
    }

    /// Same as [`Self::resolve_priority_str`] but for an optional field (no default).
    fn resolve_priority_opt(
        inputs: &NodeInputs,
        config: &Value,
        key: &str,
        pointer: &str,
        policy: &EnvPolicy,
    ) -> Result<Option<String>, String> {
        if let Some(s) = inputs.get(key).and_then(|v| v.as_str()) {
            Ok(Some(Self::expand_if_trusted(s, pointer, policy)?))
        } else if let Some(s) = config.get(key).and_then(|v| v.as_str()) {
            Ok(Some(Self::resolve_env_vars(s)?))
        } else {
            Ok(None)
        }
    }

    /// Gated variant of [`Self::resolve_env_vars_in_value`] for an `inputs`-sourced value.
    fn resolve_env_vars_in_value_gated(
        val: &Value,
        base_pointer: &str,
        policy: &EnvPolicy,
    ) -> Value {
        match val {
            Value::String(s) => Value::String(
                Self::expand_if_trusted(s, base_pointer, policy).unwrap_or_else(|_| s.clone()),
            ),
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    let child_pointer = format!("{base_pointer}/{}", escape_pointer_segment(k));
                    out.insert(
                        k.clone(),
                        Self::resolve_env_vars_in_value_gated(v, &child_pointer, policy),
                    );
                }
                Value::Object(out)
            }
            Value::Array(arr) => Value::Array(
                arr.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let child_pointer = format!("{base_pointer}/{i}");
                        Self::resolve_env_vars_in_value_gated(v, &child_pointer, policy)
                    })
                    .collect(),
            ),
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

    /// For a multipart body that arrived as data: outside the `enabled`
    /// fields, a URL string becomes a text part (never fetched) and a
    /// `{ "url": … }` part is refused. The fields the author enabled pass as is.
    fn gate_multipart_urls(
        body: &Value,
        enabled: &[&str],
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        fn gate(field: &str, v: &Value) -> Result<Value, String> {
            match v {
                Value::String(s)
                    if s.starts_with(URL_HTTPS_PREFIX) || s.starts_with(URL_HTTP_PREFIX) =>
                {
                    Ok(json!({ "value": s }))
                }
                Value::Array(a) => a
                    .iter()
                    .map(|x| gate(field, x))
                    .collect::<Result<_, _>>()
                    .map(Value::Array),
                Value::Object(o) if o.contains_key("url") => Err(format!(
                    "MultipartConfigError: field '{field}' asks the node to fetch a URL; \
                     the author enables that per field in `multipart_url_fields`"
                )),
                other => Ok(other.clone()),
            }
        }
        let Some(map) = body.as_object() else {
            return Ok(body.clone());
        };
        let mut out = serde_json::Map::new();
        for (field, v) in map {
            let v = if enabled.contains(&field.as_str()) {
                v.clone()
            } else {
                gate(field, v)?
            };
            out.insert(field.clone(), v);
        }
        Ok(Value::Object(out))
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

    #[allow(clippy::too_many_arguments)]
    async fn execute_multipart(
        &self,
        full_url: &str,
        method_str: &str,
        merged_headers: &serde_json::Map<String, Value>,
        inputs: &NodeInputs,
        config: &Value,
        policy: &EnvPolicy,
        agent_session_id: Option<&str>,
        credentials: bool,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Env-var resolution on string leaves before parsing, so `${VAR}` works
        // inside URLs and text values — gated for an `inputs` body, as on the
        // JSON path.
        let body_resolved = match inputs.get("body") {
            Some(b) => Self::resolve_env_vars_in_value_gated(b, "/body", policy),
            None => Self::resolve_env_vars_in_value(
                config
                    .get("body")
                    .ok_or("MultipartConfigError: body is required in multipart mode")?,
            ),
        };

        // A body that arrived as data may name a URL for the node to fetch only
        // in a field the author enabled (`multipart_url_fields`).
        let body_from_data = inputs.get("body").is_some()
            && !crate::dag_engine::infrastructure::env_provenance::is_authored_input(
                inputs, "body",
            );
        let body_resolved = if body_from_data {
            let enabled: Vec<&str> = Self::author_value(inputs, config, "multipart_url_fields")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|f| f.as_str()).collect())
                .unwrap_or_default();
            Self::gate_multipart_urls(&body_resolved, &enabled)?
        } else {
            body_resolved
        };

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
            fetcher: self.url_parts.clone(),
        };

        let parts_count = parts.len();
        let mut form = reqwest::multipart::Form::new();
        for spec in parts {
            form = self
                .add_part_to_form(form, spec, &resolver, max_file_size_bytes, agent_session_id)
                .await?;
        }

        // Build the outbound request — same client tuning as JSON path
        let client = Self::client_for(
            credentials,
            crate::shared::http_client::builder().http1_only(),
        )?;
        let url = Url::parse(full_url).map_err(|e| format!("Invalid URL '{full_url}': {e}"))?;
        let method = reqwest::Method::from_str(method_str)
            .map_err(|e| format!("Invalid HTTP method '{method_str}': {e}"))?;
        let mut req = client.request(method, url);
        req = req.header("User-Agent", "colmena-http-node/0.1");

        // Forward all headers EXCEPT Content-Type — reqwest will set
        // multipart/form-data; boundary=... itself. An `inputs` header wins
        // over the `config` one of the same name and is gated like on the
        // JSON path.
        let input_headers = inputs.get("headers").and_then(|v| v.as_object());
        for (k, v) in merged_headers {
            if k.eq_ignore_ascii_case("content-type") {
                continue;
            }
            if let Some(v_str) = v.as_str() {
                let v_resolved = if input_headers.is_some_and(|h| h.contains_key(k)) {
                    let pointer = format!("/headers/{}", escape_pointer_segment(k));
                    Self::expand_if_trusted(v_str, &pointer, policy)
                } else {
                    Self::resolve_env_vars(v_str)
                }
                .map_err(Self::io_err)?;
                req = req.header(k, v_resolved);
            }
        }
        // Auth: `inputs` first (gated), then `config` (always resolves, so a
        // fixed `bearer_token: "${TOKEN}"` in `config` works).
        if let Some(token) =
            Self::resolve_priority_opt(inputs, config, "bearer_token", "/bearer_token", policy)
                .map_err(Self::io_err)?
        {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(auth) =
            Self::resolve_priority_opt(inputs, config, "authorization", "/authorization", policy)
                .map_err(Self::io_err)?
        {
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
        agent_session_id: Option<&str>,
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
                let body = reqwest::Body::wrap_stream(resolved.stream);
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
                // Plan A: with a resolver, `storage_key` holds a document_id
                // of this session (a raw key is NotFound). Direct storage
                // only when no resolver is wired (pre-Plan A behavior).
                let stored = if let Some(resolver) = self.attachment_resolver.as_ref() {
                    let sid =
                        agent_session_id.ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
                            format!(
                                "AttachmentResolveError: body references \
                                 '$attachment:{storage_key}' but no agent_session_id \
                                 is available (resolver requires one)"
                            )
                            .into()
                        })?;
                    resolver.resolve(sid, &storage_key).await.map_err(
                        |e| -> Box<dyn StdError + Send + Sync> {
                            format!("AttachmentResolveError: {e}").into()
                        },
                    )?
                } else if let Some(storage) = self.storage.as_ref() {
                    storage
                        .read_stream(&storage_key)
                        .await
                        .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?
                } else {
                    return Err(format!(
                        "AttachmentNotFound: body references '$attachment:{storage_key}' \
                         but neither AttachmentStreamResolver nor OutputStorageRepository \
                         is wired"
                    )
                    .into());
                };
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
    /// All string values in `config` support `${VAR_NAME}` syntax, resolved via `std::env::var`
    /// at call time. This is the primary mechanism for injecting API keys. An `inputs` value
    /// resolves only at a pointer a tool dispatcher vouched for (see [`EnvPolicy`]).
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
        // 0. Env-provenance policy for this dispatch (see env_provenance.rs).
        // No key present (graph mode) → no `inputs` value expands; `config` does.
        let policy = EnvPolicy::from_inputs(inputs);

        // 1. Parse Configuration (Inputs > Config)
        let base_url =
            Self::resolve_priority_str(inputs, config, "base_url", "/base_url", &policy, "")
                .map_err(Self::io_err)?;

        let endpoint =
            Self::resolve_priority_str(inputs, config, "endpoint", "/endpoint", &policy, "")
                .map_err(Self::io_err)?;

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

        // The author's `base_url`: the effective one, unless it came from
        // runtime data (an edge that names it, an open tool field) — then the
        // one in `config`, if any. Credentials never leave that origin.
        let author_base_url = match inputs.get("base_url") {
            Some(_)
                if !crate::dag_engine::infrastructure::env_provenance::is_authored_input(
                    inputs, "base_url",
                ) =>
            {
                config
                    .get("base_url")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Self::resolve_env_vars(s).ok())
            }
            _ => Some(base_url.clone()),
        };
        Self::check_credential_destination(&url, author_base_url.as_deref(), inputs, config)
            .map_err(Self::io_err)?;
        let credentials = Self::carries_author_credentials(inputs, config);

        // 3. Prepare Client and Request
        // Build client forcing HTTP/1.1 to avoid HTTP/2 issues with some APIs
        let client = Self::client_for(
            credentials,
            crate::shared::http_client::builder().http1_only(),
        )?;

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
        // Input headers (override config) — model-reachable, gated per leaf.
        if let Some(headers) = inputs.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    let pointer = format!("/headers/{}", escape_pointer_segment(k));
                    let v_resolved =
                        Self::expand_if_trusted(v_str, &pointer, &policy).map_err(Self::io_err)?;
                    request_builder = request_builder.header(k, v_resolved);
                }
            }
        }

        // --- Native OAuth2 (refresh_token grant) ---
        // Parse the `auth` block (config-only) and mint a provider. Validation
        // includes mutual exclusion with bearer_token/authorization and the
        // base_url-from-inputs guard.
        let oauth_provider = self.resolve_oauth_provider(config, inputs)?;

        // Handle specific auth inputs. Read from `inputs` first (priority), then
        // fall back to `config` so delivered graphs can fix the token in `config`.
        // Values support `${ENV_VAR}` resolution (e.g. `${HUBSPOT_PRIVATE_APP_TOKEN}`).
        // Skipped entirely when native OAuth is active so we never set a second
        // Authorization header (parse_oauth_auth already rejects the combo).
        if oauth_provider.is_none() {
            if let Some(token) =
                Self::resolve_priority_opt(inputs, config, "bearer_token", "/bearer_token", &policy)
                    .map_err(Self::io_err)?
            {
                request_builder =
                    request_builder.header("Authorization", format!("Bearer {}", token));
            }
            if let Some(auth) = Self::resolve_priority_opt(
                inputs,
                config,
                "authorization",
                "/authorization",
                &policy,
            )
            .map_err(Self::io_err)?
            {
                request_builder = request_builder.header("Authorization", auth);
            }
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
                    let pointer = format!("/query_params/{}", escape_pointer_segment(k));
                    let s_resolved =
                        Self::expand_if_trusted(s, &pointer, &policy).map_err(Self::io_err)?;
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
        let extra_params = Self::collect_extra_query_params(inputs, &policy);
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
            // v1: native OAuth is wired only into the main send path below.
            // `execute_multipart` has its own send, so refuse rather than
            // silently dropping the `auth` block.
            if oauth_provider.is_some() {
                return Err(Box::new(std::io::Error::other(
                    "http_request: native OAuth (`auth`) is not supported with multipart bodies in v1",
                )) as Box<dyn StdError + Send + Sync>);
            }
            let agent_session_id = inputs
                .get("__colmena_agent_session_id")
                .and_then(|v| v.as_str());
            return self
                .execute_multipart(
                    &full_url_str,
                    method_str,
                    &merged_headers,
                    inputs,
                    config,
                    &policy,
                    agent_session_id,
                    credentials,
                )
                .await;
        }

        let body_from_inputs = inputs.get("body");
        let body_val = body_from_inputs.or_else(|| config.get("body"));

        if let Some(body) = body_val {
            if let Some(s) = body.as_str() {
                let s_resolved = if body_from_inputs.is_some() {
                    Self::expand_if_trusted(s, "/body", &policy)
                } else {
                    Self::resolve_env_vars(s)
                }
                .map_err(Self::io_err)?;
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.body(s_resolved);
            } else {
                // Resolve ${ENV_VAR} in body object string values before sending.
                // Gated per-leaf-pointer only when `body` came from `inputs`
                // (model-reachable); a `config`-sourced body always expands.
                let resolved_body = if body_from_inputs.is_some() {
                    Self::resolve_env_vars_in_value_gated(body, "/body", &policy)
                } else {
                    Self::resolve_env_vars_in_value(body)
                };
                // Then resolve any `$attachment:<id>` placeholders to data: URIs
                // by reading bytes via OutputStorageRepository. This is what
                // lets agents pass generated artifacts to external endpoints
                // without ever seeing the raw bytes in their context.
                let sid = inputs
                    .get("__colmena_agent_session_id")
                    .and_then(|v| v.as_str());
                let max = Self::limit_u64(
                    config,
                    "max_file_size_bytes",
                    Self::DEFAULT_MAX_FILE_SIZE_BYTES,
                );
                let resolved_body = self
                    .resolve_attachment_placeholders(resolved_body, sid, max)
                    .await?;
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.json(&resolved_body);
            }
        }

        // 7. Execute Request
        // Note: Headers are not easily printable from request_builder, but we can print what we added
        // println!("DEBUG: Headers: {:?}", request_builder); // RequestBuilder doesn't implement Debug nicely for headers

        let response = if let Some(provider) = oauth_provider {
            crate::dag_engine::infrastructure::nodes::http_oauth::send_with_oauth_retry(
                request_builder,
                provider,
            )
            .await?
        } else {
            request_builder.send().await?
        };
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

    /// Where the request goes, how, and with which credentials: author-set
    /// fields are config-only unless an edge names them.
    /// `endpoint`, `body` and query values stay data: they cannot change the host.
    fn author_owned_inputs(&self) -> &'static [&'static str] {
        &[
            "base_url",
            "method",
            "headers",
            "bearer_token",
            "authorization",
            "allowed_hosts",
            "multipart_url_fields",
        ]
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

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Request shape and auth come from config or inputs (inputs win); the
        // four `max_*`/`allow_*` limits are read through `limit_usize`/`limit_bool`;
        // `auth` is the config-only OAuth block; `secure` is the flag
        // `SecureValueService` reads to encrypt this node's output.
        Some(
            NodeCatalogEntry::no_config()
                .with_field("base_url", FieldSpec::of_type("string"))
                .with_field("endpoint", FieldSpec::of_type("string"))
                .with_field(
                    "method",
                    FieldSpec::of_type("string").valid_values([
                        "GET".into(),
                        "POST".into(),
                        "PUT".into(),
                        "DELETE".into(),
                        "PATCH".into(),
                    ]),
                )
                .with_field("headers", FieldSpec::of_type("object"))
                .with_field("query_params", FieldSpec::of_type("object"))
                .with_field("body", FieldSpec::of_type("any"))
                .with_field("bearer_token", FieldSpec::of_type("string"))
                .with_field("authorization", FieldSpec::of_type("string"))
                .with_field("auth", FieldSpec::of_type("object"))
                .with_field("secure", FieldSpec::of_type("boolean"))
                .with_field("max_file_size_bytes", FieldSpec::of_type("integer"))
                .with_field("max_parts", FieldSpec::of_type("integer"))
                .with_field("url_download_timeout_secs", FieldSpec::of_type("integer"))
                .with_field("allow_http_urls", FieldSpec::of_type("boolean"))
                .with_field("allowed_hosts", FieldSpec::of_type("array"))
                .with_field("multipart_url_fields", FieldSpec::of_type("array"))
                .with_reserved_input_keys(Self::RESERVED_KEYS.iter().copied())
                .with_reserved_input_keys([
                    "__colmena_session_id",
                    "__node_id",
                    "__colmena_resume_answer",
                ]),
        )
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

    #[tokio::test]
    async fn bearer_token_from_config_is_env_resolved_into_authorization_header() {
        use wiremock::matchers::header;

        // Set a unique env var so resolution is observable in the Authorization header.
        std::env::set_var("HTTP_NODE_TEST_TOKEN", "secret-abc-123");

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer secret-abc-123"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let node = HttpNode::new();
        // bearer_token lives in `config` (delivered-graph shape) with a ${ENV} ref.
        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/me",
            "method": "GET",
            "bearer_token": "${HTTP_NODE_TEST_TOKEN}"
        });
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("config bearer_token resolved + auth header sent");
        assert_eq!(out["status"], 200);

        std::env::remove_var("HTTP_NODE_TEST_TOKEN");
    }
}

#[cfg(test)]
mod session_attachment_tests {
    //! `$attachment:<id>` with the session registry wired (as `registry.rs`
    //! does): the id must be a document_id of the calling session, in a JSON
    //! body and in multipart. A raw storage_key is never read.
    use super::*;
    use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
    use crate::llm::domain::{AttachmentRegistry, ProviderKind};
    use crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
    use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
    use crate::storage::domain::{
        MockOutputStorageRepository, OutputStorageRepository, StorageError, StoredStream,
    };
    use bytes::Bytes;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const JPEG: &[u8] = &[0xDE, 0xAD]; // base64 "3q0="

    fn served(k: &str) -> Result<(), StorageError> {
        match k {
            "k1" | "k2" => Ok(()),
            _ => Err(StorageError::InvalidInput(format!("unknown key {k}"))),
        }
    }

    /// `s1` owns `doc-1 → k1` and `s2` owns `doc-2 → k2`. Storage streams both
    /// keys, so a raw key WOULD be readable; `read` has no expectation at all.
    async fn node() -> HttpNode {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        for (s, doc, key) in [("s1", "doc-1", "k1"), ("s2", "doc-2", "k2")] {
            let row = UpsertAttachmentInput {
                agent_session_id: s.into(),
                document_id: doc.into(),
                provider: ProviderKind::Generated,
                provider_file_id: key.into(),
                mime_type: "image/jpeg".into(),
                filename: "a.jpg".into(),
                size_bytes: Some(2),
                label: None,
                description: None,
                source: AttachmentSource::Path(key.into()),
                storage_key: Some(key.into()),
                origin: None,
            };
            reg.upsert(row).await.unwrap();
        }
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read_stream().returning(|k| {
            served(k)?;
            let chunk: Result<Bytes, StorageError> = Ok(Bytes::from_static(JPEG));
            let stream = Box::pin(futures::stream::once(async move { chunk }));
            let (mime_type, filename) = ("image/jpeg".into(), "a.jpg".into());
            Ok(StoredStream {
                stream,
                size_bytes: 2,
                mime_type,
                filename,
            })
        });
        let storage: Arc<dyn OutputStorageRepository> = Arc::new(storage);
        let reg: Arc<dyn AttachmentRegistry> = Arc::new(reg);
        let resolver = Arc::new(AttachmentStreamResolverImpl::new(reg, storage.clone()));
        HttpNode::new()
            .with_storage(storage)
            .with_attachment_resolver(resolver)
    }

    async fn run(body: Value, sid: Option<&str>, extra: Value, server: &MockServer) -> String {
        let node = node().await;
        let mut config = json!({ "base_url": server.uri(), "endpoint": "/", "method": "POST" });
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut inputs = HashMap::from([("body".to_string(), body)]);
        if let Some(s) = sid {
            inputs.insert("__colmena_agent_session_id".into(), json!(s));
        }
        match node.execute(&inputs, &config, &mut json!({}), None).await {
            Ok(out) => format!("status {}", out["status"]),
            Err(e) => e.to_string(),
        }
    }

    /// A server that must never be called: the request has to fail first.
    async fn untouched_server() -> MockServer {
        let server = MockServer::start().await;
        let never = Mock::given(method("POST")).respond_with(ResponseTemplate::new(200));
        never.expect(0).mount(&server).await;
        server
    }

    #[tokio::test]
    async fn json_body_attachment_placeholder_resolves_document_id_via_resolver() {
        let server = MockServer::start().await;
        Mock::given(body_json(
            json!({ "image_url": "data:image/jpeg;base64,3q0=" }),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
        let body = json!({ "image_url": "$attachment:doc-1" });
        assert_eq!(
            run(body, Some("s1"), json!({}), &server).await,
            "status 200"
        );
    }

    #[tokio::test]
    async fn json_body_attachment_placeholder_refuses_what_is_not_a_session_document_id() {
        // A raw key of this session, a raw key of another session, and a
        // document_id of another session: all readable by storage, none ours.
        for id in ["k1", "k2", "doc-2"] {
            let body = json!({ "image_url": format!("$attachment:{id}") });
            let out = run(body, Some("s1"), json!({}), &untouched_server().await).await;
            assert!(out.contains("attachment not found"), "{id}: {out}");
            let hint = "use a document_id from the attachments catalog";
            assert!(out.contains(hint), "{id}: {out}");
        }
    }

    #[tokio::test]
    async fn json_body_attachment_placeholder_needs_the_session_and_respects_the_size_cap() {
        let server = untouched_server().await;
        let body = json!({ "image_url": "$attachment:doc-1" });
        let out = run(body.clone(), None, json!({}), &server).await;
        assert!(out.contains("agent_session_id"), "{out}");
        let cap = json!({ "max_file_size_bytes": 1 });
        let out = run(body, Some("s1"), cap, &server).await;
        assert!(out.contains("FileTooLarge"), "{out}");
    }

    #[tokio::test]
    async fn multipart_attachment_refuses_a_raw_storage_key() {
        let multipart = json!({ "headers": { "Content-Type": "multipart/form-data" } });
        let body = json!({ "file": "$attachment:k1" });
        let out = run(body, Some("s1"), multipart, &untouched_server().await).await;
        assert!(out.contains("attachment not found"), "{out}");
    }

    /// Graph mode, as when a `trigger_webhook` (or a model's JSON) feeds
    /// http_request through a field-less edge: the payload's keys are
    /// flattened into the node's inputs, and this payload names
    /// `__colmena_agent_session_id` = `s2`. Returns the run's error, if any.
    async fn run_graph(sid: Option<&str>, doc: &str, server: &MockServer) -> Option<String> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        use crate::dag_engine::application::run_use_case::DagRunUseCase;
        use crate::dag_engine::infrastructure::nodes::trigger::TriggerWebhookNode;
        use futures::StreamExt;

        struct Nodes(Arc<HttpNode>);
        impl NodeRegistryPort for Nodes {
            fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
                match node_type {
                    "trigger_webhook" => Some(Arc::new(TriggerWebhookNode)),
                    "http_request" => Some(self.0.clone()),
                    _ => None,
                }
            }
            fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
                HashMap::new()
            }
        }
        let payload = json!({
            "__colmena_agent_session_id": "s2",
            "body": { "image_url": format!("$attachment:{doc}") },
        });
        let post = json!({ "base_url": server.uri(), "endpoint": "/", "method": "POST" });
        let graph = serde_json::from_value(json!({
            "nodes": {
                "hook": { "type": "trigger_webhook", "config": { "test_payload": payload } },
                "post": { "type": "http_request", "config": post },
            },
            "edges": [ { "from": "hook", "to": "post" } ],
        }))
        .unwrap();
        let uc = DagRunUseCase::new(Arc::new(Nodes(Arc::new(node().await))), None);
        let sid = sid.map(str::to_string);
        let stream = uc.execute_stream(graph, None, None, false, None, sid, None);
        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            if let Err(e) = event {
                return Some(e.to_string());
            }
        }
        None
    }

    #[tokio::test]
    async fn a_session_id_forged_in_graph_inputs_does_not_reach_another_sessions_document() {
        let out = run_graph(None, "doc-2", &untouched_server().await).await;
        let refused =
            |out: &Option<String>, why: &str| out.as_deref().is_some_and(|e| e.contains(why));
        assert!(refused(&out, "needs an agent_session_id"), "{out:?}");
        let out = run_graph(Some("s1"), "doc-2", &untouched_server().await).await;
        assert!(refused(&out, "attachment not found"), "{out:?}");
    }

    #[tokio::test]
    async fn the_engine_session_id_still_reaches_the_node_in_graph_mode() {
        let server = MockServer::start().await;
        let data_uri = json!({ "image_url": "data:image/jpeg;base64,3q0=" });
        let answer = ResponseTemplate::new(200);
        Mock::given(body_json(data_uri))
            .respond_with(answer)
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(run_graph(Some("s1"), "doc-1", &server).await, None);
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
        };
        let url = format!("{}/signed", server.uri());
        let resolved = resolver.resolve(&url).await.unwrap();
        assert_eq!(resolved.size_bytes, 4);
        // Confirm only one request was issued, and it was a GET.
        let received = server.received_requests().await.unwrap();
        assert_eq!(
            received.len(),
            1,
            "expected exactly 1 request, got {}",
            received.len()
        );
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
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
            fetcher: SignedUrlDownloader::allowing_private_hosts(),
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

    /// A body that arrives as data names a URL for the node to fetch only in
    /// a field the author enabled (`multipart_url_fields`): elsewhere the URL
    /// is sent as text and never fetched, and a `{ "url": … }` part is refused.
    #[tokio::test]
    async fn a_data_body_url_is_fetched_only_in_an_author_enabled_field() {
        let (upload, other) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&upload)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3]))
            .mount(&other)
            .await;
        let internal = format!("{}/internal", other.uri());
        let mut config = mk_config(&upload.uri(), Value::Null);
        config.as_object_mut().unwrap().remove("body");
        let run = |config: Value, body: Value| async move {
            let inputs = HashMap::from([("body".to_string(), body)]);
            HttpNode::new()
                .with_url_parts(SignedUrlDownloader::allowing_private_hosts())
                .execute(&inputs, &config, &mut serde_json::json!({}), None)
                .await
        };

        run(config.clone(), serde_json::json!({ "file": internal }))
            .await
            .unwrap();
        assert!(
            other.received_requests().await.unwrap().is_empty(),
            "a data URL was fetched"
        );
        let sent = upload.received_requests().await.unwrap();
        assert!(String::from_utf8_lossy(&sent[0].body).contains(&internal));

        let object = serde_json::json!({ "file": { "url": internal } });
        assert!(run(config.clone(), object).await.is_err());
        assert!(other.received_requests().await.unwrap().is_empty());

        config["multipart_url_fields"] = serde_json::json!(["file"]);
        run(config, serde_json::json!({ "file": internal }))
            .await
            .unwrap();
        assert_eq!(other.received_requests().await.unwrap().len(), 1);
    }

    /// A URL part on a non-public address is never fetched, and nothing is sent.
    #[tokio::test]
    async fn a_url_part_on_a_non_public_address_is_never_fetched() {
        let server = MockServer::start().await; // records any request
        let body = serde_json::json!({ "file": format!("{}/f?sig=query-value", server.uri()) });
        let config = mk_config(&server.uri(), body);
        let out = HttpNode::new()
            .with_url_parts(SignedUrlDownloader::public_only())
            .execute(&HashMap::new(), &config, &mut serde_json::json!({}), None)
            .await;
        let err = out.unwrap_err().to_string();
        assert!(err.contains("not a public address"), "{err}");
        assert!(!err.contains("query-value"), "{err}");
        assert!(server.received_requests().await.unwrap().is_empty());
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

        let node = HttpNode::new().with_url_parts(SignedUrlDownloader::allowing_private_hosts());
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

    /// Plan A — Hand-rolled stub for `AttachmentStreamResolver`.
    /// Verifies the resolver is reached with the expected `(agent_session_id,
    /// document_id)` and returns a known StoredStream.
    struct StubResolver {
        expected_agent: String,
        expected_id: String,
        bytes: Vec<u8>,
        mime: String,
        filename: String,
    }

    #[async_trait::async_trait]
    impl crate::llm::domain::attachments::AttachmentStreamResolver for StubResolver {
        async fn resolve(
            &self,
            agent_session_id: &str,
            document_id: &str,
        ) -> Result<StoredStream, crate::llm::domain::attachments::AttachmentResolveError> {
            assert_eq!(
                agent_session_id, self.expected_agent,
                "resolver got unexpected agent_session_id"
            );
            if document_id != self.expected_id {
                return Err(
                    crate::llm::domain::attachments::AttachmentResolveError::NotFound {
                        document_id: document_id.to_string(),
                    },
                );
            }
            let payload: Result<Bytes, crate::storage::domain::StorageError> =
                Ok(Bytes::from(self.bytes.clone()));
            let size = self.bytes.len() as u64;
            Ok(StoredStream {
                stream: Box::pin(stream::once(async move { payload })),
                size_bytes: size,
                mime_type: self.mime.clone(),
                filename: self.filename.clone(),
            })
        }
    }

    #[tokio::test]
    async fn multipart_uses_resolver_when_present() {
        use wiremock::matchers::{body_string_contains, header_exists};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload"))
            .and(header_exists("content-type"))
            // The wiremock body for multipart is the raw form bytes; "hello"
            // appears verbatim because the resolver yields it as the file payload.
            .and(body_string_contains("hello"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let resolver = std::sync::Arc::new(StubResolver {
            expected_agent: "agent_x".to_string(),
            expected_id: "doc-1".to_string(),
            bytes: b"hello".to_vec(),
            mime: "text/plain".to_string(),
            filename: "hello.txt".to_string(),
        });

        // Note: no storage — only the resolver. Confirms the resolver path is
        // taken when wired (and not the legacy storage path).
        let node = HttpNode::new().with_attachment_resolver(resolver);
        let body = serde_json::json!({ "file": "$attachment:doc-1" });
        let config = mk_config(&server.uri(), body);

        let mut inputs: HashMap<String, Value> = HashMap::new();
        inputs.insert(
            "__colmena_agent_session_id".to_string(),
            serde_json::json!("agent_x"),
        );

        let out = node
            .execute(&inputs, &config, &mut serde_json::json!({}), None)
            .await
            .expect("execute ok");
        assert_eq!(out["status"], 200);
    }

    #[tokio::test]
    async fn multipart_resolver_without_agent_session_id_errors() {
        let server = MockServer::start().await;
        // No mocks: the request should fail before any network I/O.

        let resolver = std::sync::Arc::new(StubResolver {
            expected_agent: "agent_x".to_string(),
            expected_id: "doc-1".to_string(),
            bytes: b"hello".to_vec(),
            mime: "text/plain".to_string(),
            filename: "hello.txt".to_string(),
        });
        let node = HttpNode::new().with_attachment_resolver(resolver);
        let body = serde_json::json!({ "file": "$attachment:doc-1" });
        let config = mk_config(&server.uri(), body);

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
            err.to_string().contains("AttachmentResolveError"),
            "got {err}"
        );
        assert!(err.to_string().contains("agent_session_id"), "got {err}");
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

#[cfg(test)]
mod oauth_integration_tests {
    use super::*;

    #[test]
    fn with_oauth_cache_sets_field() {
        use crate::google_oauth::infrastructure::OAuthProviderCache;
        let node = HttpNode::new().with_oauth_cache(std::sync::Arc::new(OAuthProviderCache::new()));
        assert!(node.oauth_cache.is_some());
    }

    #[tokio::test]
    async fn execute_authenticates_with_oauth_block() {
        use crate::google_oauth::infrastructure::OAuthProviderCache;
        use std::collections::HashMap;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let token_srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"access_token":"ya29.exec","expires_in":3600,"token_type":"Bearer"}"#,
            ))
            .mount(&token_srv)
            .await;

        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/data"))
            .and(header("Authorization", "Bearer ya29.exec"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&api)
            .await;

        let node = HttpNode::new().with_oauth_cache(std::sync::Arc::new(OAuthProviderCache::new()));
        let config = serde_json::json!({
            "base_url": api.uri(),
            "endpoint": "/data",
            "method": "GET",
            "auth": {
                "type": "oauth2_refresh_token",
                "token_url": token_srv.uri(),
                "client_id": "cid", "client_secret": "csec", "refresh_token": "rt"
            }
        });
        let inputs: HashMap<String, serde_json::Value> = HashMap::new();
        let mut state = serde_json::Value::Null;
        let out = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect("ok");
        assert_eq!(out["status"], 200);
        assert_eq!(out["body"]["ok"], true);
    }

    #[tokio::test]
    async fn execute_surfaces_revoked_refresh_token_error() {
        use crate::google_oauth::infrastructure::OAuthProviderCache;
        use std::collections::HashMap;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Token endpoint returns invalid_grant (revoked/expired refresh token).
        let token_srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
            ))
            .mount(&token_srv)
            .await;

        let node = HttpNode::new().with_oauth_cache(std::sync::Arc::new(OAuthProviderCache::new()));
        let config = serde_json::json!({
            "base_url": "https://api.example.com", "endpoint": "/data", "method": "GET",
            "auth": { "type": "oauth2_refresh_token", "token_url": token_srv.uri(),
                      "client_id": "cid", "client_secret": "csec", "refresh_token": "rt" }
        });
        let inputs: HashMap<String, serde_json::Value> = HashMap::new();
        let mut state = serde_json::Value::Null;
        let err = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect_err("revoked token must error");
        // The mint fails before any API call; the error must surface (our "OAuth:" prefix).
        assert!(
            format!("{err}").contains("OAuth"),
            "expected OAuth error, got: {err}"
        );
    }

    #[tokio::test]
    async fn execute_rejects_auth_plus_bearer_token() {
        use std::collections::HashMap;
        let node = HttpNode::new().with_oauth_cache(std::sync::Arc::new(
            crate::google_oauth::infrastructure::OAuthProviderCache::new(),
        ));
        let config = serde_json::json!({
            "base_url": "https://x", "endpoint": "/y", "bearer_token": "static",
            "auth": { "type": "oauth2_refresh_token", "token_url": "u",
                      "client_id": "c", "client_secret": "s", "refresh_token": "r" }
        });
        let inputs: HashMap<String, serde_json::Value> = HashMap::new();
        let mut state = serde_json::Value::Null;
        let err = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .expect_err("mutually exclusive");
        assert!(format!("{err}").contains("mutually exclusive"));
    }
}

#[cfg(test)]
mod extra_query_params_tests {
    use super::*;
    use std::collections::HashMap;

    fn untrusted() -> EnvPolicy {
        EnvPolicy::Restricted(Default::default())
    }

    /// Global state hands the run's `session_id` to every node; it used to
    /// reach external APIs as `?session_id=<run uuid>`.
    #[test]
    fn the_run_session_id_is_never_a_query_param() {
        let given = inputs(&[("session_id", json!("run-uuid")), ("page", json!("2"))]);
        let got = HttpNode::collect_extra_query_params(&given, &untrusted());
        assert_eq!(got.get("session_id"), None);
        assert_eq!(got.get("page"), Some(&json!("2")));
    }

    fn inputs(pairs: &[(&str, Value)]) -> NodeInputs {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<HashMap<_, _>>()
    }

    #[test]
    fn engine_internal_inputs_never_become_query_params() {
        // Exactly what DagToolExecutor injects into every tool call. `__colmena_subgraph_depth`
        // was the one missing from the allowlist and leaked to the wire.
        let given = inputs(&[
            ("__colmena_subgraph_depth", json!(0)),
            ("__colmena_session_id", json!("sess-1")),
            ("__colmena_agent_session_id", json!("agent-1")),
            ("__colmena_node_id_path", json!("a/b")),
            ("__colmena_resume_answer", json!("yes")),
            ("__node_id", json!("n1")),
        ]);
        let got = HttpNode::collect_extra_query_params(&given, &untrusted());

        assert!(got.is_empty(), "engine-internal inputs leaked: {got:?}");
    }

    #[test]
    fn reserved_keys_never_become_query_params() {
        let given = inputs(&[
            ("base_url", json!("https://api.example.com")),
            ("endpoint", json!("/v1/items")),
            ("method", json!("GET")),
            ("bearer_token", json!("t")),
            ("authorization", json!("Bearer t")),
            ("secure", json!(true)),
        ]);
        let got = HttpNode::collect_extra_query_params(&given, &untrusted());

        assert!(got.is_empty(), "reserved keys leaked: {got:?}");
    }

    #[test]
    fn caller_supplied_primitives_still_become_query_params() {
        // The filter must not be over-broad: genuine LLM-supplied params still travel.
        let given = inputs(&[
            ("page", json!("1")),
            ("limit", json!(5)),
            ("active", json!(true)),
            ("__colmena_subgraph_depth", json!(0)),
        ]);
        let got = HttpNode::collect_extra_query_params(&given, &untrusted());

        assert_eq!(got.len(), 3);
        assert_eq!(got.get("page"), Some(&json!("1")));
        assert_eq!(got.get("limit"), Some(&json!(5)));
        assert_eq!(got.get("active"), Some(&json!(true)));
    }

    #[test]
    fn non_primitive_inputs_are_ignored() {
        let given = inputs(&[
            ("obj", json!({"a": 1})),
            ("arr", json!([1, 2])),
            ("nil", Value::Null),
            ("keep", json!("yes")),
        ]);
        let got = HttpNode::collect_extra_query_params(&given, &untrusted());

        assert_eq!(got.len(), 1);
        assert_eq!(got.get("keep"), Some(&json!("yes")));
    }
}

/// Env-provenance gate. An `inputs`-sourced `${VAR}` expands only when
/// trusted; untrusted never errors.
#[cfg(test)]
mod env_provenance_gating_tests {
    use super::*;
    use crate::dag_engine::infrastructure::env_provenance::ENV_TRUSTED_PATHS_KEY;
    use std::collections::HashMap;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn restricted(pointers: &[&str]) -> Value {
        json!(pointers)
    }

    fn cfg(base_url: &str, endpoint: &str, method: &str) -> Value {
        json!({ "base_url": base_url, "endpoint": endpoint, "method": method })
    }

    /// Runs `HttpNode::execute` and unwraps — shared by every case below.
    async fn run(inputs: HashMap<String, Value>, cfg: Value) -> Value {
        HttpNode::new()
            .execute(&inputs, &cfg, &mut json!({}), None)
            .await
            .expect("execute ok")
    }

    /// Graph mode (no trusted-paths key): an edge-delivered value never expands.
    #[tokio::test]
    async fn no_key_leaves_an_inputs_bearer_token_literal() {
        std::env::set_var("HTTP_GATING_TEST_LEGACY", "legacy-secret");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer ${HTTP_GATING_TEST_LEGACY}"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let inputs = HashMap::from([(
            "bearer_token".to_string(),
            json!("${HTTP_GATING_TEST_LEGACY}"),
        )]);
        let out = run(inputs, cfg(&server.uri(), "/me", "GET")).await;
        assert_eq!(out["status"], 200);
        std::env::remove_var("HTTP_GATING_TEST_LEGACY");
    }

    /// RED on pre-gate code: an untrusted `/bearer_token` sends it literally, never errors.
    #[tokio::test]
    async fn restricted_policy_leaves_untrusted_bearer_token_literal_and_never_errors_on_missing_var(
    ) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer ${MISSING_VAR}"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let inputs = HashMap::from([
            ("bearer_token".to_string(), json!("${MISSING_VAR}")),
            (ENV_TRUSTED_PATHS_KEY.to_string(), restricted(&[])),
        ]);
        let out = run(inputs, cfg(&server.uri(), "/me", "GET")).await;
        assert_eq!(out["status"], 200);
    }

    /// RED on pre-gate code: untrusted extra-query-param, header, body leaf stay literal.
    #[tokio::test]
    async fn restricted_policy_leaves_untrusted_query_param_header_and_body_leaf_literal() {
        std::env::set_var("PROBE", "should-never-appear");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/anything"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let inputs = HashMap::from([
            ("q".to_string(), json!("${PROBE}")),
            ("headers".to_string(), json!({ "X-Target": "${PROBE}" })),
            ("body".to_string(), json!({ "a": { "b": "${PROBE}" } })),
            (ENV_TRUSTED_PATHS_KEY.to_string(), restricted(&[])),
        ]);
        let out = run(inputs, cfg(&server.uri(), "/anything", "POST")).await;
        assert_eq!(out["status"], 200);

        let received = server.received_requests().await.unwrap();
        let req = &received[0];
        assert_eq!(
            req.url
                .query_pairs()
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v.to_string()),
            Some("${PROBE}".to_string())
        );
        assert_eq!(
            req.headers.get("x-target").unwrap().to_str().unwrap(),
            "${PROBE}"
        );
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["a"]["b"], "${PROBE}");

        std::env::remove_var("PROBE");
    }

    /// The multipart path honors the policy too (it used to resolve every value):
    /// untrusted body leaf, header and bearer stay literal; `config` still resolves.
    #[tokio::test]
    async fn restricted_policy_leaves_untrusted_multipart_values_literal() {
        std::env::set_var("HTTP_GATING_TEST_MP", "multipart-test-only");
        std::env::set_var("HTTP_GATING_TEST_MP_CFG", "config-test-only");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let inputs = HashMap::from([
            (
                "headers".to_string(),
                json!({ "Content-Type": "multipart/form-data", "X-Target": "${HTTP_GATING_TEST_MP}" }),
            ),
            (
                "body".to_string(),
                json!({ "note": "${HTTP_GATING_TEST_MP}" }),
            ),
            ("bearer_token".to_string(), json!("${HTTP_GATING_TEST_MP}")),
            (ENV_TRUSTED_PATHS_KEY.to_string(), restricted(&[])),
        ]);
        let mut config = cfg(&server.uri(), "/upload", "POST");
        config["headers"] = json!({ "X-Author": "${HTTP_GATING_TEST_MP_CFG}" });
        assert_eq!(run(inputs, config).await["status"], 200);

        let req = &server.received_requests().await.unwrap()[0];
        let raw = String::from_utf8_lossy(&req.body);
        assert!(!raw.contains("multipart-test-only") && raw.contains("${HTTP_GATING_TEST_MP}"));
        let header = |k: &str| req.headers.get(k).unwrap().to_str().unwrap().to_string();
        assert_eq!(header("x-target"), "${HTTP_GATING_TEST_MP}");
        assert_eq!(header("authorization"), "Bearer ${HTTP_GATING_TEST_MP}");
        assert_eq!(header("x-author"), "config-test-only");
        std::env::remove_var("HTTP_GATING_TEST_MP");
        std::env::remove_var("HTTP_GATING_TEST_MP_CFG");
    }

    /// Pass on both: a listed header pointer expands exactly that leaf.
    #[tokio::test]
    async fn restricted_policy_expands_a_listed_header_pointer() {
        std::env::set_var("HTTP_GATING_TEST_TRUSTED", "trusted-value");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/anything"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let inputs = HashMap::from([
            (
                "headers".to_string(),
                json!({ "X-Op": "${HTTP_GATING_TEST_TRUSTED}" }),
            ),
            (
                ENV_TRUSTED_PATHS_KEY.to_string(),
                restricted(&["/headers/X-Op"]),
            ),
        ]);
        let out = run(inputs, cfg(&server.uri(), "/anything", "GET")).await;
        assert_eq!(out["status"], 200);

        let received = server.received_requests().await.unwrap();
        assert_eq!(
            received[0].headers.get("x-op").unwrap().to_str().unwrap(),
            "trusted-value"
        );
        std::env::remove_var("HTTP_GATING_TEST_TRUSTED");
    }
}

/// The author's credentials stay on the author's origin: a redirect to
/// another origin is not followed while they ride on the request.
#[cfg(test)]
mod redirect_tests {
    use super::*;
    use std::collections::HashMap;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn a_cross_origin_redirect_never_carries_author_credentials() {
        std::env::set_var("COLMENA_CLASS_TEST_REDIR", "redir-key-test-only");
        let (author, other) = (MockServer::start().await, MockServer::start().await);
        Mock::given(wiremock::matchers::any())
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", format!("{}/x", other.uri())),
            )
            .mount(&author)
            .await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&other)
            .await;
        let config = json!({
            "base_url": author.uri(), "endpoint": "/a", "method": "GET",
            "headers": { "X-Api-Key": "${COLMENA_CLASS_TEST_REDIR}" }
        });
        let out = HttpNode::new()
            .execute(&HashMap::new(), &config, &mut json!({}), None)
            .await
            .unwrap();
        let reached = other.received_requests().await.unwrap().iter().any(|r| {
            r.headers
                .get("x-api-key")
                .is_some_and(|v| v == "redir-key-test-only")
        });
        assert!(
            !reached,
            "the author's key followed a redirect to another origin"
        );
        assert_eq!(out["status"], 302);
        std::env::remove_var("COLMENA_CLASS_TEST_REDIR");
    }

    /// Without author credentials a redirect is followed as before.
    #[tokio::test]
    async fn a_redirect_without_author_credentials_is_followed() {
        let (author, other) = (MockServer::start().await, MockServer::start().await);
        Mock::given(wiremock::matchers::any())
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", format!("{}/x", other.uri())),
            )
            .mount(&author)
            .await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&other)
            .await;
        let config = json!({ "base_url": author.uri(), "endpoint": "/a", "method": "GET" });
        let out = HttpNode::new()
            .execute(&HashMap::new(), &config, &mut json!({}), None)
            .await
            .unwrap();
        assert_eq!(out["status"], 200);
    }
}
