use crate::documents::domain::artifact::{PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::patch::PatchSource;
use crate::documents::domain::{ArtifactStore, DocumentError};
use chrono::Utc;
use std::sync::Arc;

pub struct RollbackInput {
    pub artifact_id: ArtifactId,
    pub to_version: VersionId,
}

#[derive(Debug)]
pub struct RollbackOutput {
    pub new_version_id: VersionId,
    pub copied_from: VersionId,
}

pub struct RollbackUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl RollbackUseCase {
    pub async fn execute(&self, input: RollbackInput) -> Result<RollbackOutput, DocumentError> {
        let mut meta = self.store.read_meta(&input.artifact_id).await?;
        let target = self
            .store
            .read_version(&input.artifact_id, &input.to_version)
            .await?;
        let new_version = meta.current_version.next();

        let mut ir = target.ir.clone();
        if let Some(obj) = ir.as_object_mut() {
            obj.insert("version_id".into(), serde_json::json!(new_version.0));
        }

        let patch_applied = PatchApplied {
            patch: serde_json::json!({
                "artifact_id": input.artifact_id.0,
                "base_version": meta.current_version.0,
                "source": PatchSource::Agent,
                "ops": [{"op": "rollback_from", "target": input.to_version.0}]
            }),
            applied_at: Utc::now(),
            resulted_in: new_version.clone(),
            summary: PatchSummary {
                natural_language: vec![format!("Rolled back to {}", input.to_version.0)],
                structured: vec![],
            },
        };

        let version_data = VersionData {
            ir,
            rendered_binary: target.rendered_binary.clone(),
            rendered_extension: target.rendered_extension,
            patch_applied,
            blobs: vec![],
        };

        self.store
            .write_version(&input.artifact_id, &new_version, &version_data)
            .await?;
        self.store
            .set_head(
                &input.artifact_id,
                Some(&meta.current_version),
                &new_version,
            )
            .await?;
        meta.current_version = new_version.clone();
        meta.updated_at = Utc::now();
        self.store.update_meta(&input.artifact_id, &meta).await?;

        Ok(RollbackOutput {
            new_version_id: new_version,
            copied_from: input.to_version,
        })
    }
}
