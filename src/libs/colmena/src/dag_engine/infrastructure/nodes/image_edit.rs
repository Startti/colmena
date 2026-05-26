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
use crate::llm::domain::attachments::{origin, AttachmentSource, UpsertAttachmentInput};
use crate::llm::domain::{AttachmentRegistry, ProviderKind};
use crate::storage::domain::{OutputStorageRepository, StoreRequest};

pub struct ImageEditNode {
    storage: Arc<dyn OutputStorageRepository>,
    http: reqwest::Client,
    secure_values: Option<Arc<SecureValueService>>,
    attachment_registry: Option<Arc<dyn AttachmentRegistry>>,
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
        let (source_bytes, source_mime) = self.fetch_image(&source_url).await?;
        let mask = if let Some(m) = &mask_url {
            Some(self.fetch_image(m).await?)
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
                "source_url": "string (required) — data: URI or http(s) URL of the image to edit",
                "mask_url": "string (optional) — PNG with transparency marking the edit area",
                "prompt": "string (required) — describes the desired edit",
                "size": "string (optional)",
                "quality": "string (optional, openai) — low|medium|high|auto",
                "n": "integer (optional, default 1, max 10)"
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Edit an existing image given a text prompt. Source image is fetched from a \
             URL (data: or http(s)). Optional mask marks the edit region. Returns \
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
}
