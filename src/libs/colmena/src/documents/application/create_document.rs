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
    pub html_renderer: Arc<dyn IRRenderer>,
    pub html_validator: Arc<dyn IRValidator>,
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

        let (validator, renderer) = self.render_pair(input.kind);
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
            rendered_extension: input.kind.extension(),
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

    /// Returns the `(validator, renderer)` pair for `kind`, reading the same
    /// named fields the pre-refactor per-kind match arm read directly. Same
    /// shape as `ApplyPatchUseCase::render_pair`.
    fn render_pair(&self, kind: ArtifactKind) -> (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) {
        match kind {
            ArtifactKind::Excel => (&self.excel_validator, &self.excel_renderer),
            ArtifactKind::Word => (&self.word_validator, &self.word_renderer),
            ArtifactKind::Html => (&self.html_validator, &self.html_renderer),
        }
    }
}

fn default_label(kind: ArtifactKind) -> String {
    let now = Utc::now().format("%Y-%m-%d %H:%M");
    format!("Untitled {kind:?} {now}")
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
        ArtifactKind::Html => serde_json::json!({
            "kind": "html",
            "artifact_id": id.0,
            "version_id": "v1",
            "schema_version": crate::documents::domain::ir::SCHEMA_VERSION,
            "doc_props": { "title": null, "author": null, "date": null, "locale": "en" },
            "theme": "executive",
            "layout_mode": "report",
            "footer": { "enabled": false, "page_numbers": false, "custom_text": null },
            "slides": [{
                "id": "sl_initial",
                "layout": "blank",
                "title": null,
                "subtitle": null,
                "notes": null,
                "blocks": []
            }],
            "assets_referenced": []
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
            html_renderer: Arc::new(NoopRenderer),
            html_validator: Arc::new(NoopValidator),
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

    #[tokio::test]
    async fn creates_empty_html_artifact_v1() {
        use crate::documents::domain::ports::AssetStore;
        use crate::documents::infrastructure::render::HtmlRenderer;
        use crate::documents::infrastructure::storage::LocalFsAssetStore;
        use crate::documents::infrastructure::validation::HtmlValidator;

        let tmp_a = tempdir().unwrap();
        let tmp_b = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp_a.path()));
        let asset_store: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp_b.path()));
        let uc = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopRenderer),
            word_validator: Arc::new(NoopValidator),
            html_renderer: Arc::new(HtmlRenderer::new(asset_store)),
            html_validator: Arc::new(HtmlValidator),
            ids: Arc::new(CountingIdGenerator::default()),
            default_retention: 10,
        };
        let out = uc
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Html,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();
        assert_eq!(out.version_id.0, "v1");
        let stored = store.read_current(&out.artifact_id).await.unwrap();
        assert_eq!(stored.rendered_extension, "html");
    }

    // ---- Phase 0 parity net (finding #39) ----
    // Word coverage was missing before this refactor; add it as a
    // characterization test pinned against unchanged production code.
    #[tokio::test]
    async fn creates_empty_word_artifact() {
        use crate::documents::infrastructure::render::WordRenderer;
        use crate::documents::infrastructure::validation::WordValidator;

        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let uc = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(WordRenderer),
            word_validator: Arc::new(WordValidator),
            html_renderer: Arc::new(NoopRenderer),
            html_validator: Arc::new(NoopValidator),
            ids: Arc::new(CountingIdGenerator::default()),
            default_retention: 10,
        };
        let out = uc
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Word,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();
        assert_eq!(out.version_id.0, "v1");
        assert!(out.label.starts_with("Untitled Word"));
        let stored = store.read_current(&out.artifact_id).await.unwrap();
        assert_eq!(stored.rendered_extension, "docx");
        assert!(stored.rendered_binary.len() > 4);
        assert_eq!(&stored.rendered_binary[0..4], &[0x50, 0x4B, 0x03, 0x04]);
    }
}
