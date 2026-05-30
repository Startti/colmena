//! GcsAssetStore — Google Cloud Storage adapter for AssetStore.
//! Feature-flagged behind `gcs`.
//!
//! ## Object layout
//! ```text
//! {prefix}/assets/sessions/{session_id}/{asset_id}/bytes.bin
//! {prefix}/assets/sessions/{session_id}/{asset_id}/meta.json
//! {prefix}/assets/sessions/{session_id}/_manifest.json  ← asset-id list
//! ```
//!
//! ## Design notes
//! The project's GCS SDK (`google-cloud-storage` v1.x) exposes only two data-plane
//! operations on `Storage`: `write_object` and `read_object`. There is no list or
//! delete on that client. This adapter mirrors the `GcsArtifactStore` strategy:
//!
//! * **Listing** uses a per-session `_manifest.json` (last-writer-wins overwrite,
//!   same as the artifact version manifest).
//! * **Deletion** tombstones the bytes object and removes the asset from the
//!   manifest; physical GCS objects are reclaimed via bucket lifecycle rules
//!   (recommend `age > 90 days` for orphans) — identical policy to
//!   `GcsArtifactStore`.
//!
//! No unit tests — matches the existing `GcsArtifactStore` policy.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{AssetId, SessionId};
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use google_cloud_storage::client::Storage;
use serde::{Deserialize, Serialize};

// ── stored types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct StoredMeta {
    session_id: String,
    mime: String,
    size_bytes: u64,
    label: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Default)]
struct SessionManifest {
    #[serde(default)]
    asset_ids: Vec<String>,
}

// ── store ─────────────────────────────────────────────────────────────────────

pub struct GcsAssetStore {
    storage: Storage,
    /// GCS resource path: `projects/_/buckets/{bucket_name}`.
    bucket_path: String,
    /// Logical path prefix within the bucket (no leading/trailing slash).
    prefix: String,
}

impl GcsAssetStore {
    /// `bucket` is the bare bucket name (no `gs://` or `projects/` prefix).
    /// `prefix` is an optional path prefix, e.g. `"colmena/assets"`.
    pub async fn new(bucket: impl Into<String>, prefix: impl Into<String>) -> Result<Self, String> {
        let bucket_name = bucket.into();
        let prefix = prefix.into().trim_matches('/').to_string();
        let storage = Storage::builder()
            .build()
            .await
            .map_err(|e| format!("GCS client build failed: {e}"))?;
        Ok(Self {
            storage,
            bucket_path: format!("projects/_/buckets/{bucket_name}"),
            prefix,
        })
    }

    // ── key helpers ────────────────────────────────────────────────────────────

    fn asset_dir(&self, session: &SessionId, id: &AssetId) -> String {
        if self.prefix.is_empty() {
            format!("assets/sessions/{}/{}", session.as_str(), id.as_str())
        } else {
            format!(
                "{}/assets/sessions/{}/{}",
                self.prefix,
                session.as_str(),
                id.as_str()
            )
        }
    }

    fn bytes_key(&self, session: &SessionId, id: &AssetId) -> String {
        format!("{}/bytes.bin", self.asset_dir(session, id))
    }

    fn meta_key(&self, session: &SessionId, id: &AssetId) -> String {
        format!("{}/meta.json", self.asset_dir(session, id))
    }

    fn manifest_key(&self, session: &SessionId) -> String {
        if self.prefix.is_empty() {
            format!("assets/sessions/{}/_manifest.json", session.as_str())
        } else {
            format!(
                "{}/assets/sessions/{}/_manifest.json",
                self.prefix,
                session.as_str()
            )
        }
    }

    // ── low-level helpers ──────────────────────────────────────────────────────

    /// Write bytes to a GCS object. Returns the new generation number.
    async fn write(&self, key: &str, data: &[u8], content_type: &str) -> Result<i64, AssetError> {
        let object = self
            .storage
            .write_object(&self.bucket_path, key, Bytes::copy_from_slice(data))
            .set_content_type(content_type)
            .send_buffered()
            .await
            .map_err(|e| AssetError::Storage(format!("gcs write {key}: {e}")))?;
        Ok(object.generation)
    }

    async fn read_bytes(&self, key: &str) -> Result<Vec<u8>, AssetError> {
        let result = self
            .storage
            .read_object(&self.bucket_path, key)
            .send()
            .await;
        match result {
            Ok(mut reader) => {
                let mut buf = Vec::new();
                while let Some(chunk) = reader
                    .next()
                    .await
                    .transpose()
                    .map_err(|e| AssetError::Storage(format!("gcs chunk {key}: {e}")))?
                {
                    buf.extend_from_slice(&chunk);
                }
                Ok(buf)
            }
            Err(e) if e.http_status_code() == Some(404) => {
                Err(AssetError::Storage(format!("gcs not found: {key}")))
            }
            Err(e) => Err(AssetError::Storage(format!("gcs read {key}: {e}"))),
        }
    }

    // ── manifest helpers ───────────────────────────────────────────────────────

    async fn read_manifest(&self, session: &SessionId) -> Result<SessionManifest, AssetError> {
        match self.read_bytes(&self.manifest_key(session)).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| AssetError::Storage(format!("parse manifest: {e}"))),
            // 404 → no uploads yet; return empty manifest.
            Err(AssetError::Storage(ref msg)) if msg.contains("gcs not found") => {
                Ok(SessionManifest::default())
            }
            Err(e) => Err(e),
        }
    }

    async fn write_manifest(
        &self,
        session: &SessionId,
        manifest: &SessionManifest,
    ) -> Result<(), AssetError> {
        let bytes = serde_json::to_vec(manifest)
            .map_err(|e| AssetError::Storage(format!("ser manifest: {e}")))?;
        self.write(&self.manifest_key(session), &bytes, "application/json")
            .await?;
        Ok(())
    }

    // ── session scan (needed when session is unknown) ──────────────────────────

    /// Find the session that owns `id` by reading the meta.json blob at a known
    /// path. Because we don't have a list API available from `Storage`, we rely
    /// on the fact that every `upload` call registers the asset in the session
    /// manifest. This helper is therefore only useful in `read`, `delete`, and
    /// `head` where we only know the asset id and need to resolve its session.
    ///
    /// Strategy: we cannot do a prefix scan without `StorageControl`. Instead
    /// we store a global per-asset index key:
    /// `{prefix}/assets/index/{asset_id}` — a tiny JSON with just the
    /// session_id — written at upload time. This lets `find_session_for` do a
    /// single O(1) read.
    fn index_key(&self, id: &AssetId) -> String {
        if self.prefix.is_empty() {
            format!("assets/index/{}", id.as_str())
        } else {
            format!("{}/assets/index/{}", self.prefix, id.as_str())
        }
    }

    async fn find_session_for(&self, id: &AssetId) -> Result<SessionId, AssetError> {
        let bytes = self.read_bytes(&self.index_key(id)).await.map_err(|_| {
            AssetError::NotFound { id: id.clone() }
        })?;
        let session_id: String = serde_json::from_slice(&bytes)
            .map_err(|e| AssetError::Storage(format!("parse index: {e}")))?;
        Ok(SessionId::new(session_id))
    }

    async fn write_index(&self, id: &AssetId, session: &SessionId) -> Result<(), AssetError> {
        let bytes = serde_json::to_vec(session.as_str())
            .map_err(|e| AssetError::Storage(format!("ser index: {e}")))?;
        self.write(&self.index_key(id), &bytes, "application/json")
            .await?;
        Ok(())
    }
}

// ── AssetStore implementation ──────────────────────────────────────────────────

#[async_trait]
impl AssetStore for GcsAssetStore {
    async fn upload(
        &self,
        session: &SessionId,
        id: &AssetId,
        bytes: Vec<u8>,
        mime: &str,
        label: Option<&str>,
    ) -> Result<(), AssetError> {
        let size = bytes.len() as u64;

        // 1. Write the asset bytes.
        self.write(&self.bytes_key(session, id), &bytes, mime)
            .await?;

        // 2. Write the metadata sidecar.
        let meta = StoredMeta {
            session_id: session.as_str().to_string(),
            mime: mime.to_string(),
            size_bytes: size,
            label: label.map(|s| s.to_string()),
            created_at: Utc::now(),
        };
        let meta_bytes = serde_json::to_vec(&meta)
            .map_err(|e| AssetError::Storage(format!("ser meta: {e}")))?;
        self.write(&self.meta_key(session, id), &meta_bytes, "application/json")
            .await?;

        // 3. Write the global reverse-index so find_session_for is O(1).
        self.write_index(id, session).await?;

        // 4. Register in the session manifest (best-effort; last-writer-wins).
        let mut manifest = self.read_manifest(session).await?;
        if !manifest.asset_ids.contains(&id.as_str().to_string()) {
            manifest.asset_ids.push(id.as_str().to_string());
        }
        self.write_manifest(session, &manifest).await?;

        Ok(())
    }

    async fn read(&self, id: &AssetId) -> Result<(Vec<u8>, String), AssetError> {
        let session = self.find_session_for(id).await?;
        let meta_bytes = self
            .read_bytes(&self.meta_key(&session, id))
            .await
            .map_err(|_| AssetError::NotFound { id: id.clone() })?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        let bytes = self
            .read_bytes(&self.bytes_key(&session, id))
            .await
            .map_err(|_| AssetError::NotFound { id: id.clone() })?;
        Ok((bytes, meta.mime))
    }

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError> {
        let manifest = self.read_manifest(session).await?;
        let mut out = Vec::with_capacity(manifest.asset_ids.len());
        for id_str in manifest.asset_ids {
            let id = AssetId::new(&id_str);
            // Skip assets that have been deleted (meta removed).
            let meta_key = self.meta_key(session, &id);
            let meta_bytes = match self.read_bytes(&meta_key).await {
                Ok(b) => b,
                Err(_) => continue, // deleted or corrupt — skip silently
            };
            let meta: StoredMeta = match serde_json::from_slice(&meta_bytes) {
                Ok(m) => m,
                Err(_) => continue,
            };
            out.push(AssetSummary {
                id,
                session_id: SessionId::new(meta.session_id),
                mime: meta.mime,
                size_bytes: meta.size_bytes,
                label: meta.label,
                created_at: meta.created_at,
            });
        }
        Ok(out)
    }

    async fn delete(&self, id: &AssetId) -> Result<(), AssetError> {
        let session = self.find_session_for(id).await?;

        // Tombstone the bytes object (GCS lifecycle rules clean it up later).
        let _ = self
            .write(&self.bytes_key(&session, id), b"DELETED", "text/plain")
            .await;

        // Remove meta so list_by_session skips it.
        let _ = self
            .write(
                &self.meta_key(&session, id),
                b"",
                "application/octet-stream",
            )
            .await;

        // Remove from session manifest.
        let mut manifest = self.read_manifest(&session).await?;
        manifest
            .asset_ids
            .retain(|a| a != id.as_str());
        self.write_manifest(&session, &manifest).await?;

        Ok(())
    }

    async fn head(&self, id: &AssetId) -> Result<AssetSummary, AssetError> {
        let session = self.find_session_for(id).await?;
        let meta_bytes = self
            .read_bytes(&self.meta_key(&session, id))
            .await
            .map_err(|_| AssetError::NotFound { id: id.clone() })?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        Ok(AssetSummary {
            id: id.clone(),
            session_id: session,
            mime: meta.mime,
            size_bytes: meta.size_bytes,
            label: meta.label,
            created_at: meta.created_at,
        })
    }
}
