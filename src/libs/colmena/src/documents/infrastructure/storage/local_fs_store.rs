use crate::documents::domain::artifact::{ArtifactMeta, PatchApplied, VersionData};
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::ports::ArtifactStore;
use crate::documents::domain::StorageError;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs;

pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn art_dir(&self, id: &ArtifactId) -> PathBuf {
        self.root.join("artifacts").join(&id.0)
    }
    fn meta_path(&self, id: &ArtifactId) -> PathBuf {
        self.art_dir(id).join("meta.json")
    }
    fn head_path(&self, id: &ArtifactId) -> PathBuf {
        self.art_dir(id).join("HEAD")
    }
    fn version_dir(&self, id: &ArtifactId, v: &VersionId) -> PathBuf {
        self.art_dir(id).join("versions").join(&v.0)
    }

    async fn atomic_write(path: &Path, data: &[u8]) -> Result<(), StorageError> {
        let parent = path
            .parent()
            .ok_or_else(|| StorageError::Backend(format!("no parent: {}", path.display())))?;
        fs::create_dir_all(parent)
            .await
            .map_err(|e| StorageError::Backend(format!("mkdir: {e}")))?;
        let tmp = path.with_extension(format!(
            "{}.tmp",
            path.extension().and_then(|e| e.to_str()).unwrap_or("part")
        ));
        fs::write(&tmp, data)
            .await
            .map_err(|e| StorageError::Backend(format!("write tmp: {e}")))?;
        fs::rename(&tmp, path)
            .await
            .map_err(|e| StorageError::Backend(format!("rename: {e}")))?;
        Ok(())
    }

    /// Reads all blob files under `vdir/blobs/`. A missing `blobs/` directory
    /// is not an error — it means the version was written with no blobs.
    /// Non-file entries are skipped. Bytes are read binary-safe (no UTF-8
    /// coercion). Results are sorted by name for deterministic read order.
    async fn read_blobs(vdir: &Path) -> Result<Vec<(String, Vec<u8>)>, StorageError> {
        let blobs_dir = vdir.join("blobs");
        if !blobs_dir.exists() {
            return Ok(Vec::new());
        }
        let mut rd = fs::read_dir(&blobs_dir)
            .await
            .map_err(|e| StorageError::Backend(format!("readdir blobs: {e}")))?;
        let mut out = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| StorageError::Backend(format!("readdir blobs entry: {e}")))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| StorageError::Backend(format!("blob file_type: {e}")))?;
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let bytes = fs::read(entry.path())
                .await
                .map_err(|e| StorageError::Backend(format!("read blob {name}: {e}")))?;
            out.push((name, bytes));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

#[async_trait]
impl ArtifactStore for LocalFsStore {
    async fn create_artifact(&self, meta: &ArtifactMeta) -> Result<(), StorageError> {
        fs::create_dir_all(self.art_dir(&meta.artifact_id))
            .await
            .map_err(|e| StorageError::Backend(format!("mkdir: {e}")))?;
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| StorageError::Backend(format!("ser meta: {e}")))?;
        Self::atomic_write(&self.meta_path(&meta.artifact_id), &bytes).await?;
        Ok(())
    }

    async fn write_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        data: &VersionData,
    ) -> Result<(), StorageError> {
        let vdir = self.version_dir(id, version);
        fs::create_dir_all(&vdir)
            .await
            .map_err(|e| StorageError::Backend(format!("mkdir: {e}")))?;
        let ir_bytes = serde_json::to_vec_pretty(&data.ir)
            .map_err(|e| StorageError::Backend(format!("ser ir: {e}")))?;
        Self::atomic_write(&vdir.join("ir.json"), &ir_bytes).await?;
        let render_name = format!("render.{}", data.rendered_extension);
        Self::atomic_write(&vdir.join(&render_name), &data.rendered_binary).await?;
        let patch_bytes = serde_json::to_vec_pretty(&data.patch_applied)
            .map_err(|e| StorageError::Backend(format!("ser patch: {e}")))?;
        Self::atomic_write(&vdir.join("patch_applied.json"), &patch_bytes).await?;
        if !data.blobs.is_empty() {
            let blobs_dir = vdir.join("blobs");
            fs::create_dir_all(&blobs_dir)
                .await
                .map_err(|e| StorageError::Backend(format!("mkdir blobs: {e}")))?;
            for (name, bytes) in &data.blobs {
                Self::atomic_write(&blobs_dir.join(name), bytes).await?;
            }
        }
        Ok(())
    }

    async fn read_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<VersionData, StorageError> {
        let vdir = self.version_dir(id, version);
        if !vdir.exists() {
            return Err(StorageError::NotFound(format!("{}/{}", id.0, version.0)));
        }
        let ir_bytes = fs::read(vdir.join("ir.json"))
            .await
            .map_err(|e| StorageError::Backend(format!("read ir: {e}")))?;
        let ir: serde_json::Value = serde_json::from_slice(&ir_bytes)
            .map_err(|e| StorageError::Backend(format!("parse ir: {e}")))?;

        let meta = self.read_meta(id).await?;
        let ext = meta.kind.extension();
        let render = fs::read(vdir.join(format!("render.{ext}")))
            .await
            .map_err(|e| StorageError::Backend(format!("read render: {e}")))?;

        let pa_bytes = fs::read(vdir.join("patch_applied.json"))
            .await
            .map_err(|e| StorageError::Backend(format!("read patch: {e}")))?;
        let patch_applied: PatchApplied = serde_json::from_slice(&pa_bytes)
            .map_err(|e| StorageError::Backend(format!("parse patch: {e}")))?;

        let blobs = Self::read_blobs(&vdir).await?;

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
        let head = fs::read_to_string(self.head_path(id))
            .await
            .map_err(|e| StorageError::Backend(format!("read HEAD: {e}")))?;
        let v = VersionId::new(head.trim());
        self.read_version(id, &v).await
    }

    async fn list_versions(&self, id: &ArtifactId) -> Result<Vec<VersionId>, StorageError> {
        let vers_dir = self.art_dir(id).join("versions");
        if !vers_dir.exists() {
            return Ok(vec![]);
        }
        let mut rd = fs::read_dir(&vers_dir)
            .await
            .map_err(|e| StorageError::Backend(format!("readdir: {e}")))?;
        let mut out = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| StorageError::Backend(format!("readdir entry: {e}")))?
        {
            if let Some(name) = entry.file_name().to_str() {
                out.push(VersionId::new(name));
            }
        }
        out.sort_by_key(|v| v.number().unwrap_or(0));
        Ok(out)
    }

    async fn set_head(
        &self,
        id: &ArtifactId,
        expected_current: Option<&VersionId>,
        new: &VersionId,
    ) -> Result<(), StorageError> {
        let head_path = self.head_path(id);
        if let Some(expected) = expected_current {
            let cur = fs::read_to_string(&head_path).await.unwrap_or_default();
            if cur.trim() != expected.0 {
                return Err(StorageError::PreconditionFailed(format!(
                    "HEAD is {}, expected {}",
                    cur.trim(),
                    expected.0
                )));
            }
        }
        Self::atomic_write(&head_path, new.0.as_bytes()).await?;
        Ok(())
    }

    async fn delete_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<(), StorageError> {
        let vdir = self.version_dir(id, version);
        if vdir.exists() {
            fs::remove_dir_all(&vdir)
                .await
                .map_err(|e| StorageError::Backend(format!("rmdir: {e}")))?;
        }
        Ok(())
    }

    async fn read_meta(&self, id: &ArtifactId) -> Result<ArtifactMeta, StorageError> {
        let path = self.meta_path(id);
        if !path.exists() {
            return Err(StorageError::NotFound(id.0.clone()));
        }
        let bytes = fs::read(&path)
            .await
            .map_err(|e| StorageError::Backend(format!("read meta: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| StorageError::Backend(format!("parse meta: {e}")))
    }

    async fn update_meta(&self, id: &ArtifactId, meta: &ArtifactMeta) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(meta)
            .map_err(|e| StorageError::Backend(format!("ser meta: {e}")))?;
        Self::atomic_write(&self.meta_path(id), &bytes).await?;
        Ok(())
    }

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<(), StorageError> {
        let dir = self.art_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .await
                .map_err(|e| StorageError::Backend(format!("rmdir art: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::artifact::PatchSummary;
    use crate::documents::domain::ids::SessionId;
    use crate::documents::domain::ArtifactKind;
    use tempfile::tempdir;

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
    async fn create_write_read_cycle() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let meta = ArtifactMeta::initial(
            ArtifactId::new("art_01"),
            ArtifactKind::Excel,
            SessionId::new("sess_1"),
            "t".into(),
            10,
        );
        s.create_artifact(&meta).await.unwrap();
        s.write_version(
            &meta.artifact_id,
            &VersionId::initial(),
            &sample_version_data(),
        )
        .await
        .unwrap();
        s.set_head(&meta.artifact_id, None, &VersionId::initial())
            .await
            .unwrap();

        let current = s.read_current(&meta.artifact_id).await.unwrap();
        assert_eq!(current.rendered_binary, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn set_head_precondition_mismatch_fails() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        s.create_artifact(&meta).await.unwrap();
        s.set_head(&id, None, &VersionId::new("v1")).await.unwrap();
        let err = s
            .set_head(&id, Some(&VersionId::new("v999")), &VersionId::new("v2"))
            .await;
        assert!(matches!(err, Err(StorageError::PreconditionFailed(_))));
    }

    #[tokio::test]
    async fn read_version_returns_written_blobs() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        s.create_artifact(&meta).await.unwrap();
        let bytes_a = vec![10u8, 20, 30];
        let bytes_b = vec![0u8, 255, 128, 64];
        let mut data = sample_version_data();
        data.blobs = vec![
            ("a.bin".to_string(), bytes_a.clone()),
            ("b.png".to_string(), bytes_b.clone()),
        ];
        s.write_version(&id, &VersionId::initial(), &data)
            .await
            .unwrap();

        let read = s.read_version(&id, &VersionId::initial()).await.unwrap();

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
    async fn read_current_returns_written_blobs() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        s.create_artifact(&meta).await.unwrap();
        let bytes_a = vec![10u8, 20, 30];
        let bytes_b = vec![0u8, 255, 128, 64];
        let mut data = sample_version_data();
        data.blobs = vec![
            ("a.bin".to_string(), bytes_a.clone()),
            ("b.png".to_string(), bytes_b.clone()),
        ];
        s.write_version(&id, &VersionId::initial(), &data)
            .await
            .unwrap();
        s.set_head(&id, None, &VersionId::initial()).await.unwrap();

        let current = s.read_current(&id).await.unwrap();

        let mut expected: Vec<(String, Vec<u8>)> = vec![
            ("a.bin".to_string(), bytes_a),
            ("b.png".to_string(), bytes_b),
        ];
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        let mut actual = current.blobs.clone();
        actual.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn read_version_empty_blobs_stays_empty() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        s.create_artifact(&meta).await.unwrap();
        let data = sample_version_data();
        assert!(data.blobs.is_empty());
        s.write_version(&id, &VersionId::initial(), &data)
            .await
            .unwrap();

        let read = s.read_version(&id, &VersionId::initial()).await.unwrap();
        assert_eq!(read.blobs, Vec::<(String, Vec<u8>)>::new());
    }

    #[tokio::test]
    async fn read_version_non_blob_fields_unchanged() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        s.create_artifact(&meta).await.unwrap();
        let mut data = sample_version_data();
        data.ir = serde_json::json!({"kind": "excel", "sheets": ["Sheet1"]});
        data.rendered_binary = vec![9, 8, 7, 6];
        data.patch_applied.summary = PatchSummary {
            natural_language: vec!["Updated 3 cells".to_string()],
            structured: vec![serde_json::json!({"op": "set_cell"})],
        };
        data.blobs = vec![("a.bin".to_string(), vec![1, 2, 3])];

        s.write_version(&id, &VersionId::initial(), &data)
            .await
            .unwrap();
        let read = s.read_version(&id, &VersionId::initial()).await.unwrap();

        assert_eq!(read.ir, data.ir);
        assert_eq!(read.rendered_binary, data.rendered_binary);
        assert_eq!(read.rendered_extension, data.rendered_extension);
        assert_eq!(
            read.patch_applied.summary.natural_language,
            data.patch_applied.summary.natural_language
        );
        assert_eq!(
            read.patch_applied.summary.structured,
            data.patch_applied.summary.structured
        );
        assert_eq!(read.patch_applied.patch, data.patch_applied.patch);
    }

    #[tokio::test]
    async fn list_versions_sorted() {
        let tmp = tempdir().unwrap();
        let s = LocalFsStore::new(tmp.path());
        let id = ArtifactId::new("art_01");
        let meta = ArtifactMeta::initial(
            id.clone(),
            ArtifactKind::Excel,
            SessionId::new("s"),
            "t".into(),
            5,
        );
        s.create_artifact(&meta).await.unwrap();
        for v in &["v1", "v2", "v10"] {
            s.write_version(&id, &VersionId::new(*v), &sample_version_data())
                .await
                .unwrap();
        }
        let list = s.list_versions(&id).await.unwrap();
        assert_eq!(
            list.iter().map(|v| v.0.clone()).collect::<Vec<_>>(),
            vec!["v1", "v2", "v10"]
        );
    }
}
