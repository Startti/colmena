use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::{ArtifactStore, DocumentError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct GetHeadOutput {
    pub artifact_id: ArtifactId,
    pub current_version: VersionId,
    pub updated_at: DateTime<Utc>,
    pub last_source: String,
    pub summary_since: Vec<String>,
    pub versions_in_window: Vec<VersionId>,
}

pub struct GetHeadInput {
    pub artifact_id: ArtifactId,
    pub since_version: Option<VersionId>,
}

pub struct GetHeadUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl GetHeadUseCase {
    pub async fn execute(&self, input: GetHeadInput) -> Result<GetHeadOutput, DocumentError> {
        let meta = self.store.read_meta(&input.artifact_id).await?;
        let current_data = self
            .store
            .read_version(&input.artifact_id, &meta.current_version)
            .await?;
        let last_source = current_data
            .patch_applied
            .patch
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("agent")
            .to_string();

        let (summary_since, window) = if let Some(since) = input.since_version {
            let versions = self.store.list_versions(&input.artifact_id).await?;
            let since_n = since.number().unwrap_or(0);
            let mut lines = Vec::new();
            let mut in_window = Vec::new();
            for v in versions {
                if v.number().unwrap_or(0) > since_n {
                    let d = self.store.read_version(&input.artifact_id, &v).await?;
                    let src = d
                        .patch_applied
                        .patch
                        .get("source")
                        .and_then(|s| s.as_str())
                        .unwrap_or("agent")
                        .to_string();
                    if src == "user" {
                        for line in d.patch_applied.summary.natural_language {
                            lines.push(format!(
                                "[{}, user, {}] {}",
                                v.0,
                                d.patch_applied.applied_at.format("%H:%M"),
                                line
                            ));
                        }
                    }
                    in_window.push(v);
                }
            }
            (lines, in_window)
        } else {
            (vec![], vec![])
        };

        Ok(GetHeadOutput {
            artifact_id: input.artifact_id,
            current_version: meta.current_version,
            updated_at: meta.updated_at,
            last_source,
            summary_since,
            versions_in_window: window,
        })
    }
}
