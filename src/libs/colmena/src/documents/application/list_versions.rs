use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::{ArtifactStore, DocumentError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct VersionEntry {
    pub version_id: VersionId,
    pub applied_at: DateTime<Utc>,
    pub source: String,
    pub summary: Vec<String>,
}

pub struct ListVersionsUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl ListVersionsUseCase {
    pub async fn execute(
        &self,
        artifact_id: &ArtifactId,
        limit: Option<u32>,
    ) -> Result<Vec<VersionEntry>, DocumentError> {
        let versions = self.store.list_versions(artifact_id).await?;
        let mut entries: Vec<VersionEntry> = Vec::new();
        let take = limit.map(|n| n as usize).unwrap_or(versions.len());
        for v in versions.iter().rev().take(take) {
            let data = self.store.read_version(artifact_id, v).await?;
            entries.push(VersionEntry {
                version_id: v.clone(),
                applied_at: data.patch_applied.applied_at,
                source: data
                    .patch_applied
                    .patch
                    .get("source")
                    .and_then(|s| s.as_str())
                    .unwrap_or("agent")
                    .to_string(),
                summary: data.patch_applied.summary.natural_language,
            });
        }
        Ok(entries)
    }
}
