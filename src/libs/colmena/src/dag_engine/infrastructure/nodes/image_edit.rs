//! `image_edit` node. Edits an existing image given a text prompt using
//! OpenAI's `/v1/images/edits` endpoint (gpt-image-1 / dall-e-2). The source
//! image (and optional mask) are fetched from a URL — either a `data:` URI
//! (typical when the previous tool stored via `LocalCacheStorageAdapter`) or
//! an `https://` signed read URL (typical with the `HttpCallbackStorageAdapter`).
//!
//! Output shape mirrors `image_generation` so agents can chain seamlessly.

use std::error::Error as StdError;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::util::attachment_id::build_document_id;
use crate::llm::domain::attachments::{
    origin, AttachmentSource, AttachmentStreamResolver, UpsertAttachmentInput,
};
use crate::llm::domain::{AttachmentRegistry, ProviderKind};
use crate::storage::domain::{OutputStorageRepository, StoreRequest};
use futures::StreamExt;

/// Hard cap on bytes drained from a resolved `$attachment` stream into memory
/// before editing. Mirrors `http_request`'s fetch cap. Overridable via
/// `COLMENA_FILE_FETCH_MAX_BYTES`; defaults to 100 MB.
fn max_source_bytes() -> usize {
    std::env::var("COLMENA_FILE_FETCH_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100 * 1024 * 1024)
}

pub struct ImageEditNode {
    storage: Arc<dyn OutputStorageRepository>,
    http: reqwest::Client,
    secure_values: Option<Arc<SecureValueService>>,
    attachment_registry: Option<Arc<dyn AttachmentRegistry>>,
    /// Plan A: resolves `$attachment:<document_id>` (or a bare `document_id`)
    /// in `source_url` to bytes via the conversation attachment catalog. Wired
    /// from the registry alongside `http_request`'s resolver. When absent, only
    /// `data:` / `http(s)` / storage-handle sources work.
    attachment_resolver: Option<Arc<dyn AttachmentStreamResolver>>,
    #[cfg(test)]
    test_openai_base_url: Option<String>,
}

impl ImageEditNode {
    pub fn new(storage: Arc<dyn OutputStorageRepository>) -> Self {
        Self {
            storage,
            http: reqwest::Client::new(),
            secure_values: None,
            attachment_registry: None,
            attachment_resolver: None,
            #[cfg(test)]
            test_openai_base_url: None,
        }
    }

    pub fn with_secure_values(mut self, svc: Arc<SecureValueService>) -> Self {
        self.secure_values = Some(svc);
        self
    }

    pub fn with_attachment_registry(mut self, reg: Arc<dyn AttachmentRegistry>) -> Self {
        self.attachment_registry = Some(reg);
        self
    }

    /// Plan A: wire the resolver that turns `$attachment:<document_id>` (or a
    /// bare `document_id`) in `source_url` into bytes, so the LLM can edit an
    /// image generated or uploaded earlier in the conversation without ever
    /// handling a signed URL.
    pub fn with_attachment_resolver(mut self, resolver: Arc<dyn AttachmentStreamResolver>) -> Self {
        self.attachment_resolver = Some(resolver);
        self
    }

    #[cfg(test)]
    fn with_openai_base_url(mut self, url: String) -> Self {
        self.test_openai_base_url = Some(url);
        self
    }

    fn openai_base_url(&self) -> &str {
        #[cfg(test)]
        if let Some(url) = &self.test_openai_base_url {
            return url.as_str();
        }
        "https://api.openai.com"
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var = &value[2..value.len() - 1];
            std::env::var(var)
                .map_err(|_| format!("env var {var} not set (referenced by image_edit)"))
        } else {
            Ok(value.to_string())
        }
    }

    /// Resolve a `source_url`/`mask_url` to bytes, in priority order:
    ///
    /// 1. `$attachment:<document_id>` → resolve via the attachment catalog
    ///    (Plan A). This is what `image_generation`/`image_edit` instruct the
    ///    LLM to use to chain a previously-generated or uploaded image.
    /// 2. A known scheme (`data:`, `http(s)`, `local://`, `chat-attachments/`)
    ///    → fetch directly via [`Self::fetch_image`].
    /// 3. A bare token that is none of the above (e.g. a raw `document_id`
    ///    like `img_image_0` the LLM passed without the `$attachment:` prefix)
    ///    → attempt catalog resolution when a resolver is configured.
    ///
    /// Returns `(bytes, mime_type)`.
    async fn resolve_source(
        &self,
        url: &str,
        agent_session_id: Option<&str>,
    ) -> Result<(Vec<u8>, String), Box<dyn StdError + Send + Sync>> {
        if let Some(document_id) = url.strip_prefix("$attachment:") {
            return self
                .resolve_via_attachment(document_id, agent_session_id)
                .await;
        }
        if url.starts_with("local://")
            || url.starts_with("chat-attachments/")
            || url.starts_with("data:")
            || url.starts_with("http://")
            || url.starts_with("https://")
        {
            return self.fetch_image(url).await;
        }
        // Bare token — most likely a `document_id` the LLM forwarded without
        // the `$attachment:` prefix. Try the catalog resolver (its raw-key
        // fallback also covers a bare storage_key).
        if self.attachment_resolver.is_some() {
            return self.resolve_via_attachment(url, agent_session_id).await;
        }
        Err(format!(
            "image_edit: unsupported source '{url}' (expected $attachment:<document_id>, a \
             document_id, a data: URI, or an http(s) URL)"
        )
        .into())
    }

    /// Resolve a `document_id` (or raw storage_key, via the resolver's
    /// fallback) to bytes through the [`AttachmentStreamResolver`], draining
    /// the stream into memory with a defensive size cap.
    async fn resolve_via_attachment(
        &self,
        document_id: &str,
        agent_session_id: Option<&str>,
    ) -> Result<(Vec<u8>, String), Box<dyn StdError + Send + Sync>> {
        let resolver = self.attachment_resolver.as_ref().ok_or_else(
            || -> Box<dyn StdError + Send + Sync> {
                format!(
                    "image_edit: source references attachment '{document_id}' but no attachment \
                     resolver is configured on this engine"
                )
                .into()
            },
        )?;
        let sid = agent_session_id.ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
            format!(
                "image_edit: cannot resolve attachment '{document_id}' without an \
                 agent_session_id (this conversation has no stable session handle)"
            )
            .into()
        })?;

        let stored = resolver
            .resolve(sid, document_id)
            .await
            .map_err(|e| -> Box<dyn StdError + Send + Sync> { e.to_string().into() })?;

        let mime = stored.mime_type.clone();
        let cap = max_source_bytes();
        let mut buf: Vec<u8> = Vec::with_capacity((stored.size_bytes as usize).min(cap));
        let mut stream = stored.stream;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| -> Box<dyn StdError + Send + Sync> {
                format!("image_edit: error reading attachment '{document_id}' stream: {e}").into()
            })?;
            if buf.len() + chunk.len() > cap {
                return Err(format!(
                    "image_edit: attachment '{document_id}' exceeds the {cap}-byte source cap"
                )
                .into());
            }
            buf.extend_from_slice(&chunk);
        }
        if buf.is_empty() {
            return Err(format!(
                "image_edit: attachment '{document_id}' resolved to an empty body"
            )
            .into());
        }
        Ok((buf, mime))
    }

    /// Fetch image bytes from a `data:` URI, `http(s)` URL, or `local://<key>`
    /// storage handle. The last form is what `LocalCacheStorageAdapter`
    /// returns from `store()` — resolving it via `OutputStorageRepository.read`
    /// keeps the gen → edit chain working without bloating the LLM context
    /// with megabytes of base64.
    async fn fetch_image(
        &self,
        url: &str,
    ) -> Result<(Vec<u8>, String), Box<dyn StdError + Send + Sync>> {
        if url.starts_with("local://") || url.starts_with("chat-attachments/") {
            // Storage-managed handle — resolve via the storage adapter.
            let stored = self
                .storage
                .read(url)
                .await
                .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;
            return Ok((stored.bytes, stored.mime_type));
        }
        if let Some(rest) = url.strip_prefix("data:") {
            // data:<mime>;base64,<payload>
            let (header, payload) = rest
                .split_once(',')
                .ok_or("image_edit: malformed data: URI (missing comma)")?;
            let mime = header
                .split(';')
                .next()
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(payload)
                .map_err(|e| format!("image_edit: data: URI base64 decode failed: {e}"))?;
            Ok((bytes, mime))
        } else if url.starts_with("http://") || url.starts_with("https://") {
            // Some CDNs (Wikimedia, etc.) reject requests without a User-Agent.
            // Send a generic one so fetches of public images succeed by default.
            let resp = self
                .http
                .get(url)
                .header(
                    reqwest::header::USER_AGENT,
                    "colmena-image-edit/0.3 (+https://github.com/Startti/colmena)",
                )
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(format!(
                    "image_edit: fetch source url failed: status={}",
                    resp.status()
                )
                .into());
            }
            let mime = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/png")
                .split(';')
                .next()
                .unwrap_or("image/png")
                .trim()
                .to_string();
            let bytes = resp.bytes().await?.to_vec();
            if bytes.is_empty() {
                return Err("image_edit: source url returned empty body".into());
            }
            Ok((bytes, mime))
        } else {
            Err(
                format!("image_edit: unsupported url scheme (expected data:/http:/https:): {url}")
                    .into(),
            )
        }
    }
}

#[async_trait]
impl ExecutableNode for ImageEditNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let session_id = inputs
            .get("__colmena_session_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let agent_session_id = inputs
            .get("__colmena_agent_session_id")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut cfg = config.clone();
        if let Some(svc) = &self.secure_values {
            let svc_session = session_id.as_deref().unwrap_or("default");
            let _ = svc
                .inject_secrets(&mut cfg, svc_session, agent_session_id.as_deref())
                .await?;
        }

        // provider/model/api_key are infrastructure config but must be readable
        // from both `inputs` and `config` (inputs first) so the same node works
        // standalone and as an LLM tool (the executor passes config={} when
        // invoked via tool_call — see dag_tool_executor.rs ~line 934).
        let provider = inputs
            .get("provider")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                cfg.get("provider")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .ok_or("image_edit: provider is required (openai)")?;
        if provider != "openai" {
            return Err(format!(
                "image_edit: unsupported provider '{provider}' (only 'openai' is implemented today; \
                 Google Vertex image editing is on the roadmap)"
            )
            .into());
        }
        let model = inputs
            .get("model")
            .and_then(|v| v.as_str())
            .or_else(|| cfg.get("model").and_then(|v| v.as_str()))
            .unwrap_or("gpt-image-1")
            .to_string();
        let api_key_raw = inputs
            .get("api_key")
            .and_then(|v| v.as_str())
            .or_else(|| cfg.get("api_key").and_then(|v| v.as_str()))
            .ok_or("image_edit: api_key is required")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;

        // Inputs-over-config for LLM-controllable / chainable fields.
        let prompt = inputs
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| cfg.get("prompt").and_then(|v| v.as_str()).map(String::from))
            .ok_or("image_edit: prompt is required (via inputs or config)")?;
        let source_url = inputs
            .get("source_url")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                cfg.get("source_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .ok_or("image_edit: source_url is required (data:/http(s) URL via inputs or config)")?;
        let mask_url = inputs
            .get("mask_url")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                cfg.get("mask_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
        let n = inputs
            .get("n")
            .and_then(|v| v.as_u64())
            .or_else(|| cfg.get("n").and_then(|v| v.as_u64()))
            .unwrap_or(1)
            .clamp(1, 10) as u32;
        let size = inputs
            .get("size")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| cfg.get("size").and_then(|v| v.as_str()).map(String::from));
        let quality = inputs
            .get("quality")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                cfg.get("quality")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });

        // Fetch source (and optional mask) bytes.
        let (source_bytes, source_mime) = self
            .resolve_source(&source_url, agent_session_id.as_deref())
            .await?;
        let mask = if let Some(m) = &mask_url {
            Some(self.resolve_source(m, agent_session_id.as_deref()).await?)
        } else {
            None
        };

        // Build multipart form.
        let mut form = reqwest::multipart::Form::new()
            .text("model", model.clone())
            .text("prompt", prompt.clone())
            .text("n", n.to_string());

        let source_filename = match source_mime.as_str() {
            "image/png" => "source.png",
            "image/jpeg" => "source.jpg",
            "image/webp" => "source.webp",
            _ => "source.bin",
        };
        let source_part = reqwest::multipart::Part::bytes(source_bytes)
            .file_name(source_filename.to_string())
            .mime_str(&source_mime)
            .map_err(|e| format!("image_edit: invalid source mime: {e}"))?;
        form = form.part("image", source_part);

        if let Some((mask_bytes, mask_mime)) = mask {
            let mask_part = reqwest::multipart::Part::bytes(mask_bytes)
                .file_name("mask.png".to_string())
                .mime_str(&mask_mime)
                .map_err(|e| format!("image_edit: invalid mask mime: {e}"))?;
            form = form.part("mask", mask_part);
        }
        if let Some(s) = &size {
            form = form.text("size", s.clone());
        }
        if let Some(q) = &quality {
            form = form.text("quality", q.clone());
        }

        // Call OpenAI.
        let url = format!("{}/v1/images/edits", self.openai_base_url());
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&api_key)
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "image_edit: openai /v1/images/edits failed: status={status} body={body}"
            )
            .into());
        }

        let payload: Value = resp.json().await?;
        let data = payload
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or("image_edit: openai response missing `data` array")?;
        if data.is_empty() {
            return Err("image_edit: openai response `data` array is empty".into());
        }

        // Decode + persist each result.
        let prompt_preview: String = prompt.chars().take(80).collect();
        let mut out_images = Vec::with_capacity(data.len());
        for (i, entry) in data.iter().enumerate() {
            let bytes = if let Some(b64) = entry.get("b64_json").and_then(|v| v.as_str()) {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| format!("image_edit: b64 decode failed: {e}"))?
            } else if let Some(u) = entry.get("url").and_then(|v| v.as_str()) {
                self.http.get(u).send().await?.bytes().await?.to_vec()
            } else {
                return Err(
                    "image_edit: openai response entry missing both b64_json and url".into(),
                );
            };
            let filename = format!("edit_{}.png", i);
            let stored = self
                .storage
                .store(StoreRequest {
                    bytes,
                    mime_type: "image/png".to_string(),
                    filename,
                    session_id: session_id.clone(),
                    agent_session_id: agent_session_id.clone(),
                })
                .await
                .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;

            // Plan A: derive a human-friendly document_id from the filename
            // so the LLM can reference the artifact by a stable handle that is
            // not the opaque storage_key UUID.
            let document_id = build_document_id(
                &stored.filename,
                &stored.mime_type,
                &stored.storage_key,
                "img",
            );

            // Register the edited artifact so `load_attachment` can later
            // resolve it AND so `$attachment:<document_id>` placeholders work
            // in downstream nodes (e.g., http_request multipart parts).
            // Fail-soft: registry errors must not fail the edit.
            if let (Some(reg), Some(agent_sid)) =
                (self.attachment_registry.as_ref(), agent_session_id.as_ref())
            {
                let description = format!("Image edited with {}: {}", model, prompt_preview);
                let upsert = UpsertAttachmentInput {
                    agent_session_id: agent_sid.clone(),
                    document_id: document_id.clone(),
                    provider: ProviderKind::Generated,
                    // For `provider: Generated` rows, provider_file_id holds
                    // the canonical storage_key. Cross-provider upload reads
                    // bytes via OutputStorageRepository.read(this).
                    provider_file_id: stored.storage_key.clone(),
                    mime_type: stored.mime_type.clone(),
                    filename: stored.filename.clone(),
                    size_bytes: Some(stored.size_bytes),
                    label: None,
                    description: Some(description),
                    // Resolver-friendly source: read bytes back through the
                    // storage port using the storage_key as the path.
                    source: AttachmentSource::Path(stored.storage_key.clone()),
                    storage_key: Some(stored.storage_key.clone()),
                    origin: Some(origin::generated_by("image_edit")),
                };
                if let Err(e) = reg.upsert(upsert).await {
                    tracing::warn!(
                        target: "colmena::image_edit",
                        error = %e,
                        document_id = %document_id,
                        storage_key = %stored.storage_key,
                        "failed to register edited image in attachment registry — \
                         load_attachment will not see this output"
                    );
                }
            }

            out_images.push(json!({
                // Plan B (D8): attachment_id alias and url field removed.
                // The storage_key and read_url are still recorded internally
                // on the auto-registered conversation_attachments row;
                // downstream consumers (e.g., ADP frontend) resolve URLs by
                // document_id via a dedicated endpoint.
                "document_id": document_id,
                "mime_type": stored.mime_type,
                "size_bytes": stored.size_bytes,
                "description": format!("Image edited with {}: {}", model, prompt_preview),
            }));
        }

        Ok(json!({
            "output": {
                "images": out_images,
                "provider": provider,
                "model": model,
            }
        }))
    }

    fn schema(&self) -> Value {
        json!({
            "inputs": {},
            "outputs": { "output": "object" },
            "config": {
                "provider": "string (required) — openai (only supported today)",
                "model": "string (optional, default gpt-image-1)",
                "api_key": "string (required) — ${ENV_VAR} or secure-value placeholders supported",
                "source_url": "string (required) — image to edit. Accepts \"$attachment:<document_id>\" or a bare document_id (to edit an image generated/uploaded earlier in this conversation), a data: URI, or an http(s) URL",
                "mask_url": "string (optional) — PNG with transparency marking the edit area. Same accepted forms as source_url",
                "prompt": "string (required) — describes the desired edit",
                "size": "string (optional)",
                "quality": "string (optional, openai) — low|medium|high|auto",
                "n": "integer (optional, default 1, max 10)"
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Edit an existing image given a text prompt. The source image (`source_url`) \
             can be \"$attachment:<document_id>\" or a bare document_id to edit an image \
             generated or uploaded earlier in this conversation, or a data:/http(s) URL. \
             Optional mask marks the edit region. Returns \
             { images: [{ document_id, mime_type, size_bytes }], provider, model } \
             — same shape as image_generation so results can be chained. Use \
             \"$attachment:<document_id>\" in downstream tool args to forward the \
             image, or call load_attachment(document_id) to read it.",
        )
    }

    fn default_input(&self) -> Option<&str> {
        Some("source_url")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::domain::{MockOutputStorageRepository, StoredOutput};
    use mockall::predicate::always;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn stored_ok(k: &str) -> StoredOutput {
        StoredOutput {
            storage_key: k.into(),
            read_url: "data:image/png;base64,XX".into(),
            mime_type: "image/png".into(),
            filename: "edit_0.png".into(),
            size_bytes: 2,
        }
    }

    fn base_config(source_url: &str) -> Value {
        json!({
            "provider": "openai",
            "model": "gpt-image-1",
            "api_key": "sk-test",
            "source_url": source_url,
            "prompt": "make the background blue",
        })
    }

    #[tokio::test]
    async fn happy_path_fetches_source_posts_multipart_and_stores() {
        let server = MockServer::start().await;
        // Serve the source image (3 bytes, fake png).
        Mock::given(method("GET"))
            .and(path("/source.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(vec![0x89u8, 0x50, 0x4e])
                    .insert_header("content-type", "image/png"),
            )
            .mount(&server)
            .await;
        // Mock OpenAI edits — returns one b64_json image ("AAAA" → [0,0,0]).
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .with(always())
            .returning(|_| Ok(stored_ok("k1")));

        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());

        let source_url = format!("{}/source.png", server.uri());
        let out = node
            .execute(
                &HashMap::new(),
                &base_config(&source_url),
                &mut json!({}),
                None,
            )
            .await
            .expect("execute ok");

        let images = out
            .pointer("/output/images")
            .and_then(|v| v.as_array())
            .expect("images array");
        assert_eq!(images.len(), 1);
        // Plan B (D8): attachment_id alias and url field removed; only
        // document_id remains.
        assert!(images[0]["document_id"]
            .as_str()
            .unwrap()
            .starts_with("img_"));
        assert!(
            images[0].get("attachment_id").is_none(),
            "Plan B removed the attachment_id legacy alias"
        );
        assert!(
            images[0].get("url").is_none(),
            "Plan B removed the url field"
        );
        assert_eq!(out["output"]["provider"], "openai");
    }

    #[tokio::test]
    async fn data_uri_source_is_decoded_locally_without_http() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok("k")));

        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());

        // Source is inline data: URI — should NOT hit any HTTP server for the fetch step.
        let cfg = base_config("data:image/png;base64,iVBORw==");
        node.execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .expect("execute ok");
    }

    #[tokio::test]
    async fn missing_source_url_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = ImageEditNode::new(Arc::new(storage));
        let mut cfg = base_config("data:image/png;base64,AA==");
        cfg.as_object_mut().unwrap().remove("source_url");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("source_url"));
    }

    #[tokio::test]
    async fn missing_prompt_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = ImageEditNode::new(Arc::new(storage));
        let mut cfg = base_config("data:image/png;base64,AA==");
        cfg.as_object_mut().unwrap().remove("prompt");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("prompt"));
    }

    #[tokio::test]
    async fn unsupported_provider_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = ImageEditNode::new(Arc::new(storage));
        let mut cfg = base_config("data:image/png;base64,AA==");
        cfg["provider"] = json!("google");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unsupported provider"));
    }

    #[tokio::test]
    async fn openai_error_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad mask"))
            .mount(&server)
            .await;

        let storage = MockOutputStorageRepository::new();
        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());
        let err = node
            .execute(
                &HashMap::new(),
                &base_config("data:image/png;base64,AA=="),
                &mut json!({}),
                None,
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("400"));
        assert!(msg.contains("bad mask"));
    }

    #[tokio::test]
    async fn source_fetch_404_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let storage = MockOutputStorageRepository::new();
        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());

        let source_url = format!("{}/missing.png", server.uri());
        let err = node
            .execute(
                &HashMap::new(),
                &base_config(&source_url),
                &mut json!({}),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("fetch source url failed"));
    }

    #[tokio::test]
    async fn inputs_source_url_overrides_config() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok("k")));

        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());

        let mut cfg = base_config("data:image/png;base64,Y29uZmln");
        cfg["prompt"] = json!("config prompt");
        let mut inputs: NodeInputs = HashMap::new();
        // Override both — proves inputs win.
        inputs.insert("source_url".into(), json!("data:image/png;base64,aW5wdXRz"));
        inputs.insert("prompt".into(), json!("inputs prompt — should win"));

        let out = node
            .execute(&inputs, &cfg, &mut json!({}), None)
            .await
            .expect("execute ok");
        let desc = out["output"]["images"][0]["description"].as_str().unwrap();
        assert!(
            desc.contains("inputs prompt"),
            "description should reflect inputs prompt, got: {desc}"
        );
    }

    #[tokio::test]
    async fn session_ids_forwarded_to_storage() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .withf(|req| {
                req.session_id.as_deref() == Some("ses_abc")
                    && req.agent_session_id.as_deref() == Some("agent_xyz")
            })
            .returning(|_| Ok(stored_ok("k")));

        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_session_id".into(), json!("ses_abc"));
        inputs.insert("__colmena_agent_session_id".into(), json!("agent_xyz"));

        node.execute(
            &inputs,
            &base_config("data:image/png;base64,AA=="),
            &mut json!({}),
            None,
        )
        .await
        .expect("execute ok");
    }

    // -----------------------------------------------------------------------
    // Plan A — auto-registration in conversation_attachments
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn image_edit_auto_registers_artifact_in_registry() {
        use crate::llm::domain::attachments::AttachmentSource;
        use crate::llm::domain::{AttachmentRegistry, ProviderKind};
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok("sk-edit-1")));

        let registry: Arc<dyn AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );

        let node = ImageEditNode::new(Arc::new(storage))
            .with_openai_base_url(server.uri())
            .with_attachment_registry(registry.clone());

        let cfg = base_config("data:image/png;base64,iVBORw==");

        let mut inputs: NodeInputs = HashMap::new();
        // Auto-registration only fires when an agent_session_id is engine-injected.
        inputs.insert(
            "__colmena_agent_session_id".into(),
            json!("agent_autoreg_edit_1"),
        );

        let out = node
            .execute(&inputs, &cfg, &mut json!({}), None)
            .await
            .expect("execute ok");

        let images = out
            .pointer("/output/images")
            .and_then(|v| v.as_array())
            .expect("images array");
        assert_eq!(images.len(), 1);

        // Plan B (D8): tool result emits only document_id. attachment_id
        // alias and url field were removed; the storage_key is still
        // recorded internally on the auto-registered row (asserted below).
        let doc_id = images[0]["document_id"]
            .as_str()
            .expect("document_id present");
        assert!(
            doc_id.starts_with("img_"),
            "document_id should start with img_, got {doc_id}"
        );
        assert!(
            images[0].get("attachment_id").is_none(),
            "Plan B removed the attachment_id legacy alias"
        );
        assert!(
            images[0].get("url").is_none(),
            "Plan B removed the url field"
        );

        // The registry row MUST be reachable by document_id and carry the
        // generated_by:image_edit origin + storage_key.
        let entry = registry
            .lookup_by_document_id("agent_autoreg_edit_1", doc_id)
            .await
            .unwrap()
            .expect("attachment was auto-registered");
        assert_eq!(entry.storage_key.as_deref(), Some("sk-edit-1"));
        assert_eq!(entry.origin.as_deref(), Some("generated_by:image_edit"));
        assert!(matches!(entry.provider, ProviderKind::Generated));
        match entry.source {
            AttachmentSource::Path(ref p) => assert_eq!(p, "sk-edit-1"),
            ref other => panic!("expected AttachmentSource::Path, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_registry_means_no_registration_but_still_emits_document_id() {
        // Sanity check: when the node is constructed without a registry, the
        // tool result still carries document_id (so the LLM contract is
        // unchanged), and we don't crash trying to upsert. Plan B (D8)
        // removed the attachment_id alias and url field.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok("sk-2")));

        let node = ImageEditNode::new(Arc::new(storage)).with_openai_base_url(server.uri());

        let cfg = base_config("data:image/png;base64,AA==");
        let out = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap();
        let img = &out["output"]["images"][0];
        assert!(img["document_id"].is_string());
        assert!(
            img.get("attachment_id").is_none(),
            "Plan B removed the attachment_id legacy alias"
        );
        assert!(img.get("url").is_none(), "Plan B removed the url field");
    }

    // ---- Fase 1: $attachment / document_id source resolution -----------------

    /// Build a single-chunk `StoredStream` for the resolver's mock storage.
    fn make_stored_stream(body: &'static [u8], mime: &str) -> crate::storage::domain::StoredStream {
        use crate::storage::domain::{StorageError, StoredStream};
        use bytes::Bytes;
        use futures::stream;
        use std::pin::Pin;
        let s: Pin<Box<dyn futures::Stream<Item = Result<Bytes, StorageError>> + Send>> =
            Box::pin(stream::iter(vec![Ok(Bytes::from_static(body))]));
        StoredStream {
            stream: s,
            size_bytes: body.len() as u64,
            mime_type: mime.to_string(),
            filename: "src.png".to_string(),
        }
    }

    /// Build a resolver backed by a Sqlite registry pre-seeded with one row
    /// (`document_id` → `storage_key`) and a mock storage that streams `body`
    /// for that key.
    async fn resolver_with_source(
        agent_session_id: &str,
        document_id: &str,
        storage_key: &str,
        body: &'static [u8],
    ) -> Arc<dyn AttachmentStreamResolver> {
        use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
        use crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;

        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: agent_session_id.to_string(),
            document_id: document_id.to_string(),
            provider: ProviderKind::Generated,
            provider_file_id: storage_key.to_string(),
            mime_type: "image/png".to_string(),
            filename: "src.png".to_string(),
            size_bytes: Some(body.len() as u64),
            label: None,
            description: None,
            source: AttachmentSource::Path(storage_key.to_string()),
            storage_key: Some(storage_key.to_string()),
            origin: Some("generated_by:image_generation".to_string()),
        })
        .await
        .unwrap();
        let reg_arc: Arc<dyn AttachmentRegistry> = Arc::new(reg);

        let key = storage_key.to_string();
        let mut resolver_storage = MockOutputStorageRepository::new();
        resolver_storage
            .expect_read_stream()
            .withf(move |k| k == key)
            .times(1)
            .returning(move |_| Ok(make_stored_stream(body, "image/png")));

        Arc::new(AttachmentStreamResolverImpl::new(
            reg_arc,
            Arc::new(resolver_storage),
        ))
    }

    /// Mounts a mock OpenAI `/v1/images/edits` returning one image.
    async fn mount_edits(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": "AAAA" }]
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn source_attachment_placeholder_resolves_and_edits() {
        let server = MockServer::start().await;
        mount_edits(&server).await;

        let mut node_storage = MockOutputStorageRepository::new();
        node_storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok("sk-out")));

        let resolver =
            resolver_with_source("agent_edit_src", "img_src", "sk-src", b"\x89PNG").await;

        let node = ImageEditNode::new(Arc::new(node_storage))
            .with_openai_base_url(server.uri())
            .with_attachment_resolver(resolver);

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_agent_session_id".into(), json!("agent_edit_src"));

        let out = node
            .execute(
                &inputs,
                &base_config("$attachment:img_src"),
                &mut json!({}),
                None,
            )
            .await
            .expect("execute ok with $attachment source");

        let images = out
            .pointer("/output/images")
            .and_then(|v| v.as_array())
            .expect("images array");
        assert_eq!(images.len(), 1);
    }

    #[tokio::test]
    async fn source_bare_document_id_resolves_via_resolver() {
        let server = MockServer::start().await;
        mount_edits(&server).await;

        let mut node_storage = MockOutputStorageRepository::new();
        node_storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok("sk-out")));

        // Source passed WITHOUT the `$attachment:` prefix — a bare document_id.
        let resolver = resolver_with_source("agent_bare", "img_bare", "sk-bare", b"\x89PNG").await;

        let node = ImageEditNode::new(Arc::new(node_storage))
            .with_openai_base_url(server.uri())
            .with_attachment_resolver(resolver);

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_agent_session_id".into(), json!("agent_bare"));

        let out = node
            .execute(&inputs, &base_config("img_bare"), &mut json!({}), None)
            .await
            .expect("execute ok with bare document_id source");
        assert_eq!(
            out.pointer("/output/images")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );
    }

    #[tokio::test]
    async fn source_attachment_not_found_errors_clearly() {
        use crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use crate::storage::domain::StorageError;

        // Empty registry + storage that rejects the fallback raw-key read.
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        let reg_arc: Arc<dyn AttachmentRegistry> = Arc::new(reg);
        let mut resolver_storage = MockOutputStorageRepository::new();
        resolver_storage
            .expect_read_stream()
            .returning(|_| Err(StorageError::InvalidInput("unknown key".into())));
        let resolver: Arc<dyn AttachmentStreamResolver> = Arc::new(
            AttachmentStreamResolverImpl::new(reg_arc, Arc::new(resolver_storage)),
        );

        // No OpenAI mock and no store() expectation: resolution must fail first.
        let node = ImageEditNode::new(Arc::new(MockOutputStorageRepository::new()))
            .with_attachment_resolver(resolver);

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_agent_session_id".into(), json!("agent_nf"));

        let err = node
            .execute(
                &inputs,
                &base_config("$attachment:nope"),
                &mut json!({}),
                None,
            )
            .await
            .expect_err("missing attachment should error");
        let msg = err.to_string();
        assert!(
            msg.contains("not found") && msg.contains("nope"),
            "expected a clear not-found error, got: {msg}"
        );
        assert!(
            !msg.contains("unsupported url scheme"),
            "should NOT fall through to the legacy scheme error: {msg}"
        );
    }

    #[tokio::test]
    async fn source_attachment_without_resolver_errors() {
        // No resolver wired → $attachment cannot be resolved.
        let node = ImageEditNode::new(Arc::new(MockOutputStorageRepository::new()));
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_agent_session_id".into(), json!("agent_x"));
        let err = node
            .execute(
                &inputs,
                &base_config("$attachment:img_x"),
                &mut json!({}),
                None,
            )
            .await
            .expect_err("no resolver configured should error");
        assert!(
            err.to_string()
                .contains("no attachment resolver is configured"),
            "got: {}",
            err
        );
    }
}
