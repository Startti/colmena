use super::artifact::{ArtifactMeta, VersionData};
use super::error::{DocumentError, RenderError, StorageError};
use super::ids::{ArtifactId, VersionId};
use async_trait::async_trait;

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
