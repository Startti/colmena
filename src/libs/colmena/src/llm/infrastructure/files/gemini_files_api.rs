//! Files API de Gemini con resumable upload.
//! Tres fases:
//!   1) POST /upload/v1beta/files con headers X-Goog-Upload-* para iniciar.
//!      Respuesta incluye header X-Goog-Upload-URL.
//!   2) PUT(s) sobre upload_url con chunks (X-Goog-Upload-Offset / Command).
//!   3) Último PUT con command "upload, finalize" devuelve el File metadata.

use crate::llm::domain::{
    BoxedByteStream, FileProviderRepository, LlmError, ProviderFileRef, ProviderKind,
};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::{Duration as ChronoDuration, Utc};
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8 MB

/// Adapter for Gemini's Files API using the resumable upload protocol.
///
/// Three phases:
///   1. `POST /upload/v1beta/files` to start a session; response carries
///      `X-Goog-Upload-URL` header.
///   2. One or more `PUT` requests against the session URL with 8 MB chunks.
///   3. Final `PUT` with command `upload, finalize` returns the file metadata.
///
/// Files at Gemini expire 48 hours after upload.
pub struct GeminiFilesApiAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl GeminiFilesApiAdapter {
    /// Creates an adapter pointing at the production Gemini API.
    pub fn new(api_key: String) -> Self {
        Self {
            client: Self::default_client(),
            base_url: "https://generativelanguage.googleapis.com".into(),
            api_key,
        }
    }

    /// Creates an adapter with a custom `base_url` (typically a wiremock
    /// server in tests). Trims trailing `/`.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            client: Self::default_client(),
            base_url,
            api_key,
        }
    }

    fn default_client() -> Client {
        Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(600))
            .user_agent(concat!("colmena/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("default reqwest client should build")
    }

    async fn start_session(&self, mime_type: &str, filename: &str) -> Result<String, LlmError> {
        let url = format!("{}/upload/v1beta/files?key={}", self.base_url, self.api_key);
        let resp = self
            .client
            .post(&url)
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Type", mime_type)
            .header("Content-Type", "application/json")
            .json(&json!({ "file": { "display_name": filename } }))
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "google".into(),
                message: format!("session start failed: {}", e),
            })?;

        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "google".into(),
                message: format!("session start HTTP {}: {}", s, b),
            });
        }

        resp.headers()
            .get("X-Goog-Upload-URL")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::FileApiUploadFailed {
                provider: "google".into(),
                message: "missing X-Goog-Upload-URL header in start response".into(),
            })
    }

    async fn put_chunk(
        &self,
        upload_url: &str,
        offset: u64,
        chunk: Bytes,
        finalize: bool,
    ) -> Result<Option<UploadFinalizeResponse>, LlmError> {
        let cmd = if finalize {
            "upload, finalize"
        } else {
            "upload"
        };
        let resp = self
            .client
            .put(upload_url)
            .header("X-Goog-Upload-Offset", offset.to_string())
            .header("X-Goog-Upload-Command", cmd)
            .header("Content-Length", chunk.len().to_string())
            .body(chunk)
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "google".into(),
                message: format!("PUT chunk failed at offset {}: {}", offset, e),
            })?;

        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "google".into(),
                message: format!("PUT chunk HTTP {}: {}", s, b),
            });
        }

        if finalize {
            let parsed: UploadFinalizeResponse =
                resp.json()
                    .await
                    .map_err(|e| LlmError::FileApiUploadFailed {
                        provider: "google".into(),
                        message: format!("invalid finalize JSON: {}", e),
                    })?;
            Ok(Some(parsed))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Deserialize)]
struct UploadFinalizeResponse {
    file: GeminiFile,
}

#[derive(Debug, Deserialize)]
struct GeminiFile {
    /// Forma "files/abc123".
    name: String,
}

#[async_trait]
impl FileProviderRepository for GeminiFilesApiAdapter {
    async fn upload_streaming(
        &self,
        mut stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        // 1. Iniciar sesión.
        let upload_url = self.start_session(mime_type, filename).await?;

        // 2. Subir chunks.
        let mut buffer = BytesMut::with_capacity(CHUNK_SIZE);
        let mut offset: u64 = 0;
        let mut finalize_response: Option<UploadFinalizeResponse> = None;
        let mut stream_done = false;

        while !stream_done {
            // Acumular hasta CHUNK_SIZE o fin de stream.
            while buffer.len() < CHUNK_SIZE {
                match stream.next().await {
                    Some(Ok(b)) => buffer.extend_from_slice(&b),
                    Some(Err(e)) => {
                        return Err(LlmError::FileApiUploadFailed {
                            provider: "google".into(),
                            message: format!("stream read error: {}", e),
                        });
                    }
                    None => {
                        stream_done = true;
                        break;
                    }
                }
            }

            // Gemini requires non-final chunks to be EXACTLY multiples of CHUNK_SIZE.
            // If the buffer has more than CHUNK_SIZE (because the last extend pushed
            // it over) and the stream is still open, take only CHUNK_SIZE and leave
            // the rest for the next iteration. If the stream is done, take the whole
            // remaining buffer as the final (finalize) chunk — its size can be anything.
            let chunk_bytes = if !stream_done && buffer.len() >= CHUNK_SIZE {
                buffer.split_to(CHUNK_SIZE).freeze()
            } else {
                buffer.split().freeze()
            };
            let chunk_len = chunk_bytes.len() as u64;
            let is_last = stream_done && buffer.is_empty();

            let result = self
                .put_chunk(&upload_url, offset, chunk_bytes, is_last)
                .await?;

            if is_last {
                finalize_response = result;
            }
            offset += chunk_len;
        }

        let resp = finalize_response.ok_or_else(|| LlmError::FileApiUploadFailed {
            provider: "google".into(),
            message: "finalize response missing".into(),
        })?;

        // file.name viene como "files/abc123".
        let file_uri = format!("{}/v1beta/{}", self.base_url, resp.file.name);

        Ok(ProviderFileRef {
            provider: ProviderKind::Google,
            provider_file_id: file_uri,
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
            expires_at: Some(Utc::now() + ChronoDuration::hours(48)),
        })
    }

    fn ttl(&self) -> Option<Duration> {
        Some(Duration::from_secs(48 * 3600))
    }

    fn provider(&self) -> ProviderKind {
        ProviderKind::Google
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use serde_json::json;
    use wiremock::matchers::{header_regex, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_stream(content: &[u8]) -> BoxedByteStream {
        let bytes = Bytes::copy_from_slice(content);
        Box::pin(stream::iter(vec![Ok::<_, std::io::Error>(bytes)]))
    }

    #[tokio::test]
    async fn small_file_one_chunk_uploads() {
        let server = MockServer::start().await;

        // 1. Init session, mock returns upload URL.
        let upload_path = "/upload-session-xyz";
        let upload_url = format!("{}{}", server.uri(), upload_path);
        Mock::given(method("POST"))
            .and(path("/upload/v1beta/files"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Goog-Upload-URL", upload_url.as_str())
                    .set_body_string(""),
            )
            .mount(&server)
            .await;

        // 2. Finalize PUT returns file metadata.
        Mock::given(method("PUT"))
            .and(path(upload_path))
            .and(header_regex("X-Goog-Upload-Command", r"upload,\s*finalize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "file": { "name": "files/abcd1234" }
            })))
            .mount(&server)
            .await;

        let adapter = GeminiFilesApiAdapter::with_base_url("KEY".into(), server.uri());
        let r = adapter
            .upload_streaming(fake_stream(b"hello world"), "application/pdf", "x.pdf")
            .await
            .unwrap();
        assert!(r.provider_file_id.contains("files/abcd1234"));
        assert_eq!(r.provider, ProviderKind::Google);
        assert!(r.expires_at.is_some());
    }

    #[tokio::test]
    async fn session_start_failure_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/v1beta/files"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let adapter = GeminiFilesApiAdapter::with_base_url("KEY".into(), server.uri());
        let err = match adapter
            .upload_streaming(fake_stream(b"x"), "application/pdf", "x.pdf")
            .await
        {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        assert!(matches!(err, LlmError::FileApiUploadFailed { .. }));
    }

    #[test]
    fn ttl_is_48h() {
        let a = GeminiFilesApiAdapter::new("k".into());
        assert_eq!(a.ttl(), Some(Duration::from_secs(48 * 3600)));
    }
}
