use crate::documents::domain::ids::{ArtifactId, VersionId};
use crate::documents::domain::{ArtifactStore, DocumentError};
use std::sync::Arc;

pub struct ReadDocumentInput {
    pub artifact_id: ArtifactId,
    pub version: Option<VersionId>,
}

#[derive(Debug)]
pub struct ReadDocumentOutput {
    pub ir: serde_json::Value,
    pub version: VersionId,
}

pub struct ReadDocumentUseCase {
    pub store: Arc<dyn ArtifactStore>,
}

impl ReadDocumentUseCase {
    pub async fn execute(
        &self,
        input: ReadDocumentInput,
    ) -> Result<ReadDocumentOutput, DocumentError> {
        let data = match input.version {
            Some(v) => self.store.read_version(&input.artifact_id, &v).await?,
            None => self.store.read_current(&input.artifact_id).await?,
        };
        let version = VersionId::new(
            data.ir
                .get("version_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        );
        Ok(ReadDocumentOutput {
            ir: data.ir,
            version,
        })
    }
}
