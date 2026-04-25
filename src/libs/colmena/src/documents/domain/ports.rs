use super::artifact::{ArtifactMeta, ArtifactSummary, VersionData};
use super::error::{DocumentError, IndexError, RenderError, StorageError};
use super::ids::{ArtifactId, SessionId, VersionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn create_artifact(&self, meta: &ArtifactMeta) -> Result<(), StorageError>;

    async fn write_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        data: &VersionData,
    ) -> Result<(), StorageError>;

    async fn read_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<VersionData, StorageError>;

    async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError>;

    async fn list_versions(&self, id: &ArtifactId) -> Result<Vec<VersionId>, StorageError>;

    async fn set_head(
        &self,
        id: &ArtifactId,
        expected_current: Option<&VersionId>,
        new: &VersionId,
    ) -> Result<(), StorageError>;

    async fn delete_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<(), StorageError>;

    async fn read_meta(&self, id: &ArtifactId) -> Result<ArtifactMeta, StorageError>;

    async fn update_meta(&self, id: &ArtifactId, meta: &ArtifactMeta) -> Result<(), StorageError>;

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<(), StorageError>;
}

#[async_trait]
pub trait IRRenderer: Send + Sync {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError>;
    fn target_extension(&self) -> &'static str;
    fn target_mime(&self) -> &'static str;
}

pub trait IRValidator: Send + Sync {
    fn validate(&self, ir: &serde_json::Value) -> Result<(), DocumentError>;
}

pub trait IdGenerator: Send + Sync {
    fn new_artifact_id(&self) -> String;
    fn new_sheet_id(&self) -> String;
    fn new_table_id(&self) -> String;
    fn new_block_id(&self) -> String;
    fn new_run_id(&self) -> String;
    fn new_row_id(&self) -> String;
    fn new_list_item_id(&self) -> String;
}

/// Maps `session_id ↔ [artifact_ids]` so an agent can list its own artifacts and
/// the server can enforce session isolation. See spec §9.
#[async_trait]
pub trait SessionArtifactIndex: Send + Sync {
    async fn register(
        &self,
        session: &SessionId,
        id: &ArtifactId,
        meta: &ArtifactMeta,
    ) -> Result<(), IndexError>;

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<ArtifactSummary>, IndexError>;

    async fn lookup(&self, id: &ArtifactId) -> Result<Option<ArtifactSummary>, IndexError>;

    async fn update_head(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        updated_at: DateTime<Utc>,
    ) -> Result<(), IndexError>;

    async fn unregister(&self, id: &ArtifactId) -> Result<(), IndexError>;
}
