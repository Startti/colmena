use crate::documents::domain::artifact::{ArtifactMeta, PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
use crate::documents::domain::patch::PatchSource;
use crate::documents::domain::{
    ArtifactStore, DocumentError, IRRenderer, IRValidator, IdGenerator,
};
use chrono::Utc;
use std::sync::Arc;

pub struct CreateDocumentInput {
    pub kind: ArtifactKind,
    pub session_id: SessionId,
    pub label: Option<String>,
    pub retention_limit: Option<u32>,
    pub initial_ir: Option<serde_json::Value>,
    pub source: PatchSource,
}

pub struct CreateDocumentOutput {
    pub artifact_id: ArtifactId,
    pub version_id: VersionId,
    pub label: String,
    pub meta: ArtifactMeta,
}

pub struct CreateDocumentUseCase {
    pub store: Arc<dyn ArtifactStore>,
    pub excel_renderer: Arc<dyn IRRenderer>,
    pub excel_validator: Arc<dyn IRValidator>,
    pub word_renderer: Arc<dyn IRRenderer>,
    pub word_validator: Arc<dyn IRValidator>,
    pub ids: Arc<dyn IdGenerator>,
    pub default_retention: u32,
}

impl CreateDocumentUseCase {
    pub async fn execute(
        &self,
        input: CreateDocumentInput,
    ) -> Result<CreateDocumentOutput, DocumentError> {
        let artifact_id = ArtifactId::new(self.ids.new_artifact_id());
        let label = input.label.unwrap_or_else(|| default_label(input.kind));
        let retention = input.retention_limit.unwrap_or(self.default_retention);

        let meta = ArtifactMeta::initial(
            artifact_id.clone(),
            input.kind,
            input.session_id.clone(),
            label.clone(),
            retention,
        );

        let ir = input
            .initial_ir
            .unwrap_or_else(|| empty_ir(&artifact_id, input.kind));
        let mut ir = ir;
        if let Some(obj) = ir.as_object_mut() {
            obj.insert("artifact_id".into(), serde_json::json!(artifact_id.0));
            obj.insert("version_id".into(), serde_json::json!("v1"));
        }

        let (validator, renderer): (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) = match input.kind
        {
            ArtifactKind::Excel => (&self.excel_validator, &self.excel_renderer),
            ArtifactKind::Word => (&self.word_validator, &self.word_renderer),
        };
        validator.validate(&ir)?;
        let bytes = renderer.render(&ir).await?;

        let patch_applied = PatchApplied {
            patch: serde_json::json!({
                "artifact_id": artifact_id.0,
                "base_version": "",
                "source": input.source,
                "ops": []
            }),
            applied_at: Utc::now(),
            resulted_in: VersionId::initial(),
            summary: PatchSummary::default(),
        };

        let version_data = VersionData {
            ir,
            rendered_binary: bytes,
            rendered_extension: match input.kind {
                ArtifactKind::Excel => "xlsx",
                ArtifactKind::Word => "docx",
            },
            patch_applied,
            blobs: vec![],
        };

        self.store.create_artifact(&meta).await?;
        self.store
            .write_version(&artifact_id, &VersionId::initial(), &version_data)
            .await?;
        self.store
            .set_head(&artifact_id, None, &VersionId::initial())
            .await?;

        Ok(CreateDocumentOutput {
            artifact_id,
            version_id: VersionId::initial(),
            label,
            meta,
        })
    }
}

fn default_label(kind: ArtifactKind) -> String {
    let now = Utc::now().format("%Y-%m-%d %H:%M");
    let k = match kind {
        ArtifactKind::Excel => "Excel",
        ArtifactKind::Word => "Word",
    };
    format!("Untitled {k} {now}")
}

fn empty_ir(id: &ArtifactId, kind: ArtifactKind) -> serde_json::Value {
    match kind {
        ArtifactKind::Excel => serde_json::json!({
            "kind": "excel",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "workbook": { "sheets": [], "named_styles": {} }
        }),
        ArtifactKind::Word => serde_json::json!({
            "kind": "word",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "document": { "blocks": [], "named_styles": {} }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::ids::CountingIdGenerator;
    use crate::documents::infrastructure::render::ExcelRenderer;
    use crate::documents::infrastructure::storage::LocalFsStore;
    use crate::documents::infrastructure::validation::ExcelValidator;
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct NoopRenderer;
    #[async_trait]
    impl IRRenderer for NoopRenderer {
        async fn render(
            &self,
            _ir: &serde_json::Value,
        ) -> Result<Vec<u8>, crate::documents::domain::RenderError> {
            Ok(vec![])
        }
        fn target_extension(&self) -> &'static str {
            "docx"
        }
        fn target_mime(&self) -> &'static str {
            "application/octet-stream"
        }
    }
    struct NoopValidator;
    impl IRValidator for NoopValidator {
        fn validate(&self, _ir: &serde_json::Value) -> Result<(), DocumentError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn creates_empty_excel_artifact() {
        let tmp = tempdir().unwrap();
        let uc = CreateDocumentUseCase {
            store: Arc::new(LocalFsStore::new(tmp.path())),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopRenderer),
            word_validator: Arc::new(NoopValidator),
            ids: Arc::new(CountingIdGenerator::default()),
            default_retention: 10,
        };
        let out = uc
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Excel,
                session_id: SessionId::new("sess_1"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();
        assert_eq!(out.artifact_id.0, "art_01");
        assert_eq!(out.version_id, VersionId::initial());
        assert!(out.label.starts_with("Untitled Excel"));
    }
}
