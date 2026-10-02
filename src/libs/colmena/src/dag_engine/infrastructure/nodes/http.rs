//! HTTP request node — makes outbound HTTP calls from a DAG.
//!
//! ## Standalone use
//! Configure via `config`: `base_url`, `endpoint`, `method`, `headers`, `query_params`,
//! `body`, `bearer_token`, `authorization`. `config` string values support `${ENV_VAR}`
//! resolution; values arriving over edges never do (they may be a webhook payload or a
//! model's output). Input edges override config values, except that `base_url`, `method`,
//! `headers`, `bearer_token` and `authorization` only come over an edge that names them
//! (`to: "<node>.base_url"`) — see [`ExecutableNode::author_owned_inputs`]. A `base_url`
//! that comes from data dials only public addresses, unless `allowed_hosts` lists its host.
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
//!
//! When the response is a file (PDF, image, audio, video, Office document —
//! decided by the bytes, then by `Content-Type`) and the node has a storage
//! adapter, the bytes are stored as a session attachment and the output gains
//! `"files": [{ "document_id", "mime_type", "filename", "size_bytes" }]`, with
//! `body` still `null`. `max_file_size_bytes` caps the size kept; a larger
//! file, or one with no storage adapter, leaves only `body: null`.
//!
//! ## Attachments
//! In a JSON body, a whole string `"$attachment:<document_id>"` becomes a
//! `data:` URI of that document of the session, and
//! `"$attachment_url:<document_id>"` a read URL the host's storage issues for
//! it — only toward an address the author fixed (`base_url`, `endpoint` and
//! any `Host` header in `config` or a tool's `fixed` values), with no redirect
//! to another origin. In `query_params` or a multipart part the form fails.
//! The node's output and error texts show the placeholder wherever the
//! response repeats that URL or its long query values.

use crate::dag_engine::application::ports::HostTokenPort;
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::infrastructure::env_provenance::{
    escape_pointer_segment, is_authored_input, EnvPolicy,
};
use crate::dag_engine::infrastructure::nodes::util::attachment_id::build_document_id;
use crate::dag_engine::infrastructure::nodes::util::response_file;
use crate::dag_engine::infrastructure::nodes::util::session_attachment::read_session_attachment;
use crate::google_oauth::{domain::AuthTokenProvider, infrastructure::HostRefreshTokenProvider};
use crate::llm::domain::attachments::{origin, AttachmentSource, UpsertAttachmentInput};
use crate::llm::domain::ProviderKind;
use crate::llm::domain::{BoxedByteStream, LlmError};
use crate::llm::infrastructure::files::signed_url_downloader::{
    is_dial_refused, DialGuard, DialRefused,
};
use crate::llm::infrastructure::files::SignedUrlDownloader;
use crate::storage::domain::StoreRequest;
use reqwest::{Method, Url};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::str::FromStr;
use std::sync::Arc;

/// A request's token provider, plus the static token to send when it has none.
type TokenSource = (Arc<dyn AuthTokenProvider>, Option<String>);

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
    /// Fetches multipart URL parts: public addresses only. Its address rule
    /// also bounds a destination that comes from data.
    url_parts: SignedUrlDownloader,
    /// Registers a file response as a session attachment, so `load_attachment`
    /// and `$attachment:<document_id>` reach it and the host can show it.
    attachment_registry: Option<Arc<dyn crate::llm::domain::AttachmentRegistry>>,
    /// The embedder's port for host-refreshed bearer tokens. Set once, after
    /// construction, by `HashMapNodeRegistry::set_host_token_port`.
    pub(crate) host_token_port: std::sync::OnceLock<Arc<dyn HostTokenPort>>,
}

impl Default for HttpNode {
    fn default() -> Self {
        Self::new()
    }
}

const ATTACHMENT_PLACEHOLDER_PREFIX: &str = "$attachment:";
/// `"$attachment_url:<document_id>"`: a read URL the host's storage issues
/// for that document of the session, in place of its bytes.
const ATTACHMENT_URL_PLACEHOLDER_PREFIX: &str = "$attachment_url:";
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
    const RESERVED_KEYS: [&'static str; 13] = [
        "base_url",
        "endpoint",
        "method",
        "headers",
        "body",
        "query_params",     // correct key used throughout the codebase
        "query_parameters", // kept for backward compat
        "bearer_token",
        "bearer_refresh",
        "authorization",
        "secure", // internal Colmena flag — NEVER send to external APIs
        // The run's id: global state hands it to every node. An API that needs a
        // `session_id` param gets it through `query_params`.
        "session_id",
        // Author-set lifetime of an `$attachment_url:` URL — never a query param.
        "attachment_url_ttl_seconds",
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
            attachment_registry: None,
            host_token_port: std::sync::OnceLock::new(),
        }
    }

    /// The embedder's `HostTokenPort`, when it set one.
    pub fn host_token_port(&self) -> Option<Arc<dyn HostTokenPort>> {
        self.host_token_port.get().cloned()
    }

    /// Wire the session attachment registry: a file response is then
    /// registered under its `document_id` (fail-soft, as the media nodes do).
    pub fn with_attachment_registry(
        mut self,
        registry: Arc<dyn crate::llm::domain::AttachmentRegistry>,
    ) -> Self {
        self.attachment_registry = Some(registry);
        self
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

    /// The token provider for this request: the `auth` block's, or a
    /// `HostRefreshTokenProvider` for a `bearer_token` with `bearer_refresh`
    /// when the embedder set a `HostTokenPort` (with the seed as fallback).
    /// Built per execution: it carries the run's `agent_session_id`.
    /// `Ok(None)` for neither; then `bearer_refresh` is ignored.
    fn resolve_oauth_provider(
        &self,
        config: &Value,
        inputs: &NodeInputs,
        policy: &EnvPolicy,
    ) -> Result<Option<TokenSource>, Box<dyn StdError + Send + Sync>> {
        use crate::dag_engine::infrastructure::nodes::http_oauth as oauth;
        let refresh = oauth::parse_bearer_refresh(config, inputs).map_err(Self::io_err)?;
        let spec = match oauth::parse_oauth_auth(config, inputs).map_err(Self::io_err)? {
            Some(s) => s,
            None => {
                let (Some((handle, expires_at)), Some(port)) = (refresh, self.host_token_port())
                else {
                    return Ok(None);
                };
                let token = "bearer_token";
                let seed =
                    Self::resolve_priority_opt(inputs, config, token, "/bearer_token", policy);
                let seed = seed.map_err(Self::io_err)?.unwrap_or_default();
                let sid = Self::input_str(inputs, "__colmena_agent_session_id");
                let provider =
                    HostRefreshTokenProvider::new(port, handle, seed.clone(), expires_at, sid);
                return Ok(Some((Arc::new(provider), Some(seed))));
            }
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
        let provider = cache.get_or_create(&token_url, &client_id, &client_secret, &refresh_token);
        Ok(Some((provider, None)))
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

    /// Lifetime asked of the host when the author sets none: 15 minutes.
    const DEFAULT_ATTACHMENT_URL_TTL_SECS: u64 = 900;

    /// The lifetime to ask of the host for each `$attachment_url:` URL: the
    /// author's `attachment_url_ttl_seconds` (in `config`, or a tool's `fixed`
    /// value), a positive whole number of seconds, else 900. A value from
    /// runtime data is not the author's and is ignored. The host may cap it.
    fn attachment_url_ttl(inputs: &NodeInputs, config: &Value) -> Result<u64, String> {
        match Self::author_value(inputs, config, "attachment_url_ttl_seconds") {
            None | Some(Value::Null) => Ok(Self::DEFAULT_ATTACHMENT_URL_TTL_SECS),
            Some(v) => v.as_u64().filter(|n| *n > 0).ok_or_else(|| {
                "http_request: attachment_url_ttl_seconds must be a positive whole number \
                 of seconds"
                    .to_string()
            }),
        }
    }

    /// Whether a string value of `v`, at any depth, is a
    /// `"$attachment_url:<document_id>"`. Object keys are not looked at.
    fn has_attachment_url(v: &Value) -> bool {
        match v {
            Value::String(s) => s.starts_with(ATTACHMENT_URL_PLACEHOLDER_PREFIX),
            Value::Object(m) => m.values().any(Self::has_attachment_url),
            Value::Array(a) => a.iter().any(Self::has_attachment_url),
            _ => false,
        }
    }

    /// Replaces every `"$attachment_url:<document_id>"` string in `val` with
    /// a read URL the host's storage issues for that document of the session,
    /// valid for about `ttl_seconds`, and records it with its placeholder in
    /// `issued`, to scrub the output. Fails when the id is not the session's,
    /// or the host issues no URLs.
    async fn resolve_attachment_urls(
        &self,
        val: Value,
        agent_session_id: Option<&str>,
        ttl_seconds: u64,
        issued: &mut Vec<(String, String)>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        match val {
            Value::String(s) if s.starts_with(ATTACHMENT_URL_PLACEHOLDER_PREFIX) => {
                let id = &s[ATTACHMENT_URL_PLACEHOLDER_PREFIX.len()..];
                let resolver = self.attachment_resolver.as_ref().ok_or_else(|| {
                    format!(
                        "AttachmentResolveError: '{s}' needs the session's attachment registry, \
                         and this engine has none"
                    )
                })?;
                let sid = agent_session_id.ok_or_else(|| {
                    format!("AttachmentResolveError: '{s}' needs an agent_session_id")
                })?;
                let url = resolver
                    .resolve_url(sid, id, ttl_seconds)
                    .await
                    .map_err(|e| format!("AttachmentResolveError: {e}"))?
                    .ok_or_else(|| {
                        format!(
                            "AttachmentUrlUnavailable: this host does not provide attachment \
                             URLs; use \"$attachment:{id}\" for the bytes"
                        )
                    })?;
                issued.push((url.clone(), s));
                Ok(Value::String(url))
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    let v = self.resolve_attachment_urls(v, agent_session_id, ttl_seconds, issued);
                    out.insert(k, Box::pin(v).await?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for v in arr {
                    let v = self.resolve_attachment_urls(v, agent_session_id, ttl_seconds, issued);
                    out.push(Box::pin(v).await?);
                }
                Ok(Value::Array(out))
            }
            other => Ok(other),
        }
    }

    /// What the scrub replaces, longest first, each with the placeholder of
    /// its URL in `issued`: the URL, and each of its query values that
    /// decodes to 16 or more characters (a signature, a credential, an
    /// expiry token), so a partial echo loses them too. Each in the forms an
    /// echo writes: as is, percent-decoded, percent-encoded (strictly, or
    /// keeping `/`), and each of those JSON-encoded inside a string (`\/`,
    /// `\u0026` and the like) or in HTML (`&amp;`).
    fn scrub_forms(issued: &[(String, String)]) -> Vec<(String, String)> {
        let decode = |s: &str| urlencoding::decode(s).map_or(s.to_string(), |d| d.into_owned());
        let encode = |s: &str| urlencoding::encode(s).into_owned();
        // `\uXXXX` for each of `chars`, as some JSON encoders write them.
        let unicode = |s: &str, chars: &str| {
            let each = |s: String, c: char| s.replace(c, &format!("\\u{:04x}", c as u32));
            chars.chars().fold(s.to_string(), each)
        };
        let mut forms: Vec<(String, String)> = Vec::new();
        for (url, placeholder) in issued {
            let query = url.split_once('?').map_or("", |(_, q)| q);
            let query = query.split('#').next().unwrap_or_default();
            let values = query.split('&').filter_map(|p| Some(p.split_once('=')?.1));
            let long = values.filter(|v| decode(v).chars().count() >= 16);
            for token in std::iter::once(url.as_str()).chain(long) {
                let decoded = decode(token);
                let (strict, strict_of_decoded) = (encode(token), encode(&decoded));
                let percent = [
                    strict.replace("%2F", "/"),
                    strict_of_decoded.replace("%2F", "/"),
                    strict,
                    strict_of_decoded,
                    decoded,
                    token.to_string(),
                ];
                for p in percent {
                    for s in [p.replace('/', "\\/"), p] {
                        let html = s.replace('&', "&amp;");
                        for form in [unicode(&s, "&<>"), unicode(&s, "&<>="), html, s] {
                            if !form.is_empty() && forms.iter().all(|(f, _)| *f != form) {
                                forms.push((form, placeholder.clone()));
                            }
                        }
                    }
                }
            }
        }
        forms.sort_by_key(|(f, _)| std::cmp::Reverse(f.len()));
        forms
    }

    /// `s` with every form of [`Self::scrub_forms`] replaced by its placeholder.
    fn scrub_text(mut s: String, forms: &[(String, String)]) -> String {
        for (form, placeholder) in forms {
            if s.contains(form.as_str()) {
                s = s.replace(form.as_str(), placeholder);
            }
        }
        s
    }

    /// `v` with every string and object key scrubbed (see
    /// [`Self::scrub_forms`]): an issued URL never leaves the node — tool
    /// result, events, memory, the next node.
    fn scrub_issued_urls(v: Value, forms: &[(String, String)]) -> Value {
        match v {
            _ if forms.is_empty() => v,
            Value::String(s) => Value::String(Self::scrub_text(s, forms)),
            Value::Array(a) => a
                .into_iter()
                .map(|v| Self::scrub_issued_urls(v, forms))
                .collect(),
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, v)| {
                        (
                            Self::scrub_text(k, forms),
                            Self::scrub_issued_urls(v, forms),
                        )
                    })
                    .collect(),
            ),
            other => other,
        }
    }

    /// An error once a URL was issued: its text, scrubbed like the output.
    fn scrub_error(
        e: Box<dyn StdError + Send + Sync>,
        forms: &[(String, String)],
    ) -> Box<dyn StdError + Send + Sync> {
        if forms.is_empty() {
            return e;
        }
        Self::scrub_text(e.to_string(), forms).into()
    }

    const URL_FORM_NEEDS_AUTHORED_ADDRESS: &'static str =
        "http_request: \"$attachment_url:<document_id>\" needs a node configuration that \
         fixes the request's address (base_url, endpoint and any Host header, in config or as \
         a tool's fixed values); here part of it comes from runtime data (a tool argument, an \
         edge). Use \"$attachment:<document_id>\" to send the file's content instead";

    const URL_FORM_ONLY_IN_JSON_BODY: &'static str =
        "http_request: \"$attachment_url:<document_id>\" is accepted only as a whole string \
         value in a JSON body, not in query params or a multipart part. In a multipart body, \
         \"$attachment:<document_id>\" sends the file as a part";

    /// Whether the request's address is the author's: `base_url` and
    /// `endpoint` each come from `config` (absent from `inputs`) or are a
    /// tool's `fixed` value left as written (`__colmena_authored_inputs`),
    /// and so does any `Host` header (some front-ends route by it; a fixed
    /// leaf in `__colmena_authored_leaves` counts). A value from runtime
    /// data — an edge, global state, a model's argument, a `$DYNAMIC` part —
    /// is not; `allowed_hosts` does not change that.
    fn address_is_authored(inputs: &NodeInputs) -> bool {
        use crate::dag_engine::infrastructure::env_provenance::{
            listed_pointers, AUTHORED_LEAVES_KEY,
        };
        let fixed = |k: &str| !inputs.contains_key(k) || is_authored_input(inputs, k);
        let leaves = listed_pointers(inputs, AUTHORED_LEAVES_KEY);
        let fixed_host = |k: &String| {
            let leaf = format!("/headers/{}", escape_pointer_segment(k));
            !k.eq_ignore_ascii_case("host") || leaves.contains(&leaf)
        };
        let headers = inputs.get("headers").and_then(Value::as_object);
        fixed("base_url")
            && fixed("endpoint")
            && (fixed("headers") || headers.is_none_or(|h| h.keys().all(fixed_host)))
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
    fn output(status: u16, body: Value, file: Option<Value>) -> Value {
        let mut out = json!({ "status": status, "body": body });
        if let Some(file) = file {
            out["files"] = json!([file]);
        }
        out
    }

    /// Reads the response: a JSON body as `body`, or a file stored as a
    /// session attachment and described for the `files` output. Anything else
    /// (text, HTML, empty, a failed read) is `body: null`, as before.
    async fn read_response(
        &self,
        response: reqwest::Response,
        url: &str,
        inputs: &NodeInputs,
        config: &Value,
    ) -> (Value, Option<Value>) {
        let header = |name: reqwest::header::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header(reqwest::header::CONTENT_TYPE);
        let disposition = header(reqwest::header::CONTENT_DISPOSITION);
        let Ok(bytes) = response.bytes().await else {
            return (Value::Null, None);
        };
        if let Ok(json) = serde_json::from_slice::<Value>(&bytes) {
            return (json, None);
        }
        let Some(mime) = response_file::file_mime(content_type.as_deref(), &bytes) else {
            println!("[HttpNode] Response body is not JSON or is empty");
            return (Value::Null, None);
        };
        let max = Self::limit_u64(
            config,
            "max_file_size_bytes",
            Self::DEFAULT_MAX_FILE_SIZE_BYTES,
        );
        let Some(storage) = self.storage.as_ref() else {
            println!("[HttpNode] File response ({mime}) not kept: no storage adapter");
            return (Value::Null, None);
        };
        if bytes.len() as u64 > max {
            println!(
                "[HttpNode] File response ({mime}, {} bytes) not kept: max_file_size_bytes is {max}",
                bytes.len()
            );
            return (Value::Null, None);
        }
        let session_id = Self::input_str(inputs, "__colmena_session_id");
        let agent_session_id = Self::input_str(inputs, "__colmena_agent_session_id");
        let stored = match storage
            .store(StoreRequest {
                bytes: bytes.to_vec(),
                mime_type: mime.clone(),
                filename: response_file::filename(disposition.as_deref(), url, &mime),
                session_id,
                agent_session_id: agent_session_id.clone(),
            })
            .await
        {
            Ok(stored) => stored,
            Err(e) => {
                tracing::warn!(target: "colmena::http_request", error = %e,
                    "file response not kept: storage failed");
                return (Value::Null, None);
            }
        };
        let document_id = build_document_id(
            &stored.filename,
            &stored.mime_type,
            &stored.storage_key,
            "file",
        );
        if let (Some(reg), Some(agent_sid)) = (self.attachment_registry.as_ref(), agent_session_id)
        {
            let upsert = UpsertAttachmentInput {
                agent_session_id: agent_sid,
                document_id: document_id.clone(),
                provider: ProviderKind::Generated,
                provider_file_id: stored.storage_key.clone(),
                mime_type: stored.mime_type.clone(),
                filename: stored.filename.clone(),
                size_bytes: Some(stored.size_bytes),
                label: None,
                description: Some(format!(
                    "File returned by an HTTP request: {}",
                    stored.filename
                )),
                source: AttachmentSource::Path(stored.storage_key.clone()),
                storage_key: Some(stored.storage_key.clone()),
                origin: Some(origin::generated_by("http_request")),
            };
            if let Err(e) = reg.upsert(upsert).await {
                tracing::warn!(target: "colmena::http_request", error = %e,
                    document_id = %document_id,
                    "file response not registered — load_attachment will not see it");
            }
        }
        let file = json!({
            "document_id": document_id,
            "mime_type": stored.mime_type,
            "filename": stored.filename,
            "size_bytes": stored.size_bytes,
        });
        (Value::Null, Some(file))
    }

    fn input_str(inputs: &NodeInputs, key: &str) -> Option<String> {
        inputs.get(key).and_then(|v| v.as_str()).map(String::from)
    }

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
        config
            .get(key)
            .or_else(|| inputs.get(key).filter(|_| is_authored_input(inputs, key)))
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
    pub(crate) fn same_origin(a: &Url, b: &Url) -> bool {
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

    /// A destination that comes from data (not the origin of the author's
    /// `base_url`) dials only addresses the process allows (public ones
    /// unless `COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS` is set), redirects
    /// included; a host in `allowed_hosts` is dialled at any address. `None`
    /// for the author's destination, which is not checked, and when that
    /// variable is set (the plain client, system proxy included).
    fn destination_guard(
        &self,
        url: &Url,
        author_base_url: Option<&str>,
        allowed_hosts: Option<&Value>,
    ) -> Result<Option<DialGuard>, Box<dyn StdError + Send + Sync>> {
        let author = author_base_url.and_then(|b| Url::parse(b).ok());
        if author.is_some_and(|a| Self::same_origin(&a, url)) {
            return Ok(None);
        }
        let listed = Self::credential_destination_allowed(url, None, allowed_hosts).is_ok();
        // A bare `"host"` entry lists every port; `"host:port"`, only that one.
        let host = url.host_str().unwrap_or_default();
        let bare = |h: &Value| h.as_str().is_some_and(|h| h.eq_ignore_ascii_case(host));
        let bare = allowed_hosts
            .and_then(Value::as_array)
            .is_some_and(|l| l.iter().any(bare));
        let port = url.port_or_known_default().filter(|_| !bare);
        let Some(guard) = self.url_parts.guard(Some(host).filter(|_| listed), port) else {
            return Ok(None);
        };
        if guard.refuses(url) {
            return Err(Self::refused_destination());
        }
        Ok(Some(guard))
    }

    fn refused_destination() -> Box<dyn StdError + Send + Sync> {
        "http_request: a destination that comes from data connects only to a public address \
         or to a host in `allowed_hosts`, redirects included; this one is neither"
            .into()
    }

    /// A send error; the address rule's refusal says so, without the URL.
    fn send_error(e: reqwest::Error) -> Box<dyn StdError + Send + Sync> {
        if is_dial_refused(&e) {
            Self::refused_destination()
        } else {
            Box::new(e)
        }
    }

    /// A client whose redirects never take the author's credentials, or an
    /// attachment URL, to another origin (with `credentials`, only a
    /// same-origin redirect is followed; a cross-origin one is returned as
    /// is), and that dials only where `guard` allows, on every hop.
    fn client_for(
        credentials: bool,
        guard: Option<DialGuard>,
        builder: reqwest::ClientBuilder,
    ) -> reqwest::Result<reqwest::Client> {
        if !credentials && guard.is_none() {
            return builder.build();
        }
        let builder = match &guard {
            Some(g) => g.install(builder),
            None => builder,
        };
        builder
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                let first = attempt.previous().first();
                if attempt.previous().len() > 10 {
                    attempt.error("too many redirects")
                } else if guard.as_ref().is_some_and(|g| g.refuses(attempt.url())) {
                    attempt.error(DialRefused)
                } else if !credentials || first.is_some_and(|f| Self::same_origin(f, attempt.url()))
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
        guard: Option<DialGuard>,
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

        if Self::has_attachment_url(&body_resolved) {
            return Err(Self::URL_FORM_ONLY_IN_JSON_BODY.into());
        }

        // A body that arrived as data may name a URL for the node to fetch only
        // in a field the author enabled (`multipart_url_fields`).
        let body_from_data = inputs.get("body").is_some() && !is_authored_input(inputs, "body");
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
            guard,
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

        let response = req.multipart(form).send().await.map_err(Self::send_error)?;
        let status = response.status().as_u16();
        println!("[HttpNode] ← {status} ({full_url})");

        let (response_body, file) = self.read_response(response, full_url, inputs, config).await;
        Ok(Self::output(status, response_body, file))
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
            Some(_) if !is_authored_input(inputs, "base_url") => config
                .get("base_url")
                .and_then(|v| v.as_str())
                .and_then(|s| Self::resolve_env_vars(s).ok()),
            _ => Some(base_url.clone()),
        };
        Self::check_credential_destination(&url, author_base_url.as_deref(), inputs, config)
            .map_err(Self::io_err)?;
        let credentials = Self::carries_author_credentials(inputs, config);
        let allowed_hosts = Self::author_value(inputs, config, "allowed_hosts");
        let guard = self.destination_guard(&url, author_base_url.as_deref(), allowed_hosts)?;

        let body_from_inputs = inputs.get("body");
        let body_val = body_from_inputs.or_else(|| config.get("body"));
        // A JSON body, `${VAR}` expanded: from `inputs`, only where trusted.
        let json_body = body_val.filter(|b| !b.is_string()).map(|b| {
            if body_from_inputs.is_some() {
                Self::resolve_env_vars_in_value_gated(b, "/body", &policy)
            } else {
                Self::resolve_env_vars_in_value(b)
            }
        });
        // One that asks for an attachment URL, like the author's credentials,
        // follows no redirect to another origin.
        let carries_url = json_body.as_ref().is_some_and(Self::has_attachment_url);

        // 3. Prepare Client and Request
        // Build client forcing HTTP/1.1 to avoid HTTP/2 issues with some APIs
        let client = Self::client_for(
            credentials || carries_url,
            guard.clone(),
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
        let oauth_provider = self.resolve_oauth_provider(config, inputs, &policy)?;

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
        // `$attachment_url:` is replaced only in a JSON body; in `query_params`
        // it would travel as written. A flat input is not checked: in a child
        // graph, global state hands every node the parent's arguments.
        let queries = [config.get("query_params"), inputs.get("query_params")];
        if queries.into_iter().flatten().any(Self::has_attachment_url) {
            return Err(Self::URL_FORM_ONLY_IN_JSON_BODY.into());
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
            // A `bearer_refresh` here keeps its static `bearer_token`.
            if config.get("auth").is_some() {
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
                    guard,
                )
                .await;
        }

        // Every `$attachment_url:` URL this request carries, with the
        // placeholder it replaced (see `scrub_issued_urls`).
        let mut issued: Vec<(String, String)> = Vec::new();

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
            } else if let Some(resolved_body) = json_body {
                // `$attachment_url:` only toward the author's address and with
                // a valid lifetime, both checked before any attachment is read.
                if carries_url && !Self::address_is_authored(inputs) {
                    return Err(Self::URL_FORM_NEEDS_AUTHORED_ADDRESS.into());
                }
                let ttl = carries_url.then(|| Self::attachment_url_ttl(inputs, config));
                let ttl = ttl.transpose().map_err(Self::io_err)?;
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
                // `$attachment_url:<document_id>` → a URL the host issues.
                let resolved_body = match ttl {
                    Some(ttl) => {
                        self.resolve_attachment_urls(resolved_body, sid, ttl, &mut issued)
                            .await?
                    }
                    None => resolved_body,
                };
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.json(&resolved_body);
            }
        }

        // 7. Execute Request
        // Note: Headers are not easily printable from request_builder, but we can print what we added
        // println!("DEBUG: Headers: {:?}", request_builder); // RequestBuilder doesn't implement Debug nicely for headers

        // An issued attachment URL never leaves the node, in an error's text
        // either (a redirect hop's URL, say).
        let forms = Self::scrub_forms(&issued);
        let response = if let Some((provider, fallback)) = oauth_provider {
            crate::dag_engine::infrastructure::nodes::http_oauth::send_with_oauth_retry(
                request_builder,
                provider.as_ref(),
                fallback.as_deref(),
            )
            .await
        } else {
            request_builder.send().await.map_err(Self::send_error)
        }
        .map_err(|e| Self::scrub_error(e, &forms))?;
        let status = response.status().as_u16();
        println!("[HttpNode] ← {} ({})", status, full_url_str);

        // JSON body, or a file kept as a session attachment (see module docs).
        // Never log the response body — it may contain tokens, keys, or PII.
        let (response_body, file) = self
            .read_response(response, &full_url_str, inputs, config)
            .await;

        // 8. Return Output — an issued attachment URL never leaves the node.
        let output = Self::output(status, response_body, file);
        Ok(Self::scrub_issued_urls(output, &forms))
    }

    /// Human-readable description of this node type, used in LLM tool definitions.
    fn description(&self) -> Option<&str> {
        Some("Make HTTP requests to external APIs. Supports GET, POST, PUT, DELETE methods with custom headers and query parameters.")
    }

    /// Where the request goes, how, with which credentials, and the lifetime
    /// of an `$attachment_url:` URL: author-set fields are config-only unless
    /// an edge names them.
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
            "attachment_url_ttl_seconds",
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
                "body": "any",
                "files": "array of { document_id, mime_type, filename, size_bytes } (only when the response is a file)"
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
                .with_field("bearer_refresh", FieldSpec::of_type("object"))
                .with_field("secure", FieldSpec::of_type("boolean"))
                .with_field("max_file_size_bytes", FieldSpec::of_type("integer"))
                .with_field("max_parts", FieldSpec::of_type("integer"))
                .with_field("url_download_timeout_secs", FieldSpec::of_type("integer"))
                .with_field("allow_http_urls", FieldSpec::of_type("boolean"))
                .with_field("allowed_hosts", FieldSpec::of_type("array"))
                .with_field("multipart_url_fields", FieldSpec::of_type("array"))
                .with_field("attachment_url_ttl_seconds", FieldSpec::of_type("integer"))
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

/// `"$attachment_url:<document_id>"` in a JSON body, with the session
/// registry wired: `s1` owns `doc-1 → k1` and `doc-3 → k3`, `s2` owns
/// `doc-2 → k2`.
#[cfg(test)]
mod attachment_url_tests {
    use super::*;
    use crate::dag_engine::infrastructure::env_provenance::{
        AUTHORED_INPUTS_KEY, AUTHORED_LEAVES_KEY,
    };
    use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
    use crate::llm::domain::{AttachmentRegistry, ProviderKind};
    use crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
    use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
    use crate::storage::domain::{
        MockOutputStorageRepository, OutputStorageRepository, StorageError, StoreRequest,
        StoredBytes, StoredOutput, StoredStream,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wiremock::matchers::{body_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PLACEHOLDER: &str = "$attachment_url:doc-1";
    /// The signature of every URL [`UrlStorage`] issues.
    const SIG: &str = "3f9a1c07e5b24d8e9a6c1f0b7d3e5a2c4b6d8f0e1a3c5e7b9d1f3a5c7e9b1d3f";

    /// A signed read URL's shape: an object path, a credential with `@` and
    /// `/`, an expiry, a 15-character value and a hex signature.
    fn signed(key: &str) -> String {
        format!(
            "https://storage.example.test/bucket/sessions/{key}.png\
             ?X-Goog-Algorithm=GOOG4-RSA-SHA256\
             &X-Goog-Credential=svc%40proj.example.test%2F20260927%2Fauto%2Fstorage%2Fgoog4_request\
             &X-Goog-Date=20260927T120000Z&X-Goog-Expires=900&X-Goog-SignedHeaders=host\
             &userProject=example-project&alt=media&X-Goog-Signature={SIG}"
        )
    }

    /// Issues [`signed`] for the document's key; records each TTL asked.
    #[derive(Default)]
    struct UrlStorage(Mutex<Vec<u64>>);

    #[async_trait::async_trait]
    impl OutputStorageRepository for UrlStorage {
        async fn store(&self, _: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!()
        }
        async fn read(&self, _: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!()
        }
        async fn read_stream(&self, _: &str) -> Result<StoredStream, StorageError> {
            unimplemented!()
        }
        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            Ok(())
        }
        async fn read_url(&self, key: &str, ttl: u64) -> Result<Option<String>, StorageError> {
            self.0.lock().unwrap().push(ttl);
            Ok(Some(signed(key)))
        }
    }

    async fn node(storage: Arc<dyn OutputStorageRepository>) -> HttpNode {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        for (s, doc, key) in [
            ("s1", "doc-1", "k1"),
            ("s2", "doc-2", "k2"),
            ("s1", "doc-3", "k3"),
        ] {
            let row = UpsertAttachmentInput {
                agent_session_id: s.into(),
                document_id: doc.into(),
                provider: ProviderKind::Generated,
                provider_file_id: key.into(),
                mime_type: "image/png".into(),
                filename: "a.png".into(),
                size_bytes: Some(2),
                label: None,
                description: None,
                source: AttachmentSource::Path(key.into()),
                storage_key: Some(key.into()),
                origin: None,
            };
            reg.upsert(row).await.unwrap();
        }
        let reg: Arc<dyn AttachmentRegistry> = Arc::new(reg);
        let resolver = Arc::new(AttachmentStreamResolverImpl::new(reg, storage.clone()));
        HttpNode::new()
            .with_storage(storage)
            .with_attachment_resolver(resolver)
    }

    /// Runs `http` as session `s1`.
    async fn post(http: &HttpNode, mut inputs: NodeInputs, config: Value) -> Result<Value, String> {
        inputs.insert("__colmena_agent_session_id".into(), json!("s1"));
        http.execute(&inputs, &config, &mut json!({}), None)
            .await
            .map_err(|e| e.to_string())
    }

    fn cfg(server: &MockServer) -> Value {
        json!({ "base_url": server.uri(), "endpoint": "/jobs", "method": "POST" })
    }

    fn body(v: Value) -> NodeInputs {
        HashMap::from([("body".to_string(), v)])
    }

    /// A server that must never be called: the node has to fail first.
    async fn untouched() -> MockServer {
        let server = MockServer::start().await;
        let never = Mock::given(method("POST")).respond_with(ResponseTemplate::new(200));
        never.expect(0).mount(&server).await;
        server
    }

    /// Python's `urllib.parse.quote(s, safe)`: `%XX` (uppercase) for every
    /// byte but ASCII letters, digits, `-._~` and `safe`.
    fn quote(s: &str, safe: &str) -> String {
        let kept = |b: u8| b.is_ascii_alphanumeric() || b"-._~".contains(&b);
        s.bytes()
            .map(|b| match b {
                b if kept(b) || safe.as_bytes().contains(&b) => (b as char).to_string(),
                b => format!("%{b:02X}"),
            })
            .collect()
    }

    /// Each form an echo writes of an issued URL, or of its signature alone,
    /// comes back as the placeholder: in a nested value, in an object key
    /// and in an error's text. A query value of 16 characters is a token,
    /// one of 15 is kept.
    #[test]
    fn every_form_an_echo_writes_is_scrubbed() {
        let url = signed("k1");
        let forms = HttpNode::scrub_forms(&[(url.clone(), PLACEHOLDER.into())]);
        let scrub = |s: &str| {
            let v = json!({ "a": [{ "b": s }], s: 0 });
            let e = HttpNode::scrub_error(s.into(), &forms).to_string();
            (HttpNode::scrub_issued_urls(v, &forms), e)
        };
        let shown = format!("see {PLACEHOLDER}.");
        for echo in [
            url.clone(),
            url.replace('/', "\\/"), // JSON text in a string, as PHP writes it
            url.replace('&', "\\u0026"), // the same, as Go writes it
            url.replace('&', "&amp;"), // HTML
            quote(&url, "/"),        // percent-encoded, `/` kept
            quote(&url, ""),         // percent-encoded strictly
            urlencoding::decode(&url).unwrap().into_owned(), // percent-decoded
            SIG.to_string(),         // a partial echo: the signature only
            "GOOG4-RSA-SHA256".into(), // a 16-character query value
        ] {
            let expected = json!({ "a": [{ "b": shown }], shown.as_str(): 0 });
            assert_eq!(scrub(&format!("see {echo}.")), (expected, shown.clone()));
        }
        let kept = "cache: X-Goog-Expires=900&userProject=example-project";
        let expected = json!({ "a": [{ "b": kept }], kept: 0 });
        assert_eq!(scrub(kept), (expected, kept.to_string()));
    }

    /// Each part of the address is the author's when absent from `inputs`
    /// (it is in `config`) or listed as a tool's fixed value; a `Host`
    /// header is part of it (whole `headers` or its own leaf listed);
    /// `allowed_hosts` changes nothing.
    #[test]
    fn the_address_is_the_authors_only_when_each_part_is() {
        let (base, key) = ("https://api.example.test", AUTHORED_INPUTS_KEY);
        let fixed = |listed: Value| json!({ "base_url": base, "endpoint": "/jobs", key: listed });
        let listed_host = json!({ "base_url": base, "allowed_hosts": ["api.example.test"],
            key: ["allowed_hosts"] });
        let host = |more: Value| json!({ "headers": { "HOST": "a", "Accept": "b" }, key: more });
        let fixed_leaf =
            json!({ "headers": { "Host": "a" }, AUTHORED_LEAVES_KEY: ["/headers/Host"] });
        for (inputs, authored) in [
            (json!({}), true),
            (json!({ "base_url": base }), false),
            (json!({ "endpoint": "/jobs" }), false),
            (fixed(json!(["base_url", "endpoint"])), true),
            (fixed(json!(["base_url"])), false),
            (listed_host, false),
            (host(json!([])), false),
            (json!({ "headers": { "Accept": "*/*" } }), true),
            (host(json!(["headers"])), true),
            (fixed_leaf, true),
        ] {
            let inputs: NodeInputs = serde_json::from_value(inputs).unwrap();
            let got = HttpNode::address_is_authored(&inputs);
            assert_eq!(got, authored, "{inputs:?}");
        }
    }

    /// Only a whole string value is the form, at any depth: not part of a
    /// longer string, nor an object key.
    #[test]
    fn only_a_whole_string_value_is_the_url_form() {
        for (body, carries) in [
            (json!({ "a": [[PLACEHOLDER]] }), true),
            (json!([{ "img": { "src": PLACEHOLDER } }]), true),
            (json!({ "note": format!("see {PLACEHOLDER}") }), false),
            (json!({ PLACEHOLDER: "k" }), false),
            (json!({ "file": "$attachment:doc-1" }), false),
        ] {
            assert_eq!(HttpNode::has_attachment_url(&body), carries, "{body}");
        }
    }

    /// POSTs `body` (with `inputs`) to a server that must never be called;
    /// storage has no expectations, so reading any attachment panics.
    async fn refused(body: Value, mut inputs: NodeInputs, mut config: Value) -> String {
        let server = untouched().await;
        config["base_url"] = json!(server.uri());
        config["method"] = json!("POST");
        inputs.insert("body".into(), body);
        let node = HttpNode::new().with_storage(Arc::new(MockOutputStorageRepository::new()));
        let out = node.execute(&inputs, &config, &mut json!({}), None).await;
        out.unwrap_err().to_string()
    }

    /// Toward a data address the address rule answers first, before any
    /// `$attachment:` is read.
    #[tokio::test]
    async fn the_address_is_checked_before_any_attachment_is_read() {
        let both = json!({ "file": "$attachment:k1", "image_url": PLACEHOLDER });
        let data = HashMap::from([("endpoint".to_string(), json!("/jobs"))]);
        let err = refused(both, data, json!({})).await;
        assert!(err.contains("fixes the request's address"), "{err}");
    }

    /// The host's URL goes out in place of the placeholder, asked for 900 s.
    /// The API's echoes of it (as is, percent-encoded, in a JSON text, the
    /// signature alone, as a key) come back as the placeholder.
    #[tokio::test]
    async fn the_url_placeholder_becomes_the_url_the_host_issues() {
        let server = MockServer::start().await;
        let url = signed("k1");
        let echoes = [
            url.clone(),
            quote(&url, ""),
            url.replace('/', "\\/"),
            SIG.into(),
        ];
        let mut reply = json!({ "echo": echoes.map(|e| format!("see {e}.")) });
        reply[url.as_str()] = json!("as a key");
        Mock::given(body_json(json!({ "image_url": url, "keep": "x" })))
            .respond_with(ResponseTemplate::new(400).set_body_json(reply))
            .expect(1)
            .mount(&server)
            .await;
        let storage = Arc::new(UrlStorage::default());
        let sent = body(json!({ "image_url": PLACEHOLDER, "keep": "x" }));
        let out = post(&node(storage.clone()).await, sent, cfg(&server)).await;
        assert_eq!(*storage.0.lock().unwrap(), vec![900], "the default TTL");
        let shown = format!("see {PLACEHOLDER}.");
        let echo = [&shown, &shown, &shown, &shown];
        let expected = json!({ "echo": echo, PLACEHOLDER: "as a key" });
        assert_eq!(out.unwrap(), json!({ "status": 400, "body": expected }));
    }

    /// No URL is asked for an id that is not the session's (a raw key of this
    /// session, one of another session, another session's document_id) nor
    /// without a session; a host without URLs and an engine without a
    /// registry fail with a clear error. Nothing is sent.
    #[tokio::test]
    async fn the_url_form_fails_clearly_when_there_is_no_url_to_give() {
        let server = untouched().await;
        let storage = Arc::new(UrlStorage::default());
        let http = node(storage.clone()).await;
        let cache = crate::storage::infrastructure::LocalCacheStorageAdapter::new();
        let cache = node(Arc::new(cache)).await;
        let bare = HttpNode::new().with_storage(storage.clone());
        let url_of = |id: &str| body(json!({ "image_url": format!("$attachment_url:{id}") }));
        let no_url = "this host does not provide attachment URLs; use \"$attachment:doc-1\"";
        for (node, id, why) in [
            (&http, "k1", "attachment not found"),
            (&http, "k2", "attachment not found"),
            (&http, "doc-2", "attachment not found"),
            (&cache, "doc-1", no_url),
            (&bare, "doc-1", "needs the session's attachment registry"),
        ] {
            let err = post(node, url_of(id), cfg(&server)).await.unwrap_err();
            assert!(err.contains(why), "{id}: {err}");
        }
        let (doc, config) = (url_of("doc-1"), cfg(&server));
        let no_session = http.execute(&doc, &config, &mut json!({}), None).await;
        let err = no_session.unwrap_err().to_string();
        assert!(err.contains("needs an agent_session_id"), "{err}");
        assert!(storage.0.lock().unwrap().is_empty(), "a URL was asked for");
    }

    /// Placeholders nested in arrays and objects are all replaced; storage
    /// is asked once per occurrence, the same id twice included.
    #[tokio::test]
    async fn every_placeholder_is_replaced_once_per_occurrence() {
        let server = MockServer::start().await;
        let (u1, u3) = (signed("k1"), signed("k3"));
        let expected = json!({ "parts": [[u1], { "img": u3 }], "again": u1 });
        Mock::given(body_json(expected))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let storage = Arc::new(UrlStorage::default());
        let d3 = "$attachment_url:doc-3";
        let sent = json!({ "parts": [[PLACEHOLDER], { "img": d3 }], "again": PLACEHOLDER });
        let out = post(&node(storage.clone()).await, body(sent), cfg(&server)).await;
        assert_eq!(out.unwrap()["status"], 200);
        assert_eq!(*storage.0.lock().unwrap(), vec![900, 900, 900]);
    }

    /// A redirect to another origin is not followed: the URL reaches only
    /// the author's address, and the 3xx comes back as the reply.
    #[tokio::test]
    async fn a_redirect_never_takes_the_url_to_another_origin() {
        let (author, other) = (MockServer::start().await, MockServer::start().await);
        let to_other = ResponseTemplate::new(307)
            .insert_header("location", format!("{}/x", other.uri()))
            .set_body_json(json!({ "src": signed("k1") }));
        Mock::given(method("POST"))
            .respond_with(to_other)
            .expect(1)
            .mount(&author)
            .await;
        let http = node(Arc::new(UrlStorage::default())).await;
        let sent = body(json!({ "image_url": PLACEHOLDER }));
        let out = post(&http, sent, cfg(&author)).await.unwrap();
        assert!(other.received_requests().await.unwrap().is_empty());
        assert_eq!(
            out,
            json!({ "status": 307, "body": { "src": PLACEHOLDER } })
        );
    }

    /// An error after the URL was issued is scrubbed too: here, redirects
    /// within the author's origin that carry it, until there are too many.
    #[tokio::test]
    async fn an_error_text_never_carries_the_url() {
        let server = MockServer::start().await;
        let hop = format!("{}/fetch?src={}", server.uri(), quote(&signed("k1"), ""));
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(307).insert_header("location", hop))
            .mount(&server)
            .await;
        let http = node(Arc::new(UrlStorage::default())).await;
        let sent = body(json!({ "image_url": PLACEHOLDER }));
        let err = post(&http, sent, cfg(&server)).await.unwrap_err();
        assert!(
            err.contains("src=$attachment_url:doc-1): too many"),
            "{err}"
        );
        assert!(!err.contains(SIG), "{err}");
    }

    const ONLY_IN_JSON: &str = "only as a whole string value in a JSON body";

    /// In multipart, `$attachment:` already sends the file as a part: the
    /// URL form fails before any part is read and nothing is sent.
    #[tokio::test]
    async fn a_multipart_body_refuses_the_url_form() {
        let server = untouched().await;
        let storage = Arc::new(UrlStorage::default());
        let http = node(storage.clone()).await;
        let mut config = cfg(&server);
        config["headers"] = json!({ "Content-Type": "multipart/form-data" });
        let file = body(json!({ "a": "$attachment:doc-1", "file": PLACEHOLDER }));
        let err = post(&http, file, config).await.unwrap_err();
        assert!(
            err.contains(ONLY_IN_JSON) && err.contains("\"$attachment:"),
            "{err}"
        );
        assert!(storage.0.lock().unwrap().is_empty());
    }

    /// In `query_params` (the node's or the caller's) the form would travel
    /// as written: it fails before any storage read, even with a JSON body
    /// that carries it too, and nothing is sent.
    #[tokio::test]
    async fn query_params_refuse_the_url_form() {
        let server = untouched().await;
        let storage = Arc::new(UrlStorage::default());
        let http = node(storage.clone()).await;
        let q = json!({ "src": PLACEHOLDER });
        let mut in_config = cfg(&server);
        in_config["query_params"] = q.clone();
        for (extra, config) in [(None, in_config), (Some(("query_params", q)), cfg(&server))] {
            let mut inputs = body(json!({ "image_url": PLACEHOLDER }));
            inputs.extend(extra.map(|(k, v)| (k.to_string(), v)));
            let err = post(&http, inputs, config).await.unwrap_err();
            assert!(err.contains(ONLY_IN_JSON), "{err}");
        }
        assert!(storage.0.lock().unwrap().is_empty(), "a URL was asked for");
    }

    /// A flat input is left as written (in a child graph, global state hands
    /// the parent's arguments to every node): a request without the form runs.
    #[tokio::test]
    async fn a_flat_input_with_the_form_does_not_fail_the_request() {
        let server = MockServer::start().await;
        let ok = Mock::given(method("GET")).respond_with(ResponseTemplate::new(200));
        ok.expect(1).mount(&server).await;
        let http = node(Arc::new(UrlStorage::default())).await;
        let flat = HashMap::from([("image_url".to_string(), json!(PLACEHOLDER))]);
        let config = json!({ "base_url": server.uri(), "method": "GET" });
        assert_eq!(post(&http, flat, config).await.unwrap()["status"], 200);
    }

    /// Some front-ends route by `Host`: one from runtime data refuses the
    /// form even toward the author's address; the author's own goes out.
    #[tokio::test]
    async fn a_host_header_from_data_refuses_the_url_form() {
        let server = MockServer::start().await;
        let ok = Mock::given(method("POST")).respond_with(ResponseTemplate::new(200));
        ok.expect(1).mount(&server).await;
        let storage = Arc::new(UrlStorage::default());
        let http = node(storage.clone()).await;
        let host = json!({ "host": "other.example.test" });
        let mut data = body(json!({ "image_url": PLACEHOLDER }));
        data.insert("headers".into(), host.clone());
        let err = post(&http, data, cfg(&server)).await.unwrap_err();
        assert!(err.contains("fixes the request's address"), "{err}");
        assert!(storage.0.lock().unwrap().is_empty(), "a URL was asked for");
        let mut config = cfg(&server);
        config["headers"] = host;
        let authored = body(json!({ "image_url": PLACEHOLDER }));
        assert_eq!(post(&http, authored, config).await.unwrap()["status"], 200);
    }

    /// The author's value (config, or a tool's `fixed` one) reaches the host
    /// as is, above any host's cap too; a value from runtime data is ignored;
    /// none travels as a query param.
    #[tokio::test]
    async fn the_ttl_is_the_authors_value_and_never_a_data_value() {
        let server = MockServer::start().await;
        let ok = Mock::given(method("POST")).respond_with(ResponseTemplate::new(200));
        ok.expect(3).mount(&server).await;
        let storage = Arc::new(UrlStorage::default());
        let http = node(storage.clone()).await;
        let placeholder = || body(json!({ "image_url": PLACEHOLDER }));
        let ttl = "attachment_url_ttl_seconds";
        let mut config = cfg(&server);
        config[ttl] = json!(3600);
        post(&http, placeholder(), config).await.unwrap();
        let mut fixed = placeholder();
        fixed.insert(ttl.into(), json!(604_800));
        fixed.insert(AUTHORED_INPUTS_KEY.into(), json!([ttl]));
        post(&http, fixed, cfg(&server)).await.unwrap();
        let mut data = placeholder();
        data.insert(ttl.into(), json!(60));
        post(&http, data, cfg(&server)).await.unwrap();
        assert_eq!(*storage.0.lock().unwrap(), vec![3600, 604_800, 900]);
        let sent = server.received_requests().await.unwrap();
        assert!(sent.iter().all(|r| r.url.query().is_none()));
    }

    /// Anything but a positive whole number fails before any attachment is
    /// read, and nothing is sent.
    #[tokio::test]
    async fn a_ttl_that_is_not_a_positive_whole_number_is_refused() {
        let server = untouched().await;
        let storage = Arc::new(UrlStorage::default());
        let http = node(storage.clone()).await;
        for bad in [json!(0), json!(-5), json!("3600"), json!(1.5)] {
            let mut config = cfg(&server);
            config["attachment_url_ttl_seconds"] = bad.clone();
            let both = body(json!({ "image_url": PLACEHOLDER, "file": "$attachment:doc-1" }));
            let err = post(&http, both, config).await.unwrap_err();
            assert!(
                err.contains("attachment_url_ttl_seconds must be"),
                "{bad}: {err}"
            );
        }
        assert!(storage.0.lock().unwrap().is_empty(), "a URL was asked for");
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

/// CX7 `bearer_refresh`: on a 401 the node asks the host for a fresh token
/// through `HostTokenPort` and retries once; without a port, today's 401.
#[cfg(test)]
mod bearer_refresh_tests {
    use super::*;
    use crate::dag_engine::application::ports as p;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wiremock::{matchers::header, Mock, MockServer, ResponseTemplate};

    /// Returns `tok-cx7-new`, or refuses; records every request.
    struct FakePort(Mutex<Vec<p::HostTokenRequest>>, bool);
    #[async_trait::async_trait]
    impl HostTokenPort for FakePort {
        async fn fresh_token(
            &self,
            req: p::HostTokenRequest,
        ) -> Result<p::HostToken, p::HostTokenError> {
            self.0.lock().unwrap().push(req);
            let (access_token, expires_at) = ("tok-cx7-new".into(), later());
            let t = self.1.then_some(p::HostToken {
                access_token,
                expires_at,
            });
            t.ok_or(p::HostTokenError::Unauthorized("refused".into()))
        }
    }

    struct Reg(Arc<HttpNode>);
    impl p::NodeRegistryPort for Reg {
        fn get_node(&self, t: &str) -> Option<Arc<dyn ExecutableNode>> {
            (t == "http_request").then(|| self.0.clone() as _)
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    /// 401 to the seed token, 200 to the host's fresh one.
    async fn api() -> MockServer {
        let s = MockServer::start().await;
        for (tok, code) in [("tok-cx7-seed", 401), ("tok-cx7-new", 200)] {
            Mock::given(header("Authorization", format!("Bearer {tok}").as_str()))
                .respond_with(ResponseTemplate::new(code).set_body_json(json!({"code": code})))
                .mount(&s)
                .await;
        }
        s
    }

    fn node(ok: bool) -> (Arc<HttpNode>, Arc<FakePort>) {
        let (node, port) = (HttpNode::new(), Arc::new(FakePort(Mutex::default(), ok)));
        let _ = node.host_token_port.set(port.clone());
        (Arc::new(node), port)
    }

    fn config(s: &MockServer, expires_at: i64) -> Value {
        json!({ "base_url": s.uri(), "endpoint": "/x", "method": "GET",
            "bearer_token": "tok-cx7-seed",
            "bearer_refresh": { "handle": "cth1-cx7-h", "expires_at": expires_at } })
    }

    fn later() -> i64 {
        chrono::Utc::now().timestamp() + 3600
    }

    async fn auths(s: &MockServer) -> Vec<String> {
        let reqs = s.received_requests().await.unwrap();
        let auth = |r: &wiremock::Request| r.headers["authorization"].to_str().unwrap().into();
        reqs.iter().map(auth).collect()
    }

    async fn run(node: &HttpNode, config: &Value, inputs: &NodeInputs) -> Value {
        let out = node.execute(inputs, config, &mut Value::Null, None).await;
        out.expect("ok")
    }

    /// A graph run: 401, one call to the host with the rejected token's
    /// sha256 and the run's session, then 200.
    #[tokio::test]
    async fn a_401_asks_the_host_once_and_retries_with_the_fresh_token() {
        use crate::dag_engine::application::run_use_case::DagRunUseCase;
        use futures::StreamExt;
        use sha2::Digest;
        let (s, (node, port)) = (api().await, node(true));
        let get = json!({ "type": "http_request", "config": config(&s, later()) });
        let graph = json!({ "nodes": { "get": get }, "edges": [] });
        let uc = DagRunUseCase::new(Arc::new(Reg(node)), None);
        let graph = serde_json::from_value(graph).unwrap();
        let stream = uc.execute_stream(graph, None, None, false, None, Some("s1".into()), None);
        tokio::pin!(stream);
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            text.push_str(&format!("{:?}", event.expect("no run error")));
        }
        assert_eq!(
            auths(&s).await,
            ["Bearer tok-cx7-seed", "Bearer tok-cx7-new"]
        );
        // The output (not the NodeStart echo of the config) carries no handle.
        let output = text.split("NodeFinish").nth(1).expect("finished");
        assert!(
            output.contains("Number(200)") && !output.contains("cth1-cx7-"),
            "{text}"
        );
        let calls = port.0.lock().unwrap();
        let sha = format!("{:x}", sha2::Sha256::digest(b"tok-cx7-seed"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].stale_token_sha256, Some(sha));
        assert_eq!(calls[0].handle, "cth1-cx7-h");
        assert_eq!(calls[0].agent_session_id.as_deref(), Some("s1"));
    }

    /// Without a port `bearer_refresh` is ignored: the 401 comes back as today.
    #[tokio::test]
    async fn without_a_port_the_401_comes_back_as_today() {
        let s = api().await;
        let out = run(&HttpNode::new(), &config(&s, later()), &HashMap::new()).await;
        assert_eq!(out["status"], 401);
        assert_eq!(auths(&s).await, ["Bearer tok-cx7-seed"]);
    }

    /// A host that refuses leaves the request as it would be without one.
    #[tokio::test]
    async fn a_refusing_host_leaves_the_401_and_a_near_expiry_seed_goes_out() {
        let (node, port) = node(false);
        let out = run(&node, &config(&api().await, later()), &HashMap::new()).await;
        assert_eq!(
            (out["status"].as_u64(), port.0.lock().unwrap().len()),
            (Some(401), 1)
        );
        // Near expiry: the host is asked first; refused, the seed still goes out.
        let s = api().await;
        let out = run(&node, &config(&s, 0), &HashMap::new()).await;
        assert_eq!(
            (out["status"].as_u64(), port.0.lock().unwrap().len()),
            (Some(401), 2)
        );
        assert_eq!(auths(&s).await, ["Bearer tok-cx7-seed"]);
    }

    /// Near expiry with a working host: the first request already carries the
    /// fresh token, and the host was told no token was rejected.
    #[tokio::test]
    async fn a_near_expiry_seed_is_replaced_before_the_first_request() {
        let (s, (node, port)) = (api().await, node(true));
        assert_eq!(
            run(&node, &config(&s, 0), &HashMap::new()).await["status"],
            200
        );
        assert_eq!(auths(&s).await, ["Bearer tok-cx7-new"]);
        assert_eq!(port.0.lock().unwrap()[0].stale_token_sha256, None);
    }

    /// A multipart body is not refused: it keeps its static `bearer_token`.
    #[tokio::test]
    async fn a_multipart_body_keeps_the_static_bearer() {
        let (s, (node, _)) = (api().await, node(true));
        let mut c = config(&s, later());
        c["method"] = json!("POST");
        c["headers"] = json!({ "Content-Type": "multipart/form-data" });
        c["body"] = json!({ "note": "x" });
        assert_eq!(run(&node, &c, &HashMap::new()).await["status"], 401);
    }

    /// As a tool: `bearer_token` and `bearer_refresh` are `fixed` entries of
    /// `node_schema`. A `bearer_refresh` the model writes is never used.
    #[tokio::test]
    async fn a_tool_with_fixed_bearer_refresh_retries_and_a_model_one_is_ignored() {
        use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
        use crate::llm::domain::{FunctionCall, ToolCall, ToolExecutor};
        let (s, (node, port)) = (api().await, node(true));
        let block = json!({ "handle": "cth1-cx7-h", "expires_at": later() });
        let schema = json!({ "base_url": { "fixed": s.uri() }, "endpoint": { "fixed": "/x" },
            "bearer_token": { "type": "string", "fixed": "tok-cx7-seed" },
            "bearer_refresh": { "type": "object", "fixed": block },
            "q": { "type": "string", "description": "query" } });
        let cfg = json!({ "node_type": "http_request", "node_schema": schema });
        let configs = HashMap::from([("get".to_string(), serde_json::from_value(cfg).unwrap())]);
        let exec = DagToolExecutor::new(Arc::new(Reg(node.clone())), configs);
        let call = FunctionCall::new("get".into(), r#"{"q":"x"}"#.into());
        let out = exec
            .execute(&ToolCall::new("c1".into(), call))
            .await
            .unwrap();
        assert!(out.output.contains("200"), "{}", out.output);
        assert_eq!(
            auths(&s).await,
            ["Bearer tok-cx7-seed", "Bearer tok-cx7-new"]
        );
        assert_eq!(port.0.lock().unwrap().len(), 1);
        // The model's own `bearer_refresh` (not a fixed value) is ignored.
        let token = ("bearer_token".to_string(), json!("tok-cx7-seed"));
        let model: NodeInputs = [token, ("bearer_refresh".to_string(), block)].into();
        let cfg = json!({ "base_url": s.uri(), "endpoint": "/x" });
        assert_eq!(run(&node, &cfg, &model).await["status"], 401);
        assert_eq!(port.0.lock().unwrap().len(), 1);
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
            ("attachment_url_ttl_seconds", json!(3600)),
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

/// A destination that comes from data (a `base_url` the author did not set)
/// dials only public addresses, redirects included, unless `allowed_hosts`
/// lists its host; the author's own destination is not checked.
#[cfg(test)]
mod data_destination_tests {
    use super::*;
    use crate::dag_engine::infrastructure::env_provenance::AUTHORED_INPUTS_KEY;
    use std::collections::HashMap;
    use std::time::Duration;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn server(reply: ResponseTemplate) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(reply)
            .mount(&s)
            .await;
        s
    }

    fn public_only() -> HttpNode {
        HttpNode::new().with_url_parts(SignedUrlDownloader::public_only())
    }

    /// `base_url` from data; a dial that hangs (a non-routable address) times out.
    async fn run(node: &HttpNode, base_url: &str, config: Value) -> Result<Value, String> {
        let inputs = HashMap::from([("base_url".to_string(), json!(base_url))]);
        let mut state = json!({});
        let run = node.execute(&inputs, &config, &mut state, None);
        match tokio::time::timeout(Duration::from_secs(5), run).await {
            Ok(out) => out.map_err(|e| e.to_string()),
            Err(_) => Err("timed out dialling".to_string()),
        }
    }

    #[tokio::test]
    async fn a_non_public_destination_from_data_is_never_dialed() {
        let s = server(ResponseTemplate::new(200)).await;
        let by_name = s.uri().replace("127.0.0.1", "localhost");
        let multipart = json!({ "method": "POST", "body": { "a": "b" },
            "headers": { "Content-Type": "multipart/form-data" } });
        let author = json!({ "base_url": s.uri() });
        for (base, config) in [
            (s.uri(), json!({})),
            (by_name.clone(), json!({})),
            (by_name, multipart),
            ("http://169.254.169.254".to_string(), json!({})),
            ("http://10.255.0.1".to_string(), json!({})),
            ("http://[::1]:9".to_string(), json!({})),
            // The author's host on another port or scheme is data's choice too.
            ("http://127.0.0.1:9".to_string(), author.clone()),
            (s.uri().replace("http:", "https:"), author),
        ] {
            let err = run(&public_only(), &base, config).await.unwrap_err();
            assert!(err.contains("public address"), "{base}: {err}");
        }
        assert!(s.received_requests().await.unwrap().is_empty());
    }

    /// A destination from data the rule accepts (loopback, here) that
    /// redirects to a non-public address: the hop is never dialled.
    #[tokio::test]
    async fn a_redirect_from_data_to_a_non_public_address_is_not_followed() {
        let to = |loc: &str| ResponseTemplate::new(302).insert_header("location", loc);
        let s = server(to("http://10.255.0.1/x")).await;
        let loopback_as_public = SignedUrlDownloader::with_policy(|ip| ip.is_loopback(), 1024);
        let node = HttpNode::new().with_url_parts(loopback_as_public);
        let err = run(&node, &s.uri(), json!({})).await.unwrap_err();
        assert!(err.contains("public address"), "{err}");
        // A `"host:port"` entry exempts that port only.
        let s = server(to("http://127.0.0.1:9/x")).await;
        let listed = json!({ "allowed_hosts": [s.uri().trim_start_matches("http://")] });
        let err = run(&public_only(), &s.uri(), listed).await.unwrap_err();
        assert!(err.contains("public address"), "{err}");
    }

    /// The author's destination (in `config` or a tool's `fixed` value) and a
    /// host in `allowed_hosts` still connect at a non-public address; with the
    /// development opt-out there is no guard (the system proxy applies).
    #[tokio::test]
    async fn an_author_destination_or_a_listed_host_still_connects() {
        let s = server(ResponseTemplate::new(200)).await;
        let config = json!({ "base_url": s.uri() });
        let out = public_only()
            .execute(&HashMap::new(), &config, &mut json!({}), None)
            .await;
        assert_eq!(out.unwrap()["status"], 200);
        let fixed = HashMap::from([
            ("base_url".to_string(), json!(s.uri())),
            (AUTHORED_INPUTS_KEY.to_string(), json!(["base_url"])),
        ]);
        let out = public_only()
            .execute(&fixed, &json!({}), &mut json!({}), None)
            .await;
        assert_eq!(out.unwrap()["status"], 200);
        let host_port = s.uri().trim_start_matches("http://").to_string();
        let by_name = s.uri().replace("127.0.0.1", "localhost");
        for (base, listed) in [(s.uri(), host_port), (by_name, "localhost".to_string())] {
            let config = json!({ "allowed_hosts": [listed] });
            let out = run(&public_only(), &base, config).await;
            assert_eq!(out.unwrap()["status"], 200, "{base}");
        }
        assert_eq!(s.received_requests().await.unwrap().len(), 4);
        // With every address dialable (the development opt-out), no guard.
        let open = HttpNode::new().with_url_parts(SignedUrlDownloader::allowing_private_hosts());
        let url = Url::parse("http://10.255.0.1").unwrap();
        assert!(open.destination_guard(&url, None, None).unwrap().is_none());
    }
}

#[cfg(test)]
mod file_response_tests {
    use super::*;
    use crate::llm::domain::AttachmentRegistry;
    use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
    use crate::storage::domain::{MockOutputStorageRepository, StoredOutput};
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PDF: &[u8] = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj\n";
    /// The Content-Type the Despegar voucher endpoint really sends.
    const JSON_FIRST: &str = "application/json;charset=utf-8,application/pdf;charset=utf-8";

    async fn server_returning(body: &[u8], content_type: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/reservations/123/voucher"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body.to_vec(), content_type))
            .mount(&server)
            .await;
        server
    }

    fn config(server: &MockServer) -> Value {
        json!({ "base_url": server.uri(), "endpoint": "/reservations/123/voucher", "method": "GET" })
    }

    fn session_inputs() -> NodeInputs {
        HashMap::from([
            ("__colmena_session_id".to_string(), json!("run_1")),
            ("__colmena_agent_session_id".to_string(), json!("agent_1")),
        ])
    }

    fn storing_storage() -> MockOutputStorageRepository {
        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .withf(|req| {
                req.bytes == PDF
                    && req.agent_session_id.as_deref() == Some("agent_1")
                    && req.session_id.as_deref() == Some("run_1")
            })
            .returning(|req| {
                Ok(StoredOutput {
                    storage_key: "chat-attachments/u/agent_1/generated/voucher.pdf".into(),
                    read_url: "https://storage.example/signed".into(),
                    mime_type: req.mime_type,
                    filename: req.filename,
                    size_bytes: req.bytes.len() as u64,
                })
            });
        storage
    }

    #[tokio::test]
    async fn pdf_response_is_stored_registered_and_listed_in_files() {
        let server = server_returning(PDF, JSON_FIRST).await;
        let registry: Arc<dyn AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        let node = HttpNode::new()
            .with_storage(Arc::new(storing_storage()))
            .with_attachment_registry(registry.clone());

        let out = node
            .execute(&session_inputs(), &config(&server), &mut json!({}), None)
            .await
            .unwrap();

        assert_eq!(out["status"], 200);
        assert_eq!(out["body"], Value::Null);
        let file = &out["files"][0];
        assert_eq!(file["mime_type"], "application/pdf");
        assert_eq!(file["filename"], "voucher.pdf");
        assert_eq!(file["size_bytes"], PDF.len());
        let doc_id = file["document_id"].as_str().unwrap();
        assert!(doc_id.starts_with("file_voucher_"), "got {doc_id}");
        // The read URL never reaches the model.
        assert!(!out.to_string().contains("storage.example"));

        let row = registry
            .lookup_by_document_id("agent_1", doc_id)
            .await
            .unwrap()
            .expect("registered under its document_id");
        assert_eq!(row.origin.as_deref(), Some("generated_by:http_request"));
        assert_eq!(row.mime_type, "application/pdf");
    }

    #[tokio::test]
    async fn without_storage_a_file_response_stays_body_null() {
        let server = server_returning(PDF, "application/pdf").await;
        let out = HttpNode::new()
            .execute(&session_inputs(), &config(&server), &mut json!({}), None)
            .await
            .unwrap();
        assert_eq!(out, json!({ "status": 200, "body": null }));
    }

    #[tokio::test]
    async fn file_over_max_file_size_bytes_is_not_stored() {
        let server = server_returning(PDF, "application/pdf").await;
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store().never();
        let mut cfg = config(&server);
        cfg["max_file_size_bytes"] = json!(4);
        let out = HttpNode::new()
            .with_storage(Arc::new(storage))
            .execute(&session_inputs(), &cfg, &mut json!({}), None)
            .await
            .unwrap();
        assert_eq!(out, json!({ "status": 200, "body": null }));
    }

    #[tokio::test]
    async fn text_response_is_not_a_file_and_json_still_parses() {
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store().never();
        let node = HttpNode::new().with_storage(Arc::new(storage));

        let html = server_returning(b"<html>error</html>", "text/html").await;
        let out = node
            .execute(&session_inputs(), &config(&html), &mut json!({}), None)
            .await
            .unwrap();
        assert_eq!(out, json!({ "status": 200, "body": null }));

        let api = server_returning(br#"{"ok":true}"#, JSON_FIRST).await;
        let out = node
            .execute(&session_inputs(), &config(&api), &mut json!({}), None)
            .await
            .unwrap();
        assert_eq!(out, json!({ "status": 200, "body": { "ok": true } }));
    }
}
