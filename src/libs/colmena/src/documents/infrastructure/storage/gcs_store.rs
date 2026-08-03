//! GCS-backed `ArtifactStore`.
//!
//! Uses only the two operations that `google-cloud-storage` v1.x exposes publicly:
//! `write_object` and `read_object`. Object deletion is handled by GCS lifecycle
//! policies configured on the bucket (recommend `age > 90` days for orphans).
//!
//! ## Object layout
//! ```text
//! {prefix}/artifacts/{id}/meta.json
//! {prefix}/artifacts/{id}/HEAD
//! {prefix}/artifacts/{id}/_manifest.json   ← version list
//! {prefix}/artifacts/{id}/versions/{vid}/ir.json
//! {prefix}/artifacts/{id}/versions/{vid}/render.{ext}
//! {prefix}/artifacts/{id}/versions/{vid}/patch_applied.json
//! {prefix}/artifacts/{id}/versions/{vid}/blobs/{name}
//! {prefix}/artifacts/{id}/versions/{vid}/blobs.json  ← blob name index
//! ```
//!
//! `blobs.json` lists the blob names persisted for a version so
//! `read_version` can reconstruct them without a list-objects HTTP call
//! (not exposed by this adapter — see module doc above). Only written when
//! a version has at least one blob.
//!
//! ## CAS for `set_head`
//! HEAD is written with `set_if_generation_match`. First-time write uses
//! `generation=0` (create-only). Subsequent writes read HEAD first to obtain
//! the current generation, then write with that generation as the precondition.

use crate::documents::domain::artifact::{ArtifactMeta, PatchApplied, VersionData};
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::ports::ArtifactStore;
use crate::documents::domain::StorageError;
use async_trait::async_trait;
use bytes::Bytes;
use google_cloud_storage::client::Storage;
use serde::{Deserialize, Serialize};

// ── manifest type ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct VersionManifest {
    #[serde(default)]
    versions: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct BlobIndex {
    #[serde(default)]
    names: Vec<String>,
}

// ── store ─────────────────────────────────────────────────────────────────────

pub struct GcsArtifactStore {
    storage: Storage,
    /// GCS resource path: `projects/_/buckets/{bucket_name}`.
    bucket_path: String,
    /// Logical path prefix within the bucket (no leading/trailing slash).
    prefix: String,
}

impl GcsArtifactStore {
    /// `bucket` is the bare bucket name (no `gs://` or `projects/` prefix).
    /// `prefix` is an optional path prefix, e.g. `"colmena/documents"`.
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

    fn art_prefix(&self, id: &ArtifactId) -> String {
        if self.prefix.is_empty() {
            format!("artifacts/{}", id.0)
        } else {
            format!("{}/artifacts/{}", self.prefix, id.0)
        }
    }

    fn meta_key(&self, id: &ArtifactId) -> String {
        format!("{}/meta.json", self.art_prefix(id))
    }

    fn head_key(&self, id: &ArtifactId) -> String {
        format!("{}/HEAD", self.art_prefix(id))
    }

    fn manifest_key(&self, id: &ArtifactId) -> String {
        format!("{}/._manifest.json", self.art_prefix(id))
    }

    fn ir_key(&self, id: &ArtifactId, v: &VersionId) -> String {
        format!("{}/versions/{}/ir.json", self.art_prefix(id), v.0)
    }

    fn render_key(&self, id: &ArtifactId, v: &VersionId, ext: &str) -> String {
        format!("{}/versions/{}/render.{ext}", self.art_prefix(id), v.0)
    }

    fn patch_key(&self, id: &ArtifactId, v: &VersionId) -> String {
        format!(
            "{}/versions/{}/patch_applied.json",
            self.art_prefix(id),
            v.0
        )
    }

    fn blob_key(&self, id: &ArtifactId, v: &VersionId, name: &str) -> String {
        format!("{}/versions/{}/blobs/{name}", self.art_prefix(id), v.0)
    }

    fn blobs_index_key(&self, id: &ArtifactId, v: &VersionId) -> String {
        format!("{}/versions/{}/blobs.json", self.art_prefix(id), v.0)
    }

    // ── low-level helpers ──────────────────────────────────────────────────────

    async fn write(&self, key: &str, data: &[u8], content_type: &str) -> Result<i64, StorageError> {
        let object = self
            .storage
            .write_object(&self.bucket_path, key, Bytes::copy_from_slice(data))
            .set_content_type(content_type)
            .send_buffered()
            .await
            .map_err(|e| StorageError::Backend(format!("gcs write {key}: {e}")))?;
        Ok(object.generation)
    }

    async fn write_create_only(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<i64, StorageError> {
        let result = self
            .storage
            .write_object(&self.bucket_path, key, Bytes::copy_from_slice(data))
            .set_content_type(content_type)
            .set_if_generation_match(0_i64)
            .send_buffered()
            .await;
        match result {
            Ok(obj) => Ok(obj.generation),
            Err(e) if e.http_status_code() == Some(412) => Err(StorageError::PreconditionFailed(
                format!("object already exists: {key}"),
            )),
            Err(e) => Err(StorageError::Backend(format!(
                "gcs write_create {key}: {e}"
            ))),
        }
    }

    async fn write_cas(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
        expected_gen: i64,
    ) -> Result<i64, StorageError> {
        let result = self
            .storage
            .write_object(&self.bucket_path, key, Bytes::copy_from_slice(data))
            .set_content_type(content_type)
            .set_if_generation_match(expected_gen)
            .send_buffered()
            .await;
        match result {
            Ok(obj) => Ok(obj.generation),
            Err(e) if e.http_status_code() == Some(412) => Err(StorageError::PreconditionFailed(
                format!("CAS failed for {key}: expected gen {expected_gen}"),
            )),
            Err(e) => Err(StorageError::Backend(format!("gcs write_cas {key}: {e}"))),
        }
    }

    async fn read_bytes(&self, key: &str) -> Result<Vec<u8>, StorageError> {
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
                    .map_err(|e| StorageError::Backend(format!("gcs chunk {key}: {e}")))?
                {
                    buf.extend_from_slice(&chunk);
                }
                Ok(buf)
            }
            Err(e) if e.http_status_code() == Some(404) => {
                Err(StorageError::NotFound(key.to_string()))
            }
            Err(e) => Err(StorageError::Backend(format!("gcs read {key}: {e}"))),
        }
    }

    /// Read bytes and return the GCS generation alongside them (needed for CAS).
    async fn read_bytes_with_gen(&self, key: &str) -> Result<(Vec<u8>, i64), StorageError> {
        let result = self
            .storage
            .read_object(&self.bucket_path, key)
            .send()
            .await;
        match result {
            Ok(mut reader) => {
                let gen = reader.object().generation;
                let mut buf = Vec::new();
                while let Some(chunk) = reader
                    .next()
                    .await
                    .transpose()
                    .map_err(|e| StorageError::Backend(format!("gcs chunk {key}: {e}")))?
                {
                    buf.extend_from_slice(&chunk);
                }
                Ok((buf, gen))
            }
            Err(e) if e.http_status_code() == Some(404) => {
                Err(StorageError::NotFound(key.to_string()))
            }
            Err(e) => Err(StorageError::Backend(format!("gcs read {key}: {e}"))),
        }
    }

    // ── manifest helpers ───────────────────────────────────────────────────────

    async fn read_manifest(&self, id: &ArtifactId) -> Result<VersionManifest, StorageError> {
        match self.read_bytes(&self.manifest_key(id)).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| StorageError::Backend(format!("parse manifest: {e}"))),
            Err(StorageError::NotFound(_)) => Ok(VersionManifest::default()),
            Err(e) => Err(e),
        }
    }

    async fn write_manifest(
        &self,
        id: &ArtifactId,
        manifest: &VersionManifest,
    ) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec(manifest)
            .map_err(|e| StorageError::Backend(format!("ser manifest: {e}")))?;
        self.write(&self.manifest_key(id), &bytes, "application/json")
            .await?;
        Ok(())
    }

    // ── blob index helpers ─────────────────────────────────────────────────────

    /// Reads all blobs for a version via the persisted blob-index manifest.
    /// A missing index (NotFound) means the version was written with no
    /// blobs and returns an empty vec, not an error. A blob named in the
    /// index but missing on read is an integrity error and propagates.
    /// Results are sorted by name for deterministic order (mirrors
    /// local_fs_store).
    async fn read_blobs(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<Vec<(String, Vec<u8>)>, StorageError> {
        let index: BlobIndex = match self.read_bytes(&self.blobs_index_key(id, version)).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| StorageError::Backend(format!("parse blob index: {e}")))?,
            Err(StorageError::NotFound(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::with_capacity(index.names.len());
        for name in index.names {
            let bytes = self.read_bytes(&self.blob_key(id, version, &name)).await?;
            out.push((name, bytes));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

// ── ArtifactStore implementation ───────────────────────────────────────────────

#[async_trait]
impl ArtifactStore for GcsArtifactStore {
    async fn create_artifact(&self, meta: &ArtifactMeta) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| StorageError::Backend(format!("ser meta: {e}")))?;
        self.write(
            &self.meta_key(&meta.artifact_id),
            &bytes,
            "application/json",
        )
        .await?;
        // Empty initial manifest.
        self.write_manifest(&meta.artifact_id, &VersionManifest::default())
            .await?;
        Ok(())
    }

    async fn write_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        data: &VersionData,
    ) -> Result<(), StorageError> {
        // Write all version objects.
        let ir_bytes = serde_json::to_vec_pretty(&data.ir)
            .map_err(|e| StorageError::Backend(format!("ser ir: {e}")))?;
        self.write(&self.ir_key(id, version), &ir_bytes, "application/json")
            .await?;

        let mime = if data.rendered_extension == "xlsx" {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        } else {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        };
        self.write(
            &self.render_key(id, version, data.rendered_extension),
            &data.rendered_binary,
            mime,
        )
        .await?;

        let patch_bytes = serde_json::to_vec_pretty(&data.patch_applied)
            .map_err(|e| StorageError::Backend(format!("ser patch: {e}")))?;
        self.write(
            &self.patch_key(id, version),
            &patch_bytes,
            "application/json",
        )
        .await?;

        for (name, blob_bytes) in &data.blobs {
            self.write(
                &self.blob_key(id, version, name),
                blob_bytes,
                "application/octet-stream",
            )
            .await?;
        }

        if !data.blobs.is_empty() {
            let index = BlobIndex {
                names: data.blobs.iter().map(|(name, _)| name.clone()).collect(),
            };
            let index_bytes = serde_json::to_vec(&index)
                .map_err(|e| StorageError::Backend(format!("ser blob index: {e}")))?;
            self.write(
                &self.blobs_index_key(id, version),
                &index_bytes,
                "application/json",
            )
            .await?;
        }

        // Register version in manifest (best-effort; overwrite is last-writer-wins).
        let mut manifest = self.read_manifest(id).await?;
        if !manifest.versions.contains(&version.0) {
            manifest.versions.push(version.0.clone());
        }
        self.write_manifest(id, &manifest).await?;
        Ok(())
    }

    async fn read_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<VersionData, StorageError> {
        let ir_bytes = self.read_bytes(&self.ir_key(id, version)).await?;
        let ir: serde_json::Value = serde_json::from_slice(&ir_bytes)
            .map_err(|e| StorageError::Backend(format!("parse ir: {e}")))?;

        let meta = self.read_meta(id).await?;
        let ext = meta.kind.extension();
        let render = self.read_bytes(&self.render_key(id, version, ext)).await?;

        let pa_bytes = self.read_bytes(&self.patch_key(id, version)).await?;
        let patch_applied: PatchApplied = serde_json::from_slice(&pa_bytes)
            .map_err(|e| StorageError::Backend(format!("parse patch: {e}")))?;

        let blobs = self.read_blobs(id, version).await?;

        Ok(VersionData {
            ir,
            rendered_binary: render,
            rendered_extension: match meta.kind {
                crate::documents::domain::ArtifactKind::Excel => "xlsx",
                crate::documents::domain::ArtifactKind::Word => "docx",
                crate::documents::domain::ArtifactKind::Html => "html",
            },
            patch_applied,
            blobs,
        })
    }

    async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError> {
        let head_bytes = self.read_bytes(&self.head_key(id)).await?;
        let v = VersionId::new(std::str::from_utf8(&head_bytes).unwrap_or("").trim());
        self.read_version(id, &v).await
    }

    async fn list_versions(&self, id: &ArtifactId) -> Result<Vec<VersionId>, StorageError> {
        let manifest = self.read_manifest(id).await?;
        let mut versions: Vec<VersionId> = manifest
            .versions
            .into_iter()
            .map(|s| VersionId::new(&s))
            .collect();
        versions.sort_by_key(|v| v.number().unwrap_or(0));
        Ok(versions)
    }

    async fn set_head(
        &self,
        id: &ArtifactId,
        expected_current: Option<&VersionId>,
        new: &VersionId,
    ) -> Result<(), StorageError> {
        let key = self.head_key(id);
        let data = new.0.as_bytes();
        match expected_current {
            None => {
                // HEAD doesn't exist yet — create-only write.
                self.write_create_only(&key, data, "text/plain").await?;
            }
            Some(expected) => {
                // Read HEAD to verify content and get generation for CAS.
                let (cur_bytes, gen) = self.read_bytes_with_gen(&key).await?;
                let cur = std::str::from_utf8(&cur_bytes).unwrap_or("").trim();
                if cur != expected.0 {
                    return Err(StorageError::PreconditionFailed(format!(
                        "HEAD is {cur}, expected {}",
                        expected.0
                    )));
                }
                self.write_cas(&key, data, "text/plain", gen).await?;
            }
        }
        Ok(())
    }

    async fn delete_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<(), StorageError> {
        // Remove from manifest. GCS objects are left in place; configure a
        // GCS lifecycle rule (e.g. age > 90 days) to reclaim storage.
        let mut manifest = self.read_manifest(id).await?;
        manifest.versions.retain(|v| v != &version.0);
        self.write_manifest(id, &manifest).await?;
        Ok(())
    }

    async fn read_meta(&self, id: &ArtifactId) -> Result<ArtifactMeta, StorageError> {
        let bytes = self.read_bytes(&self.meta_key(id)).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| StorageError::Backend(format!("parse meta: {e}")))
    }

    async fn update_meta(&self, id: &ArtifactId, meta: &ArtifactMeta) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| StorageError::Backend(format!("ser meta: {e}")))?;
        self.write(&self.meta_key(id), &bytes, "application/json")
            .await?;
        Ok(())
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<(), StorageError> {
        // Overwrite HEAD with a tombstone and clear the manifest.
        // Actual GCS objects are left; configure lifecycle rules for cleanup.
        let tombstone = b"DELETED";
        let _ = self
            .write(&self.head_key(id), tombstone, "text/plain")
            .await;
        let _ = self.write_manifest(id, &VersionManifest::default()).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::artifact::PatchSummary;
    use crate::documents::domain::ids::SessionId;
    use crate::documents::domain::ArtifactKind;

    /// Builds a store against `COLMENA_TEST_GCS_BUCKET` (optionally scoped by
    /// `COLMENA_TEST_GCS_PREFIX`, defaulting to a nonce path so repeated runs
    /// don't collide). Returns `None` when the env var is unset so the test
    /// can skip cleanly if run without `--ignored` gating removed by mistake.
    async fn test_store() -> Option<(GcsArtifactStore, String)> {
        let bucket = std::env::var("COLMENA_TEST_GCS_BUCKET").ok()?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("colmena-test/blob-roundtrip-{nonce}");
        let store = GcsArtifactStore::new(bucket, prefix.clone())
            .await
            .expect("build GcsArtifactStore");
        Some((store, prefix))
    }

    fn sample_version_data() -> VersionData {
        VersionData {
            ir: serde_json::json!({"kind": "excel"}),
            rendered_binary: vec![1, 2, 3],
            rendered_extension: "xlsx",
            patch_applied: PatchApplied {
                patch: serde_json::json!({}),
                applied_at: chrono::Utc::now(),
                resulted_in: VersionId::initial(),
                summary: PatchSummary::default(),
            },
            blobs: vec![],
        }
    }

    #[tokio::test]
    #[ignore = "requires GCS bucket — run with `cargo test -- --ignored`"]
    async fn gcs_read_version_returns_written_blobs() {
        let Some((store, _prefix)) = test_store().await else {
            eprintln!("skip: COLMENA_TEST_GCS_BUCKET not set");
            return;
        };
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        store.create_artifact(&meta).await.unwrap();
        let bytes_a = vec![10u8, 20, 30];
        let bytes_b = vec![0u8, 255, 128, 64];
        let mut data = sample_version_data();
        data.blobs = vec![
            ("a.bin".to_string(), bytes_a.clone()),
            ("b.png".to_string(), bytes_b.clone()),
        ];
        store
            .write_version(&id, &VersionId::initial(), &data)
            .await
            .unwrap();

        let read = store
            .read_version(&id, &VersionId::initial())
            .await
            .unwrap();

        let mut expected: Vec<(String, Vec<u8>)> = vec![
            ("a.bin".to_string(), bytes_a),
            ("b.png".to_string(), bytes_b),
        ];
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        let mut actual = read.blobs.clone();
        actual.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    #[ignore = "requires GCS bucket — run with `cargo test -- --ignored`"]
    async fn gcs_read_version_empty_blobs_stays_empty() {
        let Some((store, _prefix)) = test_store().await else {
            eprintln!("skip: COLMENA_TEST_GCS_BUCKET not set");
            return;
        };
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        store.create_artifact(&meta).await.unwrap();
        let data = sample_version_data();
        assert!(data.blobs.is_empty());
        store
            .write_version(&id, &VersionId::initial(), &data)
            .await
            .unwrap();

        let read = store
            .read_version(&id, &VersionId::initial())
            .await
            .unwrap();
        assert_eq!(read.blobs, Vec::<(String, Vec<u8>)>::new());
    }
}
