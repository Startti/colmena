//! `tts` node. Synthesizes speech from a text prompt using OpenAI,
//! ElevenLabs, or Google Gemini TTS. Audio bytes are persisted via the
//! injected [`OutputStorageRepository`] (same as `image_generation`), so the
//! node returns small handles instead of inline audio.
//!
//! ## Config schema
//!
//! ```json
//! {
//!   "provider": "openai" | "elevenlabs" | "google",
//!   "model": "tts-1" | "tts-1-hd" | "gpt-4o-mini-tts"
//!          | "eleven_multilingual_v2" | "eleven_turbo_v2_5"
//!          | "gemini-2.5-flash-preview-tts",
//!   "api_key": "${OPENAI_API_KEY}",
//!   "text": "Hello world",
//!   "voice": "alloy" | "<elevenlabs_voice_id>" | "Kore",
//!   "format": "mp3" | "wav" | "opus" | "pcm",
//!   "speed": 1.0
//! }
//! ```
//!
//! ## Output shape
//!
//! ```json
//! {
//!   "output": {
//!     "audio": {
//!       "attachment_id": "...",
//!       "url": "...",
//!       "mime_type": "audio/mpeg",
//!       "size_bytes": 12345,
//!       "duration_ms": null,
//!       "description": "TTS synthesized with <model>: <text prefix>"
//!     },
//!     "provider": "openai",
//!     "model": "tts-1"
//!   }
//! }
//! ```

use std::error::Error as StdError;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
use crate::llm::domain::tts::{AudioFormat, TtsRequest};
use crate::llm::domain::{AttachmentRegistry, ProviderKind};
use crate::llm::infrastructure::build_tts_repository;
use crate::storage::domain::{OutputStorageRepository, StoreRequest};

pub struct TtsNode {
    storage: Arc<dyn OutputStorageRepository>,
    secure_values: Option<Arc<SecureValueService>>,
    attachment_registry: Option<Arc<dyn AttachmentRegistry>>,
    /// Test-only override: when present, the node uses this repository
    /// instead of building one via the factory. Bypasses provider dispatch.
    #[cfg(test)]
    test_repository: Option<Arc<dyn crate::llm::domain::tts_repository::TtsRepository>>,
}

impl TtsNode {
    pub fn new(storage: Arc<dyn OutputStorageRepository>) -> Self {
        Self {
            storage,
            secure_values: None,
            attachment_registry: None,
            #[cfg(test)]
            test_repository: None,
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
    fn with_test_repository(
        mut self,
        repo: Arc<dyn crate::llm::domain::tts_repository::TtsRepository>,
    ) -> Self {
        self.test_repository = Some(repo);
        self
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var = &value[2..value.len() - 1];
            std::env::var(var).map_err(|_| format!("env var {var} not set (referenced by tts)"))
        } else {
            Ok(value.to_string())
        }
    }
}

#[async_trait]
impl ExecutableNode for TtsNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Engine-injected scope identifiers, forwarded to the storage adapter.
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

        // Infrastructure config — read from inputs first, fall back to config.
        // When invoked as an LLM tool, the executor passes config={} and merges
        // all `node_schema.fixed` values into inputs (dag_tool_executor.rs ~934).
        let provider = inputs
            .get("provider")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                cfg.get("provider")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .ok_or("tts: provider is required (openai|elevenlabs|google)")?;
        let model = inputs
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| cfg.get("model").and_then(|v| v.as_str()).map(String::from))
            .ok_or("tts: model is required")?;
        let api_key_raw = inputs
            .get("api_key")
            .and_then(|v| v.as_str())
            .or_else(|| cfg.get("api_key").and_then(|v| v.as_str()))
            .ok_or("tts: api_key is required")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;

        // Inputs-over-config for LLM-controllable / chainable fields.
        let text = inputs
            .get("text")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| cfg.get("text").and_then(|v| v.as_str()).map(String::from))
            .ok_or("tts: text is required (via inputs or config)")?;
        let voice = inputs
            .get("voice")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| cfg.get("voice").and_then(|v| v.as_str()).map(String::from))
            .ok_or("tts: voice is required (via inputs or config)")?;
        let format_str = inputs
            .get("format")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| cfg.get("format").and_then(|v| v.as_str()).map(String::from));
        let format = match format_str {
            Some(s) => AudioFormat::from_str(&s)
                .map_err(|e| -> Box<dyn StdError + Send + Sync> { format!("tts: {e}").into() })?,
            None => AudioFormat::default(),
        };
        let speed = inputs
            .get("speed")
            .and_then(|v| v.as_f64())
            .or_else(|| cfg.get("speed").and_then(|v| v.as_f64()))
            .map(|v| v as f32);

        let repository = {
            #[cfg(test)]
            if let Some(r) = &self.test_repository {
                r.clone()
            } else {
                build_tts_repository(&provider, api_key)
                    .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?
            }
            #[cfg(not(test))]
            {
                build_tts_repository(&provider, api_key)
                    .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?
            }
        };

        let resp = repository
            .synthesize(TtsRequest {
                text: text.clone(),
                voice,
                format,
                speed,
                model: model.clone(),
            })
            .await
            .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;

        let filename = format!("speech.{}", format.file_extension());

        let stored = self
            .storage
            .store(StoreRequest {
                bytes: resp.audio_bytes,
                mime_type: resp.mime_type.clone(),
                filename,
                session_id: session_id.clone(),
                agent_session_id: agent_session_id.clone(),
            })
            .await
            .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;

        let text_preview: String = text.chars().take(80).collect();

        // Register synthesized audio in the attachment registry. Even though
        // LLMs can't "hear" today, registering enables `$attachment:<id>`
        // resolution in http_request (capability 3) — agents can ship the
        // audio to webhooks without ever seeing the bytes. Fail-soft.
        if let (Some(reg), Some(agent_sid)) =
            (self.attachment_registry.as_ref(), agent_session_id.as_ref())
        {
            let description = format!("TTS synthesized with {}: {}", model, text_preview);
            let upsert = UpsertAttachmentInput {
                agent_session_id: agent_sid.clone(),
                document_id: stored.storage_key.clone(),
                provider: ProviderKind::Generated,
                provider_file_id: stored.storage_key.clone(),
                mime_type: stored.mime_type.clone(),
                filename: stored.filename.clone(),
                size_bytes: Some(stored.size_bytes),
                label: None,
                description: Some(description),
                source: AttachmentSource::SignedUrl(stored.read_url.clone()),
                storage_key: None,
                origin: None,
            };
            if let Err(e) = reg.upsert(upsert).await {
                tracing::warn!(
                    target: "colmena::tts",
                    error = %e,
                    storage_key = %stored.storage_key,
                    "failed to register tts output in attachment registry"
                );
            }
        }
        Ok(json!({
            "output": {
                "audio": {
                    "attachment_id": stored.storage_key,
                    "url": stored.read_url,
                    "mime_type": stored.mime_type,
                    "size_bytes": stored.size_bytes,
                    "duration_ms": resp.duration_estimate_ms,
                    "description": format!("TTS synthesized with {}: {}", model, text_preview),
                },
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
                "provider": "string (required) — openai | elevenlabs | google",
                "model": "string (required)",
                "api_key": "string (required) — supports ${ENV_VAR} + secure-value placeholders",
                "text": "string (required)",
                "voice": "string (required) — provider-specific",
                "format": "string (optional, default mp3) — mp3 | wav | opus | pcm",
                "speed": "float (optional) — openai 0.25-4.0; ignored by elevenlabs"
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Synthesize speech from text via OpenAI, ElevenLabs, or Google Gemini TTS. \
             Returns an attachment handle (attachment_id, url, mime_type, size_bytes, \
             duration_ms).",
        )
    }

    fn default_input(&self) -> Option<&str> {
        Some("text")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::tts::TtsResponse;
    use crate::llm::domain::tts_repository::MockTtsRepository;
    use crate::storage::domain::{MockOutputStorageRepository, StoredOutput};
    use mockall::predicate::always;
    use std::collections::HashMap;

    fn stored_ok() -> StoredOutput {
        StoredOutput {
            storage_key: "k".into(),
            read_url: "data:audio/mpeg;base64,XX".into(),
            mime_type: "audio/mpeg".into(),
            filename: "speech.mp3".into(),
            size_bytes: 2,
        }
    }

    fn audio_resp() -> TtsResponse {
        TtsResponse {
            audio_bytes: vec![1u8, 2, 3, 4],
            mime_type: "audio/mpeg".into(),
            duration_estimate_ms: Some(500),
        }
    }

    fn base_config() -> Value {
        json!({
            "provider": "openai",
            "model": "tts-1",
            "api_key": "sk-test",
            "text": "Hola mundo, esto es una prueba.",
            "voice": "alloy",
        })
    }

    #[tokio::test]
    async fn happy_path_dispatches_to_repo_and_stores_audio() {
        let mut repo = MockTtsRepository::new();
        repo.expect_synthesize()
            .times(1)
            .with(always())
            .returning(|_req| Ok(audio_resp()));
        repo.expect_provider_name().returning(|| "openai");

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .returning(|_| Ok(stored_ok()));

        let node = TtsNode::new(Arc::new(storage)).with_test_repository(Arc::new(repo));

        let out = node
            .execute(&HashMap::new(), &base_config(), &mut json!({}), None)
            .await
            .expect("execute ok");

        assert_eq!(out["output"]["provider"], "openai");
        assert_eq!(out["output"]["model"], "tts-1");
        let audio = &out["output"]["audio"];
        assert_eq!(audio["attachment_id"], "k");
        assert_eq!(audio["mime_type"], "audio/mpeg");
        assert_eq!(audio["duration_ms"], 500);
    }

    #[tokio::test]
    async fn missing_provider_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = TtsNode::new(Arc::new(storage));
        let mut cfg = base_config();
        cfg.as_object_mut().unwrap().remove("provider");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("provider"));
    }

    #[tokio::test]
    async fn provider_can_arrive_via_inputs_for_tool_use() {
        // Simulates tool-execution path: executor passes config={} and merges
        // fixed values into inputs. All infra fields must be readable from inputs.
        let mut repo = MockTtsRepository::new();
        repo.expect_synthesize().returning(|_| Ok(audio_resp()));
        repo.expect_provider_name().returning(|| "openai");

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store().returning(|_| Ok(stored_ok()));

        let node = TtsNode::new(Arc::new(storage)).with_test_repository(Arc::new(repo));

        let config = json!({}); // tool-exec path
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("provider".into(), json!("openai"));
        inputs.insert("model".into(), json!("tts-1"));
        inputs.insert("api_key".into(), json!("sk-test"));
        inputs.insert("text".into(), json!("hola"));
        inputs.insert("voice".into(), json!("alloy"));

        node.execute(&inputs, &config, &mut json!({}), None)
            .await
            .expect("execute ok when all infra fields come via inputs");
    }

    #[tokio::test]
    async fn missing_text_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = TtsNode::new(Arc::new(storage));
        let mut cfg = base_config();
        cfg.as_object_mut().unwrap().remove("text");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("text"));
    }

    #[tokio::test]
    async fn missing_voice_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = TtsNode::new(Arc::new(storage));
        let mut cfg = base_config();
        cfg.as_object_mut().unwrap().remove("voice");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("voice"));
    }

    #[tokio::test]
    async fn inputs_text_overrides_config() {
        let mut repo = MockTtsRepository::new();
        repo.expect_synthesize()
            .times(1)
            .withf(|req: &crate::llm::domain::tts::TtsRequest| req.text == "from inputs")
            .returning(|_| Ok(audio_resp()));
        repo.expect_provider_name().returning(|| "openai");

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store().returning(|_| Ok(stored_ok()));

        let node = TtsNode::new(Arc::new(storage)).with_test_repository(Arc::new(repo));

        // config.text says "from config" but inputs.text overrides it
        let mut cfg = base_config();
        cfg["text"] = json!("from config");
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("text".into(), json!("from inputs"));

        node.execute(&inputs, &cfg, &mut json!({}), None)
            .await
            .expect("execute ok");
    }

    #[tokio::test]
    async fn invalid_format_errors() {
        let storage = MockOutputStorageRepository::new();
        let node = TtsNode::new(Arc::new(storage));
        let mut cfg = base_config();
        cfg["format"] = json!("flac");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown audio format"));
    }

    #[tokio::test]
    async fn unknown_provider_errors_via_factory() {
        // No test repo override → real factory dispatch → unknown provider rejected.
        let storage = MockOutputStorageRepository::new();
        let node = TtsNode::new(Arc::new(storage));
        let mut cfg = base_config();
        cfg["provider"] = json!("nuance");
        let err = node
            .execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown tts provider"));
    }

    #[tokio::test]
    async fn session_ids_forwarded_to_storage() {
        let mut repo = MockTtsRepository::new();
        repo.expect_synthesize().returning(|_| Ok(audio_resp()));
        repo.expect_provider_name().returning(|| "openai");

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .withf(|req| {
                req.session_id.as_deref() == Some("ses_abc")
                    && req.agent_session_id.as_deref() == Some("agent_xyz")
            })
            .returning(|_| Ok(stored_ok()));

        let node = TtsNode::new(Arc::new(storage)).with_test_repository(Arc::new(repo));

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_session_id".into(), json!("ses_abc"));
        inputs.insert("__colmena_agent_session_id".into(), json!("agent_xyz"));

        node.execute(&inputs, &base_config(), &mut json!({}), None)
            .await
            .expect("execute ok");
    }

    #[tokio::test]
    async fn env_var_api_key_resolved() {
        let mut repo = MockTtsRepository::new();
        repo.expect_synthesize().returning(|_| Ok(audio_resp()));
        repo.expect_provider_name().returning(|| "openai");

        let mut storage = MockOutputStorageRepository::new();
        storage.expect_store().returning(|_| Ok(stored_ok()));

        std::env::set_var("__COLMENA_TEST_TTS_KEY__", "sk-from-env");

        let node = TtsNode::new(Arc::new(storage)).with_test_repository(Arc::new(repo));

        let mut cfg = base_config();
        cfg["api_key"] = json!("${__COLMENA_TEST_TTS_KEY__}");
        node.execute(&HashMap::new(), &cfg, &mut json!({}), None)
            .await
            .expect("execute ok");

        std::env::remove_var("__COLMENA_TEST_TTS_KEY__");
    }
}
